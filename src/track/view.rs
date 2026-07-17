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
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use crate::bake::TankBlueprint;
use crate::spec::TrackSpec;
use crate::tank::{Roadwheel, Tank, TrackSide, ViewNode};

use super::chain::{ChainInput, ChainParams, ChainSideInput, ChainState};
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
/// View wheel-lift ease (rad/s; settle ≈ 4.7/ω ≈ 100 ms) and travel cap — same cosmetic wheel
/// doctrine as the sandbox.
const WHEEL_EASE_OMEGA: f32 = 45.0;
const WHEEL_MAX_LIFT: f32 = 0.5;
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
    /// The immutable material loop: `belt_len = pitch × count`, exact.
    belt_len: f32,
    count: usize,
    /// Side-plane pin-line circles shared by both sides: the sprocket MUST stay first — the
    /// chain's motor membership is `RouteTag::Arc(0)`, the first circle's arc.
    sprocket: (Vec2, f32),
    idler: (Vec2, f32),
    wheel_radius: f32,
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
    /// Per road wheel, front→rear.
    wheels: Vec<RigWheel>,
    /// Sprocket/idler visual mesh nodes — spun about the authored centre (`rotate_around`),
    /// because the model carries their position in vertices, not in a node transform.
    sprocket_view: Entity,
    idler_view: Entity,
    /// One entity per link, children of the tank root, local transforms in hull space.
    links: Vec<Entity>,
}

struct RigWheel {
    /// Rest pivot (hull-local — the REAL authored x, which for the Tiger's interleaved columns
    /// differs from `plane_x`) and rest rotation (preserved under spin: a future model's wheels
    /// may author non-identity rests).
    pivot: Vec3,
    rest_rotation: Quat,
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

fn chain_params(spec: &TrackSpec) -> ChainParams {
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
        max_link_angle: spec.max_link_angle,
        max_normal_speed: MAX_NORMAL_SPEED,
        tube_out: TUBE_OUT,
        tube_in: TUBE_IN,
        rebase_window: REBASE_WINDOW,
        thickness: spec.thickness,
        lateral_stations: [-spec.width / 2.0, 0.0, spec.width / 2.0],
        probe_reach: PROBE_REACH,
    }
}

