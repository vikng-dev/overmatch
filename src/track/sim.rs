//! The locomotion sim (phase B): the track model's belt forces ARE how tanks drive. The ECS
//! adapter over [`super::forces`] — one deep boundary; support, traction, and belt dynamics
//! live behind `contact_side` + `transmission::step` (one joint form),
//! this module owns queries, scheduling, capability gating, and the netcode-visible
//! [`TrackDrive`] state.
//!
//! Sim discipline (hard rules, each bought with a measured MP failure in the raycast sim this
//! replaces):
//! - Pose from tick-truth `Position`/`Rotation`, never `GlobalTransform` (render lag differs
//!   per machine).
//! - Terrain from the analytic [`TrackField`]; building and loose-piece support from a
//!   tick-start shape snapshot ([`RubbleBodies`]), checked across the complete shoe footprint.
//! - Runs every tick unconditionally (this is sim state); stays inside
//!   `SimPhase::DrivingForces` so drive samples velocity before the weapon-fire impulse.
//! - `Drive` capability gates the COMMAND, not the contact model: a dead engine still has
//!   kinetic grip (the slip law keeps resisting motion — though it creeps on slopes, ADR-0025);
//!   it just cannot thrust. The cut is not instant: the lost capability retargets the command
//!   slew, so thrust fades over ~1/[`super::drive::DRIVE_SLEW_PER_SECOND`] s — deliberate, the
//!   same shaping as a released key, making capability loss/recovery feel mechanical.

use avian3d::dynamics::rigid_body::forces::ForcesItem;
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
use crate::state::{GameplaySet, SimPhase};
use crate::tank::Tank;

#[cfg(feature = "bitprobe")]
use crate::bitprobe::BitprobeCapture;

use super::drive::{DriveAxes, shape_drive};
use super::forces::{
    BeltContact, ForceParams, GripElements, GripParams, SideInput, SideReport, SideState,
    contact_side,
};
use super::oracle::TerrainOracle;
use super::route::build_route;
use super::rubble::{
    PieceData, RubbleBodies, RubbleQuery, apply_pushes, footprint_radius, piece_impulses,
};
use super::side::Side;
use super::terrain::TrackField;
use super::transmission::{
    self, TransmissionInput, TransmissionParams, TransmissionReport, TransmissionState,
};

const SLIP_SATURATION: f32 = 0.4;

/// Per-tank belt state: server-authoritative, replicated to every client for the track view
/// (quantised, `net::wire_quant`). Hashed into the determinism trace (`hblt`).
#[derive(Component, Clone, Copy, PartialEq, Debug, Default)]
pub struct TrackDrive {
    /// Per-side belt state, `[left, right]`.
    pub sides: [TrackDriveSide; 2],
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct TrackDriveSide {
    /// Belt surface speed (m/s).
    pub speed: f32,
    /// Belt travel (m) — advects the force stations; the view's exact scroll phase. Unbounded
    /// where the tank simulates; on a replica, known only modulo [`BELT_PHASE_WRAP_M`].
    pub phase: f64,
}

/// The length (m) a replicated belt phase is known modulo: the wire carries it wrapped into
/// `[0, BELT_PHASE_WRAP_M)`, so a replica's phase jumps by this length where the authority's
/// crosses a multiple of it. Every reader of a phase takes differences through
/// [`wrapped_phase_delta`], so none needs it to divide a link pitch — and no generic length could.
/// A power of two, so the wire's power-of-two step tiles it exactly.
pub const BELT_PHASE_WRAP_M: f64 = 1024.0;

/// `to − from` as the shortest travel modulo [`BELT_PHASE_WRAP_M`]: exact for unbounded phases
/// whose true difference is under half the wrap (512 m of belt — 25 s at 20 m/s), and equally
/// right for wrapped ones.
pub fn wrapped_phase_delta(from: f64, to: f64) -> f64 {
    let delta = to - from;
    delta - BELT_PHASE_WRAP_M * (delta / BELT_PHASE_WRAP_M).round()
}

/// The shaped drive signal in [−1, 1]: `TankCommand` targets slewed through
/// [`super::drive::shape_drive`]. Sim state (not command) so every tank responds with the same
/// feel. Server-authoritative and replicated to the tank's OWNER only (its drive HUD); no other
/// client reads it.
#[derive(Component, Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct DriveSignal {
    pub throttle: f32,
    pub steer: f32,
}

