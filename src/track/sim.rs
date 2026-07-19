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

use avian3d::prelude::{
    ComputedCenterOfMass, Forces, Position, ReadRigidBodyForces, RigidBody, Rotation,
    WriteRigidBodyForces,
};
use bevy::math::{Affine3A, Vec2};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::bake::TankBlueprint;
use crate::command::TankCommand;
use crate::damage::{
    Capability, TankCapabilities, TankVolumes, VolumeFacets, capability_available,
};
use crate::spec::TransmissionArchitecture;
use crate::state::{GameplaySet, SimPhase};
use crate::tank::{Tank, TrackSide};

use super::drive::{DriveAxes, shape_drive};
use super::forces::{
    BeltContact, ForceParams, GripElements, SideInput, SideReport, SideState, contact_side,
    grip_stiffness, step_side,
};
use super::route::build_route;
use super::side::Side;
use super::terrain::TrackField;
use super::transmission::{
    self, TransmissionInput, TransmissionMode, TransmissionParams, TransmissionState,
};

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
/// instead of the per-side aggregate. Inserted only by the `--offline` composition at process
/// start; network compositions use [`ElementGripNetcode`] and sandboxes use neither. It is never
/// inserted or removed mid-session because a regime flip would reinterpret hidden elastic state.
#[derive(Resource, Default)]
pub struct ElementGripFeelTest;

/// Startup-latched marker for the network compositions' promoted per-element grip regime.
/// The server inserts it once and simulates every dynamic authority tank; the client inserts it
/// once but only its owner-predicted dynamic body uses the field (remote bodies remain static and
/// interpolated). The ordinary offline composition does not insert it, preserving its aggregate
/// behavior bit-for-bit; [`ElementGripFeelTest`] keeps the existing opt-in offline A/B path.
#[derive(Resource, Default)]
pub struct ElementGripNetcode;

/// The OFFLINE transmission feel-test override: which drivetrain adapter [`apply_track_forces`]
/// runs instead of the vehicle's declared architecture. Inserted ONLY by the `--offline`
/// composition; MP has no dial and follows the spec. Unlike the element gate this one is live (the
/// offline `T` key cycles governor → hybrid → L600): [`TankTransmission`] resets on every flip, so
/// a mid-session mode change cannot poison hidden state the way an element-regime flip would.
#[derive(Resource)]
pub struct TransmissionFeelTest(pub TransmissionMode);

/// The joint transmission's path-dependent state (gear/window/detent/direction/crank plus the
/// stage-C demand/filter/target/hill-hold scheduler state). REV 14 replicates this one atomic root
/// component: server-authoritative, predicted and rolled back for the owner, visible but not
/// predicted for remote tanks. Server and owning client both advance it through the same
/// spec-selected branch of [`apply_track_forces`]. The determinism trace hashes all 16 inventory
/// fields in stable order, with raw bits for both floats.
#[derive(Component, Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct TankTransmission(pub TransmissionState);

impl TankTransmission {
    pub fn from_spec(tp: &TransmissionParams) -> Self {
        Self(TransmissionState::from_spec(tp))
    }

    pub(crate) fn for_governor() -> Self {
        Self(TransmissionState::for_governor())
    }
}

/// The static-friction state (static-friction-design.md, ADR-0026): per-side elastic grip
/// resultants (N), `[left, right] × [longitudinal, lateral/ρ]`. Generalized forces, NOT
/// world anchors. Hashed into the determinism trace (`hblt`).
///
/// Off the wire as of REV 15: in element mode this is derived telemetry, and rolling it back from
/// ordinary replication would create the forbidden correction-free loop when the undisclosed
/// [`TrackGripElements`] differ. The aggregate offline force path remains untouched; Phase 4 decides
/// whether its compatibility law is retired. [`TrackGripEffect`] is the reconciliation effect summary.
#[derive(Component, Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TrackGrip {
    pub sides: [[f32; 2]; 2],
}

/// Local per-tick rigid-body/belt effect of track traction. The force and torque already include
/// every per-element damping contribution because they are accumulated from the final emitted
/// traction applications. Locally rollback-historied so a server anchor can be compared at the
/// tick that produced it rather than against the client's present.
#[derive(Component, Clone, Copy, PartialEq, Debug, Default)]
pub struct TrackGripEffect {
    /// Total world-space traction force on the hull (N).
    pub traction_force: Vec3,
    /// Total world-space traction torque about the hull center of mass (N*m).
    pub traction_torque: Vec3,
    /// Longitudinal ground reaction on `[left, right]` belts (N).
    pub belt_reaction: [f32; 2],
    /// Coarse, quantized digest of the complete element field. Diagnostic/request evidence only;
    /// it never triggers rollback without an exact checkpoint.
    pub field_digest: u32,
}

/// Monotonic notification that an explicit hull impulse was applied this tick.
///
/// The server's rest-epoch detector consumes generation changes so recoil and projectile hits wake
/// a parked field on the impulse tick. This is bookkeeping only: it is neither rollback state nor
/// an input to the force law, and therefore cannot gate or alter local physics.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct TrackGripWake {
    generation: u32,
}