/// Attach a [`TrackRig`] to every tank whose presentation is ready: all roadwheel sim entities
/// carry `ViewNode` links (i.e. `bind_tank_view` ran) and the per-side sprocket/idler visual
/// meshes are found in the GLB tree. Retries lazily until then — no ordering coupling to the
/// `WorldInstanceReady` observer. Also hides the model's legacy static track meshes
/// (`Track_Strip_*`/`Track_Treads_*`), which the live links replace.
fn bind_track_rigs(
    blueprint: Res<TankBlueprint>,
    tanks: Query<Entity, (With<Tank>, Without<TrackRig>)>,
    children: Query<&Children>,
    names: Query<&Name>,
    wheels: Query<(&Roadwheel, &Transform, Option<&ViewNode>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut link_assets: Local<Option<LinkAssets>>,
    mut commands: Commands,
) {
    let spec = &blueprint.spec.track;
    'tank: for root in &tanks {
        let mut side_wheels: [Vec<(Vec3, Quat, Entity)>; 2] = [Vec::new(), Vec::new()];
        let mut sprocket_view = [None, None];
        let mut idler_view = [None, None];
        let mut hide = Vec::new();
        for entity in children.iter_descendants(root) {
            if let Ok((wheel, transform, view)) = wheels.get(entity) {
                // Presentation not attached yet — try again next frame.
                let Some(view) = view else { continue 'tank };
                let si = (wheel.side == TrackSide::Right) as usize;
                side_wheels[si].push((transform.translation, transform.rotation, view.0));
            } else if let Ok(name) = names.get(entity) {
                match name.as_str() {
                    "Sprocket_L_Visual" => sprocket_view[0] = Some(entity),
                    "Sprocket_R_Visual" => sprocket_view[1] = Some(entity),
                    "Idler_L_Visual" => idler_view[0] = Some(entity),
                    "Idler_R_Visual" => idler_view[1] = Some(entity),
                    "Track_Strip_L_Visual"
                    | "Track_Strip_R_Visual"
                    | "Track_Treads_L_Visual"
                    | "Track_Treads_R_Visual" => hide.push(entity),
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

        // Feasibility gate (the schema check can't do this — it needs the wheel rests): the
        // authored material loop must close around the rest running gear. An infeasible loop
        // would otherwise become a perpetual tear/reseed churn, not one clean failure.
        let sprocket_r = spec.pitch * spec.sprocket.teeth as f32 / std::f32::consts::TAU;
        let rest_circles: Vec<(Vec2, f32)> = {
            let mut c = vec![(
                Vec2::new(spec.sprocket.center.0, spec.sprocket.center.1),
                sprocket_r,
            )];
            c.extend(
                side_wheels[0]
                    .iter()
                    .map(|(p, ..)| (Vec2::new(p.z, p.y), spec.wheel_radius)),
            );
            c.push((
                Vec2::new(spec.idler.center.0, spec.idler.center.1),
                spec.idler.radius,
            ));
            c
        };
        let belt_len = spec.pitch * spec.link_count as f32;
        let rest_route = super::route::build_route(&rest_circles, belt_len);
        let closed = rest_route.total();
        if !closed.is_finite() || (closed - belt_len).abs() > 0.005 * belt_len {
            error_once!(
                "track spec infeasible: pitch × link_count = {belt_len:.3} m cannot close the \
                 rest running gear (route closed at {closed:.3} m) — no track rig bound"
            );
            continue;
        }

        // One link mesh + material set for every link in the world (single blueprint today).
        // The small pitch gap keeps links reading as links, not a ribbon. Link 0 wears the
        // witness material: driving forward one pitch must move the lower run's witness
        // rearward one pitch and the sprocket one negative tooth step (the sign check).
        let assets = link_assets
            .get_or_insert_with(|| LinkAssets {
                mesh: meshes.add(Cuboid::new(spec.width, spec.thickness, spec.pitch * 0.96)),
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

        for entity in hide {
            commands.entity(entity).insert(Visibility::Hidden);
        }
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
        let links = [spawn_links(), spawn_links()];
        let [wl, wr] = side_wheels;
        let rig_wheels = |list: Vec<(Vec3, Quat, Entity)>| -> Vec<RigWheel> {
            list.into_iter()
                .map(|(pivot, rest_rotation, view)| RigWheel {
                    pivot,
                    rest_rotation,
                    dy: 0.0,
                    dvel: 0.0,
                    view,
                })
                .collect()
        };
        let sides = [
            RigSide {
                plane_x: -spec.plane_x,
                wheels: rig_wheels(wl),
                sprocket_view: sl,
                idler_view: il,
                links: links[0].clone(),
            },
            RigSide {
                plane_x: spec.plane_x,
                wheels: rig_wheels(wr),
                sprocket_view: sr,
                idler_view: ir,
                links: links[1].clone(),
            },
        ];
        info!(
            "track rig bound: {} links/side, {} wheels/side",
            spec.link_count,
            sides[0].wheels.len()
        );
        commands.entity(root).insert(TrackRig {
            params: chain_params(spec),
            belt_len,
            count: spec.link_count,
            // Sprocket pitch radius from tooth lock: one link advance ≡ one tooth advance.
            sprocket: (
                Vec2::new(spec.sprocket.center.0, spec.sprocket.center.1),
                sprocket_r,
            ),
            idler: (
                Vec2::new(spec.idler.center.0, spec.idler.center.1),
                spec.idler.radius,
            ),
            wheel_radius: spec.wheel_radius,
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
        let wparams = WheelParams {
            reach: rig.wheel_radius + rig.params.thickness / 2.0,
            ease_omega: WHEEL_EASE_OMEGA,
            max_lift: WHEEL_MAX_LIFT,
            lateral_stations: WHEEL_DISC_STATIONS,
            probe_reach: PROBE_REACH,
        };
        let down = affine.transform_vector3(Vec3::NEG_Y).normalize_or_zero();
        for side in &mut rig.sides {
            for wheel in &mut side.wheels {
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
            c.push(rig.sprocket);
            c.extend(
                side.wheels
                    .iter()
                    .map(|w| (Vec2::new(w.pivot.z, w.pivot.y + w.dy), rig.wheel_radius)),
            );
            c.push(rig.idler);
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
                },
                ChainSideInput {
                    circles: &circles[1],
                    belt_speed: speeds[1],
                    phase: chain_phase(phases[1]),
                    plane_x: rig.sides[1].plane_x,
                },
            ],
        };
        let mut out: [Vec<Vec2>; 2] = [Vec::new(), Vec::new()];
        let report = rig.chain.step(&input, &rig.params, field, &mut out);
        if report.tears > 0 {
            warn!("track view tear-fuse reseed × {}", report.tears);
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
                let pitch = f64::from(rig.belt_len) / n as f64;
                let q = (phases[si] / pitch).floor() as i64;
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
            // Wheels roll on the track's inner face (pin line − half plate); sprocket is
            // phase-locked at its pitch radius; idler rides the inner face at its rim. Every
            // axle angle is NEGATIVE (Bevy +X rotation moves a wheel's bottom toward −Z, and
            // positive phase scrolls the lower run toward +Z) — `spin_angle` is the single
            // flip point if a future model's conventions differ.
            let roll_r = rig.wheel_radius - rig.params.thickness / 2.0;
            let wheel_spin = Quat::from_rotation_x(spin_angle(phases[si], roll_r));
            for wheel in &side.wheels {
                if let Ok(mut tr) = views.get_mut(wheel.view) {
                    tr.translation = wheel.pivot + Vec3::Y * wheel.dy;
                    tr.rotation = wheel.rest_rotation * wheel_spin;
                }
            }
            spin_about(
                &mut views,
                side.sprocket_view,
                rig.sprocket.0,
                spin_angle(phases[si], rig.sprocket.1),
            );
            let idler_rim = rig.idler.1 - rig.params.thickness / 2.0;
            spin_about(
                &mut views,
                side.idler_view,
                rig.idler.0,
                spin_angle(phases[si], idler_rim),
            );
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

/// Rotate a mesh node about the hull-space X axis through side-plane centre `c` — the spin of a
/// visual whose pivot the model doesn't carry (vertices live in hull space, node transform is
/// identity). `translation = c − R·c` recentres the rotation on the authored circle.
fn spin_about(
    views: &mut Query<&mut Transform, Without<TrackRig>>,
    node: Entity,
    c: Vec2,
    angle: f32,
) {
    let Ok(mut tr) = views.get_mut(node) else {
        return;
    };
    let rot = Quat::from_rotation_x(angle);
    let centre = Vec3::new(0.0, c.y, c.x);
    tr.set_if_neq(Transform {
        translation: centre - rot * centre,
        rotation: rot,
        scale: Vec3::ONE,
    });
}
