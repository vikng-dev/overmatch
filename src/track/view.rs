//! Phase-A track VIEW plugin (architecture §3/§9): live tracks on every tank, zero sim risk.
//!
//! The kinematic-wrap view ([`super::wrap`]) runs per tank on the PRESENTED pose — the root
//! `Transform` after physics writeback and (on the net client) after rollback-correction
//! smoothing, before transform propagation — so links, wheels, and hull share one rendered
//! frame. Everything here is cosmetic state, reseedable from data at any instant: never
//! rollback-registered, never replayed, and mounted only by the windowed clients (ADR-0014 —
//! the server never composes this plugin).
//!
//! The wrap fits the belt around the articulated running-gear circles as a pure function of pose,
//! terrain and belt phase, plus two self-healing FILTER tiers (a hull-frame conform ease and a slack
//! spring), always on, and a third on the phase itself ([`presented_phase`], which carries the
//! drawn belt at the sim's belt SPEED between sim samples so it does not step against a hull the
//! renderer moves continuously). The filter state is client-local cosmetic memory, so a tank's
//! drawn belt is not a pure function of replicated pose + phase — which is fine, it is view-layer
//! juice, and every rotating part still reads ONE phase, so the tooth lock holds. Raw directional
//! contacts are cached on unchanged inputs and refreshed on the sim clock while moving. Only the
//! contact targets interpolate; wheel and wrap filters and belt phase advance every render frame.

use avian3d::prelude::PhysicsSystems;
use bevy::math::{Affine3A, Vec2};
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use crate::bake::TankBlueprint;
use crate::render_policy::VisualScope;
use crate::tank::{Roadwheel, Tank, TrackSide, ViewNode};

use super::belt_draw::BeltAssets;
use super::gear_phase::{gear_spin_transform, spin_angle, sprocket_tooth_tip, tooth_angle};
use super::link_view::{self, BeltFrames, LinkTemplate, Stragglers};
use super::oracle::{BlockField, TerrainOracle};
use super::rig_geom::RigGeom;
use super::shadow_proxy::{self, ProxyMode, ProxySide, ProxyStep};
use super::side::Side;
use super::sim::TrackDrive;
use super::terrain::TrackField;
use super::wheels::{WHEEL_LIFT_RISE_OMEGA, WheelParams, wheel_lift_step, wheel_lift_target};
use super::wrap;

/// Ordering owner for the track view's presented-pose read: after physics writeback (Avian has
/// written the frame's root `Transform`, interpolated in SP, wire/frame-interpolated under
/// netcode), before propagation carries the written view poses out.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackViewSet;

pub fn view_plugin(app: &mut App) {
    app.add_observer(rebind_on_reinstance);
    // The shoe template: one read of the glb's `Link` node, and the every-frame `Added<Name>` scan
    // that HIDES it (and `Link_Box`). Shared verbatim with the sandbox — the game rendered
    // procedural boxes and hid neither node until 2026-07-26, so every session drew a stray white
    // marker box and a loose shoe parked beside the hull.
    app.add_plugins(link_view::template_plugin);
    // The belt's shadow caster ([`super::shadow_proxy`]). The knob is read ONCE, at plugin build:
    // the A/B is a launch-time arm, not something a frame gets to change.
    app.insert_resource(ProxyMode::from_env());
    app.add_systems(Update, (bind_track_rigs, attach_shadow_proxies).chain());
    // Before scope resolution: the caster swap gives the ribbon its `Mesh3d` here, and a mesh that
    // resolves a frame late is drawn by the camera on the default channel for that frame.
    app.configure_sets(
        PostUpdate,
        TrackViewSet
            .after(PhysicsSystems::Writeback)
            .before(TransformSystems::Propagate)
            .before(crate::render_policy::ScopeResolution),
    );
    app.add_systems(PostUpdate, drive_track_views.in_set(TrackViewSet));
    // A belt's table reaches its bounds and the render world in the frame the view fits it: the
    // upload runs after this fit and, by `belt_draw::plugin`, before `AssetEventSystems` flushes
    // the page's `Modified` event.
    app.configure_sets(PostUpdate, super::belt_draw::BeltUpload.after(TrackViewSet));
    // The prints the drawn belts lay in the ground: they read the shoes this view places, so they
    // are composed wherever it is, and nowhere else.
    app.add_plugins(crate::track_prints::plugin);
}

/// Presented-phase heal frequency (rad/s) — settle ≈ 4.7/ω ≈ 1.2 s.
///
/// Bounded from ABOVE by the artefact it exists to remove. Easing onto a staircase puts the
/// staircase back into the drawn rate, scaled: a residual sawtooth of `speed × T_sample` entering a
/// per-frame ease of `ω·Δt` ripples the drawn rate by at most `ω · T_sample / 2` of the belt's
/// speed — under 2 % across one 64 Hz sample (MEASURED, `worst_rate_error`), a few times that
/// across the multi-sample gaps jitter opens. Every factor faster is that much of the stutter back.
///
/// Bounded from BELOW by drift alone, and only barely: the carry integrates the sim's own belt
/// speed, so the residual it accumulates is the acceleration across one sample, and the drawn belt
/// stays tooth-locked whatever the residual is (pins and sprocket read the SAME presented phase).
/// What drift costs is BELLY SCRUB — drawn travel walking away from ground travel — so the heal has
/// to close it in well under the time a driver watches one patch of ground go by, not sooner.
const PHASE_HEAL_OMEGA: f32 = 4.0;

/// One frame of the belt's PRESENTED phase: carry the drawn phase at the sim's own belt speed, then
/// ease the residual onto the sim phase. Pure; `None` (cold start, snap) adopts `sim` whole.
///
/// `phase` moves in STEPS and the hull it is bolted to does not. The sim advances it once per fixed
/// tick ([`super::sim::apply_track_forces`], at the pre-update speed — the forward-Euler relation
/// restated here), and for a tank that is not locally simulated it advances only when a replication
/// packet lands, so drawing `phase` directly stalls the belt against ground that is sliding
/// smoothly underneath it. `speed` is a VELOCITY, and a stepped velocity integrates to a continuous
/// position — carrying at `speed` closes the gaps without a solver and without a second clock.
///
/// It also cannot invent travel the vehicle has not got: `speed` is the belt's surface speed, so a
/// braked skid (`speed` 0) still stops the links dead and wheelspin still scrolls them under a
/// stationary hull. The ease bounds what the carry accumulates — see [`PHASE_HEAL_OMEGA`].
///
/// The residual is taken modulo [`super::sim::BELT_PHASE_WRAP_M`]: a replica's `sim` phase is
/// wrapped on the wire and jumps a whole wrap where the authority's crosses one, while the drawn
/// phase stays continuous.
fn presented_phase(previous: Option<f64>, sim: f64, speed: f32, dt: f32) -> f64 {
    let Some(previous) = previous else {
        return sim;
    };
    let carried = previous + f64::from(speed) * f64::from(dt);
    let heal = 1.0 - f64::from(-PHASE_HEAL_OMEGA * dt).exp();
    carried + super::sim::wrapped_phase_delta(carried, sim) * heal
}

/// The ground the drawn belt fits: each probe reads its own ground's print depth, limited by the
/// suspension's droop travel. Sampling at the hull centre would give both belts the same sink
/// when they straddle two ground types. View only: the sim's oracle is the unlowered `ground`.
struct Sunk<'a, F> {
    ground: &'a BlockField,
    depth_m: F,
    max_sink_m: f32,
}

impl<F: Fn(Vec2) -> f32> TerrainOracle for Sunk<'_, F> {
    fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
        let depth = (self.depth_m)(station.xz()).clamp(0.0, self.max_sink_m);
        self.ground
            .depth_along_lowered_ground(station, out, reach, depth)
    }
}

/// Downward terrain-probe reach (m) — for the wrap's conform and the view wheel lift.
const PROBE_REACH: f32 = 0.5;
/// Presented-pose discontinuity thresholds: a root that moves further than this in ONE frame is
/// a teleport/respawn/snap, not motion — reset the wrap filters and the wheel-lift state. The view
/// detects locally, with no published signal: works identically in SP and MP, no netcode coupling.
/// 60 km/h at 30 fps is 0.56 m/frame — half the trip threshold.
pub(crate) const SNAP_TRANSLATION: f32 = 1.2;
/// Axis chord per frame (~30°), checked on BOTH the forward and up axes — a pure roll leaves
/// forward unchanged.
pub(crate) const SNAP_AXIS: f32 = 0.5;
/// Wheel-lift probe stations across the DISC (m from the wheel's real x) — the Tiger's
/// interleaved discs are 0.158 m wide, far narrower than the shoe: probing shoe-wide at an
/// outboard wheel column would read geometry entirely beside the track.
/// Interim until the bake carries disc bounds. (The wrap's conform keeps shoe-wide lateral stations
/// at `plane_x` — [`RigSide::lateral_stations`] — because that IS the physics footprint.)
const WHEEL_DISC_STATIONS: [f32; 3] = [-0.08, 0.0, 0.08];

/// One tank's track-view state, on the root. Pure view: despawns with the root, resets its filter
/// memory on any presented-pose discontinuity.
#[derive(Component)]
struct TrackRig {
    /// The immutable material loop: `belt_len = pitch × count`, exact (MEASURED pitch, from
    /// [`RigGeom::belt_len`]).
    belt_len: f32,
    count: usize,
    /// Sprocket tooth count (authored) — the tooth-lock spins one tooth per link pitch of travel.
    teeth: u32,
    /// Measured pin-line → inner-face offset (m). A ROAD WHEEL (hub translating over the
    /// stationary belly run) rolls on the track's INNER face, so its rolling radius is its
    /// pin-line radius minus THIS — the pin does not run mid-plate, so it is not half the
    /// plate. The WRAPPED idler does NOT subtract it: wrap rotation follows the pin line.
    pin_to_inner: f32,
    /// Measured plate thickness (m) — the wrap's conform pushes the pin line to the outer face at
    /// `thickness/2`.
    thickness: f32,
    /// Per-side kinematic-wrap filter memory ([`wrap::WrapState`]). Reset on a
    /// presented-pose snap so the belly memory re-inits from the fresh pose rather than settling in.
    wrap: [wrap::WrapState; 2],
    /// Per-side PRESENTED belt phase — the phase the view actually draws, carried at render rate by
    /// [`presented_phase`]. `None` = cold start / reseed: the next frame adopts the sim phase whole.
    presented_phase: [Option<f64>; 2],
    sides: [RigSide; 2],
    /// Last frame's presented affine — the snap detector's previous frame. `None` = cold start.
    prev_affine: Option<Affine3A>,
    field_revision: Option<u64>,
    fit_dependencies: Option<FitDependencies>,
    contact_clock: ContactClock,
    fit_affine: Option<Affine3A>,
    contacts: [wrap::ContactCache; 2],
}

