//! The locomotion sim (phase B): the track model's belt forces ARE how tanks drive. The ECS
//! adapter over [`super::forces`] — one deep boundary; support, traction, and belt dynamics
//! live behind `step_side`, this module owns queries, scheduling, capability gating, and the
//! netcode-visible [`TrackDrive`] state.
//!
//! Sim discipline (hard rules, each bought with a measured MP failure in the raycast sim this
//! replaces):
//! - Pose from tick-truth `Position`/`Rotation`, never `GlobalTransform` (render lag differs
//!   per machine and freezes through rollback replays).
//! - Terrain from the analytic [`TrackField`] — pure closed-form arithmetic, no spatial
//!   queries, no BVH rollback dependency.
//! - Runs every replayed tick (NO `Replaying` gate — this is sim state); stays inside
//!   `SimPhase::DrivingForces` so drive samples velocity before the weapon-fire impulse.
//! - `Drive` capability gates the COMMAND, not the contact model: a dead engine still has
//!   kinetic grip (the slip law keeps resisting motion — though it creeps on slopes, ADR-0025);
//!   it just cannot thrust. The cut is not instant: the lost capability retargets the command
//!   slew, so thrust fades over ~1/[`super::drive::DRIVE_SLEW_PER_SECOND`] s — deliberate, the
//!   same shaping as a released key, making capability loss/recovery feel mechanical.

use avian3d::prelude::{Forces, Position, ReadRigidBodyForces, Rotation, WriteRigidBodyForces};
use bevy::math::{Affine3A, Vec2};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::bake::TankBlueprint;
use crate::command::TankCommand;
use crate::damage::{
    Capability, TankCapabilities, TankVolumes, VolumeFacets, capability_available,
};
use crate::state::{GameplaySet, SimPhase};
use crate::tank::{Tank, TrackSide};

use super::drive::{DriveAxes, shape_drive};
use super::forces::{
    BeltContact, ForceParams, GripElements, SideInput, SideState, grip_stiffness, step_side,
};
use super::route::build_route;
use super::side::Side;
use super::terrain::TrackField;

// Surface friction policy (ADR-0007 bucket 3: a property of the track–ground PAIR, destined
// for the terrain/ground-type mechanic — deliberately not vehicle spec).
const MU: f32 = 0.9;
/// Wong/Merritt firm-ground turning-resistance ratio — the lower lateral grip budget that
/// lets a heavy tank pivot at all.
const LATERAL_GRIP_RATIO: f32 = 0.55;
const SLIP_SATURATION: f32 = 0.4;

/// Per-tank tracked-drivetrain sim state: owner-predicted, replicated to remotes, rolled
/// back — the `LinearVelocity` registration pattern in `net::protocol` (replicate + predict +
/// float-threshold rollback condition). Hashed into the determinism trace (`hblt`).
#[derive(Component, Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TrackDrive {
    /// The shaped drive signal in [−1, 1]: `TankCommand` targets slewed through
    /// [`super::drive::shape_drive`]. Sim state (not command) so every tank responds with the
    /// same feel.
    pub throttle: f32,
    pub steer: f32,
    /// Per-side belt state, `[left, right]`.
    pub sides: [TrackDriveSide; 2],
}

#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TrackDriveSide {
    /// Belt surface speed (m/s).
    pub speed: f32,
    /// Unbounded belt travel (m) — advects the force stations; the view's exact scroll phase.
    pub phase: f64,
}

/// Startup-latched marker for the OFFLINE element-grip feel test (element-promotion-checklist.md
/// Q1, phase 2): when present, [`apply_track_forces`] runs the per-element isotropic shear regime
/// instead of the per-side aggregate. Inserted ONLY by the `--offline` composition
/// (`crate::run_offline`) at process start — never by the net client, the net server, or the
/// sandboxes — and never inserted or removed mid-session (regime flips would reinterpret hidden
/// elastic state; see the checklist's mid-session-connection section).
#[derive(Resource, Default)]
pub struct ElementGripFeelTest;

/// The static-friction state (static-friction-design.md, ADR-0026): per-side elastic grip
/// resultants (N), `[left, right] × [longitudinal, lateral/ρ]`. Generalized forces, NOT
/// world anchors. Owner-predicted, replicated, rolled back like [`TrackDrive`] — but a
/// SEPARATE component because grip is measured in newtons and needs its own attributed
/// rollback threshold. Hashed into the determinism trace (`hblt`).
#[derive(Component, Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TrackGrip {
    pub sides: [[f32; 2]; 2],
}