/// The joint transmission's path-dependent state (gear/window/detent/direction/crank plus the
/// stage-C demand/filter/target/hill-hold scheduler state). Server-authoritative, advanced only
/// where the body is dynamic through [`apply_track_forces`], and
/// replicated as one atomic snapshot to the tank's OWNER only; every other listener reads the
/// crank through [`EngineRpm`]. The determinism trace hashes every field of the exhaustive
/// projection (`transmission::transmission_state_projection`) in stable order, with raw bits for
/// both floats.
#[derive(Component, Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct TankTransmission(pub TransmissionState);

impl TankTransmission {
    pub fn from_spec(tp: &TransmissionParams) -> Self {
        Self(TransmissionState::from_spec(tp))
    }
}

/// The public crank speed, in whole rpm: the one transmission fact a listener other than the
/// tank's owner needs (its engine sound). Written from [`TankTransmission`] by
/// [`publish_engine_rpm`] wherever the body simulates, constructed beside it at spawn, and
/// replicated to every client.
///
/// Error against the crank: ≤ 0.5 rpm (round to nearest) over 0–65 535 rpm; a crank outside that
/// band saturates to its bound, a NaN one reads 0. Postcard writes the `u16` as a varint:
/// 2 bytes from 128 to 16 383 rpm.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EngineRpm(pub u16);

impl EngineRpm {
    pub fn of(transmission: &TankTransmission) -> Self {
        // `as` saturates at both bounds and maps NaN to 0.
        Self((transmission.0.omega_e / transmission::RPM_TO_RAD).round() as u16)
    }

    pub fn rpm(self) -> f32 {
        f32::from(self.0)
    }
}

/// Per-side summed element grip force (N), `[left, right] × [longitudinal, lateral]` —
/// derived telemetry over [`TrackGripElements`] (the authoritative state). Generalized
/// forces, NOT world anchors. Hashed into the determinism trace (`hblt`).
///
/// Off the wire as of REV 15: rolling derived telemetry back from ordinary replication would
/// create the forbidden correction-free loop when the undisclosed [`TrackGripElements`]
/// differ. [`TrackGripEffect`] is the reconciliation effect summary.
#[derive(Component, Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TrackGrip {
    pub sides: [[f32; 2]; 2],
}

/// Local per-tick rigid-body/belt effect of track traction. The force and torque already include
/// every per-element damping contribution because they are accumulated from the final emitted
/// traction applications.
#[derive(Component, Clone, Copy, PartialEq, Debug, Default)]
pub struct TrackGripEffect {
    /// Total world-space traction force on the hull (N).
    pub traction_force: Vec3,
    /// Total world-space traction torque about the hull center of mass (N*m).
    pub traction_torque: Vec3,
    /// Longitudinal ground reaction on `[left, right]` belts (N).
    pub belt_reaction: [f32; 2],
    /// Coarse, quantized digest of the complete element field. Diagnostic evidence only.
    pub field_digest: u32,
}

/// Apply an explicit hull impulse at a world point (retaining its torque arm). Forces-based, so
/// avian wakes a sleeping body on application.
pub(crate) fn apply_explicit_impulse(mut forces: ForcesItem<'_, '_>, impulse: Vec3, point: Vec3) {
    forces.apply_linear_impulse_at_point(impulse, point);
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
/// REV-15 fixed-size invariant: `contact_side` never resizes at runtime, because a runtime
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
    loop_pts: [Vec<Vec2>; 2],
    count: usize,
    plane_x: [f32; 2],
    /// The contact envelope's per-position reach profile ((z, travel) knots — 0 at the
    /// unsprung sprocket/idler, full droop at the road wheels), ridden via
    /// [`super::forces::TravelField`] every tick.
    travel_knots: [Vec<(f32, f32)>; 2],
    /// Per-side collocation columns ([`SideInput::columns`], indexed by `Side::index()`):
    /// the MEASURED shoe faces + centre from
    /// [`RigGeom::grip_column_offsets`](super::rig_geom::RigGeom::grip_column_offsets) — signed
    /// per-side because the shoe is not centred on its pins (~17 mm outboard on the Tiger).
    columns: [[(f32, f32); 3]; 2],
    params: ForceParams,
    /// DERIVED bound over both sides' complete support slabs, in body-local metres.
    rubble_radius: f32,
    /// The declared joint transmission tables, derived against the measured geometry.
    trans: TransmissionParams,
}

