//! Phase-A track VIEW plugin (architecture §3/§9): live tracks on every tank, zero sim risk.
//!
//! The simulated chain ([`super::chain`]) runs per tank on the PRESENTED pose — the root
//! `Transform` after physics writeback and (on the net client) after rollback-correction
//! smoothing, before transform propagation — so links, wheels, and hull share one rendered
//! frame. Everything here is cosmetic state, reseedable from data at any instant: never
//! rollback-registered, never replayed, and mounted only by the windowed clients (ADR-0014 —
//! the server never composes this plugin).
//!
//! Tier policy (architecture §6) is deliberately ABSENT: the alpha is 1v1, so every tank gets
//! the chain (~0.7 ms/frame/tank worst-case). Tiers return when tank counts demand them —
//! promote by projected link pitch in pixels, which also makes transitions sub-pixel by
//! construction (per the 2026-07-17 tier discussion).

use avian3d::prelude::PhysicsSystems;
use bevy::math::{Affine3A, Vec2};
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use crate::bake::TankBlueprint;
use crate::spec::TrackSpec;
use crate::tank::{Roadwheel, Tank, TrackSide, ViewNode};

use super::chain::{ChainInput, ChainParams, ChainSideInput, ChainState};
use super::forces::phase_decompose;
use super::rig_geom::RigGeom;
use super::side::Side;
use super::sim::TrackDrive;
use super::terrain::TrackField;
use super::wheels::{WheelParams, wheel_lift_step, wheel_lift_target};

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
    app.add_systems(Update, bind_track_rigs);
    app.configure_sets(
        PostUpdate,
        TrackViewSet
            .after(PhysicsSystems::Writeback)
            .before(TransformSystems::Propagate),
    );
    app.add_systems(PostUpdate, drive_track_views.in_set(TrackViewSet));
}

// Solver QUALITY policy — global, never per-vehicle (architecture §7: a new tank is data; these
// are the sandbox step-23/24 values the feel verdict was given on). Vehicle DATA comes from
// `TrackSpec`.
const SUBSTEP: f32 = 1.0 / 120.0;
const MAX_SUBSTEPS: usize = 8;
const SWEEPS: usize = 4;
const HALF_LIFE_TAN: f32 = 0.60;
const HALF_LIFE_NORM: f32 = 0.060;
const MOTOR_TAU: f32 = 0.05;
const BEND_STIFFNESS: f32 = 2.0;
const MAX_NORMAL_SPEED: f32 = 4.0;
const TUBE_OUT: f32 = 0.30;
const TUBE_IN: f32 = 0.40;
const REBASE_WINDOW: f32 = 0.35;
const PROBE_REACH: f32 = 0.5;
/// View wheel-lift ease (rad/s; settle ≈ 4.7/ω ≈ 100 ms) — same cosmetic wheel doctrine as the
/// sandbox. The travel band is no longer a constant: `max_lift` is the spec's bump stop and
/// `max_droop` the chain-clamped droop (`TrackGear::max_droop`), both read per frame.
const WHEEL_EASE_OMEGA: f32 = 45.0;
/// Presented-pose discontinuity thresholds: a root that moves further than this in ONE frame is
/// a teleport/respawn/snap-correction, not motion — reset the chain, belt differentiator, and
/// wheel-lift state. `render_error` publishes no signal (it consumes oversized corrections
/// silently), so the view detects locally: works identically in SP and MP, no netcode coupling.
/// 60 km/h at 30 fps is 0.56 m/frame — half the trip threshold. MUST stay below
/// `render_error`'s snap thresholds (2 m / 60°) so every unsmoothed correction trips this too —
/// `net::render_error` pins that bracket in a test. `pub(crate)` for exactly that test.
pub(crate) const SNAP_TRANSLATION: f32 = 1.2;
/// Axis chord per frame (~30°), checked on BOTH the forward and up axes — a pure roll leaves
/// forward unchanged.
pub(crate) const SNAP_AXIS: f32 = 0.5;
/// Wheel-lift probe stations across the DISC (m from the wheel's real x) — the Tiger's
/// interleaved discs are 0.158 m wide, far narrower than the shoe: probing shoe-wide at an
/// outboard wheel column would read geometry entirely beside the track (codex finding C).
/// Interim until the bake carries disc bounds. The CHAIN keeps shoe-wide stations at `plane_x`.
const WHEEL_DISC_STATIONS: [f32; 3] = [-0.08, 0.0, 0.08];