impl TrackGripWake {
    pub(crate) fn record_impulse(&mut self, impulse: Vec3) {
        if impulse != Vec3::ZERO {
            self.generation = self.generation.wrapping_add(1);
        }
    }

    pub(crate) fn generation(self) -> u32 {
        self.generation
    }
}

/// Last tick's contact telemetry per side — viz/diagnostics ONLY (debug force arrows, the
/// grounded count, traces). Rewritten every tick, never hashed, never rolled back.
#[derive(Component, Default)]
pub struct TrackContacts(pub [Vec<BeltContact>; 2]);

/// The per-element grip state, `[left, right]` (one [`GripElements`] per side): one world-space
/// shear vector + loss dwell per material link × lateral column. REV 15 transmits one exact
/// owner-private initialization snapshot, then restores this component from local rollback history;
/// sparse exact checkpoints provide later authoritative convergence.
///
/// Constructed at tank spawn with both slabs pre-sized `link_count * 3`
/// ([`Self::for_links`], called by the authoritative/shared root construction path) — the
/// REV-15 fixed-size invariant: `step_side` never resizes at runtime, because a runtime
/// rebuild silently erases strain a rollback replay would then trust. Attached to EVERY tank
/// authority root. A predicted joining replica waits for that exact fixed-size snapshot before its
/// body attaches; interpolated remotes neither receive nor simulate the private field.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[require(TrackGripEffect)]
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

    /// Whether both fixed slabs match the spec-authored material-link count.
    pub fn is_sized_for(&self, link_count: usize) -> bool {
        let expected = link_count * 3;
        self.sides
            .iter()
            .all(|side| side.strain.len() == expected && side.dwell.len() == expected)
    }
}

/// Coarse field digest used by [`TrackGripEffect`] and the replicated anchor. Strain axes are
/// projected to signed 8-bit bins across the force law's exact `[-K, K]` range before FNV-1a. This
/// deliberately ignores sub-bin float noise; the exact checkpoint/hash path remains raw-bit exact.
pub(crate) fn coarse_grip_digest(elements: &TrackGripElements) -> u32 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut write = |byte: u8| {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for (side_index, side) in elements.sides.iter().enumerate() {
        write(side_index as u8);
        for (element, (&strain, &dwell)) in side.strain.iter().zip(&side.dwell).enumerate() {
            for byte in (element as u16).to_le_bytes() {
                write(byte);
            }
            for axis in strain.to_array() {
                let bin = (axis / super::forces::GRIP_SHEAR_MODULUS_M * 127.0)
                    .round()
                    .clamp(-127.0, 127.0) as i8;
                write(bin.to_le_bytes()[0]);
            }
            write(dwell);
        }
    }
    (hash ^ (hash >> 32)) as u32
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
    /// The declared joint transmission, if the spec authors one.
    trans: Option<TransmissionParams>,
    /// Spec-selected adapter. `Governor` means the transmission block is absent.
    mode: TransmissionMode,
}

impl TrackGear {
    /// The declared joint transmission params, if the spec authored one. Read-only accessor for
    /// the offline drive HUD (`crate::run_offline`); the field stays private so only the drive step
    /// and HUD legend consume it.
    pub fn trans(&self) -> Option<&TransmissionParams> {
        self.trans.as_ref()
    }

    /// Per-side reflected belt inertia used by the anchor's physical belt-speed error metric.
    pub(crate) fn belt_inertia(&self) -> f32 {
        self.params.inertia
    }

    /// Test-only variant fixture seam: headless gates may vary a declared transmission capability
    /// without rebuilding the Tiger asset/spec. Production callers get read-only params.
    #[cfg(test)]
    pub(crate) fn trans_mut(&mut self) -> Option<&mut TransmissionParams> {
        self.trans.as_mut()
    }
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

    // The declared transmission, derived from the authored tables against the spec's OWN
    // sprocket radius (tiger-transmission-data.md rule: speeds are the anchors, reductions
    // derive, so the ladder survives the 19-vs-20-tooth discrepancy).
    if let Some(tr) = &spec.powertrain.transmission {
        info!(
            "declared transmission: {:?}, {}F/{}R",
            tr.architecture,
            tr.gearbox.forward_speeds_kmh.len(),
            tr.gearbox.reverse_speeds_kmh.len()
        );
    }
    let trans = spec
        .transmission_params()
        .expect("TankSpec transmission was validated before TrackGear construction");
    let mode = spec
        .powertrain
        .transmission
        .as_ref()
        .map_or(TransmissionMode::Governor, |tr| match tr.architecture {
            TransmissionArchitecture::Hybrid => TransmissionMode::Hybrid,
            TransmissionArchitecture::FixedRadii => TransmissionMode::FixedRadii,
        });