impl TrackGear {
    /// The declared joint transmission params. Read-only accessor for the shared drive HUD; the
    /// field stays private so only the drive step and HUD consume it.
    pub fn trans(&self) -> &TransmissionParams {
        &self.trans
    }

    /// The chain-clamped droop (m): the maximum wheel-travel knot — how far below its rest line a
    /// road wheel may drop. `0.0` when the profile is empty. The track view spans its cosmetic
    /// wheels this far below rest so a fully-drooped wheel's drawn belt wrap stays feasible at the
    /// chain-limited link count (the knots' peak IS `RigGeom::droop_travel(..).effective`, floored).
    pub(crate) fn max_droop(&self, side: Side) -> f32 {
        self.travel_knots[side.index()]
            .iter()
            .map(|&(_, travel)| travel)
            .fold(0.0_f32, f32::max)
    }

    /// The measured plate face offset (m): pin line → outer ground face (`pin_to_outer`). The view's
    /// wheel-probe reach adds it to the pin-line wheel radius — no mid-plate assumption.
    pub(crate) fn face_offset(&self) -> f32 {
        self.params.face_offset
    }

    #[cfg(feature = "bitprobe")]
    pub(crate) fn bitprobe_startup(&self, out: &mut crate::bitprobe::StartupBuilder) {
        out.u32("track_gear.count", self.count as u32);
        for side in Side::ALL {
            let si = side.index();
            out.f32(&format!("track_gear.{side:?}.plane_x"), self.plane_x[si]);
            for (index, point) in self.loop_pts[si].iter().enumerate() {
                out.vec2(&format!("track_gear.{side:?}.route[{index}]"), *point);
            }
        }
        let p = &self.params;
        out.f32("force.face_offset", p.face_offset);
        out.f32("force.free_travel", p.free_travel);
        for side in Side::ALL {
            for (index, (z, travel)) in self.travel_knots[side.index()].iter().copied().enumerate()
            {
                out.f32(&format!("force.{side:?}.travel_knots[{index}].z"), z);
                out.f32(
                    &format!("force.{side:?}.travel_knots[{index}].travel"),
                    travel,
                );
            }
        }
        for (si, side) in ["left", "right"].into_iter().enumerate() {
            for (index, (offset, weight)) in self.columns[si].iter().copied().enumerate() {
                out.f32(&format!("force.columns.{side}[{index}].offset"), offset);
                out.f32(&format!("force.columns.{side}[{index}].weight"), weight);
            }
        }
        out.f32("force.support_stiffness_per_m", p.support_stiffness_per_m);
        out.f32("force.support_damping_per_m", p.support_damping_per_m);
        out.f32("force.engage_depth", p.engage_depth);
        out.f32("force.probe_reach", p.probe_reach);
        out.f32("force.slip_saturation", p.slip_saturation);
        out.f32("force.inertia", p.inertia);
        let grip = p.grip.expect("the game's running gear grips");
        out.f32("force.grip.track", grip.track.dynamic_coefficient);
        out.f32("force.grip.ground", grip.ground.dynamic_coefficient);
        out.f32("force.grip.side_weight", grip.side_weight);

        let tp = &self.trans;
        out.f32("transmission.engine.idle_rpm", tp.engine.idle_rpm);
        out.f32("transmission.engine.governed_rpm", tp.engine.governed_rpm);
        for (index, (rpm, torque)) in tp.engine.torque_nm.iter().copied().enumerate() {
            out.f32(&format!("transmission.engine.torque[{index}].rpm"), rpm);
            out.f32(
                &format!("transmission.engine.torque[{index}].torque_nm"),
                torque,
            );
        }
        for (index, value) in tp.gears_fwd.iter().copied().enumerate() {
            out.f32(&format!("transmission.gears_fwd[{index}]"), value);
        }
        for (index, value) in tp.gears_rev.iter().copied().enumerate() {
            out.f32(&format!("transmission.gears_rev[{index}]"), value);
        }
        out.f32("transmission.sprocket_radius", tp.sprocket_radius);
        out.f32("transmission.shift_up_rpm", tp.shift_up_rpm);
        out.f32("transmission.shift_down_rpm", tp.shift_down_rpm);
        for (index, (tight, wide)) in tp.steer_kappa.iter().copied().enumerate() {
            out.f32(&format!("transmission.steer_kappa[{index}].tight"), tight);
            out.f32(&format!("transmission.steer_kappa[{index}].wide"), wide);
        }
        for (index, (tight, wide)) in tp.steer_radii_m.iter().copied().enumerate() {
            out.f32(&format!("transmission.steer_radii[{index}].tight"), tight);
            out.f32(&format!("transmission.steer_radii[{index}].wide"), wide);
        }
        out.f32("transmission.steer_capacity_n", tp.steer_capacity_n);
        out.f32("transmission.neutral_d_full", tp.neutral_d_full);
        out.f32("transmission.recirculation", tp.recirculation);
        out.f32("transmission.brake_capacity_n", tp.brake_capacity_n);
        out.f32("transmission.brake_static_factor", tp.brake_static_factor);
        out.f32("transmission.drag_fraction", tp.drag_fraction);
        out.f32("transmission.engine_inertia", tp.engine_inertia);
        out.f32("transmission.clutch_capacity", tp.clutch_capacity);
        out.u32("transmission.shift_ticks", u32::from(tp.shift_ticks));
        out.u32(
            "transmission.shift_addressing",
            match tp.shift_addressing {
                super::transmission::ShiftAddressing::Direct => 0,
                super::transmission::ShiftAddressing::Sequential => 1,
            },
        );
        out.f32("transmission.peak_torque_rpm", tp.peak_torque_rpm);
        out.f32("transmission.peak_torque_nm", tp.peak_torque_nm);
    }