/// One tank's track-view state, on the root. Pure view: despawns with the root, resets to a
/// canonical cold start on any discontinuity.
#[derive(Component)]
struct TrackRig {
    params: ChainParams,
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
    chain: ChainState,
    sides: [RigSide; 2],
    /// Last frame's presented affine — the belt differentiator's and the substep
    /// interpolation's previous frame. `None` = cold start.
    prev_affine: Option<Affine3A>,
    field_revision: Option<u64>,
}

struct RigSide {
    /// Signed track-centreline x (left −, right +).
    plane_x: f32,
    /// The chain's lateral terrain stations ([`RigGeom::grip_stations`]): the physics
    /// collocation columns as signed hull-x offsets from this side's plane — the measured
    /// shoe faces, per-side because the shoe is not centred on its pins.
    lateral_stations: [f32; 3],
    /// This side's hull-fixed sprocket pin-line circle (side-plane `(z, y)` centre + pitch radius),
    /// from [`RigGeom::rest`]. The sprocket MUST stay first in the chain's circle list — its motor
    /// membership is `RouteTag::Arc(0)`, the first circle's arc.
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
    /// One entity per link, children of the tank root, local transforms in hull space.
    links: Vec<Entity>,
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

/// The shared link render assets (one blueprint today): the box mesh, the belt material, and
/// the witness material link 0 wears.
#[derive(Clone)]
struct LinkAssets {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    witness: Handle<StandardMaterial>,
}

/// A GLB re-instantiation (hot reload during authoring) replaces the wheel/sprocket view
/// entities a bound rig points at: drop the rig and its links, and `bind_track_rigs` rebinds
/// against the fresh tree next frame (re-hiding the fresh legacy meshes too).
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

fn chain_params(spec: &TrackSpec, geom: &RigGeom) -> ChainParams {
    ChainParams {
        substep: SUBSTEP,
        max_substeps: MAX_SUBSTEPS,
        sweeps: SWEEPS,
        half_life_tan: HALF_LIFE_TAN,
        half_life_norm: HALF_LIFE_NORM,
        node_mass: spec.link_mass,
        hinge_torque: spec.hinge_torque,
        motor_tau: MOTOR_TAU,
        bend_stiffness: BEND_STIFFNESS,
        // Radians from here down: `LinkAngleSpec` owns the degrees→radians seam.
        link_angle_inward: spec.link_angle.inward(),
        link_angle_outward: spec.link_angle.outward(),
        max_normal_speed: MAX_NORMAL_SPEED,
        tube_out: TUBE_OUT,
        tube_in: TUBE_IN,
        rebase_window: REBASE_WINDOW,
        // Measured off the glb markers: the chain's ground-face push models the FULL plate
        // as `thickness/2`. (Its lateral stations are per-side — [`RigSide::lateral_stations`].)
        thickness: geom.thickness,
        probe_reach: PROBE_REACH,
    }
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
    tanks: Query<Entity, (With<Tank>, Without<TrackRig>)>,
    children: Query<&Children>,
    names: Query<&Name>,
    wheels: Query<(&Roadwheel, &Transform, Option<&ViewNode>)>,
    // Sprocket/idler REST poses and their tooth-tip mesh calibration are read here at bind: node
    // transforms for the rest capture and the mesh-space→hull composition, mesh primitives for the
    // sprocket tooth ring.
    node_transforms: Query<&Transform>,
    primitives: Query<&Mesh3d>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut link_assets: Local<Option<LinkAssets>>,
    mut commands: Commands,
) {
    // The measured running gear lands with `TrackGear` on the first sim frame; until then there is
    // nothing to bind the numeric geometry against.
    let Some(geom) = geom else {
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
        // circle carrying its own centre AND pin-line radius. The live chain articulates the wheel
        // CENTRES off the presented pose; only the RADII (and the hull-fixed sprocket/idler circles)
        // come from here.
        let rest = [Side::Left, Side::Right].map(|s| geom.rest.get(s).clone());

        // Feasibility gate (the schema check can't do this — it needs the running-gear circles): the
        // material loop must close around the rest gear, or an infeasible loop becomes a perpetual
        // tear/reseed churn rather than one clean failure. Measured on the right side — the sides are
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
        let (Some(sl_tip), Some(sr_tip)) = (
            sprocket_tooth_tip(
                sl,
                &sl_rest,
                &children,
                &node_transforms,
                &primitives,
                &meshes,
                geom.teeth,
            ),
            sprocket_tooth_tip(
                sr,
                &sr_rest,
                &children,
                &node_transforms,
                &primitives,
                &meshes,
                geom.teeth,
            ),
        ) else {
            continue;
        };

        // One link mesh + material set for every link in the world (single blueprint today).
        // The small pitch gap keeps links reading as links, not a ribbon. Link 0 wears the
        // witness material: driving forward one pitch must move the lower run's witness
        // rearward one pitch and the sprocket one negative tooth step (the sign check).
        let assets = link_assets
            .get_or_insert_with(|| LinkAssets {
                mesh: meshes.add(Cuboid::new(geom.width, geom.thickness, geom.pitch * 0.96)),
                material: materials.add(StandardMaterial {
                    base_color: Color::srgb(0.10, 0.10, 0.11),
                    perceptual_roughness: 0.85,
                    metallic: 0.4,
                    ..default()
                }),
                witness: materials.add(StandardMaterial {
                    base_color: Color::srgb(0.55, 0.15, 0.08),
                    perceptual_roughness: 0.85,
                    metallic: 0.4,
                    ..default()
                }),
            })
            .clone();

        let mut spawn_links = || -> Vec<Entity> {
            (0..spec.link_count)
                .map(|i| {
                    let material = if i == 0 {
                        assets.witness.clone()
                    } else {
                        assets.material.clone()
                    };
                    commands
                        .spawn((
                            Mesh3d(assets.mesh.clone()),
                            MeshMaterial3d(material),
                            // Buried until the first solve writes real poses (the rig lands via
                            // commands, so the first chain step is next frame).
                            Transform::from_xyz(0.0, -1000.0, 0.0),
                            ChildOf(root),
                        ))
                        .id()
                })
                .collect()
        };
        let [links_l, links_r] = [spawn_links(), spawn_links()];
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
            params: chain_params(spec, &geom),
            belt_len,
            count: spec.link_count,
            teeth: geom.teeth,
            pin_to_inner: geom.model.pin_to_inner,
            chain: ChainState::default(),
            sides,
            prev_affine: None,
            field_revision: None,
        });
    }
}