/// Last tick's contact telemetry per side — viz/diagnostics ONLY (debug force arrows, the
/// grounded count, traces). Rewritten every tick, never hashed, never rolled back.
#[derive(Component, Default)]
pub struct TrackContacts(pub [Vec<BeltContact>; 2]);

/// The per-element grip state, `[left, right]` (one [`GripElements`] per side): one world-space
/// shear vector + loss dwell per material link × lateral column. A plain LOCAL component —
/// NOT registered in the net protocol, never serialized, never hashed (this is REV 13; the
/// wire promotion is REV 14, element-netcode-design.md).
///
/// Constructed at tank spawn with both slabs pre-sized `link_count * 3`
/// ([`Self::for_links`], called from the two root-construction paths in `tank::spawn`) — the
/// REV-14 fixed-size invariant: `step_side` never resizes at runtime, because a runtime
/// rebuild silently erases strain a rollback replay would then trust. Attached to EVERY tank
/// root (MP included): construction belongs to the one shared spawn path, sized synchronously
/// from the same spec that sizes `TankSim`. MP never reads or writes it — `apply_track_forces`
/// only touches it under the offline [`ElementGripFeelTest`] gate, so on the net client/server
/// it is inert zeroed memory.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct TrackGripElements {
    pub sides: [GripElements; 2],
}

impl TrackGripElements {
    /// Both sides pre-sized for `link_count` material links (see the type doc).
    pub fn for_links(link_count: usize) -> Self {
        Self {
            sides: [
                GripElements::for_links(link_count),
                GripElements::for_links(link_count),
            ],
        }
    }
}

/// The blueprint's running gear as force-station geometry, built once (single blueprint
/// today; per-variant when a second vehicle lands): the closed rest pin-line loop, the
/// station count, the side planes, and the assembled [`ForceParams`].
#[derive(Resource)]
pub struct TrackGear {
    loop_pts: Vec<Vec2>,
    count: usize,
    plane_x: f32,
    params: ForceParams,
}

pub fn sim_plugin(app: &mut App) {
    // Lazy one-shot: the blueprint lands at Startup (bake); the gear builds on the first
    // frame after and never again.
    app.add_systems(
        PreUpdate,
        init_track_gear
            .run_if(resource_exists::<TankBlueprint>.and_then(not(resource_exists::<TrackGear>))),
    );
    app.add_systems(
        FixedUpdate,
        apply_track_forces
            .in_set(SimPhase::DrivingForces)
            .in_set(GameplaySet),
    );
}

/// Build [`TrackGear`] from the baked blueprint: same rest circles as the view's feasibility
/// gate, closed via `build_route` with the authored material length (station pitch is then
/// EXACTLY the spec pitch — loop length ≡ pitch × count).
fn init_track_gear(blueprint: Res<TankBlueprint>, mut commands: Commands) {
    let spec = &blueprint.spec.track;
    let sprocket_r = spec.pitch * spec.sprocket.teeth as f32 / std::f32::consts::TAU;
    let mut circles = vec![(
        Vec2::new(spec.sprocket.center.0, spec.sprocket.center.1),
        sprocket_r,
    )];
    let mut wheels: Vec<Vec2> = blueprint
        .geometry
        .roadwheels
        .iter()
        .filter(|(_, side)| *side == TrackSide::Left)
        .map(|&(node, _)| {
            let t = blueprint.geometry.nodes[node].root_position;
            Vec2::new(t.z, t.y)
        })
        .collect();
    wheels.sort_by(|a, b| a.x.total_cmp(&b.x));
    circles.extend(wheels.into_iter().map(|c| (c, spec.wheel_radius)));
    circles.push((
        Vec2::new(spec.idler.center.0, spec.idler.center.1),
        spec.idler.radius,
    ));

    let belt_len = spec.pitch * spec.link_count as f32;
    let route = build_route(&circles, belt_len);
    let mut loop_pts = route.pts.clone();
    if loop_pts.first() != loop_pts.last()
        && let Some(&first) = loop_pts.first()
    {
        loop_pts.push(first);
    }

    commands.insert_resource(TrackGear {
        loop_pts,
        count: spec.link_count,
        plane_x: spec.plane_x,
        params: ForceParams {
            thickness: spec.thickness,
            columns: [
                (-spec.width / 2.0, 1.0 / 6.0),
                (0.0, 2.0 / 3.0),
                (spec.width / 2.0, 1.0 / 6.0),
            ],
            support_stiffness_per_m: spec.support.stiffness_per_m,
            support_damping_per_m: spec.support.damping_per_m,
            engage_depth: spec.support.engage,
            probe_reach: 0.5,
            mu: MU,
            lateral_ratio: LATERAL_GRIP_RATIO,
            slip_saturation: SLIP_SATURATION,
            max_speed: spec.powertrain.max_speed,
            engine_power: spec.powertrain.power,
            engine_force: spec.powertrain.force,
            governor_gain: spec.powertrain.governor_gain,
            inertia: spec.powertrain.inertia,
            // Derived from authored mass via the declared park target (forces.rs) — not a
            // spec field: the target is model policy, the vehicle datum is its weight.
            grip_stiffness: grip_stiffness(MU, blueprint.spec.mass * 9.81),
        },
    });
}

