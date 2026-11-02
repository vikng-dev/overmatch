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
//! - The game's damage model ([`drive_target`]): a command exists only while someone serves the
//!   Driver station. The driver's competence, the engine's and the transmission's held shares
//!   each scale what that part delivers ([`DriveSupply`]); driver intent, service brakes, engine
//!   drag and contact grip keep their construction-defined meaning. A supply change takes effect
//!   on the tick it happens. A locked clutch passes a torque step on that tick; inertia only
//!   limits changes of crank speed, not changes of supply or transmitted capacity.

use avian3d::dynamics::rigid_body::forces::ForcesItem;
use avian3d::prelude::{
    ComputedCenterOfMass, Position, ReadRigidBodyForces, RigidBody, Rotation, WriteRigidBodyForces,
};
use bevy::math::{Affine3A, Vec2};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::bake::TankBlueprint;
use crate::command::ParsedTankCommand;
use crate::damage::{
    Capability, DriveView, TankCapabilities, TankVolumes, VolumeFacets, evaluate_view,
    part_qualities,
};
use crate::state::{GameplaySet, SimPhase};
use crate::tank::Tank;

#[cfg(feature = "bitprobe")]
use crate::bitprobe::BitprobeCapture;

use super::drive::{DriveAxes, DriveIntent, DriveTarget, shape_drive};
use super::forces::{
    BeltContact, ForceParams, GripElements, GripParams, SideInput, SideReport, SideState,
    contact_side,
};
use super::oracle::TerrainOracle;
use super::route::build_route;
use super::rubble::{
    BeltSource, PieceData, RubbleBodies, RubbleQuery, apply_pushes, footprint_radius,
    piece_impulses,
};
use super::side::Side;
use super::terrain::TrackField;
use super::transmission::{
    self, DriveSupply, TransmissionInput, TransmissionParams, TransmissionReport, TransmissionState,
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
    /// Per-source trace, so multi-tank headless probes read their own solve. Never replicated.
    #[cfg(feature = "bitprobe")]
    pub(crate) transmission_probe: crate::bitprobe::TransmissionProbe,
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

    /// Coupled translation mass for a torque-cut landing: hull plus both belt inertias (kg).
    #[cfg(all(test, feature = "bitprobe"))]
    pub(crate) fn landing_mass(&self) -> f32 {
        let grip = self.params.grip.expect("a driving probe has a gripped rig");
        2.0 * grip.side_weight / super::derive::G + 2.0 * self.params.inertia
    }

    /// DERIVED full service-brake deceleration along the direction of motion (m/s^2).
    /// The brakes and material pair bound force; hull mass plus both reflected belt inertias
    /// must be arrested. A nonpositive result names a grade these brakes cannot hold.
    pub(crate) fn braking_deceleration(&self, grade_sine: f32, direction: f32) -> f32 {
        let grip = self
            .params
            .grip
            .expect("admitted running gear has a material-pair contact law");
        let weight = 2.0 * grip.side_weight;
        let mass = weight / super::derive::G;
        let normal = weight * (1.0 - grade_sine * grade_sine).sqrt();
        let available = (2.0 * self.trans.brake_capacity_n).min(grip.on_ground() * normal);
        (available + direction * weight * grade_sine) / (mass + 2.0 * self.params.inertia)
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
    app.insert_resource(super::load::DriverResolution::load());
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
/// ([`super::envelope`]). Pin pitch and axle positions come from the sim artifact; the sheet's
/// contact radii and teeth complete the circles and the reflected-inertia derivation.
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
        .transmission_params(&blueprint.spec.roadwheels, geom.wrap_radii(), geom.plane_x)
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
            inertia: spec
                .reflected_inertia(&blueprint.spec.roadwheels, geom.wrap_radii())
                .expect("running-gear admission certified the construction inertia"),
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
    target: DriveTarget,
    affine: Affine3A,
    dt: f32,
    vel_at: impl Fn(Vec3) -> Vec3,
    rig: BeltRig<'_>,
) -> ([SideReport; 2], TransmissionReport) {
    let DriveTarget { intent, supply } = target;
    let (target, brake) = match intent {
        DriveIntent::Axes(axes) => (axes, false),
        DriveIntent::Brake => (DriveAxes::default(), true),
        DriveIntent::Unmanned => {
            // No one holds the controls, so none stays applied while it slews out.
            *rig.signal = DriveSignal::default();
            (DriveAxes::default(), false)
        }
    };
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
        let report = contact_side(
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
            rolling: [reports[0].rolling_resistance, reports[1].rolling_resistance],
            perceived_load: super::load::observe(
                reports
                    .iter()
                    .flat_map(|report| report.perceived_support.iter()),
                affine.transform_vector3(Vec3::NEG_Z)
                    * if rig.transmission.reverse { -1.0 } else { 1.0 },
                gear.params.grip.map_or(0.0, |grip| 2.0 * grip.side_weight),
                super::load::turn_radius(&gear.trans, rig.transmission),
            ),
            dt,
            brake,
            supply,
        },
    );
    for (si, report) in reports.iter().enumerate() {
        let pre_speed = rig.drive.sides[si].speed;
        rig.drive.sides[si] = TrackDriveSide {
            speed: tr.next_speeds[si],
            // Phase advects at the PRE-update speed — contact evaluation used those stations.
            phase: rig.drive.sides[si].phase + f64::from(pre_speed * dt),
        };
        rig.grip.sides[si] = [report.state.grip.x, report.state.grip.y];
    }
    (reports, tr)
}

/// The game's damage model: a phase hold imposes service brakes. Otherwise only someone
/// serving the Driver station can command axes or hold the brakes for repair; without that
/// occupant repair remains work and cannot become a remote brake toggle.
pub(crate) fn drive_target(
    command: &ParsedTankCommand,
    tank: Option<&TankVolumes>,
    caps: Option<&TankCapabilities>,
    volumes: &Query<VolumeFacets>,
) -> DriveTarget {
    let supply = drive_supply(tank, caps, volumes);
    let intent = if command.held() {
        DriveIntent::Brake
    } else if caps
        .zip(tank)
        .is_some_and(|(caps, tank)| caps.drive_controls.permits(&part_qualities(tank, volumes)))
    {
        if tank.is_some_and(|tank| crate::damage::repair::underway(tank, volumes)) {
            DriveIntent::Brake
        } else {
            DriveIntent::Axes(DriveAxes {
                throttle: command.throttle,
                steer: command.steer,
            })
        }
    } else {
        DriveIntent::Unmanned
    };
    DriveTarget { intent, supply }
}

