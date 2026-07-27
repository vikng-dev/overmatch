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
//! spring), always on. The filter state is client-local cosmetic memory, so a remote tank's drawn
//! belt is not a pure function of replicated pose + phase — which is fine, it is view-layer juice,
//! and the pins/wheels/gear spin still derive from replicated state alone. It is cheap enough
//! (~56 µs/tank/frame) that every tank gets it — no tier policy (architecture §6); tiers by
//! projected link pitch return only if tank counts ever demand them.

use avian3d::prelude::PhysicsSystems;
use bevy::math::{Affine3A, Vec2};
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use crate::bake::TankBlueprint;
use crate::tank::{Roadwheel, Tank, TrackSide, ViewNode};

use super::gear_phase::{gear_spin_transform, spin_angle, sprocket_tooth_tip, tooth_angle};
use super::link_view::{self, LinkFrame, LinkTemplate};
use super::rig_geom::RigGeom;
use super::side::Side;
use super::sim::TrackDrive;
use super::terrain::TrackField;
use super::wheels::{WHEEL_LIFT_RISE_OMEGA, WheelParams, wheel_lift_step, wheel_lift_target};
use super::wrap;

/// Ordering owner for the track view's presented-pose read: after physics writeback (Avian has
/// written the frame's root `Transform`, interpolated in SP, wire/frame-interpolated under
/// netcode), before propagation carries the written view poses out. The net client additionally
/// orders this set after its rollback-correction smoothing (`RenderErrorApplied`) — that edge
/// lives in `net::render_error`, which owns the set, because the net-boundary guard keeps this
/// module from naming the netcode (same inversion as `camera::OrbitCameraSet`).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackViewSet;

pub fn view_plugin(app: &mut App) {
    app.add_observer(rebind_on_reinstance);
    // The shoe template: one read of the glb's `Link` node, and the every-frame `Added<Name>` scan
    // that HIDES it (and `Link_Box`). Shared verbatim with the sandbox — the game rendered
    // procedural boxes and hid neither node until 2026-07-26, so every session drew a stray white
    // marker box and a loose shoe parked beside the hull.
    app.add_plugins(link_view::template_plugin);
    app.add_systems(Update, bind_track_rigs);
    app.configure_sets(
        PostUpdate,
        TrackViewSet
            .after(PhysicsSystems::Writeback)
            .before(TransformSystems::Propagate),
    );
    app.add_systems(PostUpdate, drive_track_views.in_set(TrackViewSet));
}

/// Downward terrain-probe reach (m) — for the wrap's conform and the view wheel lift.
const PROBE_REACH: f32 = 0.5;
/// Presented-pose discontinuity thresholds: a root that moves further than this in ONE frame is
/// a teleport/respawn/snap-correction, not motion — reset the wrap filters and the wheel-lift
/// state. `render_error` publishes no signal (it consumes oversized corrections silently), so the
/// view detects locally: works identically in SP and MP, no netcode coupling. 60 km/h at 30 fps is
/// 0.56 m/frame — half the trip threshold. MUST stay below `render_error`'s snap thresholds
/// (2 m / 60°) so every unsmoothed correction trips this too — `net::render_error` pins that
/// bracket in a test. `pub(crate)` for exactly that test.
pub(crate) const SNAP_TRANSLATION: f32 = 1.2;
/// Axis chord per frame (~30°), checked on BOTH the forward and up axes — a pure roll leaves
/// forward unchanged.
pub(crate) const SNAP_AXIS: f32 = 0.5;
/// Wheel-lift probe stations across the DISC (m from the wheel's real x) — the Tiger's
/// interleaved discs are 0.158 m wide, far narrower than the shoe: probing shoe-wide at an
/// outboard wheel column would read geometry entirely beside the track (codex finding C).
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
    sides: [RigSide; 2],
    /// Last frame's presented affine — the snap detector's previous frame. `None` = cold start.
    prev_affine: Option<Affine3A>,
    field_revision: Option<u64>,
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
    /// One entity per link, children of the tank root, local transforms in hull space. Real
    /// instanced SHOES ([`super::link_view`]) — the same template, placer and mirrored left-side
    /// mesh the sandbox draws, so the two tools cannot disagree about what this track looks like.
    links: Vec<Entity>,
    /// This side's mesh→canonical-pin-frame correction, captured from the template at bind (it is
    /// `Copy`, and the template is immutable) so the per-frame placer never touches the resource.
    link_frame: LinkFrame,
    /// The shoe's own lateral centre on this side (`RigGeom::link_center_x`) — NOT `plane_x`: the
    /// Tiger's shoe is authored ~16.85 mm outboard of the pin plane, and anchoring it on the pin
    /// plane would lose the authored overhang.
    link_center_x: f32,
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
    /// The GLB view node lift + spin write to (never the sim entity).
    view: Entity,
}