    /// Test-only variant fixture seam: headless gates may vary a declared transmission capability
    /// without rebuilding the Tiger asset/spec. Production callers get read-only params.
    #[cfg(test)]
    pub(crate) fn trans_mut(&mut self) -> &mut TransmissionParams {
        &mut self.trans
    }

    /// Test-only variant fixture seam for the contact law, as [`Self::trans_mut`] is for the
    /// transmission.
    #[cfg(test)]
    pub(crate) fn params_mut(&mut self) -> &mut ForceParams {
        &mut self.params
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
        // Chained: the published rpm is this tick's crank, which `apply_track_forces` writes.
        (apply_track_forces, publish_engine_rpm)
            .chain()
            .in_set(SimPhase::DrivingForces)
            .in_set(GameplaySet),
    );
}

/// Mirror each simulated crank onto its public [`EngineRpm`]. The same dynamic-body gate as
/// [`apply_track_forces`]: a replica's `EngineRpm` arrives from its authority and is never
/// rewritten locally. `set_if_neq`, so an unchanged whole rpm costs no change tick and no resend.
fn publish_engine_rpm(
    mut tanks: Query<(&RigidBody, &TankTransmission, &mut EngineRpm), With<Tank>>,
) {
    for (body, transmission, mut rpm) in &mut tanks {
        if matches!(*body, RigidBody::Dynamic) {
            rpm.set_if_neq(EngineRpm::of(transmission));
        }
    }
}