/// The drive step: shape the command, run each side's belt force model at the tick-truth
/// pose, apply the returned forces in report order, commit the new belt state.
fn apply_track_forces(
    time: Res<Time>,
    field: Res<TrackField>,
    gear: Option<Res<TrackGear>>,
    // The offline element-grip gate (element-promotion-checklist.md Q1): present only in the
    // `--offline` composition. Read as `Option` so every other composition runs unchanged.
    feel: Option<Res<ElementGripFeelTest>>,
    volumes: Query<VolumeFacets>,
    mut tanks: Query<
        (
            &Position,
            &Rotation,
            Forces,
            &TankCommand,
            &mut TrackDrive,
            &mut TrackGrip,
            &mut TrackGripElements,
            &mut TrackContacts,
            Option<&TankVolumes>,
            Option<&TankCapabilities>,
        ),
        With<Tank>,
    >,
) {
    let Some(gear) = gear else {
        return;
    };
    let Some(oracle) = field.field.as_ref() else {
        return;
    };
    let dt = time.delta_secs();
    for (
        pos,
        rot,
        mut forces,
        command,
        mut drive,
        mut grip,
        mut grip_elements,
        mut contacts,
        tank_volumes,
        tank_caps,
    ) in &mut tanks
    {
        // Drive gates THRUST, not grip: a dead driver/engine/transmission retargets the
        // command slew to zero (a fade over ~1/DRIVE_SLEW_PER_SECOND, see the module doc)
        // while the full contact model keeps running, so the tracks keep their kinetic grip.
        let drive_ok = capability_available(tank_volumes, tank_caps, Capability::Drive, &volumes);
        let target = if drive_ok {
            DriveAxes {
                throttle: command.throttle,
                steer: command.steer,
            }
        } else {
            DriveAxes::default()
        };
        let shaped = shape_drive(
            DriveAxes {
                throttle: drive.throttle,
                steer: drive.steer,
            },
            target,
            dt,
        );
        drive.throttle = shaped.throttle;
        drive.steer = shaped.steer;
        let side_commands = shaped.side_commands();

        let affine = Affine3A::from_rotation_translation(rot.0, pos.0);

        // Fixed left-then-right — the accumulation order is part of determinism. `plane_x`'s
        // sign is the side's (`Side::plane_x` is an exact ±1 flip); `sides`/`grip.sides` stay
        // bare `[T; 2]` (replicated wire shape), indexed by `side.index()`.
        for side in Side::ALL {
            let si = side.index();
            let input = SideInput {
                loop_pts: &gear.loop_pts,
                count: gear.count,
                plane_x: side.plane_x(gear.plane_x),
                command: side_commands[si],
            };
            let ds = drive.sides[si];
            let state = SideState {
                speed: ds.speed,
                phase: ds.phase,
                grip: bevy::math::Vec2::new(grip.sides[si][0], grip.sides[si][1]),
            };
            let report = step_side(
                &input,
                state,
                affine,
                dt,
                &gear.params,
                oracle,
                |p| forces.velocity_at_point(p),
                // The element-regime gate. SAFETY ARGUMENT (this branch is the whole REV-13
                // story): `ElementGripFeelTest` is inserted ONLY by the offline composition
                // (`run_offline`) — the net client, net server, and headless server never
                // insert it, so on every MP path this expression is `None`, exactly the
                // literal `None` that stood here before the gate existed: MP behavior is
                // BIT-unchanged, and the unregistered `TrackGripElements` slabs are never
                // read or mutated (they cannot enter prediction/rollback). The regime is
                // startup-latched — never flipped mid-session (see the resource doc).
                match &feel {
                    Some(_) => Some(&mut grip_elements.sides[si]),
                    None => None,
                },
            );
            // Apply in report order — accumulation order is part of determinism.
            for app in &report.apps {
                forces.apply_force_at_point(app.force, app.point);
            }
            drive.sides[si] = TrackDriveSide {
                speed: report.state.speed,
                phase: report.state.phase,
            };
            grip.sides[si] = [report.state.grip.x, report.state.grip.y];
            contacts.0[si] = report.contacts;
        }
    }
}