/// A GLB re-instantiation (hot reload during authoring) replaces the wheel/sprocket view
/// entities a bound rig points at: drop the rig and its links, and `bind_track_rigs` rebinds
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
        for &link in &side.links {
            commands.entity(link).despawn();
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
    meshes: Res<Assets<Mesh>>,
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
                match name.as_str() {
                    "Sprocket_L" => sprocket_view[0] = Some(entity),
                    "Sprocket_R" => sprocket_view[1] = Some(entity),
                    "Idler_L" => idler_view[0] = Some(entity),
                    "Idler_R" => idler_view[1] = Some(entity),
                    _ => {}
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
        // present) just retries next frame rather than binding a mis-meshed sprocket or leaking the
        // link entities a partial bind would spawn.
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
                &meshes,
                geom.teeth,
            ),
            sprocket_tooth_tip(
                sr,
                sr_rest.compute_affine(),
                &children,
                &node_transforms,
                &primitives,
                &meshes,
                geom.teeth,
            ),
        ) else {
            continue;
        };

        // The shoe pool: one instance of the glb's OWN link mesh per material link, per side, the
        // left side wearing the template's genuine mirror. Every shoe is the same dark steel — the
        // red "witness" link 0 used to wear was a dev sign-check for "does driving forward move the
        // lower run rearward", and that invariant is now pinned by `link_view`'s slot-rotation test
        // rather than by a differently-coloured link in every shipped session.
        let pool = |commands: &mut Commands, side: Side| -> Vec<Entity> {
            (0..spec.link_count)
                .map(|_| link_view::spawn_link(commands, &template, side, root))
                .collect()
        };
        let [links_l, links_r] = [
            pool(&mut commands, Side::Left),
            pool(&mut commands, Side::Right),
        ];
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
                links: links_l,
                link_frame: template.frame(Side::Left),
                link_center_x: geom.link_center_x(Side::Left),
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
                links: links_r,
                link_frame: template.frame(Side::Right),
                link_center_x: geom.link_center_x(Side::Right),
            },
        ];
        info!(
            "track rig bound: {} links/side, {} wheels/side; sprocket tooth tips L {:.2}° R {:.2}°",
            spec.link_count,
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
            sides,
            prev_affine: None,
            field_revision: None,
        });
    }
}

/// The per-frame seam: read each tank's presented root pose and replicated belt phase, lift the
/// view wheels off the terrain field, fit the wrap, and write every view transform — all before
/// propagation, so the whole tank renders one consistent frame.
fn drive_track_views(
    time: Res<Time>,
    track: Res<TrackField>,
    // The wheel-travel band: `TrackGear` carries the loop-clamped droop (its travel knots' peak),
    // the blueprint spec carries the bump stop. Both are single-blueprint globals today, read
    // per frame — well below the SystemParam ceiling.
    gear: Res<super::sim::TrackGear>,
    blueprint: Res<TankBlueprint>,
    mut tanks: Query<(&Transform, &TrackDrive, &mut TrackRig)>,
    mut views: Query<&mut Transform, Without<TrackRig>>,
) {
    let Some(field) = track.field.as_ref() else {
        return;
    };
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
        }
        rig.prev_affine = Some(affine);
        rig.field_revision = track.revision;

        // Belt phase from the SIM (phase B): the owner's predicted `TrackDrive`, a remote's
        // replicated one — real belt travel, so a braked skid stops the links and wheelspin scrolls
        // them honestly. The wrap needs only the phase (the drawn belt's motion IS the phase
        // advancing; there is no solver to feed a belt speed to).
        let phases = [drive.sides[0].phase, drive.sides[1].phase];

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
                let target = wheel_lift_target(field, &affine, down, wheel.pivot, &wparams);
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
        let out = wrap::step(&input, field, &mut rig.wrap);

        for (si, side) in rig.sides.iter().enumerate() {
            // The SHOES, on this frame's drawn pin joints: link `i` spans joint `i` → `i+1`, its
            // pin midpoint on the route and its own measured centre on `link_center_x`. The
            // entity↔joint map rotates with the belt phase, so a shoe's identity rides the belt
            // instead of wandering one link per pitch — all of it in `link_view`, which the sandbox
            // drives with the same call.
            link_view::place_links(
                &side.link_frame,
                side.link_center_x,
                &out[si].joints,
                phases[si],
                rig.belt_len / rig.count.max(1) as f32,
                &side.links,
                |link, pose| {
                    if let (Some(pose), Ok(mut tr)) = (pose, views.get_mut(link)) {
                        *tr = pose;
                    }
                },
            );
            // Wheels roll on the track's INNER face (pin line − `pin_to_inner`, the measured inner
            // offset — the pin does not run mid-plate, so it is not half the plate). Every axle
            // angle is NEGATIVE (Bevy +X rotation moves a wheel's bottom toward −Z, and positive
            // phase scrolls the lower run toward +Z) — `spin_angle` is the single flip point if a
            // future model's conventions differ. Per-station radius → per-wheel roll radius + spin.
            for wheel in &side.wheels {
                if let Ok(mut tr) = views.get_mut(wheel.view) {
                    let roll_r = wheel.radius - rig.pin_to_inner;
                    let spin = Quat::from_rotation_x(spin_angle(phases[si], roll_r));
                    tr.translation = wheel.pivot + Vec3::Y * wheel.dy;
                    tr.rotation = wheel.rest_rotation * spin;
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