/// Build [`TrackGear`] from the baked blueprint: the marker-derived running gear
/// ([`RigGeom`](super::rig_geom::RigGeom) — the SAME derivation the track sandbox proved),
/// closed via `build_route` with the material length, and the contact-envelope support law
/// calibrated on that loop so the authored rest pose is the flat-ground equilibrium
/// ([`super::envelope`]). The RON's sprocket/idler/wheel-radius circle fields are no longer
/// read here — geometry is measured, the spec keeps counts and the ride model.
fn init_track_gear(blueprint: Res<TankBlueprint>, mut commands: Commands) {
    let spec = &blueprint.spec.track;
    let sus = spec.suspension.params();
    let geom = super::rig_geom::RigGeom::build(
        &blueprint,
        // The SIM artifact — the same file the ballistic walk is extracted from, so the running
        // gear and the armour are measured off one set of accessor bytes (ADR-0035).
        &crate::assets::asset_root().join(crate::tank::TANK_SIM_GLB_PATH),
        spec.sprocket.teeth,
        spec.link_count,
        &sus,
    );
    let belt_len = geom.belt_len();
    let loop_pts = Side::ALL.map(|side| {
        let mut points = build_route(geom.rest.get(side), belt_len).pts;
        let first = *points
            .first()
            .expect("a certified running gear has a closed route");
        if points.last() != Some(&first) {
            points.push(first);
        }
        points
    });
    let droop = geom.droop_travel(&sus);
    let columns = Side::ALL.map(|side| geom.grip_column_offsets(side));
    let travel_knots = Side::ALL.map(|side| {
        super::envelope::wheel_travel_knots(
            geom.rest.get(side),
            geom.side_droop_travel(side, &sus).effective,
        )
    });
    let cal = super::envelope::calibrate(
        &[Side::Left, Side::Right].map(|s| super::envelope::EnvelopeSide {
            loop_pts: &loop_pts[s.index()],
            plane_x: geom.side_plane_x(s),
            knots: &travel_knots[s.index()],
            columns: columns[s.index()],
        }),
        spec.link_count,
        geom.model.pin_to_outer,
        spec.suspension.engage,
        0.5,
        blueprint.spec.mass * super::derive::G,
        droop.effective,
        sus.ride_frequency,
        sus.damping_ratio,
    );
    info!(
        "envelope law: travel {:.1} mm ({}), k {:.0} kN/m per m, c {:.2} kN·s/m per m \
         (ride {:.2} Hz, ζ {:.2})",
        cal.free_travel * 1e3,
        if droop.chain_limited() {
            "CHAIN-limited"
        } else {
            "spring-limited"
        },
        cal.stiffness_per_m / 1e3,
        cal.damping_per_m / 1e3,
        sus.ride_frequency,
        sus.damping_ratio,
    );

    // The declared transmission, derived from the authored tables against the MEASURED sprocket
    // radius and half-tread (tiger-transmission-data.md rule: speeds are the anchors, reductions
    // derive, so the ladder survives the 19-vs-20-tooth discrepancy).
    let gearbox = &spec.powertrain.transmission.gearbox;
    info!(
        "declared transmission: {}F/{}R",
        gearbox.forward_speeds_kmh.len(),
        gearbox.reverse_speeds_kmh.len()
    );
    // Measured geometry: the sprocket pin-line pitch radius is the rest running gear's first
    // circle (chord-exact `derive::sprocket_pitch_radius(geom.pitch, teeth)` by construction), and
    // the half-tread is `geom.plane_x` — the same measured values the belt/envelope ride.
    let trans = spec
        .transmission_params(geom.rest.get(Side::Right)[0].1, geom.plane_x)
        .expect("TankSpec transmission was validated before TrackGear construction");

    let mut pin_radius = 0.0_f32;
    for side in Side::ALL {
        for point in &loop_pts[side.index()] {
            pin_radius =
                pin_radius.max(Vec3::new(geom.side_plane_x(side), point.y, point.x).length());
        }
    }
    let column = columns
        .iter()
        .flatten()
        .map(|(x, _)| x.abs())
        .fold(0.0_f32, f32::max);
    let travel = travel_knots
        .iter()
        .flatten()
        .map(|(_, t)| *t)
        .fold(0.0_f32, f32::max);
    let rubble_radius = footprint_radius(pin_radius, column, geom.model.pin_to_outer, travel, 0.5);
    commands.insert_resource(TrackGear {
        loop_pts,
        count: spec.link_count,
        plane_x: Side::ALL.map(|side| geom.side_plane_x(side)),
        travel_knots,
        columns,
        trans,
        rubble_radius,
        params: ForceParams {
            // The contact-envelope suspension: measured face datum, free length at the
            // droop envelope, spring/damper calibrated above — the authored rest pose is
            // the flat-ground equilibrium by construction (nothing about the support law
            // is authored except the ride model in the RON's `suspension:` block).
            face_offset: geom.model.pin_to_outer,
            free_travel: cal.free_travel,
            support_stiffness_per_m: cal.stiffness_per_m,
            support_damping_per_m: cal.damping_per_m,
            engage_depth: spec.suspension.engage,
            probe_reach: 0.5,
            slip_saturation: SLIP_SATURATION,
            inertia: spec.powertrain.inertia,
            grip: Some(GripParams::of(&blueprint.spec)),
        },
    });

    // Keep the marker-derived running gear alive as the game's single geometry source: the track
    // VIEW (`track::view`) reads its rest circles / pitch / plate / plane directly, and the bit
    // probe echoes the measured scalars. Built once here alongside `TrackGear`, never re-derived.
    commands.insert_resource(geom);
}