/// The game's damage model: Drive's requirement read once per part kind, each where it acts.
/// The crew view (stations at their competence, modules whole) scales the fueling the driver
/// delivers; the engine view scales combustion; the transmission view scales transmitted
/// capacity. `TankSpec::validate` admits no other module in Drive's requirement. A tank whose
/// volumes or capabilities are not yet bound supplies nothing: nobody drives it.
fn drive_supply(
    tank: Option<&TankVolumes>,
    caps: Option<&TankCapabilities>,
    volumes: &Query<VolumeFacets>,
) -> DriveSupply {
    let (Some(tank), Some(caps)) = (tank, caps) else {
        return DriveSupply {
            competence: 0.0,
            combustion: 0.0,
            transmitted: 0.0,
        };
    };
    let requirement = caps.requirement(Capability::Drive);
    let quality = part_qualities(tank, volumes);
    let view = |stage| evaluate_view(requirement, &quality, stage);
    DriveSupply {
        competence: view(DriveView::Crew),
        combustion: view(DriveView::Engine),
        transmitted: view(DriveView::Transmission),
    }
}

/// The drive step: shape the command, run each side's belt force model at the tick-truth
/// pose, apply the returned forces in report order, commit the new belt state.
///
/// Every tank reads the same tick-start support snapshot and excludes its own hull by identity.
/// The recipients' sides are sized after every pass, over all tanks' contacts at once
/// ([`piece_impulses`]), and written before recoil and physics.
#[cfg_attr(
    feature = "bitprobe",
    expect(
        clippy::too_many_arguments,
        reason = "ECS adapter over terrain, tanks and piece reactions, plus bitprobe capture"
    )
)]
fn apply_track_forces(
    time: Res<Time>,
    field: Res<TrackField>,
    gear: Option<Res<TrackGear>>,
    #[cfg(feature = "bitprobe")] mut bitprobe: Option<ResMut<BitprobeCapture>>,
    volumes: Query<VolumeFacets>,
    (rubble, mut transfers): (
        RubbleQuery,
        ResMut<crate::contact_transfer::ContactTransfers>,
    ),
    mut pieces_query: Query<PieceData>,
    mut tanks: Query<
        (
            Entity,
            &Position,
            &Rotation,
            &ComputedCenterOfMass,
            &RigidBody,
            &ParsedTankCommand,
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
    let mut sources = Vec::new();
    let mut near = Vec::new();
    for (entity, pos, _, _, body, ..) in &tanks {
        // Replicas have no authoritative driving pass and gather no support shapes.
        if *body == RigidBody::Dynamic {
            let local: Vec<_> = rubble
                .near(pos.0, gear.rubble_radius)
                .into_iter()
                .filter(|&collider| rubble.body_of(collider) != entity)
                .collect();
            near.extend_from_slice(&local);
            sources.push((entity, local));
        }
    }
    sources.sort_unstable_by_key(|(entity, _)| *entity);
    let entities: Vec<_> = sources.iter().map(|(entity, _)| *entity).collect();
    near.sort_unstable();
    near.dedup();
    // A hull's shape is read only if another tank's footprint query reached it. All recipients
    // come from the same body query, before any source applies driving forces.
    let mut pieces = RubbleBodies::gather(&rubble, &near, &pieces_query);
    for (entity, local) in sources {
        let (
            _,
            pos,
            rot,
            center_of_mass,
            _,
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
        pieces.select_source(entity, &local);
        let (mut forces, ..) = pieces_query
            .get_mut(entity)
            .expect("a dynamic driving hull carries Avian force state");
        #[cfg(feature = "bitprobe")]
        if let Some(capture) = bitprobe.as_deref_mut() {
            capture.tanks_seen += 1;
            capture.command = [command.throttle, command.steer];
        }
        let target = drive_target(command, tank_volumes, tank_caps, &volumes);

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
        #[cfg(feature = "bitprobe")]
        {
            effect.transmission_probe = tr.bitprobe;
        }
        for (si, report) in reports.into_iter().enumerate() {
            pieces.record_static_contacts(entity, &report.contacts, &mut transfers);
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
    // Every pass has read the tick-start snapshot, so the recipients' sides are sized after
    // them all, over every tank's contacts at once, by one law for every recipient body.
    // These writes stay in DrivingForces, before recoil and Avian integration.
    let contacts = entities.iter().flat_map(|&entity| {
        let (.., contacts, _) = tanks
            .get(entity)
            .expect("a tank gathered for this synchronous tick remains alive");
        let source = BeltSource::read(entity, &pieces_query);
        contacts
            .0
            .iter()
            .flatten()
            .map(move |contact| (source, contact))
    });
    let pushes = piece_impulses(&mut pieces, contacts, dt);
    pieces.record_moving_contacts(&pushes, &mut transfers, dt);
    apply_pushes(&pushes, &mut pieces_query);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::Part;

    /// The production adapter's app with real data-derived running gear on a flat field (top
    /// face y = 0), no gravity, one 64 Hz tick per update. Returns it with the fixture hull
    /// height at which a belt face presses a surface at y = 0.25 by 1 cm.
    fn adapter_app() -> (App, f32) {
        use avian3d::prelude::*;
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
            crate::bake::plugin,
        ))
        .init_asset::<Mesh>()
        .init_resource::<crate::contact_transfer::ContactTransfers>()
        .insert_resource(Gravity::ZERO)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )))
        .insert_resource(TrackField {
            revision: None,
            field: Some(super::super::oracle::BlockField::new(vec![
                super::super::oracle::TerrainBlock::new(
                    Vec3::new(0.0, -0.5, 0.0),
                    Quat::IDENTITY,
                    Vec3::new(200.0, 1.0, 200.0),
                ),
            ])),
        })
        .add_systems(
            PreUpdate,
            init_track_gear.run_if(not(resource_exists::<TrackGear>)),
        );
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app.update();
        let gear = app.world().resource::<TrackGear>();
        let low = gear
            .loop_pts
            .iter()
            .flatten()
            .map(|p| p.y)
            .fold(f32::INFINITY, f32::min);
        let upper_y = 0.25 - low + gear.params.face_offset - 0.01;
        (app, upper_y)
    }

    /// A fixture tank root: a 30 t box hull with the gear's carried driving state.
    fn fixture_tank(app: &mut App, at: Vec3) -> Entity {
        use avian3d::prelude::*;
        let gear = app.world().resource::<TrackGear>();
        let trans = TankTransmission::from_spec(gear.trans());
        let count = gear.count;
        app.world_mut()
            .spawn((
                Tank,
                Position(at),
                Rotation::default(),
                RigidBody::Dynamic,
                Collider::cuboid(12.0, 0.5, 12.0),
                Mass(30_000.0),
                AngularInertia::new(Vec3::splat(30_000.0)),
                NoAutoMass,
                NoAutoAngularInertia,
                NoAutoCenterOfMass,
                CenterOfMass(Vec3::ZERO),
                TrackGripElements::for_links(count),
                DriveSignal::default(),
                trans,
            ))
            .id()
    }

    /// Avian's tick-start velocity state on `body`, then the production adapter once.
    fn run_adapter(app: &mut App, bodies: &[(Entity, Vec3)]) {
        use avian3d::prelude::*;
        use bevy::ecs::system::RunSystemOnce;
        for &(body, velocity) in bodies {
            app.world_mut().entity_mut(body).insert((
                LinearVelocity(velocity),
                AngularVelocity::ZERO,
                avian3d::dynamics::integrator::VelocityIntegrationData::default(),
            ));
        }
        app.world_mut().run_system_once(apply_track_forces).unwrap();
    }

    /// Fixture bodies run the production adapter with real data-derived running gear. The box
    /// hulls, mass and inertias are fixture data; no vehicle-specific geometry is a pass condition.
    /// A stacked upper hull approaches the lower at `upper_closing` (m/s).
    fn support_fixture(
        reverse: bool,
        stacked: bool,
        sensor: bool,
        upper_closing: f32,
    ) -> (App, Vec<Entity>) {
        use avian3d::prelude::*;
        let (mut app, upper_y) = adapter_app();
        let mut order = if stacked { vec![0, 1] } else { vec![0] };
        if reverse {
            order.reverse();
        }
        let mut roots = vec![Entity::PLACEHOLDER; order.len()];
        for index in order {
            let y = if stacked && index == 1 { upper_y } else { 0.0 };
            let root = fixture_tank(&mut app, Vec3::Y * y);
            if (stacked && index == 1) || (!stacked && sensor) {
                app.world_mut().entity_mut(root).insert(Sensor);
            }
            roots[index] = root;
        }
        // Let Avian publish body state and spatial bounds; the driving adapter is not scheduled.
        app.update();
        app.update();
        let velocities: Vec<_> = roots
            .iter()
            .enumerate()
            .map(|(index, &root)| {
                let closing = if stacked && index == 1 {
                    upper_closing
                } else {
                    0.0
                };
                (root, Vec3::NEG_Y * closing)
            })
            .collect();
        run_adapter(&mut app, &velocities);
        (app, roots)
    }

    /// The carried driving state, reaction telemetry and physics increments are compared by
    /// their bits; entity identities and contact telemetry's collider labels are excluded.
    fn support_bits(app: &App, roots: &[Entity]) -> Vec<u64> {
        use avian3d::prelude::{AngularVelocity, LinearVelocity};
        let mut bits = Vec::new();
        for &root in roots {
            let world = app.world();
            let increments = world
                .get::<avian3d::dynamics::integrator::VelocityIntegrationData>(root)
                .unwrap();
            for v in [
                world.get::<LinearVelocity>(root).unwrap().0,
                world.get::<AngularVelocity>(root).unwrap().0,
                increments.linear_increment,
                increments.angular_increment,
            ] {
                bits.extend(v.to_array().map(|x| u64::from(x.to_bits())));
            }
            for side in world.get::<TrackDrive>(root).unwrap().sides {
                bits.push(u64::from(side.speed.to_bits()));
                bits.push(side.phase.to_bits());
            }
            let carried = postcard::to_allocvec(&(
                world.get::<DriveSignal>(root).unwrap(),
                world.get::<TrackGrip>(root).unwrap(),
                world.get::<TrackGripElements>(root).unwrap(),
                world.get::<TankTransmission>(root).unwrap(),
            ))
            .unwrap();
            bits.extend(carried.into_iter().map(u64::from));
            let effect = world.get::<TrackGripEffect>(root).unwrap();
            for v in [effect.traction_force, effect.traction_torque] {
                bits.extend(v.to_array().map(|x| u64::from(x.to_bits())));
            }
            bits.extend(effect.belt_reaction.map(|x| u64::from(x.to_bits())));
            bits.push(u64::from(effect.field_digest));
        }
        bits
    }

    /// A tank is a solid support by its construction. The production adapter must carry an
    /// upper belt on a lower hull and stage its opposite impulse onto that hull before physics;
    /// excluding all Tank recipients or forgetting the final write fails here.
    #[test]
    fn another_tank_carries_the_belt_and_receives_its_reaction() {
        use avian3d::prelude::LinearVelocity;
        let (app, roots) = support_fixture(false, true, false, 0.1);
        let lower_contacts = app.world().get::<TrackContacts>(roots[0]).unwrap();
        assert!(
            lower_contacts
                .0
                .iter()
                .flatten()
                .all(|c| c.collider != Some(roots[0])),
            "a hull reached by another tank must still exclude itself"
        );
        let contacts = app.world().get::<TrackContacts>(roots[1]).unwrap();
        let carried: f32 = contacts
            .0
            .iter()
            .flatten()
            .filter(|c| c.collider == Some(roots[0]))
            .map(|c| c.load)
            .sum();
        let reaction = app.world().get::<LinearVelocity>(roots[0]).unwrap().0.y;
        println!(
            "MEASURED upper belt load {carried:.3} N; lower hull reaction velocity {reaction:.6} m/s"
        );
        assert!(
            contacts
                .0
                .iter()
                .flatten()
                .any(|c| c.collider == Some(roots[0]) && c.load > 0.0),
            "the upper belt must bear on the lower hull"
        );
        assert!(
            app.world().get::<LinearVelocity>(roots[0]).unwrap().0.y < 0.0,
            "the lower hull must receive the belt's reaction"
        );
    }

    /// A hull is held by its own belt law, which responds to force, so a load resting on it
    /// reaches that law whole: the lower hull of a parked stack takes the exact opposite of every
    /// force the upper belt applied through it, though nothing closes. Stacked hulls rely on the
    /// lower carrying the upper's load; a loose piece's closing-speed clamp gives a resting
    /// lower hull nothing and fails here.
    #[test]
    fn a_load_resting_on_a_hull_reaches_it_whole() {
        use avian3d::prelude::LinearVelocity;
        let (app, roots) = support_fixture(false, true, false, 0.0);
        let applied: Vec3 = app
            .world()
            .get::<TrackContacts>(roots[1])
            .unwrap()
            .0
            .iter()
            .flatten()
            .filter(|c| c.collider == Some(roots[0]))
            .map(|c| c.normal * c.load + c.traction)
            .sum();
        assert!(applied.y > 0.0, "the upper belt bears on the lower hull");
        let impulse = applied * app.world().resource::<Time>().delta_secs();
        // FIXTURE DATA: the lower hull's 30 t.
        let momentum = app.world().get::<LinearVelocity>(roots[0]).unwrap().0 * 30_000.0;
        println!("MEASURED resting reaction {impulse} N s; lower hull momentum {momentum} N s");
        assert!(
            (momentum + impulse).length() <= 1e-4 * impulse.length(),
            "the lower hull's momentum {momentum} must be the opposite of {impulse}"
        );
    }

    /// One reaction law for every recipient: a body under a belt takes its side by its mass and
    /// inertia at the patches and the belt's springs there, never by what it is. A parked upper
    /// hull resting on a driving tank, and on a body with the same collider, mass and inertia but
    /// no running gear, gives each the same momentum, the exact opposite of what the upper belt
    /// applied. Stacked hulls and heavy fallen construction rely on it; a rule keyed on the
    /// body's kind (a list of tanks) fails here. FIXTURE DATA: [`fixture_tank`]'s 30 t box hulls.
    #[test]
    fn identical_receivers_take_the_same_reaction_whatever_they_are() {
        use avian3d::prelude::*;
        let run = |driving: bool| {
            let (mut app, upper_y) = adapter_app();
            let lower = if driving {
                fixture_tank(&mut app, Vec3::ZERO)
            } else {
                app.world_mut()
                    .spawn((
                        Position(Vec3::ZERO),
                        Rotation::default(),
                        RigidBody::Dynamic,
                        Collider::cuboid(12.0, 0.5, 12.0),
                        Mass(30_000.0),
                        AngularInertia::new(Vec3::splat(30_000.0)),
                        NoAutoMass,
                        NoAutoAngularInertia,
                        NoAutoCenterOfMass,
                        CenterOfMass(Vec3::ZERO),
                    ))
                    .id()
            };
            let upper = fixture_tank(&mut app, Vec3::Y * upper_y);
            app.world_mut().entity_mut(upper).insert(Sensor);
            app.update();
            app.update();
            run_adapter(&mut app, &[(lower, Vec3::ZERO), (upper, Vec3::ZERO)]);
            let dt = app.world().resource::<Time>().delta_secs();
            let on_lower: Vec<_> = app
                .world()
                .get::<TrackContacts>(upper)
                .unwrap()
                .0
                .iter()
                .flatten()
                .filter(|c| c.collider == Some(lower))
                .map(|c| {
                    (
                        c.normal * c.load + c.traction,
                        c.point,
                        c.normal,
                        c.stiffness,
                    )
                })
                .collect();
            let impulse = on_lower.iter().map(|c| c.0).sum::<Vec3>() * dt;
            assert!(impulse.y > 0.0, "the upper belt bears on the lower body");
            // DERIVED: the explicit-limit reading of the fixture's 30 t, 30 000 kg m² box.
            let rate: f32 = on_lower
                .iter()
                .map(|&(_, point, normal, stiffness)| {
                    stiffness * (1.0 / 30_000.0 + point.cross(normal).length_squared() / 30_000.0)
                })
                .sum();
            let momentum = app.world().get::<LinearVelocity>(lower).unwrap().0 * 30_000.0;
            println!(
                "MEASURED driving {driving}: reaction {impulse} N s, momentum {momentum} N s, \
                 dt² Σk/m {}",
                rate * dt * dt
            );
            (impulse, momentum)
        };
        let (tank_impulse, tank_momentum) = run(true);
        let (body_impulse, body_momentum) = run(false);
        assert!(
            (tank_impulse - body_impulse).length() <= 1e-6 * tank_impulse.length(),
            "the identical lower surfaces carry the same belt: {tank_impulse} and {body_impulse}"
        );
        for (impulse, momentum) in [(tank_impulse, tank_momentum), (body_impulse, body_momentum)] {
            assert!(
                (momentum + impulse).length() <= 1e-4 * impulse.length(),
                "the lower body's momentum {momentum} must be the opposite of {impulse}"
            );
        }
    }

    /// Both passes read one tick-start snapshot: exchanging entity order cannot change any
    /// body's driving or reaction bits. Each hull relies on other tanks' reactions being staged.
    #[test]
    fn exchanging_tank_entity_order_preserves_every_driving_and_reaction_bit() {
        let (first, a) = support_fixture(false, true, false, 0.1);
        let (second, b) = support_fixture(true, true, false, 0.1);
        assert_eq!(support_bits(&first, &a), support_bits(&second, &b));
    }

    /// Two tanks' belts closing on one light loose plank, so each contact's clamp binds and
    /// every impulse changes what the next is sized against. Exchanging the tanks' entity order
    /// cannot change either tank's bits or the plank's: piece reactions are sized in one
    /// canonical order of the contacts' geometry, never in tank-entity order. FIXTURE DATA: two
    /// 30 t box hulls 4 m apart across, a 200 kg plank under their inner tracks, closing 0.1 m/s.
    #[test]
    fn exchanging_tank_entity_order_preserves_a_shared_pieces_reaction() {
        use avian3d::prelude::*;
        let run = |reverse: bool| {
            let (mut app, upper_y) = adapter_app();
            let mut sides = [-2.0_f32, 2.0];
            if reverse {
                sides.reverse();
            }
            let mut tanks: Vec<(f32, Entity)> = sides
                .iter()
                .map(|&x| {
                    let tank = fixture_tank(&mut app, Vec3::new(x, upper_y, 0.0));
                    app.world_mut().entity_mut(tank).insert(Sensor);
                    (x, tank)
                })
                .collect();
            tanks.sort_by(|a, b| a.0.total_cmp(&b.0));
            let plank = app
                .world_mut()
                .spawn((
                    Position(Vec3::Y * 0.15),
                    Rotation::default(),
                    RigidBody::Dynamic,
                    Collider::cuboid(4.0, 0.2, 1.0),
                    Mass(200.0),
                ))
                .id();
            app.update();
            app.update();
            let tanks: Vec<Entity> = tanks.into_iter().map(|(_, tank)| tank).collect();
            let mut velocities: Vec<_> = tanks.iter().map(|&t| (t, Vec3::NEG_Y * 0.1)).collect();
            velocities.push((plank, Vec3::ZERO));
            run_adapter(&mut app, &velocities);
            for &tank in &tanks {
                assert!(
                    app.world()
                        .get::<TrackContacts>(tank)
                        .unwrap()
                        .0
                        .iter()
                        .flatten()
                        .any(|c| c.collider.is_some() && c.load > 0.0),
                    "both belts bear on the plank"
                );
            }
            let mut bits = support_bits(&app, &tanks);
            for v in [
                app.world().get::<LinearVelocity>(plank).unwrap().0,
                app.world().get::<AngularVelocity>(plank).unwrap().0,
            ] {
                bits.extend(v.to_array().map(|x| u64::from(x.to_bits())));
            }
            bits
        };
        assert_eq!(run(false), run(true));
    }

    /// On open ground, a lone hull is never its own support. A sensor reference removes the
    /// hull from spatial admission entirely; making it solid must retain every driving bit.
    #[test]
    fn a_lone_tank_on_open_ground_retains_every_field_only_bit() {
        let (reference, a) = support_fixture(false, false, true, 0.0);
        let (solid, b) = support_fixture(false, false, false, 0.0);
        assert_eq!(support_bits(&reference, &a), support_bits(&solid, &b));
    }

    /// The game's damage model: the production selector reads each part of Drive where it acts.
    /// The station's crew decides whether a command exists and scales delivered fueling; each
    /// module scales only what it produces. A selector that folds the views into one share, or
    /// keeps intent with nobody at the station, fails a row. Fixture: ten-point pools and
    /// normalized input axes.
    #[test]
    fn the_driver_station_decides_intent_and_each_part_scales_its_own_supply() {
        use crate::ballistics::ComponentHealth;
        use crate::command::TankCommand;
        use crate::damage::{CrewStation, Crewman, Dead, FunctionRole, Group, VolumeOf};
        use bevy::ecs::system::RunSystemOnce;
        let foreign = crate::damage::competence(CrewStation::Loader, CrewStation::Driver);
        let axes = DriveIntent::Axes(DriveAxes {
            throttle: 0.8,
            steer: -0.4,
        });
        for (home, out, engine, gearbox, intent, competence) in [
            (CrewStation::Driver, false, 1.0, 1.0, axes, 1.0),
            (CrewStation::Loader, false, 1.0, 1.0, axes, foreign),
            (CrewStation::Driver, false, 0.5, 1.0, axes, 1.0),
            (CrewStation::Driver, false, 1.0, 0.5, axes, 1.0),
            (CrewStation::Driver, false, 0.0, 1.0, axes, 1.0),
            (
                CrewStation::Driver,
                true,
                1.0,
                1.0,
                DriveIntent::Unmanned,
                0.0,
            ),
        ] {
            let mut world = World::new();
            let tank = world
                .spawn(TankCapabilities::new(
                    std::collections::HashMap::from([(
                        Capability::Drive,
                        vec![
                            Group::Single(Part::Driver),
                            Group::Single(Part::Engine),
                            Group::Single(Part::Transmission),
                        ],
                    )]),
                    crate::damage::ControlStaffing::Any(vec![vec![
                        crate::damage::CrewStation::Driver,
                    ]]),
                ))
                .id();
            let driver = world
                .spawn((CrewStation::Driver, Crewman { home }, VolumeOf(tank)))
                .id();
            if out {
                world.entity_mut(driver).insert(Dead);
            }
            for (role, share) in [
                (FunctionRole::Engine, engine),
                (FunctionRole::Transmission, gearbox),
            ] {
                let mut health = ComponentHealth::full(10);
                health.set_held((health.capacity() as f32 * share) as u32);
                world.spawn((role, health, VolumeOf(tank)));
            }
            let supply = DriveSupply {
                competence,
                combustion: engine,
                transmitted: gearbox,
            };
            let command = ParsedTankCommand::try_from(TankCommand {
                throttle: 0.8,
                steer: -0.4,
                ..default()
            })
            .unwrap();
            world
                .run_system_once(
                    move |tanks: Query<(&TankVolumes, &TankCapabilities)>,
                          facets: Query<VolumeFacets>| {
                        let (volumes, caps) = tanks.get(tank).unwrap();
                        assert_eq!(
                            drive_target(&command, Some(volumes), Some(caps), &facets),
                            DriveTarget { intent, supply }
                        );
                        assert_eq!(
                            drive_target(
                                &ParsedTankCommand::hold(),
                                Some(volumes),
                                Some(caps),
                                &facets
                            ),
                            DriveTarget {
                                intent: DriveIntent::Brake,
                                supply
                            },
                            "a phase hold changes intent, never the available supply"
                        );
                    },
                )
                .unwrap();
        }
    }

    /// The game's damage model: a fixture hull driven through the production adapter, with
    /// its longitudinal speed integrated from the adapter's own traction at the vehicle mass the
    /// grip law carries (DERIVED: twice its side weight over g). Drive requires
    /// `[Driver, Engine, Transmission]`; the 336 HP pools and the along-track `grade` force are
    /// fixture data, never an asset pass condition.
    struct DrivenHull {
        app: App,
        root: Entity,
        driver: Entity,
        engine: Entity,
        mass: f32,
        speed: f32,
        distance: f32,
    }

    impl DrivenHull {
        const POOL: u32 = 336;

        fn new() -> Self {
            use crate::ballistics::ComponentHealth;
            use crate::damage::{CrewStation, Crewman, FunctionRole, Group, Part, VolumeOf};
            let (mut app, roots) = support_fixture(false, false, true, 0.0);
            let root = roots[0];
            app.world_mut()
                .entity_mut(root)
                .insert(TankCapabilities::new(
                    [(
                        Capability::Drive,
                        vec![
                            Group::Single(Part::Driver),
                            Group::Single(Part::Engine),
                            Group::Single(Part::Transmission),
                        ],
                    )]
                    .into(),
                    crate::damage::ControlStaffing::Any(vec![vec![
                        crate::damage::CrewStation::Driver,
                    ]]),
                ));
            let driver = app
                .world_mut()
                .spawn((
                    CrewStation::Driver,
                    Crewman {
                        home: CrewStation::Driver,
                    },
                    VolumeOf(root),
                ))
                .id();
            let engine = app
                .world_mut()
                .spawn((
                    FunctionRole::Engine,
                    ComponentHealth::full(Self::POOL),
                    VolumeOf(root),
                ))
                .id();
            app.world_mut().spawn((
                FunctionRole::Transmission,
                ComponentHealth::full(Self::POOL),
                VolumeOf(root),
            ));
            let grip = app
                .world()
                .resource::<TrackGear>()
                .params
                .grip
                .expect("the shipped running gear carries a grip law");
            let mass = 2.0 * grip.side_weight / 9.81;
            Self {
                app,
                root,
                driver,
                engine,
                mass,
                speed: 0.0,
                distance: 0.0,
            }
        }

        fn engine_held(&mut self, hp: u32) {
            use crate::ballistics::{ComponentHealth, POOL_UNITS_PER_HP};
            self.app
                .world_mut()
                .entity_mut(self.engine)
                .insert(ComponentHealth::holding(Self::POOL, hp * POOL_UNITS_PER_HP));
        }

        fn rpm(&self) -> f32 {
            self.app
                .world()
                .get::<TankTransmission>(self.root)
                .unwrap()
                .0
                .omega_e
                / super::super::transmission::RPM_TO_RAD
        }

        /// One adapter tick under `throttle`, plus a forward along-track `grade` force (N).
        fn tick(&mut self, throttle: f32, grade: f32) {
            use crate::command::TankCommand;
            self.app.world_mut().entity_mut(self.root).insert(
                ParsedTankCommand::try_from(TankCommand {
                    throttle,
                    ..default()
                })
                .unwrap(),
            );
            run_adapter(&mut self.app, &[(self.root, Vec3::NEG_Z * self.speed)]);
            let traction = self
                .app
                .world()
                .get::<TrackGripEffect>(self.root)
                .unwrap()
                .traction_force
                .z;
            self.speed += (-traction + grade) / self.mass / 64.0;
            self.distance += self.speed / 64.0;
        }

        fn drive_to(&mut self, speed: f32) {
            for _ in 0..64 * 60 {
                if self.speed >= speed {
                    return;
                }
                self.tick(1.0, 0.0);
            }
            panic!("the fixture never reached {speed} m/s: {}", self.speed);
        }
    }

    /// The game's damage model: with the Driver station's crew member out of action and no
    /// backfill, no command exists, so a held S neither brakes nor swaps the ladder. The tank
    /// coasts exactly as it does with the controls released. Keeping intent alive (full service
    /// brakes on a held S) fails the bit comparison.
    #[test]
    fn a_driver_out_of_action_leaves_no_command_to_brake_with() {
        let mut runs = [-1.0, 0.0].map(|throttle| {
            let mut hull = DrivenHull::new();
            hull.drive_to(5.0);
            hull.app
                .world_mut()
                .entity_mut(hull.driver)
                .insert(crate::damage::Dead);
            hull.distance = 0.0;
            let start = hull.speed;
            let mut stopped = None;
            for tick in 0..64 * 20 {
                hull.tick(throttle, 0.0);
                if stopped.is_none() && hull.speed <= 0.01 {
                    stopped = Some((tick, hull.distance));
                }
            }
            (hull, start, stopped)
        });
        let [(held, start, stopped), (released, ..)] = &mut runs;
        println!(
            "MEASURED driver out at {start} m/s, S held: stopped {stopped:?} (tick, m); after 20 s \
             speed {} m/s, distance {} m",
            held.speed, held.distance
        );
        assert_eq!(
            (held.speed.to_bits(), held.distance.to_bits()),
            (released.speed.to_bits(), released.distance.to_bits())
        );
        let mut served = DrivenHull::new();
        served.drive_to(5.0);
        served.distance = 0.0;
        for _ in 0..64 * 20 {
            served.tick(-1.0, 0.0);
            if served.speed <= 0.01 {
                break;
            }
        }
        assert!(
            held.distance > served.distance,
            "an unmanned coast must travel farther than a serving driver's full brake: {} vs {}",
            held.distance,
            served.distance
        );
        assert!(
            !held
                .app
                .world()
                .get::<TankTransmission>(held.root)
                .unwrap()
                .0
                .reverse,
            "no ladder swap without a driver"
        );
    }

    /// The game's damage model: engine braking is a loss of the engine's construction, so a
    /// half engine pool must not coast down a released descent faster than a whole one. Scaling
    /// motoring drag with the pool fails. Fixture: a 10 degree along-track gravity share.
    #[test]
    fn a_reduced_engine_keeps_its_engine_braking_on_a_descent() {
        let speeds = [DrivenHull::POOL, DrivenHull::POOL / 2].map(|held| {
            let mut hull = DrivenHull::new();
            hull.drive_to(5.0);
            hull.engine_held(held);
            let grade = hull.mass * 9.81 * 10.0_f32.to_radians().sin();
            for _ in 0..64 * 10 {
                hull.tick(0.0, grade);
            }
            hull.speed
        });
        println!(
            "MEASURED 10 degree descent released 10 s: whole engine {} m/s, half engine {} m/s",
            speeds[0], speeds[1]
        );
        assert!(
            speeds[1] <= speeds[0] * 1.001,
            "half engine coasts faster: {speeds:?}"
        );
    }

    /// The game's damage model: construction friction remains when combustion is absent.
    /// A held pedal cannot extend a free hull's coast compared with a released pedal.
    #[test]
    fn empty_engine_held_pedal_cannot_extend_free_hull_coast() {
        let runs = [1.0, 0.0].map(|pedal| {
            let mut hull = DrivenHull::new();
            hull.drive_to(5.0); // Fixture rolling initial condition.
            hull.engine_held(0);
            hull.distance = 0.0;
            for _ in 0..64 * 3 {
                hull.tick(pedal, 0.0);
            }
            (hull.distance, hull.speed)
        });
        println!("MEASURED empty-engine free coast over 3 s: held/released {runs:?}");
        assert!(runs[0].0 <= runs[1].0 + 1e-5, "{runs:?}");
    }

    /// The game's damage model: an engine whose HP pool empties while driving stops producing
    /// combustion torque, but its crank keeps turning under stored inertia and the clutch. A
    /// construction drag remains under a held pedal, so the public speed follows the coasting
    /// belt rather than holding its last pitch; clutch re-anchors need not be monotonic. On recovery
    /// the clutch engages against that tracked crank and the tank pulls away again.
    #[test]
    fn an_emptied_engine_runs_down_with_the_belt_and_recovers_from_there() {
        let mut hull = DrivenHull::new();
        for _ in 0..64 * 60 {
            hull.tick(1.0, 0.0);
            if hull.rpm() >= 2400.0 {
                break;
            }
        }
        let at = hull.rpm();
        assert!(at >= 2400.0, "the fixture reached only {at} rpm");
        hull.engine_held(0);
        let mut trace = Vec::new();
        for tick in 0..64 {
            hull.tick(1.0, 0.0);
            if tick % 16 == 15 {
                trace.push(hull.rpm());
            }
        }
        println!(
            "MEASURED engine emptied at {at} rpm, W held: rpm every 0.25 s {trace:?}; speed {} m/s",
            hull.speed
        );
        assert!(
            trace.iter().any(|rpm| rpm.to_bits() != at.to_bits()) && hull.rpm() < at,
            "the crank follows the coasting belt, never freezes: {at} then {trace:?}"
        );
        let (run_down, slowest) = (hull.rpm(), hull.speed);
        hull.engine_held(DrivenHull::POOL);
        hull.tick(1.0, 0.0);
        let first = hull.rpm();
        for _ in 0..64 {
            hull.tick(1.0, 0.0);
        }
        println!(
            "MEASURED recovery: {run_down} rpm, first tick {first} rpm, after 1 s {} rpm and {} m/s",
            hull.rpm(),
            hull.speed
        );
        assert!(
            (first - run_down).abs() < 0.1 * at,
            "the clutch engages against the tracked crank: {run_down} -> {first}"
        );
        assert!(hull.speed > slowest, "a recovered engine pulls away again");
    }

    /// The game's damage model: free-hull measurements distinguish propulsion from belt slip,
    /// and nobody at the Driver station can set the park lever at a grade's turning point.
    #[test]
    fn free_hull_measurements() {
        let mut reduced = DrivenHull::new();
        reduced.engine_held(30); // Fixture HP held of the fixture 336 HP pool.
        for _ in 0..64 * 5 {
            reduced.tick(1.0, 0.0);
        }
        println!("MEASURED 30/336 engine W after 5 s: {} m/s", reduced.speed);
        let mut grade = DrivenHull::new();
        grade.drive_to(5.0);
        grade
            .app
            .world_mut()
            .entity_mut(grade.driver)
            .insert(crate::damage::Dead);
        let uphill = -grade.mass * 9.81 * 10.0_f32.to_radians().sin(); // Fixture 10 degree grade.
        let mut turned = false;
        for _ in 0..64 * 20 {
            grade.tick(0.0, uphill);
            turned |= grade.speed < -0.1;
        }
        let parked = grade
            .app
            .world()
            .get::<TankTransmission>(grade.root)
            .unwrap()
            .0
            .park;
        println!(
            "MEASURED unmanned 10 degree uphill: rolls back {turned}, park {parked}, final {} m/s",
            grade.speed
        );
        assert!(!parked, "nobody at the station can set the park lever");
        assert!(turned, "the free hull must roll back after coasting uphill");
        assert!(
            reduced.speed > 0.0,
            "a positive full-pedal combustion supply pulls away"
        );
    }

    /// The game's damage model: zero combustion and zero stored crank energy cannot power
    /// a pivot. Integrate the adapter's traction torque into a free hull's yaw velocity.
    #[test]
    fn stopped_engine_cannot_pivot_a_free_hull() {
        use avian3d::prelude::*;
        use bevy::ecs::system::RunSystemOnce;
        let mut hull = DrivenHull::new();
        hull.engine_held(0);
        hull.app
            .world_mut()
            .get_mut::<TankTransmission>(hull.root)
            .unwrap()
            .0
            .omega_e = 0.0;
        let mut yaw = 0.0;
        let mut angle = 0.0;
        let mut position = hull.app.world().get::<Position>(hull.root).unwrap().0;
        let mut velocity = Vec3::ZERO;
        for _ in 0..64 {
            hull.app.world_mut().entity_mut(hull.root).insert((
                ParsedTankCommand::try_from(crate::command::TankCommand {
                    steer: 1.0,
                    ..default()
                })
                .unwrap(),
                Position(position),
                Rotation(Quat::from_rotation_y(angle)),
                LinearVelocity(velocity),
                AngularVelocity(Vec3::Y * yaw),
                avian3d::dynamics::integrator::VelocityIntegrationData::default(),
            ));
            hull.app
                .world_mut()
                .run_system_once(apply_track_forces)
                .unwrap();
            let effect = hull.app.world().get::<TrackGripEffect>(hull.root).unwrap();
            // Fixture yaw inertia is the 30,000 kg m² authored by fixture_tank. Translation
            // uses the same data-derived mass as the free longitudinal hull fixture.
            yaw += effect.traction_torque.y / 30_000.0 / 64.0;
            velocity += effect.traction_force / hull.mass / 64.0;
            angle += yaw / 64.0;
            position += velocity / 64.0;
        }
        println!("MEASURED empty stopped engine full steer after 1 s: {yaw} rad/s");
        assert_eq!(yaw, 0.0, "a stopped crank has no energy to give the belts");
    }

    /// The game's damage model: Backup means one complete crew-and-parts path. The adapter
    /// cannot borrow the engine from an unavailable Driver path and the crew from another.
    #[test]
    fn backup_paths_are_evaluated_together() {
        assert_paths_are_evaluated_together(crate::damage::GradedGroup::Backup);
    }

    /// The game's damage model: a free hull cannot move on spliced Pool contributors.
    #[test]
    fn pool_paths_are_evaluated_together() {
        assert_paths_are_evaluated_together(crate::damage::GradedGroup::Pool);
    }

    fn assert_paths_are_evaluated_together(
        group: fn(Vec<crate::damage::Member>) -> crate::damage::GradedGroup,
    ) {
        use crate::damage::{CrewStation, Crewman, Dead, FunctionRole, Group, Part, VolumeOf};
        use bevy::ecs::system::RunSystemOnce;
        let mut hull = DrivenHull::new();
        hull.app.world_mut().entity_mut(hull.driver).insert(Dead);
        hull.app
            .world_mut()
            .entity_mut(hull.root)
            .insert(TankCapabilities::new(
                [(
                    Capability::Drive,
                    vec![Group::Graded(group(vec![
                        (1.0, vec![Part::Driver, Part::Engine]),
                        (1.0, vec![Part::Commander, Part::Transmission]),
                    ]))],
                )]
                .into(),
                crate::damage::ControlStaffing::Any(vec![vec![crate::damage::CrewStation::Driver]]),
            ));
        hull.app.world_mut().spawn((
            CrewStation::Commander,
            Crewman {
                home: CrewStation::Commander,
            },
            VolumeOf(hull.root),
        ));
        let root = hull.root;
        hull.app
            .world_mut()
            .run_system_once(
                move |mut modules: Query<(
                    &FunctionRole,
                    &mut crate::ballistics::ComponentHealth,
                )>| {
                    for (role, mut hp) in &mut modules {
                        if *role == FunctionRole::Transmission {
                            hp.set_held(0);
                        }
                    }
                },
            )
            .unwrap();
        hull.app
            .world_mut()
            .run_system_once(
                move |tanks: Query<(&TankVolumes, &TankCapabilities)>,
                      facets: Query<VolumeFacets>| {
                    let (volumes, caps) = tanks.get(root).unwrap();
                    let supply = drive_supply(Some(volumes), Some(caps), &facets);
                    assert_eq!(supply.share(), 0.0, "no complete Backup path: {supply:?}");
                },
            )
            .unwrap();
        for _ in 0..64 {
            hull.tick(1.0, 0.0);
        }
        assert_eq!(hull.speed, 0.0);
        // An engine out of action cannot remove an available occupant's brake intent.
        hull.app
            .world_mut()
            .entity_mut(hull.driver)
            .remove::<Dead>();
        hull.engine_held(0);
        hull.app
            .world_mut()
            .entity_mut(root)
            .insert(TankCapabilities::new(
                [(
                    Capability::Drive,
                    vec![Group::Graded(group(vec![(
                        1.0,
                        vec![Part::Driver, Part::Engine],
                    )]))],
                )]
                .into(),
                crate::damage::ControlStaffing::Any(vec![vec![crate::damage::CrewStation::Driver]]),
            ));
        hull.app
            .world_mut()
            .run_system_once(
                move |tanks: Query<(&TankVolumes, &TankCapabilities)>,
                      facets: Query<VolumeFacets>| {
                    let (volumes, caps) = tanks.get(root).unwrap();
                    let supply = drive_supply(Some(volumes), Some(caps), &facets);
                    assert_eq!(supply.competence, 1.0);
                    assert_eq!(supply.combustion, 0.0);
                    assert_eq!(supply.share(), 0.0);
                },
            )
            .unwrap();
    }

    /// The game's damage model: repair is work, not a remote service-brake control. With
    /// nobody at the Driver station, toggling the repair timer must leave a free hull's coast
    /// bit-identical; a phase hold remains an operator-imposed brake.
    #[test]
    fn unmanned_repair_does_not_brake() {
        let speeds = [false, true].map(|repairing| {
            let mut hull = DrivenHull::new();
            hull.drive_to(5.0);
            hull.engine_held(168); // Fixture reduced pool, admissible repair work.
            hull.app
                .world_mut()
                .entity_mut(hull.driver)
                .insert(crate::damage::Dead);
            hull.app.world_mut().entity_mut(hull.engine).insert(
                crate::damage::repair::ModuleRepair {
                    elapsed: repairing.then_some(0.0),
                },
            );
            for _ in 0..64 {
                hull.tick(0.0, 0.0);
            }
            hull.speed
        });
        assert_eq!(
            speeds[0].to_bits(),
            speeds[1].to_bits(),
            "repair changed the unmanned coast: {speeds:?}"
        );
    }

    /// The controller's deceleration estimate must scale with the whole vehicle rather than
    /// retaining a force or mass from its fixture. A brake-only or mass-only scale mutant fails.
    #[test]
    fn braking_capability_is_invariant_under_uniform_vehicle_scaling() {
        let mut app = App::new();
        app.add_plugins((crate::bake::plugin, sim_plugin));
        app.world_mut().run_schedule(Startup);
        app.world_mut().run_schedule(PreUpdate);
        let mut gear = app.world_mut().resource_mut::<TrackGear>();
        let before = gear.braking_deceleration(0.1, -1.0);
        let scale = 3.0; // Fixture scale, dimensionless.
        gear.params.inertia *= scale;
        gear.params.grip.as_mut().unwrap().side_weight *= scale;
        gear.trans.brake_capacity_n *= scale;
        assert!((gear.braking_deceleration(0.1, -1.0) - before).abs() < 1e-5);
        assert!(gear.braking_deceleration(0.1, 1.0) > gear.braking_deceleration(0.1, -1.0));
    }

    /// The drive controller relies on construction reaching the actual brake predictor.
    /// FIXTURE DATA: the declared vehicle and doubled complete-link mass at fixed total mass.
    /// A mutant retaining a fixed inertia, omitting a side, or dropping hull mass fails.
    #[test]
    fn doubled_link_mass_brakes_slower_by_its_derived_inertia() {
        let measure = |scale: f32| {
            let mut app = App::new();
            app.add_plugins((crate::bake::plugin, sim_plugin));
            app.world_mut().run_schedule(Startup);
            let mut spec = super::super::rig_geom::tiger_spec();
            let link_inertia = spec.track.link_count as f32 * spec.track.link_mass.0;
            spec.track.link_mass.0 *= scale;
            app.world_mut().resource_mut::<TankBlueprint>().spec = std::sync::Arc::new(spec);
            app.world_mut().run_schedule(PreUpdate);
            let gear = app.world().resource::<TrackGear>();
            let mass = gear.params.grip.unwrap().side_weight * 2.0 / super::super::derive::G;
            let inertia = gear.params.inertia;
            let decel = gear.braking_deceleration(0.0, 1.0);
            // DERIVED full-throttle, no-slip flat acceleration at the peak-torque operating
            // point in the highest gear: crank inertia is reflected separately through k^2.
            let k = gear.trans.gears_fwd.last().unwrap() / gear.trans.sprocket_radius;
            let crank_mass = gear.trans.engine_inertia * k * k;
            let drive_force = gear.trans.torque_at(gear.trans.peak_torque_rpm) * k;
            println!(
                "DERIVED construction: scale={scale}, links={link_inertia} kg/side, inertia={inertia} kg/side, mass={mass} kg, brake={decel} m/s^2, flat peak-torque top-gear acceleration={} m/s^2, drive_force={drive_force} N, reflected_crank={crank_mass} kg",
                drive_force / (mass + 2.0 * inertia + crank_mass)
            );
            (inertia, decel, mass, link_inertia)
        };
        let (before, decel, mass, link_inertia) = measure(1.0);
        let (after, slower, _, _) = measure(2.0);
        assert!((after - before - link_inertia).abs() < 1e-3);
        let expected = decel * (mass + 2.0 * before) / (mass + 2.0 * after);
        assert!(slower < decel);
        assert!((slower - expected).abs() < 1e-5);
    }

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
            // FIXTURE: preserve the idler's reflected inertia while changing its contact geometry:
            // the moment scales with the square of the pin-line radius the belt turns it at.
            let pin_to_inner = super::super::rig_geom::tiger_rig().model.pin_to_inner;
            let radius = spec.track.idler.radius.left;
            let pin_ratio = (0.9 * radius + pin_to_inner) / (radius + pin_to_inner);
            spec.track.idler.radius.left *= 0.9;
            spec.track.idler.spin_inertia_kgm2.0.left *= pin_ratio * pin_ratio;
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