impl TrackRig {
    /// A contact sample reads the presented pose, coalescing any fixed steps since the last
    /// render. Discontinuities and non-pose dependencies discard both target histories.
    fn begin_contact_frame(
        &mut self,
        affine: Affine3A,
        dependencies: FitDependencies,
        refresh: bool,
        snapped: bool,
    ) -> bool {
        if snapped || self.fit_dependencies.as_ref() != Some(&dependencies) {
            self.contacts = Default::default();
            self.fit_affine = None;
            for side in &mut self.sides {
                for wheel in &mut side.wheels {
                    wheel.target = Default::default();
                }
            }
        }
        self.fit_dependencies = Some(dependencies);
        // PostUpdate reads the presented hull after writeback. Several fixed steps in one render
        // frame coalesce into the latest target; frames between steps never cast moving fit rays.
        let wheel_changed = self.fit_affine != Some(affine);
        if refresh || self.fit_affine.is_none() {
            self.fit_affine = Some(affine);
        }
        wheel_changed
    }
}

/// Every non-pose input to the exact contact queries besides terrain, whose revision change is
/// a snap and already discards the targets. Rig geometry is immutable for a bind; reinstance
/// constructs a fresh rig. Resource changes invalidate targets even on a parked hull.
#[derive(PartialEq)]
struct FitDependencies {
    print: Option<u32>,
    pressure: Option<f32>,
    gear: u32,
    blueprint: u32,
}

/// Where a presented frame falls between the two latest contact samples, on the fixed clock.
///
/// A sample is taken on the first render frame after the fixed clock advances, from that frame's
/// presented pose, and stamped with the clock's `elapsed`. Ticks that pass before one render frame
/// coalesce into one sample, so two stamps can be several ticks apart. The blend cursor trails the
/// frame's fixed time, `elapsed + overstep`, by one `timestep`. It lies between the two stamps
/// because `elapsed` advances in whole timesteps and Bevy's fixed loop leaves
/// `overstep < timestep`; a stamp never postdates the pose it was probed from. So a target is at
/// most one tick older than the presented pose at any frame rate (DERIVED). With stamps one tick
/// apart the weight is `Time::<Fixed>::overstep_fraction` exactly.
#[derive(Default)]
struct ContactClock {
    previous: std::time::Duration,
    current: Option<std::time::Duration>,
}

impl ContactClock {
    /// `(refresh, alpha)`: whether this frame takes a new sample, and the blend weight from the
    /// previous sample's value toward the current one's.
    fn advance(&mut self, fixed: &Time<Fixed>) -> (bool, f32) {
        let tick = fixed.elapsed();
        let refresh = self.current != Some(tick);
        if refresh {
            self.previous = self.current.unwrap_or(tick);
            self.current = Some(tick);
        }
        // One sample (a cold clock, or a fixed clock that stepped back) is the whole blend.
        let span = tick.saturating_sub(self.previous);
        if span.is_zero() {
            return (refresh, 1.0);
        }
        let cursor = (tick + fixed.overstep()).saturating_sub(fixed.timestep());
        let alpha = cursor.saturating_sub(self.previous).as_secs_f32() / span.as_secs_f32();
        (refresh, alpha.min(1.0))
    }
}

#[derive(Default)]
struct ContactTarget {
    previous: f32,
    current: Option<f32>,
}

impl ContactTarget {
    fn sample(
        &mut self,
        refresh: bool,
        changed: bool,
        alpha: f32,
        probe: impl FnOnce() -> f32,
    ) -> f32 {
        match self.current {
            None => {
                let value = probe();
                self.previous = value;
                self.current = Some(value);
            }
            Some(value) if refresh => {
                self.previous = value;
                if changed {
                    self.current = Some(probe());
                }
            }
            _ => {}
        }
        self.previous + (self.current.unwrap() - self.previous) * alpha
    }
}

struct RigSide {
    /// Signed track-centreline x (left −, right +).
    plane_x: f32,
    /// The wrap's lateral terrain stations ([`RigGeom::grip_stations`]): the physics collocation
    /// columns as signed hull-x offsets from this side's plane — the measured shoe faces, per-side
    /// because the shoe is not centred on its pins.
    lateral_stations: [f32; 3],
    /// This side's hull-fixed sprocket pin-line circle (side-plane `(z, y)` centre + pitch radius),
    /// from [`RigGeom::rest`]. The sprocket MUST stay first in the circle list — the wrap keys its
    /// end arcs off that order (first = sprocket, last = idler).
    sprocket: (Vec2, f32),
    /// This side's hull-fixed idler pin-line circle, from [`RigGeom::rest`].
    idler: (Vec2, f32),
    /// Per road wheel, front→rear.
    wheels: Vec<RigWheel>,
    /// Sprocket/idler visual mesh nodes. Each now carries its own axle-origin translation in its
    /// authored node transform (bake invariant `rotating_nodes_carry_their_own_axle_origin`), so
    /// they are posed by composing the belt spin ONTO that captured REST transform
    /// (`gear_spin_transform`) — never by overwriting translation/scale, which would drag the node
    /// to the tank origin the moment the spin is near zero.
    sprocket_view: Entity,
    /// The sprocket node's authored REST transform, captured at bind — the pose the spin rides on.
    sprocket_rest: Transform,
    /// Where zero belt phase seats the first pin on this side (`RigGeom::belt_origin_angle`), and
    /// where this sprocket's mesh carries a tooth TIP at rest (`sprocket_tooth_tip`, measured off
    /// the glb). Together they PHASE-lock the teeth to the pins, not merely rate-lock them.
    sprocket_origin: f32,
    sprocket_tooth_tip: f32,
    idler_view: Entity,
    /// The idler node's authored REST transform, captured at bind (see `sprocket_rest`).
    idler_rest: Transform,
    /// This side's BELT ([`link_view::ShoeBelt`]): the identity-posed entity that draws every shoe
    /// of the side from its [`BeltFrames`] table — the same template, placement and mirrored
    /// left-side mesh the sandbox draws, so the two tools cannot disagree about what this track
    /// looks like. A despawn of it takes the table and gives its block of the shared GPU table back.
    belt: Entity,
    /// The shoe's own lateral centre on this side (`RigGeom::link_center_x`) — NOT `plane_x`: the
    /// shoe is authored outboard of the pin plane, and the shadow ribbon's section is centred on it.
    link_center_x: f32,
    /// This side's SHADOW CASTER ([`super::shadow_proxy`]): the low-poly ribbon that casts the
    /// belt's shadow so the 1 520-triangle shoes do not have to. `None` under [`ProxyMode::Off`]
    /// (the shoes cast, exactly as they shipped) and for the one frame between the rig binding and
    /// [`attach_shadow_proxies`] running.
    proxy: Option<ProxySide>,
}

struct RigWheel {
    /// Rest pivot (hull-local — the REAL authored x, which for the Tiger's interleaved columns
    /// differs from `plane_x`) and rest rotation (preserved under spin: a future model's wheels
    /// may author non-identity rests).
    pivot: Vec3,
    rest_rotation: Quat,
    /// This station's PIN-LINE radius (m), from the measured [`RigGeom::rest`] circle — per station,
    /// not one shared wheel radius (the Tiger's interleaved discs alternate ~1.5 mm).
    radius: f32,
    /// Cosmetic lift state.
    dy: f32,
    dvel: f32,
    target: ContactTarget,
    /// The GLB view node lift + spin write to (never the sim entity).
    view: Entity,
}

/// A GLB re-instantiation (hot reload during authoring) replaces the wheel/sprocket view
/// entities a bound rig points at: drop the rig and its belts, and `bind_track_rigs` rebinds
/// against the fresh tree next frame.
fn rebind_on_reinstance(
    ready: On<WorldInstanceReady>,
    rigs: Query<&TrackRig>,
    mut commands: Commands,
) {
    let Ok(rig) = rigs.get(ready.entity) else {
        return;
    };
    for side in &rig.sides {
        // The belt carries its table and its block of the shared GPU table, so one despawn per side
        // releases the whole draw (its block back to the table, its count's meshes to eviction).
        commands.entity(side.belt).despawn();
        // The shadow ribbon goes with them — `attach_shadow_proxies` spawns a fresh one (and a fresh
        // mesh asset) against the rebound rig, and a leaked one would keep casting the old belt.
        if let Some(proxy) = &side.proxy {
            commands.entity(proxy.entity).despawn();
        }
    }
    commands.entity(ready.entity).remove::<TrackRig>();
}