/// One tank's mutable belt/drivetrain state for [`belt_tick`] — the caller owns the storage
/// (ECS components for the live sim, plain fields for the recoil microsim).
pub(crate) struct BeltRig<'a> {
    pub drive: &'a mut TrackDrive,
    pub signal: &'a mut DriveSignal,
    pub grip: &'a mut TrackGrip,
    pub elements: &'a mut TrackGripElements,
    pub transmission: &'a mut TransmissionState,
}

/// ONE tank's complete drivetrain tick: command slew, both contact patches at their pre-tick
/// belt speeds, the joint transmission solve, belt speed/phase commit. THE shared law —
/// [`apply_track_forces`] (the ECS adapter) runs exactly this function, so every caller gets the
/// sim's own arithmetic, never a copy of it.
///
/// Returns the per-side reports (force applications in emission order — the caller applies or
/// integrates them, left then right, verbatim) and the transmission report. Belt state
/// (`drive.sides`, `grip.sides`) is committed in here; forces/telemetry stay with the caller.
#[expect(
    clippy::too_many_arguments,
    reason = "the complete per-tank driving tick"
)]
pub(crate) fn belt_tick<O: TerrainOracle>(
    gear: &TrackGear,
    oracle: &O,
    pieces: &RubbleBodies,
    target: DriveAxes,
    affine: Affine3A,
    dt: f32,
    vel_at: impl Fn(Vec3) -> Vec3,
    rig: BeltRig<'_>,
) -> ([SideReport; 2], TransmissionReport) {
    let shaped = shape_drive(
        DriveAxes {
            throttle: rig.signal.throttle,
            steer: rig.signal.steer,
        },
        target,
        dt,
    );
    rig.signal.throttle = shaped.throttle;
    rig.signal.steer = shaped.steer;
    let side_commands = shaped.side_commands();

    // Transmission-design §2 scheduling: evaluate BOTH contact patches at their pre-tick belt
    // speeds, solve the joint transmission once, integrate both speeds, advect both phases.
    // Within a tick force application never feeds back into `vel_at`, so contact evaluation
    // order cannot change the numbers. `sides`/`grip.sides` stay bare `[T; 2]` (replicated wire
    // shape), indexed by `side.index()`; `plane_x`'s sign is the side's (`Side::plane_x` is an
    // exact ±1 flip).
    let mut reports: [SideReport; 2] = [SideReport::default(), SideReport::default()];
    let mut live = [false; 2];
    for side in Side::ALL {
        let si = side.index();
        let input = SideInput {
            loop_pts: &gear.loop_pts[si],
            count: gear.count,
            plane_x: gear.plane_x[si],
            columns: gear.columns[si],
            travel: super::forces::TravelField {
                knots: &gear.travel_knots[si],
            },
        };
        let ds = rig.drive.sides[si];
        let state = SideState {
            speed: ds.speed,
            phase: ds.phase,
            grip: bevy::math::Vec2::new(rig.grip.sides[si][0], rig.grip.sides[si][1]),
        };
        let (report, ok) = contact_side(
            &input,
            state,
            affine,
            dt,
            &gear.params,
            oracle,
            pieces,
            &vel_at,
            &mut rig.elements.sides[si],
        );
        reports[si] = report;
        live[si] = ok;
    }
    let tr = transmission::step(
        &gear.params,
        &gear.trans,
        rig.transmission,
        &TransmissionInput {
            throttle: rig.signal.throttle,
            steer: rig.signal.steer,
            side_commands,
            speeds: [rig.drive.sides[0].speed, rig.drive.sides[1].speed],
            reactions: [reports[0].belt_reaction, reports[1].belt_reaction],
            dt,
        },
    );
    for (si, report) in reports.iter().enumerate() {
        if live[si] {
            let pre_speed = rig.drive.sides[si].speed;
            rig.drive.sides[si] = TrackDriveSide {
                speed: tr.next_speeds[si],
                // Phase advects at the PRE-update speed — `contact_side` evaluated the
                // stations there.
                phase: rig.drive.sides[si].phase + f64::from(pre_speed * dt),
            };
        }
        rig.grip.sides[si] = [report.state.grip.x, report.state.grip.y];
    }
    (reports, tr)
}