    commands.insert_resource(TrackGear {
        loop_pts,
        count: spec.link_count,
        plane_x: spec.plane_x,
        trans,
        mode,
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
    // Present only in network compositions. Dynamic authority/owner bodies use elements; static
    // interpolated client bodies never do.
    net_elements: Option<Res<ElementGripNetcode>>,
    // Offline-only adapter override. MP leaves it absent and follows `TrackGear::mode`, derived
    // from the spec; a missing transmission block selects the untouched Governor fallback.
    trans_feel: Option<Res<TransmissionFeelTest>>,
    volumes: Query<VolumeFacets>,
    mut tanks: Query<
        (
            &Position,
            &Rotation,
            &ComputedCenterOfMass,
            &RigidBody,
            Forces,
            &TankCommand,
            &mut TrackDrive,
            &mut TrackGrip,
            &mut TrackGripElements,
            &mut TrackGripEffect,
            &mut TankTransmission,
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
    let mode = trans_feel.as_ref().map(|r| r.0).unwrap_or(gear.mode);
    let dt = time.delta_secs();
    for (
        pos,
        rot,
        center_of_mass,
        body,
        mut forces,
        command,
        mut drive,
        mut grip,
        mut grip_elements,
        mut grip_effect,
        mut trans_state,
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
        // avian3d 0.7 `ForcesItem` keeps this helper private; this is its version-pinned source
        // expression (`query_data.rs`): position + rotation * local computed COM.
        let center_of_mass = pos.0 + rot.0 * center_of_mass.0;
        let elements_enabled =
            feel.is_some() || (net_elements.is_some() && matches!(*body, RigidBody::Dynamic));
        let mut effect = TrackGripEffect::default();

        // The JOINT drivetrain branch: MP selects the declared architecture; the offline-only
        // [`TransmissionFeelTest`] can override it. A spec-less vehicle or explicit Governor
        // override falls through to the untouched legacy loop below.
        let joint = match (mode, gear.trans.as_ref()) {
            (TransmissionMode::Governor, _) | (_, None) => None,
            (m, Some(tp)) => Some((m, tp)),
        };
        if let Some((mode, tp)) = joint {
            // Transmission-design §2 scheduling: evaluate BOTH contact patches at their
            // pre-tick belt speeds, solve the joint transmission once, integrate both
            // speeds, advect both phases. Emitting all of L's forces then all of R's keeps
            // the legacy accumulation order — within a tick force application never feeds
            // back into `vel_at`, so contact evaluation order cannot change the numbers.
            let mut reports: [SideReport; 2] = [SideReport::default(), SideReport::default()];
            let mut live = [false; 2];
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
                let (report, ok) = contact_side(
                    &input,
                    state,
                    affine,
                    dt,
                    &gear.params,
                    oracle,
                    |p| forces.velocity_at_point(p),
                    // Same startup-latched element-regime gate as the legacy loop.
                    elements_enabled.then_some(&mut grip_elements.sides[si]),
                );
                reports[si] = report;
                live[si] = ok;
            }
            let tr = transmission::step(
                mode,
                &gear.params,
                Some(tp),
                &mut trans_state.0,
                &TransmissionInput {
                    throttle: drive.throttle,
                    steer: drive.steer,
                    side_commands,
                    speeds: [drive.sides[0].speed, drive.sides[1].speed],
                    reactions: [reports[0].belt_reaction, reports[1].belt_reaction],
                    dt,
                },
            );
            for (si, report) in reports.into_iter().enumerate() {
                effect.belt_reaction[si] = report.belt_reaction;
                for contact in &report.contacts {
                    effect.traction_force += contact.traction;
                    effect.traction_torque +=
                        (contact.point - center_of_mass).cross(contact.traction);
                }
                for app in &report.apps {
                    forces.apply_force_at_point(app.force, app.point);
                }
                if live[si] {
                    let pre_speed = drive.sides[si].speed;
                    drive.sides[si] = TrackDriveSide {
                        speed: tr.next_speeds[si],
                        // Phase advects at the PRE-update speed, like the legacy tail.
                        phase: drive.sides[si].phase + f64::from(pre_speed * dt),
                    };
                }
                grip.sides[si] = [report.state.grip.x, report.state.grip.y];
                contacts.0[si] = report.contacts;
            }
            effect.field_digest = coarse_grip_digest(&grip_elements);
            *grip_effect = effect;
            continue;
        }

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
                // Startup-latched regime selection. Offline defaults to the aggregate law unless
                // its feel-test resource is present. Network compositions insert
                // `ElementGripNetcode`; dynamic authority/owner bodies use elements while static
                // interpolated remotes do not simulate their private field.
                elements_enabled.then_some(&mut grip_elements.sides[si]),
            );
            effect.belt_reaction[si] = report.belt_reaction;
            for contact in &report.contacts {
                effect.traction_force += contact.traction;
                effect.traction_torque += (contact.point - center_of_mass).cross(contact.traction);
            }
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
        effect.field_digest = coarse_grip_digest(&grip_elements);
        *grip_effect = effect;
    }
}