/// Attach a [`TrackRig`] to every tank whose presentation is ready: all roadwheel sim entities
/// carry `ViewNode` links (i.e. `bind_tank_view` ran), the per-side `Sprocket_*`/`Idler_*` rig
/// meshes are found in the GLB tree, and the measured [`RigGeom`] is live. Retries lazily until
/// then — no ordering coupling to the `WorldInstanceReady` observer. All numeric geometry (pitch,
/// plate, sprocket/idler/wheel circles) comes from `RigGeom`; the GLB scan only finds the entities
/// to spin.
#[expect(
    clippy::too_many_arguments,
    reason = "attaches a rig: blueprint, rig, template, tanks, hierarchy, names, wheels, transforms and the belt assets"
)]
fn bind_track_rigs(
    blueprint: Res<TankBlueprint>,
    geom: Option<Res<RigGeom>>,
    // The shoe template ([`super::link_view`]): the glb's own `Link` mesh, its mirror, the shared
    // material and both sides' canonical pin frames. Bound by the shared plugin as soon as the
    // scene lands; until then there are no shoes to pool, so the whole rig waits.
    template: Option<Res<LinkTemplate>>,
    tanks: Query<Entity, (With<Tank>, Without<TrackRig>)>,
    children: Query<&Children>,
    names: Query<&Name>,
    wheels: Query<(&Roadwheel, &Transform, Option<&ViewNode>)>,
    // Sprocket/idler REST poses and their tooth-tip mesh calibration are read here at bind: node
    // transforms for the rest capture and the mesh-space→hull composition, mesh primitives for the
    // sprocket tooth ring.
    node_transforms: Query<&Transform>,
    primitives: Query<&Mesh3d>,
    // The belt's draw: its repeated meshes (and the sprocket's, read for the tooth ring), the
    // shoe's material, and each belt's block of the shared table.
    mut belt_assets: BeltAssets,
    mut commands: Commands,
) {
    // The measured running gear lands with `TrackGear` on the first sim frame; until then there is
    // nothing to bind the numeric geometry against.
    let Some(geom) = geom else {
        return;
    };
    let Some(template) = template else {
        return;
    };
    let spec = &blueprint.spec.track;
    let belt_len = geom.belt_len();
    'tank: for root in &tanks {
        let mut side_wheels: [Vec<(Vec3, Quat, Entity)>; 2] = [Vec::new(), Vec::new()];
        let mut sprocket_view = [None, None];
        let mut idler_view = [None, None];
        for entity in children.iter_descendants(root) {
            if let Ok((wheel, transform, view)) = wheels.get(entity) {
                // Presentation not attached yet — try again next frame.
                let Some(view) = view else { continue 'tank };
                let si = (wheel.side == TrackSide::Right) as usize;
                side_wheels[si].push((transform.translation, transform.rotation, view.0));
            } else if let Ok(name) = names.get(entity) {
                if let Some(side) = spec.sprocket.nodes.side_of(name) {
                    sprocket_view[side] = Some(entity);
                } else if let Some(side) = spec.idler.nodes.side_of(name) {
                    idler_view[side] = Some(entity);
                }
            }
        }
        let [Some(sl), Some(sr)] = sprocket_view else {
            continue;
        };
        let [Some(il), Some(ir)] = idler_view else {
            continue;
        };
        if side_wheels.iter().any(Vec::is_empty) {
            continue;
        }
        for side in &mut side_wheels {
            side.sort_by(|a, b| a.0.z.total_cmp(&b.0.z));
        }

        // Per-side REST running gear from the measured rig: `[sprocket, road wheels…, idler]`, each
        // circle carrying its own centre AND pin-line radius. The live wrap articulates the wheel
        // CENTRES off the presented pose; only the RADII (and the hull-fixed sprocket/idler circles)
        // come from here.
        let rest = [Side::Left, Side::Right].map(|s| geom.rest.get(s).clone());

        // Feasibility gate (the schema check can't do this — it needs the running-gear circles): the
        // material loop must close around the rest gear, or every frame silently draws a belt that
        // cannot exist rather than failing once, loudly. Measured on the right side — the sides are
        // mirror images in the side plane and one loop length serves both.
        let closed = super::route::build_route(&rest[1], belt_len).total();
        if !closed.is_finite() || (closed - belt_len).abs() > 0.005 * belt_len {
            error_once!(
                "track rig infeasible: pitch × link_count = {belt_len:.3} m cannot close the rest \
                 running gear (route closed at {closed:.3} m) — no track rig bound"
            );
            continue;
        }

        // A view wheel count that disagrees with the measured rig would zip a station against a
        // neighbour's radius — refuse rather than paint the wrong circle.
        if side_wheels
            .iter()
            .zip(&rest)
            .any(|(w, r)| w.len() != r.len().saturating_sub(2))
        {
            error_once!(
                "track rig: view wheel count disagrees with the measured rig — no track rig bound"
            );
            continue;
        }

        // Capture the sprocket/idler REST transforms (the pose the belt spin rides on) and measure
        // each sprocket's tooth-tip phase off its own mesh — the phase-lock calibration. Both must
        // succeed BEFORE anything is spawned: a mesh a frame late (or a node transform not yet
        // present) just retries next frame rather than binding a mis-meshed sprocket or leaving a
        // half-bound rig (a belt drawn on one side only) behind.
        let (Ok(&sl_rest), Ok(&sr_rest), Ok(&il_rest), Ok(&ir_rest)) = (
            node_transforms.get(sl),
            node_transforms.get(sr),
            node_transforms.get(il),
            node_transforms.get(ir),
        ) else {
            continue;
        };
        // The game's gear nodes are hull-framed (their parent chain to the root is identity — the
        // same premise the road-wheel poses rely on), so a node's REST transform IS its
        // hull-from-node affine.
        let (Some(sl_tip), Some(sr_tip)) = (
            sprocket_tooth_tip(
                sl,
                sl_rest.compute_affine(),
                &children,
                &node_transforms,
                &primitives,
                &belt_assets.meshes,
                geom.teeth,
            ),
            sprocket_tooth_tip(
                sr,
                sr_rest.compute_affine(),
                &children,
                &node_transforms,
                &primitives,
                &belt_assets.meshes,
                geom.teeth,
            ),
        ) else {
            continue;
        };

        // The belts: one entity per side drawing the glb's OWN link mesh once per material link, the
        // left side wearing the template's genuine mirror. Every rung's shoe must be checked
        // repeatable and the shoe's material loaded before either side spawns, so a bind never
        // half-lands.
        if !belt_assets.ready(&template, spec.link_count) {
            continue;
        }
        let table = |side: Side| {
            BeltFrames::new(
                template.frame(side),
                geom.link_center_x(side),
                spec.link_count,
            )
        };
        let (frames_l, frames_r) = (table(Side::Left), table(Side::Right));
        let (Some(draw_l), Some(draw_r)) = (
            belt_assets.draw(&template, Side::Left, &frames_l),
            belt_assets.draw(&template, Side::Right, &frames_r),
        ) else {
            continue;
        };
        let belt_l = link_view::spawn_belt(&mut commands, Side::Left, root, frames_l, draw_l);
        let belt_r = link_view::spawn_belt(&mut commands, Side::Right, root, frames_r, draw_r);
        let [wl, wr] = side_wheels;
        // Zip each view wheel (sorted front→rear) with its measured pin-line radius from the rest
        // circles' MIDDLE (`rest[0]` is the sprocket, `rest.last()` the idler). Counts were checked
        // equal above.
        let rig_wheels = |list: Vec<(Vec3, Quat, Entity)>, rest: &[(Vec2, f32)]| -> Vec<RigWheel> {
            list.into_iter()
                .zip(rest[1..rest.len() - 1].iter())
                .map(|((pivot, rest_rotation, view), &(_, radius))| RigWheel {
                    pivot,
                    rest_rotation,
                    radius,
                    dy: 0.0,
                    dvel: 0.0,
                    target: Default::default(),
                    view,
                })
                .collect()
        };
        let sides = [
            RigSide {
                plane_x: -geom.plane_x,
                lateral_stations: geom.grip_stations(Side::Left),
                sprocket: rest[0][0],
                idler: *rest[0].last().expect("a side always has an idler"),
                wheels: rig_wheels(wl, &rest[0]),
                sprocket_view: sl,
                sprocket_rest: sl_rest,
                sprocket_origin: geom.belt_origin_angle(Side::Left),
                sprocket_tooth_tip: sl_tip,
                idler_view: il,
                idler_rest: il_rest,
                belt: belt_l,
                link_center_x: geom.link_center_x(Side::Left),
                proxy: None,
            },
            RigSide {
                plane_x: geom.plane_x,
                lateral_stations: geom.grip_stations(Side::Right),
                sprocket: rest[1][0],
                idler: *rest[1].last().expect("a side always has an idler"),
                wheels: rig_wheels(wr, &rest[1]),
                sprocket_view: sr,
                sprocket_rest: sr_rest,
                sprocket_origin: geom.belt_origin_angle(Side::Right),
                sprocket_tooth_tip: sr_tip,
                idler_view: ir,
                idler_rest: ir_rest,
                belt: belt_r,
                link_center_x: geom.link_center_x(Side::Right),
                proxy: None,
            },
        ];
        info!(
            "track rig bound: {} links/side drawn by one belt entity each ({}), {} \
             wheels/side; sprocket tooth tips L {:.2}° R {:.2}°",
            spec.link_count,
            template.chain_summary(),
            sides[0].wheels.len(),
            sl_tip.to_degrees(),
            sr_tip.to_degrees(),
        );
        commands.entity(root).insert(TrackRig {
            belt_len,
            count: spec.link_count,
            teeth: geom.teeth,
            pin_to_inner: geom.model.pin_to_inner,
            thickness: geom.thickness,
            wrap: Default::default(),
            presented_phase: [None; 2],
            sides,
            prev_affine: None,
            field_revision: None,
            fit_dependencies: None,
            contact_clock: Default::default(),
            fit_affine: None,
            contacts: Default::default(),
        });
    }
}

/// Hang the belt's SHADOW CASTER on a freshly bound rig: one ribbon entity per side
/// ([`super::shadow_proxy`] — read its module doc first; the mechanism that hides the ribbon from
/// the camera is not the obvious one, and the obvious one is broken upstream).
///
/// The ribbon entity is spawned WITHOUT a mesh, and this system does NOT silence the shoes. Both
/// happen in [`drive_track_views`], on the frame the first fit creates the ribbon's mesh asset —
/// see [`shadow_proxy::ProxySide::mesh`] for why neither an empty mesh nor shoes silenced against
/// one may exist for even a frame.
///
/// A separate system rather than more parameters on [`bind_track_rigs`], which already takes eleven:
/// this one needs the material writer, and the ribbon's mesh cannot be built until the belt has
/// been fitted once anyway. The `.chain()` that orders it after the bind puts a sync point between
/// them, so `Added<TrackRig>` sees the rig in the bind's own frame.
fn attach_shadow_proxies(
    mode: Res<ProxyMode>,
    geom: Option<Res<RigGeom>>,
    mut rigs: Query<(Entity, &mut TrackRig), Added<TrackRig>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    // One material for every proxy in the session — the ribbons differ in MESH, never in look.
    mut material: Local<Option<Handle<StandardMaterial>>>,
    mut commands: Commands,
) {
    if !mode.silences_links() || rigs.is_empty() {
        return;
    }
    let Some(geom) = geom else {
        return;
    };
    let material = material
        .get_or_insert_with(|| {
            materials.add(if *mode == ProxyMode::Visible {
                shadow_proxy::visible_probe_material()
            } else {
                shadow_proxy::proxy_material()
            })
        })
        .clone();
    for (root, mut rig) in &mut rigs {
        for (si, side) in rig.sides.iter_mut().enumerate() {
            let proxy = commands.spawn((
                Name::new(if si == 0 {
                    "Track shadow proxy L"
                } else {
                    "Track shadow proxy R"
                }),
                // Invisible to every camera, visible to every light — the ribbon's whole contract,
                // declared instead of smuggled through a material (`render_policy`). It overrides
                // the tank root's scope, so it keeps casting whichever view the player is in;
                // `ProxyMode::Visible` is the one mode that wants to be LOOKED at, so it becomes
                // ordinary world geometry instead.
                if *mode == ProxyMode::Visible {
                    VisualScope::WORLD_SOLID
                } else {
                    VisualScope::SHADOW_PROXY
                },
                MeshMaterial3d(material.clone()),
                // Identity: the ribbon is authored directly in hull space, the frame the belt
                // joints already live in.
                Transform::IDENTITY,
                ChildOf(root),
            ));
            side.proxy = Some(ProxySide::new(
                proxy.id(),
                shadow_proxy::Section::from_shoe(
                    side.link_center_x,
                    geom.model.width,
                    geom.model.pin_to_inner,
                    geom.model.pin_to_outer,
                ),
            ));
        }
        // Intent only — deliberately says nothing about triangles or silenced shoes, because at this
        // point neither exists. `drive_track_views` logs the MEASURED geometry when the swap lands.
        info!(
            "track shadow proxy: {mode:?} — 2 ribbons spawned for {} shoes/side, awaiting first fit",
            rig.count
        );
    }
}