/// The drive step: shape the command, run each side's belt force model at the tick-truth
/// pose, apply the returned forces in report order, commit the new belt state.
///
/// Tanks run in entity order against one snapshot of the near pieces. Each contact advances
/// that snapshot's body velocity before the next impulse is clamped, including another tank's
/// contact on the same piece. Collider shapes keep the tick-start poses until Avian integrates.
#[expect(
    clippy::too_many_arguments,
    reason = "ECS adapter over terrain, tanks and piece reactions"
)]
fn apply_track_forces(
    time: Res<Time>,
    field: Res<TrackField>,
    gear: Option<Res<TrackGear>>,
    #[cfg(feature = "bitprobe")] mut bitprobe: Option<ResMut<BitprobeCapture>>,
    volumes: Query<VolumeFacets>,
    rubble: RubbleQuery,
    mut pieces_query: Query<PieceData, Without<Tank>>,
    mut tanks: Query<
        (
            Entity,
            &Position,
            &Rotation,
            &ComputedCenterOfMass,
            &RigidBody,
            Forces,
            &TankCommand,
            &mut TrackDrive,
            &mut DriveSignal,
            &mut TrackGrip,
            &mut TrackGripElements,
            &mut TrackGripEffect,
            &mut TankTransmission,
            &mut TrackContacts,
            (Option<&TankVolumes>, Option<&TankCapabilities>),
        ),
        With<Tank>,
    >,
) {
    #[cfg(feature = "bitprobe")]
    if let Some(capture) = bitprobe.as_deref_mut() {
        capture.clear_tick();
    }
    // Before spawn, and on a replica-only client, no dynamic tank needs driving construction.
    if !tanks
        .iter()
        .any(|(_, _, _, _, body, ..)| *body == RigidBody::Dynamic)
    {
        return;
    }
    let gear = gear
        .as_deref()
        .expect("a dynamic tank is admitted only after startup constructs TrackGear");
    let oracle = field
        .field
        .as_ref()
        .expect("a dynamic tank is admitted only after its world publishes the terrain field");
    let dt = time.delta_secs();
    let mut entities = Vec::new();
    let mut near = Vec::new();
    for (entity, pos, _, _, body, ..) in &tanks {
        // Replicas have no authoritative driving pass and gather no support shapes.
        if *body == RigidBody::Dynamic {
            entities.push(entity);
            near.extend(rubble.near(pos.0, gear.rubble_radius));
        }
    }
    entities.sort_unstable();
    near.sort_unstable();
    near.dedup();
    let mut pieces = RubbleBodies::gather(&rubble, &near, &pieces_query);
    for entity in entities {
        let (
            _,
            pos,
            rot,
            center_of_mass,
            _,
            mut forces,
            command,
            mut drive,
            mut signal,
            mut grip,
            mut grip_elements,
            mut grip_effect,
            mut trans_state,
            mut contacts,
            (tank_volumes, tank_caps),
        ) = tanks
            .get_mut(entity)
            .expect("a tank gathered for this synchronous tick remains alive");
        #[cfg(feature = "bitprobe")]
        if let Some(capture) = bitprobe.as_deref_mut() {
            capture.tanks_seen += 1;
            capture.command = [command.throttle, command.steer];
        }
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

        let affine = Affine3A::from_rotation_translation(rot.0, pos.0);
        // avian3d 0.7 `ForcesItem` keeps this helper private; this is its version-pinned
        // source expression (`query_data.rs`): position + rotation * local computed COM.
        let center_of_mass = pos.0 + rot.0 * center_of_mass.0;
        let mut effect = TrackGripEffect::default();

        // The shared per-tank drivetrain tick — contact evaluation, joint transmission, belt
        // commit — with this adapter supplying Avian's live velocity field.
        #[cfg_attr(
            not(feature = "bitprobe"),
            expect(
                unused_mut,
                unused_variables,
                reason = "only the bitprobe capture consumes"
            )
        )]
        let (mut reports, tr) = belt_tick(
            gear,
            oracle,
            &pieces,
            target,
            affine,
            dt,
            |p| forces.velocity_at_point(p),
            BeltRig {
                drive: &mut drive,
                signal: &mut signal,
                grip: &mut grip,
                elements: &mut grip_elements,
                transmission: &mut trans_state.0,
            },
        );
        #[cfg(feature = "bitprobe")]
        if let Some(capture) = bitprobe.as_deref_mut() {
            for (side, report) in reports.iter_mut().enumerate() {
                capture.contact_inputs[side] = std::mem::take(&mut report.bitprobe_contacts);
                capture.element_outputs[side] = std::mem::take(&mut report.bitprobe_elements);
            }
            capture.belt_reaction = [reports[0].belt_reaction, reports[1].belt_reaction];
            capture.transmission = tr.bitprobe;
        }
        for (si, report) in reports.into_iter().enumerate() {
            let pushes = piece_impulses(&mut pieces, &report.contacts, dt);
            apply_pushes(&pushes, &mut pieces_query);
            effect.belt_reaction[si] = report.belt_reaction;
            for contact in &report.contacts {
                effect.traction_force += contact.traction;
                effect.traction_torque += (contact.point - center_of_mass).cross(contact.traction);
            }
            // Apply in report order — accumulation order is part of determinism.
            for app in &report.apps {
                forces.apply_force_at_point(app.force, app.point);
            }
            contacts.0[si] = report.contacts;
        }
        effect.field_digest = coarse_grip_digest(&grip_elements);
        *grip_effect = effect;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both force routes must retain their authored side's geometry. A mutant that copies the
    /// right loop, placement or travel profile would make the asymmetric fixture indistinguishable.
    #[test]
    fn asymmetric_construction_reaches_both_force_routes() {
        let mut app = App::new();
        app.add_plugins((crate::bake::plugin, sim_plugin));
        app.world_mut().run_schedule(Startup);
        {
            let mut blueprint = app.world_mut().resource_mut::<TankBlueprint>();
            let mut spec = super::super::rig_geom::tiger_spec();
            spec.track.pin_plane_x.left -= 0.2;
            spec.track.idler.radius.left *= 0.9;
            blueprint.spec = std::sync::Arc::new(spec);
        }
        app.world_mut().run_schedule(PreUpdate);
        let gear = app.world().resource::<TrackGear>();
        let geom = app.world().resource::<super::super::rig_geom::RigGeom>();
        assert_eq!(gear.plane_x[0], geom.side_plane_x(Side::Left));
        assert_eq!(gear.plane_x[1], geom.side_plane_x(Side::Right));
        assert_ne!(gear.plane_x[0], -gear.plane_x[1]);
        assert_ne!(gear.loop_pts[0], gear.loop_pts[1]);
        assert_ne!(gear.travel_knots[0], gear.travel_knots[1]);
    }
}