/// The per-frame seam: read each tank's presented root pose, derive the no-slip belt, lift the
/// view wheels off the terrain field, step the chain, and write every view transform —
/// all before propagation, so the whole tank renders one consistent frame.
fn drive_track_views(
    time: Res<Time>,
    track: Res<TrackField>,
    // The wheel-travel band: `TrackGear` carries the chain-clamped droop (its travel knots' peak),
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
        // Discontinuity: teleport / respawn / snap-consumed correction / terrain swap → the
        // chain's canonical cold start and re-based wheel lift (old-terrain lift must not seed
        // the cold chain). Rotation is checked on BOTH forward and up axes — a pure roll
        // leaves forward unchanged.
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
            rig.chain = ChainState::default();
        }
        rig.prev_affine = Some(affine);
        rig.field_revision = track.revision;

        // Belt truth from the SIM (phase B): the owner's predicted `TrackDrive`, a remote's
        // replicated one — real belt speed and phase, so a braked skid stops the links and
        // wheelspin scrolls them honestly. The old presented-pose no-slip derivation is gone.
        let speeds = [drive.sides[0].speed, drive.sides[1].speed];
        let phases = [drive.sides[0].phase, drive.sides[1].phase];

        // View wheel lift: probe the field at each wheel's REAL position across its DISC (not
        // the shoe), ease the lift (implicit rise / ballistic fall), then the chain wraps the
        // lifted circles. On a snap the lift re-bases to the fresh target instantly.
        let mut wparams = WheelParams {
            // `reach` is set PER WHEEL below: this station's pin-line radius (the circle the chain
            // wraps) + the measured plate face offset to its ground face — the sandbox's form, no
            // mid-plate `thickness/2` assumption and no single shared wheel radius.
            reach: 0.0,
            ease_omega: WHEEL_EASE_OMEGA,
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

        // The chain: side-plane circles (sprocket FIRST — the motor arc), articulated wheel
        // centres, hull-local gravity.
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
        // The chain wraps phase by the material loop itself: belt_len = pitch × count exactly,
        // so a whole-loop wrap shifts link identity by `count` ≡ 0 — seamless by construction.
        let chain_phase = |phase: f64| phase.rem_euclid(f64::from(rig.belt_len)) as f32;
        let g3 = affine.inverse().transform_vector3(Vec3::NEG_Y * 9.81);
        let input = ChainInput {
            dt,
            affine,
            gravity_local: Vec2::new(g3.z, g3.y),
            belt_len: rig.belt_len,
            count: rig.count,
            sides: [
                ChainSideInput {
                    circles: &circles[0],
                    belt_speed: speeds[0],
                    phase: chain_phase(phases[0]),
                    plane_x: rig.sides[0].plane_x,
                    lateral_stations: rig.sides[0].lateral_stations,
                },
                ChainSideInput {
                    circles: &circles[1],
                    belt_speed: speeds[1],
                    phase: chain_phase(phases[1]),
                    plane_x: rig.sides[1].plane_x,
                    lateral_stations: rig.sides[1].lateral_stations,
                },
            ],
        };
        let mut out: [Vec<Vec2>; 2] = [Vec::new(), Vec::new()];
        let report = rig.chain.step(&input, &rig.params, field, &mut out);
        if report.tears > 0 {
            // `debug!`, not `warn!`: a tear-fuse reseed is a cosmetic self-heal (this view is
            // reseedable-from-data by construction, never sim/rollback state), and it recurs per
            // frame while a tank grinds terrain — so at `warn` it flooded the console. Matches the
            // sibling overrun line below. Raise with `RUST_LOG=overmatch=debug` to watch tears.
            debug!("track view tear-fuse reseed × {}", report.tears);
        }
        if report.overruns > 0 {
            debug!(
                "track view overrun reseed × {} (frame hitch)",
                report.overruns
            );
        }

        for (si, side) in rig.sides.iter().enumerate() {
            // Links: joint i → i+1, box centred on the pin-line midpoint, +Z along the tangent.
            // `from_rotation_x(-ang)` maps local +Z to (z, y) = (cos ang, sin ang) — the tangent.
            let pts = &out[si];
            if pts.len() == side.links.len() {
                // Joint slots shift material identity by one every pitch of travel (the
                // chain resamples at `phase mod pitch`), so a fixed entity↔slot binding
                // makes any per-link identity — the witness paint today, damage/texture
                // later — wander one link per pitch. Rotate the mapping by the whole-pitch
                // quotient: entity m always wears material link m, and the witness RIDES
                // the belt.
                let n = side.links.len() as i64;
                let pitch = rig.belt_len / n as f32;
                // The whole-pitch quotient from the canonical decomposition (the offset half
                // is the chain's own sampling concern) — one home for the wrap arithmetic.
                let (q, _) = phase_decompose(phases[si], pitch);
                for (i, _) in pts.iter().enumerate() {
                    let link = side.links[(i as i64 - q).rem_euclid(n) as usize];
                    let a = pts[i];
                    let b = pts[(i + 1) % pts.len()];
                    let t = b - a;
                    if t.length_squared() < 1e-8 {
                        continue;
                    }
                    let mid = (a + b) / 2.0;
                    if let Ok(mut tr) = views.get_mut(link) {
                        *tr = Transform::from_translation(Vec3::new(side.plane_x, mid.y, mid.x))
                            .with_rotation(Quat::from_rotation_x(-t.y.atan2(t.x)));
                    }
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
            // over the stationary belly run rolls at the inner-face radius (2026-07-25 review:
            // the earlier `− pin_to_inner` here over-rotated the idler ~7%).
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

/// Belt travel → axle angle, wrapped per wheel circumference in f64 BEFORE the f32 cast, so a
/// long match's accumulated travel never erodes spin precision. The negative sign is the one
/// place the phase→rotation convention lives.
fn spin_angle(phase: f64, radius: f32) -> f32 {
    let circumference = f64::from(radius) * std::f64::consts::TAU;
    -(phase.rem_euclid(circumference) / f64::from(radius)) as f32
}

/// A gear node's local transform for `angle` of spin about the hull's lateral (+X) axis, composed
/// onto its captured REST pose. Mirrors the sandbox's `wheel_view::gear_transform` for the
/// hull-fixed roles (no travel `dy`): the rest translation and scale are kept verbatim, and the
/// spin PRE-multiplies the rest rotation — so the node keeps its authored axle-origin translation
/// (never dragged to the tank origin) and a baked orientation flip cannot reverse the spin.
fn gear_spin_transform(rest: &Transform, angle: f32) -> Transform {
    Transform {
        translation: rest.translation,
        rotation: Quat::from_rotation_x(angle) * rest.rotation,
        scale: rest.scale,
    }
}

// ---------------------------------------------------------------------------------------------
// Sprocket tooth-phase lock — ported from `track_sandbox::wheel_view` (the same deliberate mirror
// as `spin_angle` above). See that module's doc for the full derivation; the short version is that
// one link of belt travel is EXACTLY one tooth of rotation (radius never enters), so the teeth stay
// seated in the same pin gaps forever, and `phase / pitch_radius` is wrong because a chord is
// shorter than its arc (it drifts a whole tooth every ~244 links).
// ---------------------------------------------------------------------------------------------

/// Where a tooth TIP must sit at belt travel `travel`: `origin` (the angle zero phase seats pin 0
/// at) + ½ tooth (a tip bisects the pin pair straddling the arc-length origin) + one tooth per
/// pitch of travel, wrapped per sprocket revolution in `f64` before the `f32` cast.
fn tooth_tip_angle(travel: f64, pitch: f32, teeth: u32, origin: f32) -> f32 {
    let tooth = std::f32::consts::TAU / teeth as f32;
    let per_revolution = f64::from(pitch) * f64::from(teeth);
    let turn = (travel.rem_euclid(per_revolution) / per_revolution * std::f64::consts::TAU) as f32;
    origin + tooth / 2.0 + turn
}

/// Belt travel → sprocket node spin about the hull's lateral axis (rad): put this mesh's own tooth
/// tip where [`tooth_tip_angle`] wants a tip. The difference is tip-minus-target because a positive
/// spin about +X DECREASES a side-plane angle (the same flip [`spin_angle`] carries). A degenerate
/// rig (no pitch or no teeth) parks the sprocket rather than emitting a NaN transform.
fn tooth_angle(travel: f64, pitch: f32, teeth: u32, origin: f32, mesh_tip: f32) -> f32 {
    if f64::from(pitch) * f64::from(teeth) <= 1e-6 {
        return 0.0;
    }
    mesh_tip - tooth_tip_angle(travel, pitch, teeth, origin)
}

/// Fraction of the rim radius a vertex must reach to count as tooth-TIP land.
const TIP_BAND: f32 = 0.98;
/// Quantile of the in-plane radii that anchors "this is the rim", and how far past it a vertex may
/// still sit and count — a ring of hundreds of vertices always clears the quantile, a stray never
/// does (a raw `max` is the one statistic a single stray destroys).
const RIM_QUANTILE: f32 = 0.95;
const RIM_BAND: f32 = 1.01;
/// How sharply the tip band must cluster on a `teeth`-fold grid before the measurement is believed
/// (the mean resultant length of the fitted harmonic). Anything below is not a sprocket of this
/// tooth count — the caller leaves the node unbound and retries.
const TOOTH_CONCENTRATION: f32 = 0.25;

/// The `teeth`-fold phase of the tip land in a sprocket's `(radius, side-plane angle)` cloud,
/// reduced to `[0, τ/teeth)`. The estimator is the `teeth`-th circular harmonic of the tip band:
/// `arg(Σ e^{i·teeth·α})` — the definition of the phase of a `teeth`-fold symmetry, exact for a
/// symmetric tip land whatever its width, with the harmonic's magnitude falling out as the
/// confidence the ring really has that symmetry. `None` if the cloud does not read as a tooth ring.
/// Ported verbatim from `track_sandbox::wheel_view::measure_tooth_tip_angle`.
fn measure_tooth_tip_angle(polar: &[(f32, f32)], teeth: u32) -> Option<f32> {
    if teeth == 0 || polar.len() < teeth as usize {
        return None;
    }
    let mut radii: Vec<f32> = polar.iter().map(|&(r, _)| r).collect();
    radii.sort_by(f32::total_cmp);
    let rim = radii[((RIM_QUANTILE * (radii.len() - 1) as f32) as usize).min(radii.len() - 1)];
    let tip = radii.iter().rev().find(|r| **r <= rim * RIM_BAND)?;
    let band = tip * TIP_BAND;

    // `f64` for the accumulation only: the sum runs over thousands of terms and its ARGUMENT is the
    // whole answer, so cancellation in the tail is not something to hand to an f32 accumulator.
    let (mut sx, mut sy, mut n) = (0.0_f64, 0.0_f64, 0_u32);
    for (_, angle) in polar.iter().filter(|&&(r, _)| r >= band) {
        let harmonic = f64::from(teeth) * f64::from(*angle);
        sx += harmonic.cos();
        sy += harmonic.sin();
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let concentration = sx.hypot(sy) / f64::from(n);
    if concentration < f64::from(TOOTH_CONCENTRATION) {
        warn!(
            "track rig: a sprocket's rim does not read as a {teeth}-fold tooth ring \
             (concentration {concentration:.3} over {n} rim vertices, need {TOOTH_CONCENTRATION}) \
             — the sprocket cannot be phase-locked to teeth that are not there; retrying"
        );
        return None;
    }
    let tooth = std::f32::consts::TAU / teeth as f32;
    Some(((sy.atan2(sx) / f64::from(teeth)) as f32).rem_euclid(tooth))
}

/// Measure the tooth-tip phase of a bound sprocket node from its own mesh, in the hull side plane
/// `(z, y)`. The game's gear nodes are hull-framed (their parent chain to the root is identity —
/// the same premise the road-wheel poses and the old spin already relied on), so the node's REST
/// transform IS its hull-from-node affine; the mesh hangs on the node's PRIMITIVE children. `None`
/// if a primitive mesh is not readable yet or the cloud does not read as a `teeth`-fold star — the
/// caller retries next frame.
fn sprocket_tooth_tip(
    node: Entity,
    rest: &Transform,
    children: &Query<&Children>,
    transforms: &Query<&Transform>,
    primitives: &Query<&Mesh3d>,
    meshes: &Assets<Mesh>,
    teeth: u32,
) -> Option<f32> {
    let hull_from_node = rest.compute_affine();
    // The node ORIGIN is the axle (the bake invariant guarantees it), so it is the centre every
    // tooth angle is measured about — never a vertex statistic, which on a toothed rim is not a
    // circle in the first place.
    let axle = hull_from_node.transform_point3(Vec3::ZERO);
    let mut polar: Vec<(f32, f32)> = Vec::new();
    for child in children.get(node).ok()?.iter() {
        let Ok(primitive) = primitives.get(child) else {
            continue;
        };
        let mesh = meshes.get(&primitive.0)?;
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            continue;
        };
        let hull_from_mesh = hull_from_node
            * transforms
                .get(child)
                .map_or(Affine3A::IDENTITY, Transform::compute_affine);
        polar.extend(positions.iter().map(|p| {
            let v = hull_from_mesh.transform_point3(Vec3::from(*p)) - axle;
            // The side plane, by definition: every axle in a tank's running gear is lateral, so the
            // tooth ring lives in `(z, y)` — the plane the route and `belt_origin_angle` speak.
            (Vec2::new(v.z, v.y).length(), v.y.atan2(v.z))
        }));
    }
    measure_tooth_tip_angle(&polar, teeth)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gear-node rest compose (Defect A part 1): the belt spin rides ON the captured REST
    /// transform. Translation and scale must survive untouched — this is the whole fix, since the
    /// sprocket/idler nodes now carry their own axle-origin translation and the old `spin_about`
    /// overwrote it (dragging them to the tank origin at phase zero). The spin must be a pure +X
    /// rotation composed in PARENT space (pre-multiplied), not about the node's own axis.
    #[test]
    fn gear_spin_keeps_the_rest_translation_and_scale() {
        let rest = Transform {
            translation: Vec3::new(1.67, 0.51, -1.88),
            rotation: Quat::from_rotation_y(std::f32::consts::PI),
            scale: Vec3::splat(0.8),
        };
        let angle = -0.7;
        let posed = gear_spin_transform(&rest, angle);

        // The axle-origin translation and authored scale are carried verbatim — the Defect A fix.
        assert_eq!(posed.translation, rest.translation);
        assert_eq!(posed.scale, rest.scale);
        // Zero spin is exactly the rest pose (no drag to origin, unlike the old `c − R·c` form).
        assert_eq!(gear_spin_transform(&rest, 0.0).rotation, rest.rotation);

        // The spin is a pure +X rotation composed in parent space (PRE-multiplied). Post-multiplying
        // would spin about the node's own baked-flipped axis — the two are genuinely different here.
        let want = Quat::from_rotation_x(angle) * rest.rotation;
        assert!(posed.rotation.angle_between(want) < 1e-5);
        let wrong = rest.rotation * Quat::from_rotation_x(angle);
        assert!(posed.rotation.angle_between(wrong) > 1.0);
    }

    /// The tooth-lock mapping (Defect A part 2), the one property the game needs from the port: one
    /// link of belt travel is exactly one tooth of rotation, seated on the mesh tip — so the teeth
    /// never drift off the pins. The rate/drift/gullet math itself is exhaustively pinned by
    /// `track_sandbox::wheel_view`'s tests against the shipped Tiger; this only checks the game's
    /// copy carries the same tooth-per-pitch rate and the seating constant.
    #[test]
    fn one_link_of_travel_is_one_tooth() {
        const TEETH: u32 = 20;
        const PITCH: f32 = 0.130;
        const ORIGIN: f32 = 1.5199;
        const MESH_TIP: f32 = 0.0163;
        let tooth = std::f64::consts::TAU / f64::from(TEETH);
        let spin = |travel: f64| f64::from(tooth_angle(travel, PITCH, TEETH, ORIGIN, MESH_TIP));
        let seated = spin(0.0);
        for links in [1_i32, 20, 244, 5_000] {
            let travel = f64::from(links) * f64::from(PITCH);
            let residual = (seated - spin(travel) - f64::from(links) * tooth)
                .rem_euclid(std::f64::consts::TAU);
            assert!(
                residual.min(std::f64::consts::TAU - residual) < 1e-5,
                "{links} links did not land on a whole tooth",
            );
        }
        // A degenerate rig parks the sprocket rather than emitting a NaN transform.
        assert_eq!(tooth_angle(1.0, PITCH, 0, ORIGIN, MESH_TIP), 0.0);
        assert_eq!(tooth_angle(1.0, 0.0, TEETH, ORIGIN, MESH_TIP), 0.0);
    }
}