/// The per-frame seam: read each tank's presented root pose and replicated belt phase, lift the
/// view wheels off the terrain field, fit the wrap, and write every view transform — all before
/// propagation, so the whole tank renders one consistent frame.
#[expect(
    clippy::too_many_arguments,
    reason = "the per-frame track view: clock, field, gear, blueprint, proxy mode and meshes, tanks, views and belt tables"
)]
fn drive_track_views(
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    track: Res<TrackField>,
    // The wheel-travel band: `TrackGear` carries the loop-clamped droop (its travel knots' peak),
    // the blueprint spec carries the bump stop. Both are single-blueprint globals today, read
    // per frame — well below the SystemParam ceiling.
    gear: Res<super::sim::TrackGear>,
    blueprint: Res<TankBlueprint>,
    // The shadow ribbon's arm and the assets it writes into: one `Mesh` per tank SIDE, created by
    // its first fit and rewritten in place under [`ProxyMode::Dynamic`] (never shared — two tanks
    // stand on different ground).
    mode: Res<ProxyMode>,
    mut proxy_meshes: ResMut<Assets<Mesh>>,
    mut tanks: Query<(&Transform, &TrackDrive, &mut TrackRig)>,
    mut views: Query<&mut Transform, Without<TrackRig>>,
    // Each side's frame table, which its belt draws, the prints stamp and the rung selector reads.
    mut belts: Query<&mut BeltFrames>,
    // The caster swap's writes: the ribbon's `Mesh3d` and `VisualScope::PROXIED_CASTER` onto this
    // side's belt, on the frame its ribbon first exists. Once per side per bind, never per frame.
    mut commands: Commands,
    // The belt sink: the depth the ground's shader draws a print at, and the running gear the
    // tank's ground pressure is read from. Absent without a printed world, where nothing sinks.
    sink: (
        Option<Res<crate::track_prints::PrintDepths>>,
        Option<Res<RigGeom>>,
    ),
) {
    let Some(ground) = track.field.as_ref() else {
        return;
    };
    let (depths, geom) = sink;
    let pressure_kpa = geom
        .as_deref()
        .map(crate::track_prints::tank_ground_pressure_kpa);
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (root, drive, mut rig) in &mut tanks {
        let rig = &mut *rig;
        let affine =
            Affine3A::from_scale_rotation_translation(root.scale, root.rotation, root.translation);
        // Discontinuity: teleport / respawn / snap-consumed correction / terrain swap → reset the
        // wrap filter memory (so it re-inits from the fresh pose's raw targets) and re-base the
        // wheel lift (old-terrain lift must not seed the fresh belly). The filters self-heal anyway,
        // but the explicit reset avoids a one-fall-period settle from stale memory. Rotation is
        // checked on BOTH forward and up axes — a pure roll leaves forward unchanged.
        let prev = rig.prev_affine.unwrap_or(affine);
        let axis_jump = |axis: Vec3| {
            affine
                .transform_vector3(axis)
                .distance(prev.transform_vector3(axis))
                > SNAP_AXIS
        };
        let snapped = (affine.translation - prev.translation).length() > SNAP_TRANSLATION
            || axis_jump(Vec3::Z)
            || axis_jump(Vec3::Y)
            || rig.field_revision != track.revision;
        if snapped {
            for state in &mut rig.wrap {
                state.reset();
            }
            rig.presented_phase = [None; 2];
        }
        let dependencies = FitDependencies {
            print: depths.as_ref().map(|d| d.last_changed().get()),
            pressure: pressure_kpa,
            gear: gear.last_changed().get(),
            blueprint: blueprint.last_changed().get(),
        };
        let (refresh, alpha) = rig.contact_clock.advance(&fixed);
        let wheel_changed = rig.begin_contact_frame(affine, dependencies, refresh, snapped);
        rig.prev_affine = Some(affine);
        rig.field_revision = track.revision;

        // THE BELT SINK (view only, ADR-0014): the drawn belt and its wheels fit a ground lowered
        // by the depth the ground's shader draws this tank's print at, under a full press, so the
        // track sits in its own rut instead of riding the undisturbed surface above it. The sim,
        // the oracle and the collider read the ground as it is.
        let field = &Sunk {
            ground,
            depth_m: |xz| match (&depths, pressure_kpa) {
                (Some(depths), Some(kpa)) => depths.full_press_m(xz, kpa),
                _ => 0.0,
            },
            max_sink_m: gear.max_droop(),
        };

        // Belt phase from the SIM (phase B): a simulated hull's `TrackDrive` written by the
        // fixed tick (SP), a replicated hull's INTERPOLATED one (MP — every hull, own included;
        // an interpolated hull's belt speed/phase is a CURSOR-clock value, sampled by the
        // protocol's `track_drive_lerp` on the same clock that places the hull, never the
        // arrival-fresh packet that leads it) — real belt travel, so a braked skid stops the
        // links and wheelspin scrolls them honestly. The wrap needs only the phase (the drawn
        // belt's motion IS the phase advancing; there is no solver to feed a belt speed to).
        //
        // Carried to THIS frame by [`presented_phase`] before it is drawn: the sim advances that
        // phase once per fixed tick and an interpolated tank once per frame at the cursor,
        // either way in steps, on a hull the renderer is moving continuously underneath it.
        let phases = [0, 1].map(|si| {
            let side = drive.sides[si];
            let phase = presented_phase(rig.presented_phase[si], side.phase, side.speed, dt);
            rig.presented_phase[si] = Some(phase);
            phase
        });

        // View wheel lift: probe the field at each wheel's REAL position across its DISC (not
        // the shoe), ease the lift (implicit rise / ballistic fall), then the wrap fits the belt
        // around the lifted circles. On a snap the lift re-bases to the fresh target instantly.
        let mut wparams = WheelParams {
            // `reach` is set PER WHEEL below: this station's pin-line radius (the circle the wrap
            // fits) + the measured plate face offset to its ground face — the sandbox's form, no
            // mid-plate `thickness/2` assumption and no single shared wheel radius.
            reach: 0.0,
            ease_omega: WHEEL_LIFT_RISE_OMEGA,
            max_lift: blueprint.spec.track.suspension.bump_stop,
            max_droop: gear.max_droop(),
            lateral_stations: WHEEL_DISC_STATIONS,
            probe_reach: PROBE_REACH,
        };
        let down = affine.transform_vector3(Vec3::NEG_Y).normalize_or_zero();
        for side in &mut rig.sides {
            for wheel in &mut side.wheels {
                wparams.reach = wheel.radius + gear.face_offset();
                let target = wheel.target.sample(refresh, wheel_changed, alpha, || {
                    wheel_lift_target(field, &affine, down, wheel.pivot, &wparams)
                });
                if snapped {
                    wheel.dy = target;
                    wheel.dvel = 0.0;
                } else {
                    wheel_lift_step(&mut wheel.dy, &mut wheel.dvel, target, dt, &wparams);
                }
            }
        }

        // The kinematic wrap: side-plane circles (sprocket FIRST, idler LAST — the wrap keys its end
        // arcs off that order), articulated wheel centres.
        let circles: [Vec<(Vec2, f32)>; 2] = [0, 1].map(|si| {
            let side = &rig.sides[si];
            let mut c = Vec::with_capacity(side.wheels.len() + 2);
            c.push(side.sprocket);
            c.extend(
                side.wheels
                    .iter()
                    .map(|w| (Vec2::new(w.pivot.z, w.pivot.y + w.dy), w.radius)),
            );
            c.push(side.idler);
            c
        });
        let input = wrap::WrapInput {
            dt,
            affine,
            belt_len: rig.belt_len,
            count: rig.count,
            // Material pitch = belt_len / count exactly — the pin registration the sprocket lock
            // assumes. The wrap wraps phase by the material loop itself, so a whole-loop advance
            // shifts link identity by `count` ≡ 0 — seamless by construction.
            pitch: rig.belt_len / rig.count.max(1) as f32,
            thickness: rig.thickness,
            probe_reach: PROBE_REACH,
            // The game draws no reference loop — skip the throwaway `sag_span` + `Vec` per side.
            reference: false,
            sides: [0, 1].map(|si| wrap::WrapSideInput {
                circles: &circles[si],
                plane_x: rig.sides[si].plane_x,
                lateral_stations: rig.sides[si].lateral_stations,
                phase: phases[si],
            }),
        };
        // Both feel tiers are unconditional — parameter-free laws, so there is nothing to dial.
        let out = wrap::step_cached(
            &input,
            field,
            &mut rig.wrap,
            &mut rig.contacts,
            refresh,
            alpha,
        );

        for (si, side) in rig.sides.iter_mut().enumerate() {
            // The SHOES, on this frame's drawn pin joints: link `i` spans joint `i` → `i+1`, its
            // pin midpoint on the route and its own measured centre on `link_center_x`. The
            // record↔joint map rotates with the belt phase, so a shoe's identity rides the belt
            // instead of wandering one link per pitch — all of it in `link_view`, which the sandbox
            // drives with the same call. An unchanged table stays unmarked: its upload, its bounds
            // and every reader key on the change.
            if let Ok(mut frames) = belts.get_mut(side.belt)
                && frames.bypass_change_detection().fit(
                    &out[si].joints,
                    phases[si],
                    rig.belt_len / rig.count.max(1) as f32,
                    Stragglers::Keep,
                )
            {
                frames.set_changed();
            }
            // The SHADOW CASTER, on the same joints ([`super::shadow_proxy`]). Under `Static` this
            // runs once and the ribbon rides the hull unchanged; live modes retain the topology
            // and update positions only when these hull-local joints change.
            //
            // This is also where the CASTER SWAP happens, and it is atomic on purpose: the ribbon
            // gets its mesh and the shoes are silenced on the frame the first fit closes a loop,
            // never at spawn and never apart ([`shadow_proxy::ProxySide::mesh`]). Both writes are
            // resolved this frame: `TrackViewSet` runs before `ScopeResolution`.
            if let Some(proxy) = &mut side.proxy {
                match proxy.fit(
                    proxy_meshes.reborrow(),
                    &out[si].joints,
                    mode.follows_belt(),
                ) {
                    ProxyStep::Idle => {}
                    // The shoes keep rendering at full detail to the camera at every distance — this
                    // is a CASTER swap, not an LOD. `PROXIED_CASTER` is the preset that says so: it
                    // stops the belt casting and leaves it RECEIVING, and it deliberately keeps
                    // inheriting the tank's channel, so a silenced belt still follows its tank into
                    // and out of the gunner optic. MEASURED triangles, never `count * 8`: the
                    // number in this line is the number of triangles that exist.
                    ProxyStep::Silence { mesh, triangles } => {
                        commands.entity(proxy.entity).insert(Mesh3d(mesh));
                        commands
                            .entity(side.belt)
                            .insert(VisualScope::PROXIED_CASTER);
                        info!(
                            "track shadow proxy: {mode:?} side {si} — ribbon built, {triangles} \
                             triangles MEASURED; {} shoes silenced",
                            rig.count
                        );
                    }
                    ProxyStep::Overdue => warn!(
                        "track shadow proxy: side {si} still has no ribbon after {} frames — the \
                         {} real shoes are carrying the belt's shadow at full cost",
                        shadow_proxy::PROXY_READY_GRACE_FRAMES,
                        rig.count
                    ),
                }
            }
            // Wheels roll on the track's INNER face (pin line − `pin_to_inner`, the measured inner
            // offset — the pin does not run mid-plate, so it is not half the plate). Every axle
            // angle is NEGATIVE (Bevy +X rotation moves a wheel's bottom toward −Z, and positive
            // phase scrolls the lower run toward +Z) — `spin_angle` is the single flip point if a
            // future model's conventions differ. Per-station radius → per-wheel roll radius + spin.
            for wheel in &side.wheels {
                if let Ok(mut tr) = views.get_mut(wheel.view) {
                    let roll_r = wheel.radius - rig.pin_to_inner;
                    let spin = Quat::from_rotation_x(spin_angle(phases[si], roll_r));
                    let pose = Transform {
                        translation: wheel.pivot + Vec3::Y * wheel.dy,
                        rotation: wheel.rest_rotation * spin,
                        ..*tr
                    };
                    tr.set_if_neq(pose);
                }
            }
            // The sprocket and idler both carry their own axle-origin translation, so they are
            // posed by composing the spin onto their captured REST transform (never overwriting it):
            // the rest T·S stays, the spin pre-multiplies about the hull's lateral axis. The
            // sprocket is TOOTH-LOCKED — one tooth pitch of rotation per link pitch of travel,
            // seated on its measured tooth tip — NOT `phase / pitch_radius`, which under-rotates by
            // the chord/arc ratio and drifts a whole tooth every ~244 links (~32 m). The idler is
            // toothless but WRAPPED: the belt segment around it rotates about the hub at the
            // pin-line rate (the inextensible pin polygon sets wrap rotation — the sprocket case
            // minus teeth), so it spins at `phase / pin_radius`. Only a wheel whose hub TRANSLATES
            // over the stationary belly run rolls at the inner-face radius; subtracting
            // `pin_to_inner` here over-rotates the idler by ~7%.
            let pitch = rig.belt_len / rig.count as f32;
            if let Ok(mut tr) = views.get_mut(side.sprocket_view) {
                let angle = tooth_angle(
                    phases[si],
                    pitch,
                    rig.teeth,
                    side.sprocket_origin,
                    side.sprocket_tooth_tip,
                );
                tr.set_if_neq(gear_spin_transform(&side.sprocket_rest, angle));
            }
            if let Ok(mut tr) = views.get_mut(side.idler_view) {
                let angle = spin_angle(phases[si], side.idler.1);
                tr.set_if_neq(gear_spin_transform(&side.idler_rest, angle));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::link_view::LinkFrame;
    use super::*;

    /// Tank-part presentation consumes fresh view facts, then writes running gear before globals
    /// and belt upload consume it, and gives the ribbon its mesh before scope resolution reads it.
    /// Audit the production registrations without running a renderer; removing a required order
    /// must expose the conflicting pair or the missing path regardless of executor order.
    #[test]
    fn presented_tank_parts_have_ordered_producers_and_consumers() {
        use bevy::ecs::schedule::{LogLevel, NodeId, ScheduleBuildSettings, ScheduleLabel};

        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            TransformPlugin,
        ))
        .add_plugins((
            crate::view::plugin,
            crate::geometry_lod::view_plugin,
            crate::render_policy::plugin,
            view_plugin,
        ));
        for label in [Update.intern(), PostUpdate.intern()] {
            app.world_mut().schedule_scope(label, |world, schedule| {
                schedule.set_build_settings(ScheduleBuildSettings {
                    ambiguity_detection: LogLevel::Warn,
                    use_shortnames: true,
                    report_sets: false,
                    ..default()
                });
                schedule
                    .initialize(world)
                    .expect("presentation schedule builds");
                let graph = schedule.graph();
                let names: std::collections::HashMap<_, _> = schedule
                    .systems()
                    .expect("initialized schedule")
                    .map(|(key, system)| (key, system.name().shortname().to_string()))
                    .collect();
                for (a, b, _) in graph.conflicting_systems().iter() {
                    let (a, b) = (&names[a], &names[b]);
                    let pair = [a.as_str(), b.as_str()];
                    let contains = |name| pair.contains(&name);
                    assert!(
                        !(contains("bind_link_template") && contains("compose_view_profile")
                            || contains("drive_track_views")
                                && (contains("upload_belt_frames")
                                    || contains("mark_dirty_trees")
                                    || contains("propagate_parent_transforms")
                                    || contains("sync_simple_transforms"))),
                        "presented tank part has an unordered producer/consumer: {a} / {b}"
                    );
                }
                // Conflict detection rejects a missing edge; the built topological order also
                // rejects a reversed edge, where the consumer would read the previous value.
                let order: Vec<_> = schedule
                    .systems()
                    .expect("initialized schedule")
                    .map(|(_, system)| system.name().shortname().to_string())
                    .collect();
                let required: &[(&str, &str)] = if label == Update.intern() {
                    &[("compose_view_profile", "bind_link_template")]
                } else {
                    &[
                        ("drive_track_views", "upload_belt_frames"),
                        ("drive_track_views", "mark_dirty_trees"),
                        ("mark_dirty_trees", "propagate_parent_transforms"),
                        ("propagate_parent_transforms", "sync_simple_transforms"),
                    ]
                };
                for &(producer, consumer) in required {
                    let index = |name| order.iter().position(|system| system == name).expect(name);
                    assert!(
                        index(producer) < index(consumer),
                        "{producer} must produce this frame's value before {consumer} reads it"
                    );
                }
                // The ribbon's `Mesh3d` lands by command, which no access conflict sees, and the
                // flattened order above leaves two unordered systems in either order. A path of
                // ordering edges from `TrackViewSet` to `resolve_visual_scopes` (through the
                // sets' members: the system joins `ScopeResolution` by hierarchy, the order edge
                // targets the set) is what puts the ribbon on the shadow-only channel in its first
                // frame, instead of the camera drawing it on the default one.
                if label == PostUpdate.intern() {
                    let system = |name: &str| {
                        schedule
                            .systems()
                            .expect("initialized schedule")
                            .find(|(_, system)| system.name().shortname().to_string() == name)
                            .map(|(key, _)| NodeId::System(key))
                            .expect(name)
                    };
                    let from = graph
                        .system_sets
                        .get_key(TrackViewSet.intern())
                        .map(NodeId::Set)
                        .expect("TrackViewSet");
                    let to = system("resolve_visual_scopes");
                    let reached = super::super::schedule_path::orders(graph, from, to);
                    assert!(
                        reached,
                        "the caster swap must be ordered before scope resolution"
                    );
                }
            });
        }
    }

    /// The shipped rig's construction world (`bake` and `sim` plugins, `Startup` and `PreUpdate`
    /// only: a presentation fixture has no physics or asset-loader clock) and its measured
    /// running gear. Fixture data: no pass condition reads a count from it.
    fn construction_app() -> (App, RigGeom) {
        let mut app = App::new();
        app.add_plugins((crate::bake::plugin, super::super::sim::sim_plugin));
        app.world_mut().run_schedule(Startup);
        app.world_mut().run_schedule(PreUpdate);
        let geom = app.world().resource::<RigGeom>().clone();
        (app, geom)
    }

    /// A rig on `geom` with its wheel, sprocket, idler and belt entities spawned, as
    /// `bind_track_rigs` leaves it: no shadow proxy yet.
    fn fixture_rig(world: &mut World, geom: &RigGeom) -> TrackRig {
        let sides = [Side::Left, Side::Right].map(|side| {
            let circles = geom.rest.get(side);
            let plane_x = geom.plane_x * if side == Side::Left { -1.0 } else { 1.0 };
            let spawn_pose = |world: &mut World| world.spawn(Transform::IDENTITY).id();
            RigSide {
                plane_x,
                lateral_stations: geom.grip_stations(side),
                sprocket: circles[0],
                idler: *circles.last().unwrap(),
                wheels: circles[1..circles.len() - 1]
                    .iter()
                    .map(|&(center, radius)| RigWheel {
                        pivot: Vec3::new(plane_x, center.y, center.x),
                        rest_rotation: Quat::IDENTITY,
                        radius,
                        dy: 0.0,
                        dvel: 0.0,
                        target: Default::default(),
                        view: spawn_pose(world),
                    })
                    .collect(),
                sprocket_view: spawn_pose(world),
                sprocket_rest: Transform::IDENTITY,
                sprocket_origin: geom.belt_origin_angle(side),
                sprocket_tooth_tip: 0.0,
                idler_view: spawn_pose(world),
                idler_rest: Transform::IDENTITY,
                belt: world
                    .spawn((
                        Transform::IDENTITY,
                        BeltFrames::new(
                            LinkFrame::default(),
                            geom.link_center_x(side),
                            geom.link_count,
                        ),
                    ))
                    .id(),
                link_center_x: geom.link_center_x(side),
                proxy: None,
            }
        });
        TrackRig {
            belt_len: geom.belt_len(),
            count: geom.link_count,
            teeth: geom.teeth,
            pin_to_inner: geom.model.pin_to_inner,
            thickness: geom.thickness,
            wrap: Default::default(),
            presented_phase: [None; 2],
            sides,
            prev_affine: None,
            field_revision: None,
            fit_dependencies: None,
            contact_clock: Default::default(),
            fit_affine: None,
            contacts: Default::default(),
        }
    }

    /// One side's ribbon section, from the measured shoe.
    fn fixture_section(geom: &RigGeom, side: Side) -> shadow_proxy::Section {
        shadow_proxy::Section::from_shoe(
            geom.link_center_x(side),
            geom.model.width,
            geom.model.pin_to_inner,
            geom.model.pin_to_outer,
        )
    }

    /// Invariant: an exactly settled, standing tank writes no belt table, wheel transform or ribbon
    /// asset, and a moving belt writes its tables and NO per-link transform: the only transforms
    /// the view writes are the running gear's. Transform propagation, the belt upload and render
    /// extraction rely on both. The shipped rig is fixture data; the pass conditions are generic and
    /// independent of its counts. ASSUMED fixture cadence: stationary warmup followed by repeated
    /// identical presentation frames.
    #[test]
    fn settled_standing_tank_writes_no_belt_wheel_or_ribbon_data() {
        #[derive(Resource, Default)]
        struct Writes {
            transforms: Vec<Entity>,
            tables: Vec<Entity>,
            meshes: bool,
        }
        fn count_writes(
            mut writes: ResMut<Writes>,
            poses: Query<Entity, (Changed<Transform>, Without<TrackRig>)>,
            tables: Query<Entity, Changed<BeltFrames>>,
            meshes: Res<Assets<Mesh>>,
        ) {
            writes.transforms = poses.iter().collect();
            writes.tables = tables.iter().collect();
            writes.meshes = meshes.is_changed();
        }
        let (mut app, geom) = construction_app();
        let world = app.world_mut();
        let mut rig = fixture_rig(world, &geom);
        for (side, rig_side) in [Side::Left, Side::Right].into_iter().zip(&mut rig.sides) {
            let entity = world.spawn(Transform::IDENTITY).id();
            rig_side.proxy = Some(ProxySide::new(entity, fixture_section(&geom, side)));
        }
        let tank = world
            .spawn((
                Transform::from_translation(Vec3::Y * geom.hull_rest_y),
                TrackDrive::default(),
                rig,
            ))
            .id();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(ProxyMode::Dynamic);
        world.insert_resource(TrackField {
            revision: Some(0),
            field: Some(print_ground(vec![])),
        });
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Time::<Fixed>::default());
        world.init_resource::<Writes>();
        let mut schedule = Schedule::default();
        schedule.add_systems((drive_track_views, count_writes).chain());
        let dt = std::time::Duration::from_secs_f64(1.0 / 128.0);
        for frame in 0..1024 {
            world.resource_mut::<Time>().advance_by(dt);
            world.resource_mut::<Time<Fixed>>().advance_by(dt);
            schedule.run(world);
            let writes = world.resource::<Writes>();
            if frame == 0 {
                assert!(
                    !writes.transforms.is_empty() && writes.tables.len() == 2 && writes.meshes,
                    "cold placement must write"
                );
            }
            if frame >= 768 {
                assert!(
                    writes.transforms.is_empty(),
                    "settled pose write on frame {frame}"
                );
                assert!(
                    writes.tables.is_empty(),
                    "settled belt table write on frame {frame}"
                );
                assert!(
                    !writes.meshes,
                    "settled ribbon asset write on frame {frame}"
                );
            }
        }
        // A phase change must wake every live output path again, even on an unchanged hull.
        for side in &mut world.get_mut::<TrackDrive>(tank).unwrap().sides {
            side.phase += 0.03; // ASSUMED fixture travel in metres.
        }
        schedule.run(world);
        let rig = world.get::<TrackRig>(tank).unwrap();
        let gear: Vec<Entity> = rig
            .sides
            .iter()
            .flat_map(|side| {
                side.wheels
                    .iter()
                    .map(|wheel| wheel.view)
                    .chain([side.sprocket_view, side.idler_view])
            })
            .collect();
        let belts: Vec<Entity> = rig.sides.iter().map(|side| side.belt).collect();
        let writes = world.resource::<Writes>();
        assert!(
            !writes.transforms.is_empty() && writes.meshes,
            "belt motion must resume writes"
        );
        assert!(
            writes.transforms.iter().all(|moved| gear.contains(moved)),
            "the view wrote a transform that is not a wheel, sprocket or idler: a belt's shoes move \
             through its table, never through a per-link transform"
        );
        let mut tables = writes.tables.clone();
        tables.sort();
        let mut expected = belts;
        expected.sort();
        assert_eq!(tables, expected, "a moving belt writes both its tables");
    }

    /// Invariant: no empty mesh reaches a render entity. On every frame of a bound rig's life,
    /// the frames before its first fit included, every `Mesh3d` names a mesh with positions and
    /// vertices, no mesh asset is empty, and a ribbon gets its `Mesh3d` on the frame its belt is
    /// silenced, never apart. The renderer relies on the first two (why:
    /// [`shadow_proxy::ProxySide::mesh`]); the caster swap relies on the third. The late fit here is a replica bound before its
    /// interpolated `TrackDrive` is visible (`drive_track_views` skips a tank without one), the
    /// route ASSUMED for the client's quits on a respawn wave; the frame count is ASSUMED, the rig
    /// is fixture data, and no pass condition reads its counts.
    #[test]
    fn no_empty_mesh_reaches_a_render_entity_before_or_after_the_first_fit() {
        /// Frames the fixture runs before the belt state lands. ASSUMED.
        const LATE_FRAMES: usize = 5;
        let (mut app, geom) = construction_app();
        let world = app.world_mut();
        let rig = fixture_rig(world, &geom);
        let belts: Vec<Entity> = rig.sides.iter().map(|side| side.belt).collect();
        let tank = world
            .spawn(Transform::from_translation(Vec3::Y * geom.hull_rest_y))
            .id();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<StandardMaterial>::default());
        world.insert_resource(ProxyMode::Dynamic);
        world.insert_resource(TrackField {
            revision: Some(0),
            field: Some(print_ground(vec![])),
        });
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Time::<Fixed>::default());
        let mut schedule = Schedule::default();
        schedule.add_systems((attach_shadow_proxies, drive_track_views).chain());
        world.entity_mut(tank).insert(rig);
        let dt = std::time::Duration::from_secs_f64(1.0 / 128.0);
        for frame in 0..LATE_FRAMES + 3 {
            if frame == LATE_FRAMES {
                world.entity_mut(tank).insert(TrackDrive::default());
            }
            world.resource_mut::<Time>().advance_by(dt);
            world.resource_mut::<Time<Fixed>>().advance_by(dt);
            schedule.run(world);

            let drawn: Vec<Handle<Mesh>> = world
                .query::<&Mesh3d>()
                .iter(world)
                .map(|mesh| mesh.0.clone())
                .collect();
            let meshes = world.resource::<Assets<Mesh>>();
            let drawable = |mesh: &Mesh| {
                mesh.attribute(Mesh::ATTRIBUTE_POSITION).is_some() && mesh.count_vertices() > 0
            };
            assert!(
                meshes.iter().all(|(_, mesh)| drawable(mesh)),
                "an empty mesh asset on frame {frame}"
            );
            for mesh in &drawn {
                assert!(
                    meshes.get(mesh).is_some_and(drawable),
                    "a Mesh3d names an empty or absent mesh on frame {frame}"
                );
            }
            let rig = world.get::<TrackRig>(tank).expect("the rig");
            for (side, &belt) in rig.sides.iter().zip(&belts) {
                let proxy = side.proxy.as_ref().expect("attached on the bind frame");
                let ribbon = world.get::<Mesh3d>(proxy.entity).map(|mesh| mesh.id());
                let silenced = world.get::<VisualScope>(belt) == Some(&VisualScope::PROXIED_CASTER);
                assert_eq!(
                    ribbon.is_some(),
                    frame >= LATE_FRAMES,
                    "a ribbon has a mesh from the frame its first fit lands, frame {frame}"
                );
                assert_eq!(
                    ribbon.is_some(),
                    silenced,
                    "the ribbon's mesh and its silenced shoes arrive together, frame {frame}"
                );
                assert_eq!(ribbon, proxy.mesh.as_ref().map(Handle::id));
            }
        }
    }

    /// Invariant: once pose, terrain/print inputs and wheel articulation settle, the view emits
    /// no fit rays. Match-scale rendering relies on that reuse; shoes, phase and the filters must
    /// still advance. ASSUMED fixture geometry and cadence below are independent of tank assets.
    #[test]
    fn settled_tank_fit_casts_zero_rays_while_phase_and_filters_continue() {
        use std::cell::Cell;
        struct Counted {
            field: BlockField,
            rays: Cell<usize>,
        }
        impl TerrainOracle for Counted {
            fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
                self.rays.set(self.rays.get() + 1);
                self.field.depth_along(station, out, reach)
            }
        }
        let field = Counted {
            field: print_ground(vec![super::super::oracle::TerrainBlock::new(
                Vec3::new(0.0, 0.03, 0.8),
                Quat::IDENTITY,
                Vec3::new(3.0, 0.06, 0.2),
            )]),
            rays: Cell::new(0),
        };
        let affine = Affine3A::from_translation(Vec3::Y * 0.5);
        let params = WheelParams {
            reach: 0.5,
            ease_omega: WHEEL_LIFT_RISE_OMEGA,
            max_lift: 0.2,
            max_droop: 0.15,
            lateral_stations: WHEEL_DISC_STATIONS,
            probe_reach: PROBE_REACH,
        };
        let mut target = ContactTarget::default();
        let (mut dy, mut dvel) = (0.0, 0.0);
        let mut states = Default::default();
        let mut reference_states = Default::default();
        let mut contacts = Default::default();
        let mut first_joints = Vec::new();
        for frame in 0..768 {
            let refresh = frame % 2 == 0;
            let alpha = if refresh { 0.0 } else { 0.5 };
            let lift = target.sample(refresh, false, alpha, || {
                wheel_lift_target(&field, &affine, Vec3::NEG_Y, Vec3::ZERO, &params)
            });
            wheel_lift_step(&mut dy, &mut dvel, lift, 1.0 / 128.0, &params);
            let circles = [
                (Vec2::new(2.0, 0.0), 0.5),
                (Vec2::new(0.0, dy), 0.48),
                (Vec2::new(-2.0, 0.0), 0.5),
            ];
            let input = wrap::WrapInput {
                dt: 1.0 / 128.0,
                affine,
                belt_len: 11.5,
                count: 80,
                pitch: 11.5 / 80.0,
                thickness: 0.04,
                probe_reach: PROBE_REACH,
                reference: false,
                sides: [-1.0, 1.0].map(|plane_x| wrap::WrapSideInput {
                    circles: &circles,
                    plane_x,
                    lateral_stations: [-0.2, 0.0, 0.2],
                    phase: frame as f64 * 0.001,
                }),
            };
            let out = wrap::step_cached(&input, &field, &mut states, &mut contacts, refresh, alpha);
            if frame == 511 {
                assert!(
                    field.rays.get() > 0,
                    "the cold fit must exercise the exact oracle"
                );
                field.rays.set(0);
                first_joints = out[0].joints.clone();
            }
            if frame >= 512 {
                assert_eq!(
                    field.rays.get(),
                    0,
                    "settled view fit recast on frame {frame}"
                );
                // The reference's own probes are excluded from the cache's ray count.
                let raw = wrap::step(&input, &field.field, &mut reference_states);
                if frame == 767 {
                    assert_ne!(first_joints, out[0].joints, "phase must keep scrolling");
                    for si in 0..2 {
                        for (a, b) in out[si].joints.iter().zip(&raw[si].joints) {
                            assert!(
                                a.distance(*b) < 1e-5,
                                "parked shape must match the exact fit"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Invariant: a moving wheel refreshes only on the sim clock, interpolates its raw target,
    /// and finishes the blend when it stops. Render-rate wheel filtering relies on continuous
    /// targets; a cold/reseeded contact must adopt the exact probe immediately.
    #[test]
    fn moving_contact_targets_interpolate_without_render_frame_probes() {
        let mut target = ContactTarget::default();
        assert_eq!(target.sample(false, false, 0.5, || 0.0), 0.0);
        assert_eq!(target.sample(true, true, 0.0, || 0.1), 0.0);
        assert_eq!(
            target.sample(false, true, 0.5, || panic!("between ticks")),
            0.05
        );
        assert_eq!(
            target.sample(true, false, 0.0, || panic!("unchanged pose")),
            0.1
        );
        assert_eq!(target.sample(false, false, 0.5, || panic!("settled")), 0.1);
    }

    /// Invariant: moving wrap probes follow the sim clock even as render geometry articulates;
    /// a terrain/print reseed must discard old targets immediately. Belt clearance and print
    /// consumers rely on that boundary. ASSUMED synthetic geometry/cadence, independent of assets.
    #[test]
    fn moving_wrap_contacts_wait_for_ticks_and_reseed_without_stale_depths() {
        use std::cell::Cell;
        struct Counted(Cell<usize>);
        impl TerrainOracle for Counted {
            fn depth_along(&self, station: Vec3, out: Vec3, _: f32) -> f32 {
                self.0.set(self.0.get() + 1);
                if out.y < -0.2 {
                    -station.y / -out.y
                } else {
                    0.0
                }
            }
        }
        let oracle = Counted(Cell::new(0));
        let mut circles = [
            (Vec2::new(2.0, 0.0), 0.5),
            (Vec2::ZERO, 0.48),
            (Vec2::new(-2.0, 0.0), 0.5),
        ];
        let mut states = Default::default();
        let mut contacts = Default::default();
        fn input(circles: &[(Vec2, f32)], affine: Affine3A) -> wrap::WrapInput<'_> {
            wrap::WrapInput {
                dt: 1.0 / 128.0,
                affine,
                belt_len: 11.5,
                count: 80,
                pitch: 11.5 / 80.0,
                thickness: 0.04,
                probe_reach: PROBE_REACH,
                reference: false,
                sides: [-1.0, 1.0].map(|plane_x| wrap::WrapSideInput {
                    circles,
                    plane_x,
                    lateral_stations: [-0.2, 0.0, 0.2],
                    phase: 0.0,
                }),
            }
        }
        let start = Affine3A::from_translation(Vec3::Y * 0.5);
        wrap::step_cached(
            &input(&circles, start),
            &oracle,
            &mut states,
            &mut contacts,
            true,
            0.0,
        );
        assert!(oracle.0.replace(0) > 0);
        // Articulation can change the route's station count between contact samples.
        circles[1].0.y = -0.1;
        let moved = Affine3A::from_translation(Vec3::new(0.0, 0.45, 0.02));
        wrap::step_cached(
            &input(&circles, moved),
            &oracle,
            &mut states,
            &mut contacts,
            false,
            0.5,
        );
        assert_eq!(
            oracle.0.get(),
            0,
            "a render frame must reuse the sampled depth profiles"
        );
        wrap::step_cached(
            &input(&circles, moved),
            &oracle,
            &mut states,
            &mut contacts,
            true,
            0.0,
        );
        assert!(
            oracle.0.replace(0) > 0,
            "the next sim tick must refresh changed contacts"
        );
        wrap::step_cached(
            &input(&circles, moved),
            &oracle,
            &mut states,
            &mut contacts,
            false,
            0.5,
        );
        assert_eq!(oracle.0.get(), 0);
        let frame = input(&circles, moved);
        let reseeded = wrap::step_cached(
            &frame,
            &oracle,
            &mut Default::default(),
            &mut Default::default(),
            false,
            0.0,
        );
        assert!(
            oracle.0.get() > 0,
            "invalidated targets refresh even between ticks"
        );
        let exact = wrap::step(&frame, &oracle, &mut Default::default());
        for si in 0..2 {
            assert_eq!(
                reseeded[si].joints, exact[si].joints,
                "reseed must adopt exact contacts"
            );
        }
    }

    /// Invariant: the belt follows the ground at most one tick late, the product cost #155
    /// promises, even when one render frame coalesces several fixed ticks. A cursor that trails
    /// the previous render frame instead of one tick breaks it. ASSUMED cadences: 32 fps (two
    /// ticks a frame), 20 fps (three or four) and a hitching 5/47/11 ms cycle (none to three).
    #[test]
    fn contact_targets_lag_at_most_one_tick_below_tick_rate() {
        for cycle in [&[1.0 / 32.0][..], &[1.0 / 20.0], &[0.005, 0.047, 0.011]] {
            follow_rising_ground(&render_frames(cycle));
        }
    }

    /// Invariant: above the tick rate consecutive samples are one tick apart, so the blend weight
    /// is Bevy's overstep fraction bit for bit and the targets stay at most one tick late. The
    /// committed track-fit stills rely on that identity at their 128 Hz capture step. ASSUMED
    /// cadences: 120 and 128 fps.
    #[test]
    fn contact_targets_lag_at_most_one_tick_above_tick_rate() {
        for cycle in [&[1.0 / 120.0][..], &[1.0 / 128.0]] {
            let frames = render_frames(cycle);
            let alphas = follow_rising_ground(&frames);
            for (fixed, alpha) in frames.iter().zip(alphas) {
                // Before the first tick the clock holds one sample, which is the whole blend.
                if fixed.elapsed() > std::time::Duration::ZERO {
                    assert_eq!(
                        alpha.to_bits(),
                        fixed.overstep_fraction().to_bits(),
                        "blend weight at {:?} + {:?}",
                        fixed.elapsed(),
                        fixed.overstep()
                    );
                }
            }
        }
    }

    /// One second (ASSUMED span) of render frames cycling the given frame durations in seconds,
    /// each paired with the 64 Hz fixed clock as Bevy's `run_fixed_main_schedule` leaves it:
    /// whole timesteps expended into `elapsed`, the remainder kept as overstep.
    fn render_frames(cycle: &[f64]) -> Vec<Time<Fixed>> {
        let mut fixed = Time::<Fixed>::default();
        let mut frames = vec![fixed];
        for &frame_s in cycle.iter().cycle() {
            fixed.accumulate_overstep(std::time::Duration::from_secs_f64(frame_s));
            while fixed.overstep() >= fixed.timestep() {
                let step = fixed.timestep();
                fixed.discard_overstep(step);
                fixed.advance_by(step);
            }
            if (fixed.elapsed() + fixed.overstep()).as_secs_f64() > 1.0 {
                return frames;
            }
            frames.push(fixed);
        }
        unreachable!("a non-empty cycle repeats until the span ends")
    }

    /// Runs a wheel target and both wrap contact caches through `frames` on one rig clock, over
    /// ground that rises uniformly with presented time (`elapsed + overstep`): each blended target
    /// then decodes to the presented time of the pose it stands for. Asserts every target's age
    /// is at most one tick and returns each frame's blend weight. ASSUMED synthetic geometry,
    /// independent of tank assets.
    fn follow_rising_ground(frames: &[Time<Fixed>]) -> Vec<f32> {
        use std::cell::Cell;
        struct Rising(Cell<f32>);
        impl TerrainOracle for Rising {
            fn depth_along(&self, _: Vec3, _: Vec3, _: f32) -> f32 {
                self.0.get()
            }
        }
        // ASSUMED rise rate (m/s): one second of it stays inside the probe reach.
        const RISE: f32 = 0.1;
        // ASSUMED decoding tolerance (s): f32 rounding, far below the extra tick of a late cursor.
        const TOLERANCE_S: f32 = 1e-5;
        // Front to rear is increasing z, so the bottom run's outward normals face the ground.
        let circles = [
            (Vec2::new(-2.0, 0.0), 0.5),
            (Vec2::ZERO, 0.48),
            (Vec2::new(2.0, 0.0), 0.5),
        ];
        let ground = Rising(Cell::new(0.0));
        let mut clock = ContactClock::default();
        let mut wheel = ContactTarget::default();
        let mut states: [wrap::WrapState; 2] = Default::default();
        let mut contacts = Default::default();
        frames
            .iter()
            .map(|fixed| {
                let presented = (fixed.elapsed() + fixed.overstep()).as_secs_f32();
                ground.0.set(RISE * presented);
                let (refresh, alpha) = clock.advance(fixed);
                let lift = wheel.sample(refresh, true, alpha, || RISE * presented);
                let input = wrap::WrapInput {
                    // ASSUMED filter step: rising ground takes the belly ease's instant rise.
                    dt: 1.0 / 64.0,
                    affine: Affine3A::from_translation(Vec3::Z * presented),
                    belt_len: 11.5,
                    count: 80,
                    pitch: 11.5 / 80.0,
                    thickness: 0.04,
                    probe_reach: PROBE_REACH,
                    reference: false,
                    sides: [-1.0, 1.0].map(|plane_x| wrap::WrapSideInput {
                        circles: &circles,
                        plane_x,
                        lateral_stations: [-0.2, 0.0, 0.2],
                        phase: 0.0,
                    }),
                };
                wrap::step_cached(&input, &ground, &mut states, &mut contacts, refresh, alpha);
                let tick = fixed.timestep().as_secs_f32();
                // Mid-span between the road wheel and the idler: a ground-facing belly station.
                let belly = [0, 1].map(|si| states[si].eased_depth(1.0));
                for (what, target) in [
                    ("wheel", lift),
                    ("left belly", belly[0]),
                    ("right belly", belly[1]),
                ] {
                    let age = presented - target / RISE;
                    assert!(
                        age <= tick + TOLERANCE_S,
                        "{what} target {age} s behind the presented pose at {presented} s; \
                         one tick is {tick} s"
                    );
                }
                alpha
            })
            .collect()
    }

    /// A flat heightmap at zero with optional raised blocks, synthetic fixture geometry.
    fn print_ground(blocks: Vec<super::super::oracle::TerrainBlock>) -> BlockField {
        let grid = crate::terrain_grid::HeightGrid::new(
            vec![0.0; 9].into(),
            3,
            crate::terrain_grid::TerrainExtent {
                world_size_m: 20.0,
                height_offset_m: 0.0,
                height_span_m: 1.0,
            },
        );
        BlockField::new(blocks).with_height(Some(grid))
    }

    /// The view's terrain probes must sink by the ground beneath each belt, bounded by suspension
    /// droop. A hull-centre lookup gives both sides the same depth at a ground-type boundary; an
    /// unbounded offset lets the wrap conform below the wheels' reachable ground.
    #[test]
    fn belt_sink_samples_each_probe_and_stops_at_droop_travel() {
        let ground = print_ground(Vec::new());
        let sunk = Sunk {
            ground: &ground,
            depth_m: |xz: Vec2| if xz.x < 0.0 { 0.02 } else { 0.15 },
            max_sink_m: 0.06, // Synthetic suspension travel, m.
        };
        for (x, expected) in [(-1.0, -0.02), (1.0, -0.06)] {
            let depth = sunk.depth_along(Vec3::new(x, 0.0, 0.0), Vec3::NEG_Y, 0.5);
            assert!(
                (depth - expected).abs() < 1e-6,
                "probe at x={x}: its own ground depth, capped by droop: {depth}"
            );
        }
    }

    /// Cosmetic print depth belongs to the heightmap's terrain shader. The belt must retain
    /// contact with an obstacle's unprinted top while it sinks into soil beside it; translating
    /// the whole oracle instead makes the wheels and wrap visibly enter that solid obstacle.
    #[test]
    fn belt_sink_lowers_printed_terrain_and_leaves_obstacles_at_their_drawn_height() {
        let ground = print_ground(vec![super::super::oracle::TerrainBlock::new(
            Vec3::Y * 0.5,
            Quat::IDENTITY,
            Vec3::ONE,
        )]);
        let sunk = Sunk {
            ground: &ground,
            depth_m: |_| 0.06,
            max_sink_m: 0.1,
        };
        let obstacle = Vec3::Y;
        assert_eq!(
            sunk.depth_along(obstacle, Vec3::NEG_Y, 0.5),
            ground.depth_along(obstacle, Vec3::NEG_Y, 0.5),
            "the obstacle's top never sinks"
        );
        let soil = Vec3::X * 2.0;
        assert!((sunk.depth_along(soil, Vec3::NEG_Y, 0.5) + 0.06).abs() < 1e-6);
        assert_eq!(
            ground.depth_along(soil, Vec3::NEG_Y, 0.5),
            0.0,
            "sim ground"
        );
    }

    /// A 120 fps render frame — faster than the 64 Hz sample rate, which is the whole problem.
    const DT: f32 = 1.0 / 120.0;
    const SPEED: f32 = 4.0;

    /// Steady drive: `sim` moves once every `gap` frames by exactly the travel of those frames —
    /// the staircase a fixed-tick sim (or a replication stream) hands the renderer. Returns the
    /// worst per-frame scroll RATE error, in m/s, over the settled tail: raw, then presented.
    fn worst_rate_error(gap: u32) -> (f64, f64) {
        let step = f64::from(SPEED) * f64::from(DT) * f64::from(gap);
        let (mut sim, mut drawn) = (0.0_f64, presented_phase(None, 0.0, SPEED, DT));
        let (mut raw, mut presented) = (0.0_f64, 0.0_f64);
        for frame in 1..2000 {
            let (was_sim, was_drawn) = (sim, drawn);
            if frame % gap == 0 {
                sim += step;
            }
            drawn = presented_phase(Some(drawn), sim, SPEED, DT);
            if frame > 1000 {
                let rate = |d: f64| (d / f64::from(DT) - f64::from(SPEED)).abs();
                raw = raw.max(rate(sim - was_sim));
                presented = presented.max(rate(drawn - was_drawn));
            }
        }
        (raw, presented)
    }

    /// THE artefact. Drawing the sim phase directly stalls the belt for every frame no sample
    /// reached and lurches it on the one that did — an error of the belt's whole speed, growing
    /// with the gap. The carry holds the belt's own speed across the same gaps.
    #[test]
    fn the_drawn_scroll_rate_survives_the_gaps_between_samples() {
        // MEASURED ripple at ω = 4: 0.067 / 0.202 / 0.482 m/s. These bounds sit half again above
        // that, so doubling ω fails all three rather than sneaking one through.
        for (gap, tolerance) in [(2, 0.10), (4, 0.30), (8, 0.70)] {
            let (raw, presented) = worst_rate_error(gap);
            // A stalled frame alone is already the full belt speed out.
            assert!(
                raw >= f64::from(SPEED) - 1e-9,
                "gap {gap}: raw error {raw} should stall a whole {SPEED} m/s",
            );
            assert!(
                presented < tolerance,
                "gap {gap}: presented error {presented} exceeds {tolerance} m/s",
            );
            // And the point of the exercise: the drawn rate is orders better, not marginally.
            assert!(
                presented * 10.0 < raw,
                "gap {gap}: presented {presented} vs raw {raw}",
            );
        }
    }

    /// The ease is what keeps the carry from walking away from the ground — a belly that scrubs is
    /// the failure a left-open residual causes. A whole metre of it must be down to 1 % inside two
    /// seconds, which is the lower bound on [`PHASE_HEAL_OMEGA`]: halve the rate and this fails.
    #[test]
    fn the_heal_closes_a_residual_the_carry_cannot() {
        let mut phase = 0.0;
        let mut sim = 1.0;
        for _ in 0..(2.0 / DT) as u32 {
            sim += f64::from(SPEED) * f64::from(DT);
            phase = presented_phase(Some(phase), sim, SPEED, DT);
        }
        assert!(
            (sim - phase).abs() < 0.01,
            "a 1 m residual must be 99 % gone in two seconds, not {}",
            sim - phase,
        );
    }

    /// The carry is what makes the ease ZERO-LAG at speed, and nothing above pins it: a plain
    /// low-pass on the phase draws just as smoothly, sitting a constant `speed / ω` — a whole metre
    /// of belt at cruise — behind, so the tracks would spin up a quarter-second late and keep
    /// scrolling after the tank stopped. Steady drive must settle onto the sim phase ITSELF.
    #[test]
    fn a_steady_belt_draws_no_lag_behind_the_sim_phase() {
        let mut sim = 0.0_f64;
        let mut drawn = presented_phase(None, sim, SPEED, DT);
        for _ in 0..2000 {
            sim += f64::from(SPEED) * f64::from(DT);
            drawn = presented_phase(Some(drawn), sim, SPEED, DT);
        }
        assert!(
            (sim - drawn).abs() < 0.001,
            "steady drive must draw the sim phase, not {} m behind it",
            sim - drawn,
        );
    }

    /// A replica's phase arrives wrapped (`sim::BELT_PHASE_WRAP_M`), so where the authority's
    /// crosses a whole wrap the replica's jumps back by one. The drawn belt must not see it: fed
    /// the wrapped phase, it moves frame by frame exactly as it does fed the unbounded one. A
    /// residual taken without the wrap would scroll a kilometre of belt through the heal.
    #[test]
    fn a_wrapped_replica_phase_draws_the_unbounded_belt() {
        let wrap = crate::track::sim::BELT_PHASE_WRAP_M;
        let mut sim = wrap - 3.0;
        let mut unbounded = presented_phase(None, sim, SPEED, DT);
        let mut wrapped = presented_phase(None, sim.rem_euclid(wrap), SPEED, DT);
        let mut crossed = false;
        for frame in 1..600 {
            if frame % 2 == 0 {
                sim += f64::from(SPEED) * 2.0 * f64::from(DT);
            }
            crossed |= sim >= wrap;
            let (was_unbounded, was_wrapped) = (unbounded, wrapped);
            unbounded = presented_phase(Some(unbounded), sim, SPEED, DT);
            wrapped = presented_phase(Some(wrapped), sim.rem_euclid(wrap), SPEED, DT);
            assert!(
                ((unbounded - was_unbounded) - (wrapped - was_wrapped)).abs() < 1e-9,
                "frame {frame}: the wrapped feed drew a different step",
            );
        }
        assert!(crossed, "the run crosses the wrap");
    }

    /// Reseeding is the escape hatch for every discontinuity the ease is too slow for, so it must
    /// land exactly on the sim phase rather than merely near it.
    #[test]
    fn a_reseed_lands_exactly_on_the_sim_phase() {
        let drifted = presented_phase(Some(0.0), 1000.0, SPEED, DT);
        assert_ne!(drifted, 1000.0);
        assert_eq!(presented_phase(None, 1000.0, SPEED, DT), 1000.0);
    }
}
