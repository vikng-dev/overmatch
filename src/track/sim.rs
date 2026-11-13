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
//! - Terrain from the analytic [`TrackField`]; construction and loose-piece support from a
//!   tick-start shape snapshot ([`RubbleBodies`]), checked across the complete shoe footprint.
//! - Runs every tick unconditionally (this is sim state); stays inside
//!   `SimPhase::DrivingForces` so drive samples velocity before the weapon-fire impulse.
//! - The game's damage model ([`drive_target`]): axes actuate only while someone serves the
//!   authored control stations. Task throughput is resolved once over complete paths;
//!   local source conditions remain separate ([`DriveSupply`]). Driver intent, service brakes, engine
//!   drag and contact grip keep their construction-defined meaning. A supply change takes effect
//!   on the tick it happens. A locked clutch passes a torque step on that tick; inertia only
//!   limits changes of crank speed, not changes of supply or transmitted capacity.

use avian3d::dynamics::rigid_body::forces::ForcesItem;
use avian3d::prelude::{
    ComputedAngularInertia, ComputedCenterOfMass, ComputedMass, Forces, Gravity, Position,
    ReadRigidBodyForces, RigidBody, Rotation, SleepTimer, Sleeping, WriteRigidBodyForces,
};
use bevy::ecs::query::QueryData;
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::math::{Affine3A, Vec2};
use bevy::prelude::*;
use bevy::utils::Parallel;
use serde::{Deserialize, Serialize};

use crate::bake::TankBlueprint;
use crate::command::ParsedTankCommand;
use crate::damage::{
    Capability, TankCapabilities, TankVolumes, VolumeFacets, evaluate, live_quality, part_facts,
};
use crate::state::{GameplaySet, SimPhase};
use crate::tank::Tank;

#[cfg(feature = "bitprobe")]
use crate::bitprobe::BitprobeCapture;

use super::drive::{DriveAxes, DriveIntent, DriveTarget, shape_drive};
use super::forces::{
    BeltContact, ForceParams, GripElements, GripParams, PROBE_REACH, SideInput, SideReport,
    SideState, contact_side,
};
use super::oracle::TerrainOracle;
use super::route::build_route;
use super::rubble::{
    PieceData, PieceReadData, RubbleBodies, RubbleQuery, apply_pushes, footprint_radius, reactions,
};
use super::side::Side;
use super::terrain::TrackField;
use super::transmission::{
    self, DriveSupply, TransmissionInput, TransmissionParams, TransmissionReport, TransmissionState,
};

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

/// Resolution: replicated travel span (m). The wire carries belt phase wrapped into
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
/// clutch regime, park lever and reversal pause). Server-authoritative, advanced only
/// where the body is dynamic through [`apply_track_forces`], and
/// replicated as one atomic snapshot to the tank's OWNER only; every other listener reads the
/// crank through [`EngineRpm`]. The determinism trace hashes every field of the exhaustive
/// projection (`transmission::transmission_state_projection`) in stable order, with raw bits for
/// the crank float.
#[derive(Component, Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct TankTransmission(pub TransmissionState);

impl TankTransmission {
    pub fn from_spec(tp: &TransmissionParams) -> Self {
        Self(TransmissionState::from_spec(tp))
    }
}

/// The public crank rotation magnitude, in whole rpm: the one transmission fact a listener other than the
/// tank's owner needs (its engine sound). Written from [`TankTransmission`] by
/// [`publish_engine_rpm`] wherever the body simulates, constructed beside it at spawn, and
/// replicated to every client.
///
/// Error against the crank magnitude: ≤ 0.5 rpm (round to nearest) over 0–65 535 rpm; a crank
/// outside that band saturates to its bound. Nonfinite sim state is a bug. Postcard writes the `u16` as a varint:
/// 2 bytes from 128 to 16 383 rpm.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EngineRpm(pub u16);

impl EngineRpm {
    pub fn of(transmission: &TankTransmission) -> Self {
        let omega = transmission.0.omega_e;
        assert!(
            omega.is_finite(),
            "public RPM requires a finite simulated crank"
        );
        Self((omega.abs() / transmission::RPM_TO_RAD).round() as u16)
    }

    pub fn rpm(self) -> f32 {
        f32::from(self.0)
    }
}

/// Per-side summed element grip force (N), `[left, right] × [longitudinal, lateral]`.
/// Derived telemetry over [`TrackGripElements`], hashed into the determinism trace (`hblt`).
/// These are generalized forces, not world anchors or replicated state.
#[derive(Component, Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TrackGrip {
    pub sides: [[f32; 2]; 2],
}

/// Local per-tick hull/belt effect of ground contact. Hull force and torque include grip,
/// element damping and rolling drag; belt reaction carries grip alone.
#[derive(Component, Clone, Copy, PartialEq, Debug, Default)]
pub struct TrackGripEffect {
    /// Total world-space grip and rolling force on the hull (N).
    pub tangential_force: Vec3,
    /// Current contact capacity (N), before mobilization, for prospective stopping.
    pub traction_capacity_n: f32,
    /// Delivered world-space rolling force on the hull (N), for prospective stopping.
    pub rolling_force: Vec3,
    /// Total world-space grip and rolling torque about the hull center of mass (N*m).
    pub tangential_torque: Vec3,
    /// Longitudinal ground reaction on `[left, right]` belts (N).
    pub belt_reaction: [f32; 2],
    /// Coarse, quantized digest of the complete element field. Diagnostic evidence only.
    #[cfg(any(test, feature = "bitprobe"))]
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

/// The last driving pass's contact telemetry per side — viz/diagnostics (debug force arrows, the
/// grounded count, traces). Rewritten by every pass; a resting tank keeps its last pass's, the
/// support it rests on ([`NotResting`]). Never hashed, never rolled back.
#[derive(Component, Default)]
pub struct TrackContacts(pub [Vec<BeltContact>; 2]);

/// The contact inputs of one drivetrain step ([`TransmissionInput`]): each belt's reaction and its
/// slope against its own speed, and the Driver's reading.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Loads {
    reactions: [f32; 2],
    reaction_slopes: [f32; 2],
    sensed: Option<super::sense::Sensed>,
}

/// What a tank's last driving pass leaves ([`apply_track_forces`]). Never hashed, never rolled
/// back.
#[derive(Component, Default)]
pub(crate) struct LastPass {
    /// Its drivetrain step's contact inputs, `None` before its first pass. A resting tank's are
    /// the ones it came to rest under, which its drivetrain keeps stepping on
    /// ([`wake_driven_tanks`]).
    loads: Option<Loads>,
    /// The pass found the tank at rest ([`apply_track_forces`]); one that did not holds it awake
    /// ([`hold_unsettled_awake`]).
    at_rest: bool,
    /// The impulses its belts put on dynamic bodies ([`reactions`]) and the colliders it sampled
    /// as support, sorted and distinct, for the shared commit ([`commit_track_contacts`]).
    pushes: Vec<super::rubble::Push>,
    colliders: Vec<Entity>,
}

/// Each collider some tank's last driving pass sampled as support, with the tanks it carries.
/// Kept at the contact commit, so a resting tank keeps the entries of the pass it rested under; a
/// collider that goes, is disabled or moves finds the resting tanks it carries here
/// (`crate::debris`), and a tank that goes leaves it.
#[derive(Resource, Default)]
pub(crate) struct SampledSupports {
    tanks: bevy::ecs::entity::EntityHashMap<Vec<Entity>>,
    colliders: bevy::ecs::entity::EntityHashMap<Vec<Entity>>,
}

impl SampledSupports {
    /// The tanks whose last pass sampled `collider`.
    pub(crate) fn carried_by(&self, collider: Entity) -> &[Entity] {
        self.tanks.get(&collider).map_or(&[], Vec::as_slice)
    }

    /// `tank`'s last pass sampled `colliders`, sorted and distinct.
    fn record(&mut self, tank: Entity, colliders: &[Entity]) {
        if self.colliders.get(&tank).map(Vec::as_slice) == Some(colliders) {
            return;
        }
        self.forget(tank);
        for &collider in colliders {
            self.tanks.entry(collider).or_default().push(tank);
        }
        self.colliders.insert(tank, colliders.to_vec());
    }

    fn forget(&mut self, tank: Entity) {
        for collider in self.colliders.remove(&tank).into_iter().flatten() {
            let carried = self
                .tanks
                .get_mut(&collider)
                .expect("every recorded collider indexes the tanks that sampled it");
            carried.retain(|&other| other != tank);
            if carried.is_empty() {
                self.tanks.remove(&collider);
            }
        }
    }
}

/// A tank that goes leaves the support index.
fn forget_sampled_supports(gone: On<Remove, TrackContacts>, mut index: ResMut<SampledSupports>) {
    index.forget(gone.entity);
}

/// Simulation per-element shear and loss dwell, one field per side. Constructed at spawn
/// at the force station/lateral resolution; `contact_side` preserves that membership so a
/// runtime rebuild cannot silently erase accumulated strain. Clients render the server stream.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[require(TrackGripEffect, LastPass)]
pub struct TrackGripElements {
    pub sides: [GripElements; 2],
}

impl TrackGripElements {
    /// Both sides pre-sized for `count` advected force cells × lateral points, independent of drawn links.
    pub fn for_links(count: usize, lateral_points: usize) -> Self {
        Self {
            sides: [
                GripElements::for_stations(count, lateral_points),
                GripElements::for_stations(count, lateral_points),
            ],
        }
    }
}

/// Coarse diagnostic field digest used by [`TrackGripEffect`]. Strain axes are
/// projected to signed logarithmic bins before FNV-1a. Four bins per octave at a
/// 0.5 mm Resolution distinguish recoverable shear without coupling to solver compliance.
/// Determinism traces retain raw-bit state.
#[cfg(any(test, feature = "bitprobe"))]
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
                // Resolution: diagnostic quantization, independent of constitutive stiffness.
                const DIGEST_STEP_M: f32 = 0.0005;
                const BINS_PER_OCTAVE: f32 = 4.0;
                let bin = (axis.signum() * (axis.abs() / DIGEST_STEP_M).ln_1p()
                    / std::f32::consts::LN_2
                    * BINS_PER_OCTAVE)
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
    /// quadrature across the MEASURED shoe bounds from
    /// [`RigGeom::grip_column_offsets`](super::rig_geom::RigGeom::grip_column_offsets).
    /// Offsets are signed per side; the shoe need not be centred on its pins.
    columns: [super::forces::LateralColumns; 2],
    params: ForceParams,
    construction: transmission::DriveConstruction,
    /// DERIVED bound over both sides' complete support slabs, in body-local metres.
    rubble_radius: f32,
    /// The declared joint transmission tables, derived against the measured geometry.
    trans: TransmissionParams,
}

impl TrackGear {
    /// The calibrated envelope law the belt segments carry against loose pieces.
    pub(crate) fn segment_law(&self) -> super::belt_segment::SegmentLaw {
        super::belt_segment::SegmentLaw {
            stiffness_per_m: self.params.support_stiffness_per_m,
            damping_per_m: self.params.support_damping_per_m,
        }
    }

    /// The declared joint transmission params. Read-only accessor for the shared drive HUD; the
    /// field stays private so only the drive step and HUD consume it.
    pub fn trans(&self) -> &TransmissionParams {
        &self.trans
    }

    /// The grip law's spawn construction.
    #[cfg(test)]
    pub(crate) fn grip(&self) -> GripParams {
        self.construction.grip()
    }

    /// Coupled translation mass for a torque-cut landing: hull plus both belt inertias (kg).
    #[cfg(all(test, feature = "bitprobe"))]
    pub(crate) fn landing_mass(&self) -> f32 {
        transmission::coupled_mass(&self.construction)
    }

    /// The construction the Driver prices candidates against.
    #[cfg(all(test, feature = "bitprobe"))]
    pub(crate) fn construction(&self) -> transmission::DriveConstruction {
        self.construction
    }

    /// DERIVED full service-brake deceleration along the direction of motion (m/s^2).
    /// Grip is limited by brakes and observed capacity; delivered rolling acts alongside it.
    /// `rolling_braking_n` is the contact law's rolling force projected opposite travel, so
    /// its support-relative speed and fade are read once at delivery. Hull mass plus both
    /// reflected belt inertias must be arrested. Nonpositive means the stop is unaffordable.
    pub(crate) fn braking_deceleration(
        &self,
        grade_sine: f32,
        direction: f32,
        contact_capacity_n: f32,
        rolling_braking_n: f32,
    ) -> f32 {
        let grip = self.construction.grip();
        let weight = 2.0 * grip.side_weight;
        let available =
            (2.0 * self.trans.brake_capacity_n).min(contact_capacity_n) + rolling_braking_n;
        (available + direction * weight * grade_sine)
            / transmission::coupled_mass(&self.construction)
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

    /// The measured ground face offset (m): pin line → the shoe's sole (`pin_to_outer`). The view's
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
            for (index, (offset, weight)) in self.columns[si].samples().iter().copied().enumerate()
            {
                out.f32(&format!("force.columns.{side}[{index}].offset"), offset);
                out.f32(&format!("force.columns.{side}[{index}].weight"), weight);
            }
        }
        out.f32("force.support_stiffness_per_m", p.support_stiffness_per_m);
        out.f32("force.support_damping_per_m", p.support_damping_per_m);
        out.f32("force.engage_depth", p.engage_depth);
        out.f32("force.probe_reach", p.probe_reach);
        out.f32("force.inertia", self.construction.belt_inertia);
        let grip = self.construction.grip();
        out.f32("force.grip.track", grip.track.dynamic_coefficient);
        out.f32("force.grip.nominal_pressure_kpa", grip.nominal_pressure_kpa);
        out.f32("force.grip.side_weight", grip.side_weight);
        out.f32("force.grip.hull_coupling", grip.hull_coupling);

        let tp = &self.trans;
        out.f32("transmission.engine.idle_rpm", tp.engine.idle_rpm);
        out.f32("transmission.engine.governed_rpm", tp.engine.governed_rpm);
        out.f32(
            "transmission.engine.crank_limit_rpm",
            tp.engine.crank_limit_rpm,
        );
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
        out.u32("transmission.launch_gears", u32::from(tp.launch_gears));
        out.u32("transmission.max_step", u32::from(tp.max_step));
        for (index, (tight, wide)) in tp.steer_kappa.iter().copied().enumerate() {
            out.f32(&format!("transmission.steer_kappa[{index}].tight"), tight);
            out.f32(&format!("transmission.steer_kappa[{index}].wide"), wide);
        }
        for (index, (tight, wide)) in tp.steer_radii_m.iter().copied().enumerate() {
            out.f32(&format!("transmission.steer_radii[{index}].tight"), tight);
            out.f32(&format!("transmission.steer_radii[{index}].wide"), wide);
        }
        for (index, (tight, wide)) in tp.reverse_steer_kappa.iter().copied().enumerate() {
            out.f32(
                &format!("transmission.reverse_steer_kappa[{index}].tight"),
                tight,
            );
            out.f32(
                &format!("transmission.reverse_steer_kappa[{index}].wide"),
                wide,
            );
        }
        for (index, (tight, wide)) in tp.reverse_steer_radii_m.iter().copied().enumerate() {
            out.f32(
                &format!("transmission.reverse_steer_radii[{index}].tight"),
                tight,
            );
            out.f32(
                &format!("transmission.reverse_steer_radii[{index}].wide"),
                wide,
            );
        }
        out.f32("transmission.steer_capacity_n", tp.steer_capacity_n);
        out.f32("transmission.neutral_d_full", tp.neutral_d_full);
        out.u32(
            "transmission.neutral_mean_held",
            u32::from(tp.neutral_mean == super::transmission::NeutralMean::Held),
        );
        out.f32("transmission.recirculation", tp.recirculation);
        out.f32("transmission.brake_capacity_n", tp.brake_capacity_n);
        out.f32("transmission.brake_static_factor", tp.brake_static_factor);
        out.f32("transmission.drag_fraction", tp.drag_fraction);
        out.f32("transmission.engine_inertia", tp.engine_inertia);
        out.f32("transmission.clutch_capacity", tp.clutch_capacity);
        out.u32("transmission.shift_ticks", u32::from(tp.shift_ticks.get()));
        out.f32("transmission.peak_torque_rpm", tp.peak_torque_rpm);
        out.f32("transmission.peak_torque_nm", tp.peak_torque_nm);
    }

    /// Test-only construction variation for contact-law fixtures.
    #[cfg(test)]
    pub(crate) fn params_mut(&mut self) -> &mut ForceParams {
        &mut self.params
    }
}

pub fn sim_plugin(app: &mut App) {
    app.add_plugins((super::world_contact::plugin, super::belt_segment::plugin));
    app.init_resource::<DriveSupports>()
        .init_resource::<SampledSupports>()
        .add_observer(forget_sampled_supports);
    app.insert_resource(super::transmission::DriverResolution::load());
    // Lazy one-shot: the blueprint lands at Startup (bake); the gear builds on the first
    // frame after and never again.
    app.add_systems(
        PreUpdate,
        init_track_gear
            .run_if(resource_exists::<TankBlueprint>.and_then(not(resource_exists::<TrackGear>))),
    );
    app.add_systems(
        FixedUpdate,
        // RPM reads the completed crank; contact commits need not delay its projection.
        (
            driving_systems(),
            publish_engine_rpm.after(apply_track_forces),
        )
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
    tanks
        .par_iter_mut()
        .for_each(|(body, transmission, mut rpm)| {
            if matches!(*body, RigidBody::Dynamic) {
                rpm.set_if_neq(EngineRpm::of(transmission));
            }
        });
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
    let gear = TrackGear::from_construction(&blueprint, &geom);
    commands.insert_resource(gear.segment_law());
    commands.insert_resource(gear);
    // Keep the marker-derived running gear alive as the game's single geometry source: the track
    // VIEW (`track::view`) reads its rest circles / pitch / plate / plane directly, and the bit
    // probe echoes the measured scalars. Built once here alongside `TrackGear`, never re-derived.
    commands.insert_resource(geom);
}

impl TrackGear {
    pub(crate) fn from_construction(
        blueprint: &TankBlueprint,
        geom: &super::rig_geom::RigGeom,
    ) -> Self {
        let resolution = super::forces::CONTACT_RESOLUTION;
        let spec = &blueprint.spec.track;
        let sus = spec.suspension.params();
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
        let columns = Side::ALL
            .map(|side| super::forces::LateralColumns::from(geom.grip_column_offsets(side)));
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
            resolution.stations,
            geom.model.pin_to_outer,
            spec.suspension.engage,
            PROBE_REACH,
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
            .map(|columns| {
                let (lo, hi) = columns.bounds();
                lo.abs().max(hi.abs())
            })
            .fold(0.0_f32, f32::max);
        let travel = travel_knots
            .iter()
            .flatten()
            .map(|(_, t)| *t)
            .fold(0.0_f32, f32::max);
        let rubble_radius = footprint_radius(
            pin_radius,
            column,
            geom.model.pin_to_outer,
            travel,
            PROBE_REACH,
        );
        let inertia = spec
            .reflected_inertia(&blueprint.spec.roadwheels, geom.wrap_radii())
            .expect("running-gear admission certified the construction inertia");
        let grip = GripParams::of(
            &blueprint.spec,
            crate::track::rig_geom::tank_ground_pressure_kpa(geom),
            crate::track::rig_geom::com_rest_height_m(&blueprint.geometry, geom),
        );
        info!(
            "grip law: hull coupling {:.3}, shortest shear length {:.2} mm per unit grip at the fixed step",
            grip.hull_coupling,
            grip.step_shear_per_grip_m(crate::sim_tick::sim_timestep().as_secs_f32()) * 1e3,
        );
        let construction = transmission::DriveConstruction::new(grip, inertia);
        Self {
            loop_pts,
            count: resolution.stations,
            plane_x: Side::ALL.map(|side| geom.side_plane_x(side)),
            travel_knots,
            columns,
            trans,
            rubble_radius,
            construction,
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
                probe_reach: PROBE_REACH,

                grip: Some(grip),
            },
        }
    }
}

/// The hull as one driving tick reads it (the game's driving model): its velocity field at the
/// tick's start and its explicit response to the forces the tick applies.
#[derive(Clone, Copy)]
pub(crate) struct HullBody {
    /// The centre of mass's velocity (m/s).
    pub linear: Vec3,
    /// rad/s.
    pub angular: Vec3,
    pub center_of_mass: Vec3,
    /// 1/kg, and the world-frame inverse inertia (1/(kg m²)); zero for a hull held still.
    pub inverse_mass: f32,
    pub inverse_inertia: Mat3,
    /// The acceleration the world adds to the applied forces (m/s²).
    pub gravity: Vec3,
}

impl HullBody {
    pub(crate) fn velocity_at(&self, point: Vec3) -> Vec3 {
        self.linear + self.angular.cross(point - self.center_of_mass)
    }

    /// The change over `dt`, under `apps`, of the hull's velocity field as the hull sees it: at
    /// `origin`, and in angular velocity. The hull turning under its own velocity is part of the
    /// frame, so a steady turn reads no change.
    fn step<'a>(
        &self,
        apps: impl Iterator<Item = &'a super::forces::ForceApp>,
        origin: Vec3,
        dt: f32,
    ) -> (Vec3, Vec3) {
        let (force, torque) = apps.fold((Vec3::ZERO, Vec3::ZERO), |(f, t), app| {
            (
                f + app.force,
                t + (app.point - self.center_of_mass).cross(app.force),
            )
        });
        let spin = self.inverse_inertia * torque * dt;
        let accel = force * self.inverse_mass + self.gravity - self.angular.cross(self.linear);
        (accel * dt + spin.cross(origin - self.center_of_mass), spin)
    }
}

/// What the transmission integrates each belt against: the reaction it meets if it holds its
/// speed while the hull moves under every force `reports` apply this tick, and that reaction's
/// slope against its own speed.
pub(crate) fn belt_reactions(
    reports: &[SideReport; 2],
    hull: &HullBody,
    origin: Vec3,
    dt: f32,
) -> ([f32; 2], [f32; 2]) {
    let motion = hull.step(reports.iter().flat_map(|r| &r.apps), origin, dt);
    (
        reports
            .each_ref()
            .map(|r| r.belt_reaction - r.reaction_slope.under(motion)),
        reports.each_ref().map(|r| r.reaction_slope.belt),
    )
}

/// The Driver feels the tick ([`super::sense`]) with the belts at `belts` (m/s, `[left, right]`),
/// then each belt takes on its solver contact's belt reaction. The ledger reads each contact's traction on the hull along the heading; the belt
/// reaction is the drive's load along each contact's travel, partly vertical around a wrap.
pub(crate) fn feel_tick(
    sense: &mut super::sense::DriverSense,
    reports: &mut [SideReport; 2],
    hard: [Option<super::stop_contacts::HardReaction>; 2],
    hull: &HullBody,
    belts: [f32; 2],
    affine: Affine3A,
    dt: f32,
) -> Option<super::sense::Sensed> {
    let forward = affine.transform_vector3(Vec3::NEG_Z);
    let sensed = sense.tick(
        hull.linear,
        forward,
        (belts[0] + belts[1]) * 0.5,
        1.0 / hull.inverse_mass,
        super::sense::Contacts {
            soft: reports
                .each_ref()
                .map(|report| report.contacts.iter().map(|c| c.traction).sum()),
            grip: reports.iter().map(|report| report.grip_budget).sum(),
            hard,
        },
        dt,
    );
    for (report, hard) in reports.iter_mut().zip(hard) {
        if let Some(hard) = hard {
            report.belt_reaction += hard.belt;
        }
    }
    sensed
}

/// One tank's mutable belt/drivetrain state for [`belt_tick`] — the caller owns the storage
/// (ECS components for the live sim, plain fields for the recoil microsim).
pub(crate) struct BeltRig<'a> {
    pub drive: &'a mut TrackDrive,
    pub signal: &'a mut DriveSignal,
    pub grip: &'a mut TrackGrip,
    pub elements: &'a mut TrackGripElements,
    pub transmission: &'a mut TransmissionState,
    pub sense: &'a mut super::sense::DriverSense,
    /// Last physics step's hard-contact reaction; None means that side had no solver contact,
    /// preserving the spring-only tick.
    pub hard_reactions: [Option<super::stop_contacts::HardReaction>; 2],
}

/// ONE tank's complete drivetrain tick: command slew, both contact patches at their pre-tick
/// belt speeds, the joint transmission solve, belt speed/phase commit. THE shared law —
/// [`apply_track_forces`] (the ECS adapter) runs exactly this function, so every caller gets the
/// sim's own arithmetic, never a copy of it.
///
/// The drivetrain's tick under `target`, its belts at `speeds` (m/s) meeting `loads`: the
/// Driver's command slews `signal` ([`shape_drive`]) and the joint transmission steps `state`.
fn drivetrain_tick(
    gear: &TrackGear,
    target: DriveTarget,
    signal: &mut DriveSignal,
    state: &mut TransmissionState,
    speeds: [f32; 2],
    loads: Loads,
    dt: f32,
) -> TransmissionReport {
    let DriveTarget { intent, supply } = target;
    let axes = match intent {
        DriveIntent::Axes(axes) if supply.control_authority => axes,
        DriveIntent::Axes(_) => {
            *signal = DriveSignal::default();
            DriveAxes::default()
        }
        DriveIntent::Brake => DriveAxes::default(),
    };
    let shaped = shape_drive(
        DriveAxes {
            throttle: signal.throttle,
            steer: signal.steer,
        },
        axes,
        dt,
    );
    signal.throttle = shaped.throttle;
    signal.steer = shaped.steer;
    transmission::step(
        &gear.construction,
        &gear.trans,
        state,
        &TransmissionInput {
            intent,
            throttle: signal.throttle,
            steer: signal.steer,
            side_commands: shaped.side_commands(),
            speeds,
            reactions: loads.reactions,
            reaction_slopes: loads.reaction_slopes,
            sensed: loads.sensed,
            dt,
            supply,
        },
    )
}

/// Returns the per-side reports (force applications in emission order — the caller applies or
/// integrates them, left then right, verbatim), the transmission report and the loads it stepped
/// on. Belt state (`drive.sides`, `grip.sides`) is committed in here; forces/telemetry stay with
/// the caller.
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
    hull: HullBody,
    rig: BeltRig<'_>,
) -> ([SideReport; 2], TransmissionReport, Loads) {
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
            |p| hull.velocity_at(p),
            &mut rig.elements.sides[si],
        );
        reports[si] = report;
    }
    let sensed = feel_tick(
        rig.sense,
        &mut reports,
        rig.hard_reactions,
        &hull,
        [rig.drive.sides[0].speed, rig.drive.sides[1].speed],
        affine,
        dt,
    );
    let (reactions, reaction_slopes) =
        belt_reactions(&reports, &hull, affine.translation.into(), dt);
    let loads = Loads {
        reactions,
        reaction_slopes,
        sensed,
    };
    let tr = drivetrain_tick(
        gear,
        target,
        rig.signal,
        rig.transmission,
        [rig.drive.sides[0].speed, rig.drive.sides[1].speed],
        loads,
        dt,
    );
    rig.sense.drove(&tr);
    for (si, report) in reports.iter().enumerate() {
        let pre_speed = rig.drive.sides[si].speed;
        rig.drive.sides[si] = TrackDriveSide {
            // A capacity-satisfied brake is a stationary surface, exactly; a rigid friction
            // row must not amplify floating-point stop-force cancellation into belt motion.
            speed: if rig.hard_reactions[si].is_some() && tr.held[si] {
                0.0
            } else {
                tr.next_speeds[si]
            },
            // Phase advects at the PRE-update speed — contact evaluation used those stations.
            phase: rig.drive.sides[si].phase + f64::from(pre_speed * dt),
        };
        rig.grip.sides[si] = [report.state.grip.x, report.state.grip.y];
    }
    (reports, tr, loads)
}

/// The game's damage model: a phase hold imposes service brakes independently of staffing.
/// Repair holds brakes only with staffed controls; the Driver masks unstaffed axis requests.
// Holds and staffing mask actuation, never the player's local lever. Releasing a hold or
// restoring staffing resumes the latest notch, including taps made while controls were absent.
pub(crate) fn drive_target(
    command: &ParsedTankCommand,
    tank: Option<&TankVolumes>,
    caps: Option<&TankCapabilities>,
    volumes: &Query<VolumeFacets>,
) -> DriveTarget {
    let supply = drive_supply(tank, caps, volumes);
    let repairing = supply.control_authority
        && tank.is_some_and(|tank| crate::damage::repair::underway(tank, volumes));
    let intent = if command.held() || repairing {
        DriveIntent::Brake
    } else {
        DriveIntent::Axes(DriveAxes {
            throttle: command.throttle,
            steer: command.steer,
        })
    };
    DriveTarget { intent, supply }
}

/// The game's damage model: an unbound tank has no controls, task throughput or sources.
fn drive_supply(
    tank: Option<&TankVolumes>,
    caps: Option<&TankCapabilities>,
    volumes: &Query<VolumeFacets>,
) -> DriveSupply {
    let (Some(tank), Some(caps)) = (tank, caps) else {
        return DriveSupply {
            control_authority: false,
            task_rate: 0.0,
            engine_in_action: false,
            transmission_condition: 0.0,
        };
    };
    let facts = part_facts(tank, volumes);
    supply_from_parts(caps, &facts.quality, &facts.staffed)
}

fn supply_from_parts(
    caps: &TankCapabilities,
    quality: &std::collections::HashMap<crate::damage::Part, f32>,
    staffed: &std::collections::HashSet<crate::damage::CrewStation>,
) -> DriveSupply {
    DriveSupply {
        control_authority: caps.drive_controls.permits(staffed),
        task_rate: evaluate(caps.requirement(Capability::Drive), quality),
        engine_in_action: live_quality(quality, crate::damage::Part::Engine) > 0.0,
        transmission_condition: live_quality(quality, crate::damage::Part::Transmission),
    }
}

/// The complete dynamic driving bundle; an absent force item is a broken Avian bundle,
/// not a query filter that may silently remove an admitted tank.
#[derive(QueryData)]
#[query_data(mutable)]
struct DriveTank {
    entity: Entity,
    position: &'static Position,
    rotation: &'static Rotation,
    center_of_mass: &'static ComputedCenterOfMass,
    mass: &'static ComputedMass,
    angular_inertia: &'static ComputedAngularInertia,
    body: &'static RigidBody,
    command: &'static ParsedTankCommand,
    drive: &'static mut TrackDrive,
    signal: &'static mut DriveSignal,
    grip: &'static mut TrackGrip,
    elements: &'static mut TrackGripElements,
    effect: &'static mut TrackGripEffect,
    transmission: &'static mut TankTransmission,
    sense: &'static mut super::sense::DriverSense,
    contacts: &'static mut TrackContacts,
    last: &'static mut LastPass,
    forces: Option<Forces>,
    volumes: Option<&'static TankVolumes>,
    capabilities: Option<&'static TankCapabilities>,
}

/// Tick-start geometry and source inertia, consumed by commit_track_contacts. Support shape
/// copies live only between snapshot publication and the same update's contact commit.
#[derive(Resource, Default)]
struct DriveSupports {
    pieces: RubbleBodies<'static>,
    sources: Vec<(Entity, Vec<Entity>)>,
    hard_contacts: std::collections::BTreeMap<Entity, Vec<super::stop_contacts::BeltImpulse>>,
    hard_dt: f32,
}

/// The tanks whose driving pass runs this tick: every tank but a resting one. A resting tank is
/// one Avian's island sleep holds still, with whatever carries it: terrain and construction
/// through the sampled support, loose pieces through its island. Every pass over its last rest
/// time found it at rest ([`apply_track_forces`]), so its hull and every contact are as its last
/// pass found them: its [`TrackContacts`] keep that support, fixed construction keeps that pass's
/// reaction (`crate::contact_transfer`, IMPLICIT CONTACTS), and its drivetrain steps on that
/// pass's loads ([`wake_driven_tanks`]). It rests until something wakes it: drive demand, a
/// drivetrain step that no longer holds both belts, a support that goes, is disabled, turns sensor
/// or changes (`crate::debris`'s wakes), or a contact or impulse from a moving body (Avian's island
/// wake).
type NotResting = (With<Tank>, Without<Sleeping>);

/// The Driver asks the drive for something, a pedal or a steer off zero, or a belt still coasts at
/// the transmission's own parked speed or more. Driven belts answer through their contacts, which
/// a resting tank's last pass no longer reads: under a pedal the lever holds a belt only against
/// rollback, so whether it holds turns on the sign of a reaction that jitters about zero at rest.
/// MEASURED once from rest under full throttle, with the trial stepping on the last pass's loads
/// (a probe on `track_scorecard`'s ground and pieces-on-ground carriers, not kept): an awake
/// twin's belts crept from the first tick and launched 9 ticks before the trial released them.
fn demand(command: &ParsedTankCommand, drive: &TrackDrive) -> bool {
    command.throttle != 0.0
        || command.steer != 0.0
        || drive
            .sides
            .iter()
            .any(|side| side.speed.abs() >= super::transmission::PARK_ENGAGE_SPEED)
}

/// Snapshot -> owner-local calculation -> cross-owner effects, all in this fixed update.
/// Drive demand ([`demand`]) wakes a resting tank before this tick's driving pass. Without it,
/// the resting tank's drivetrain steps while its hull rests: the transmission steps on its last
/// pass's loads with the belts at rest, as Avian holds them. A step that holds both belts and
/// slides no brake leaves them at rest, and so every contact, and commits as that tick's whole
/// pass: a repaired engine spools up under a resting hull. Any other step is discarded
/// and the tank wakes, so the tick's full pass steps the drivetrain once, from the same state. The
/// Driver's ledger reads the hull, which gives it nothing while it rests. Removing `Sleeping` is
/// Avian's own wake of the island (`wake_on_remove_sleeping`, which zeroes the timer); the
/// chain's sync point applies it before [`gather_drive_supports`].
fn wake_driven_tanks(
    time: Res<Time>,
    gear: Option<Res<TrackGear>>,
    mut tanks: Query<
        (
            Entity,
            &RigidBody,
            &ParsedTankCommand,
            &TrackDrive,
            (&mut DriveSignal, &mut TankTransmission),
            &LastPass,
            (Option<&TankVolumes>, Option<&TankCapabilities>),
        ),
        (With<Tank>, With<Sleeping>),
    >,
    volumes: Query<VolumeFacets>,
    commands: ParallelCommands,
) {
    let dt = time.delta_secs();
    tanks.par_iter_mut().for_each(
        |(entity, body, command, drive, (mut signal, mut transmission), last, (tank, caps))| {
            if *body != RigidBody::Dynamic {
                return;
            }
            if !demand(command, drive) {
                let gear = gear
                    .as_deref()
                    .expect("a dynamic tank is admitted only after startup constructs TrackGear");
                let loads = last
                    .loads
                    .expect("a tank rests only after driving passes found it at rest");
                let (mut held_signal, mut state) = (*signal, transmission.0);
                let tr = drivetrain_tick(
                    gear,
                    drive_target(command, tank, caps, &volumes),
                    &mut held_signal,
                    &mut state,
                    [0.0; 2],
                    loads,
                    dt,
                );
                if tr.held == [true; 2] && !tr.brakes_slide {
                    signal.set_if_neq(held_signal);
                    transmission.set_if_neq(TankTransmission(state));
                    return;
                }
            }
            commands.command_scope(|mut commands| {
                commands.entity(entity).remove::<Sleeping>();
            });
        },
    );
}

/// A pass that did not find its tank at rest restarts the tank's rest timer, which Avian's island
/// sleep reads after this update (`update_sleeping_states`, `dynamics/solver/islands/
/// sleeping.rs`): the tank and its island rest only once a whole rest time of passes found it at
/// rest. Avian's `Forces` item holds the timer through the pass and writes it only for a force.
fn hold_unsettled_awake(mut tanks: Query<(&RigidBody, &LastPass, &mut SleepTimer), NotResting>) {
    tanks.par_iter_mut().for_each(|(body, last, mut timer)| {
        // Replicas have no driving pass.
        if *body == RigidBody::Dynamic && !last.at_rest {
            timer.0 = 0.0;
        }
    });
}

fn driving_systems() -> ScheduleConfigs<ScheduleSystem> {
    (
        wake_driven_tanks,
        gather_drive_supports,
        apply_track_forces,
        hold_unsettled_awake,
        commit_track_contacts,
    )
        .chain()
        .into_configs()
}

#[expect(
    clippy::too_many_arguments,
    reason = "tick-start geometry and solver readings share one snapshot boundary"
)]
fn gather_drive_supports(
    stages: Option<Res<crate::cost::stages::Stages>>,
    gear: Option<Res<TrackGear>>,
    rubble: RubbleQuery,
    bodies: Query<PieceReadData>,
    tanks: Query<(Entity, Option<&Position>, &RigidBody), NotResting>,
    mut supports: ResMut<DriveSupports>,
    transfers: Res<crate::contact_transfer::ContactTransfers>,
    faces: Query<(
        &super::stop_contacts::BeltFace,
        &avian3d::prelude::ColliderOf,
    )>,
    mut gathered: Local<Parallel<Vec<(Entity, Vec<Entity>)>>>,
) {
    use crate::cost::stages::{Stage, Stages, Work};
    let _stage = Stages::start(stages.as_deref(), Stage::BeltContacts);
    supports.hard_contacts = super::stop_contacts::by_owner(&transfers, &faces);
    supports.hard_dt = transfers.dt();
    tanks.par_iter().for_each(|(entity, position, body)| {
        // Replicas have no authoritative driving pass and gather no support shapes.
        if *body != RigidBody::Dynamic {
            return;
        }
        let position = position.expect(
            "a dynamic tank carries its complete spawn-built driving state, including Position",
        );
        let gear = gear
            .as_deref()
            .expect("a dynamic tank is admitted only after startup constructs TrackGear");
        let mut local = rubble.near(position.0, gear.rubble_radius);
        if let Some(stages) = stages.as_deref() {
            stages.add(Work::BeltNearColliders, local.len() as u64);
        }
        local.retain(|&collider| rubble.body_of(collider) != entity);
        gathered.scope(|sources| sources.push((entity, local)));
    });
    gathered.drain_into(&mut supports.sources);
    let mut near = Vec::new();
    for (_, local) in &supports.sources {
        near.extend_from_slice(local);
    }
    supports.sources.sort_unstable_by_key(|(source, _)| *source);
    near.sort_unstable();
    near.dedup();
    // Gathering reads body state once, before any source applies its own forces.
    supports.pieces = RubbleBodies::gather(&rubble, &near, &bodies);
    supports.pieces.stages = stages.as_deref().cloned();
}

/// Each tank reads the common tick-start snapshot and changes only its own driving and force state.
/// Bevy 0.19.1's QueryParIter batches on ComputeTaskPool; worker assignment has no effect order.
#[expect(
    clippy::too_many_arguments,
    reason = "one owner reads terrain, construction and solver impulses before its joint solve"
)]
fn apply_track_forces(
    time: Res<Time>,
    gravity: Res<Gravity>,
    field: Res<TrackField>,
    gear: Option<Res<TrackGear>>,
    #[cfg(feature = "bitprobe")] mut bitprobe: Option<ResMut<BitprobeCapture>>,
    volumes: Query<VolumeFacets>,
    supports: Res<DriveSupports>,
    faces: Query<(
        &super::stop_contacts::BeltFace,
        &avian3d::prelude::ColliderOf,
    )>,
    mut tanks: Query<DriveTank, NotResting>,
) {
    #[cfg(feature = "bitprobe")]
    if let Some(capture) = bitprobe.as_deref_mut() {
        capture.clear_tick();
    }
    // Before spawn, and on a replica-only client, no dynamic tank needs driving construction.
    if supports.sources.is_empty() {
        return;
    }
    let gear = gear
        .as_deref()
        .expect("a dynamic tank is admitted only after startup constructs TrackGear");
    let oracle = field
        .field
        .as_ref()
        .expect("a dynamic tank is admitted only after its world publishes the terrain field");
    for (source, _) in &supports.sources {
        assert!(
            tanks.contains(*source),
            "a dynamic tank carries its complete spawn-built driving state"
        );
    }
    let dt = time.delta_secs();
    #[cfg(feature = "bitprobe")]
    let captured = std::sync::OnceLock::new();
    #[cfg(feature = "bitprobe")]
    let capture_source = bitprobe.as_ref().map(|_| {
        supports
            .sources
            .last()
            .expect("dynamic sources are nonempty")
            .0
    });
    tanks.par_iter_mut().for_each(|mut tank| {
        if *tank.body != RigidBody::Dynamic {
            return; // Replicas consume the server stream.
        }
        let index = supports
            .sources
            .binary_search_by_key(&tank.entity, |(source, _)| *source)
            .expect("every dynamic tank belongs to the synchronous tick-start snapshot");
        let pieces = supports
            .pieces
            .for_source(tank.entity, &supports.sources[index].1);
        let mut forces = tank
            .forces
            .take()
            .expect("a dynamic driving hull carries Avian force state");
        let target = drive_target(tank.command, tank.volumes, tank.capabilities, &volumes);
        let affine = Affine3A::from_rotation_translation(tank.rotation.0, tank.position.0);
        // avian3d 0.7 ForcesItem's global COM expression in query_data.rs.
        let center_of_mass = tank.position.0 + tank.rotation.0 * tank.center_of_mass.0;
        // avian3d 0.7 dynamics/integrator/mod.rs: the increments are inverse mass times force
        // plus gravity, and the rotated inverse inertia times torque. A tank locks no axis and
        // carries no damping; the solver's gyroscopic term is left out.
        let hull = HullBody {
            linear: forces.linear_velocity(),
            angular: forces.angular_velocity(),
            center_of_mass,
            inverse_mass: tank.mass.inverse(),
            inverse_inertia: tank
                .angular_inertia
                .rotated(tank.rotation.0)
                .inverse()
                .to_mat3(),
            gravity: gravity.0,
        };
        #[cfg_attr(
            not(feature = "bitprobe"),
            expect(
                unused_mut,
                reason = "only bitprobe takes its probes out of the reports"
            )
        )]
        let (mut reports, tr, loads) = belt_tick(
            gear,
            oracle,
            &pieces,
            target,
            affine,
            dt,
            hull,
            BeltRig {
                drive: &mut tank.drive,
                signal: &mut tank.signal,
                grip: &mut tank.grip,
                elements: &mut tank.elements,
                transmission: &mut tank.transmission.0,
                sense: &mut tank.sense,
                hard_reactions: super::stop_contacts::reactions(
                    supports
                        .hard_contacts
                        .get(&tank.entity)
                        .map_or(&[], Vec::as_slice),
                    supports.hard_dt,
                    tank.position.0,
                    tank.rotation.0,
                    &faces,
                ),
            },
        );
        #[cfg(feature = "bitprobe")]
        if capture_source == Some(tank.entity) {
            // The single-source diagnostic selects the greatest tank entity, never the last worker.
            captured
                .set(BitprobeCapture {
                    tanks_seen: supports.sources.len() as u32,
                    command: [tank.command.throttle, tank.command.steer],
                    contact_inputs: reports
                        .each_mut()
                        .map(|report| std::mem::take(&mut report.bitprobe_contacts)),
                    element_outputs: reports
                        .each_mut()
                        .map(|report| std::mem::take(&mut report.bitprobe_elements)),
                    belt_reaction: [reports[0].belt_reaction, reports[1].belt_reaction],
                    transmission: tr.bitprobe,
                })
                .expect("a parallel query visits its selected capture source exactly once");
        }
        // Avian's rest test reads motion alone. The pass finds the tank at rest only without
        // drive demand, with both belts held, which lands them at rest, no brake or every
        // gripping patch sliding, and every sampled support fixed: nothing tells a tank that a
        // moving support moved.
        let grips = reports.iter().any(|report| report.grip_budget > 0.0);
        let grip_slides = grips && !reports.iter().any(|report| report.grip_reserve);
        let contacts = || reports.iter().flat_map(|report| &report.contacts);
        let last = &mut *tank.last;
        last.loads = Some(loads);
        last.at_rest = !demand(tank.command, &tank.drive)
            && tr.held == [true; 2]
            && !tr.brakes_slide
            && !grip_slides
            && contacts().all(|contact| contact.collider.is_none_or(|c| pieces.fixed(c)));
        last.pushes = reactions(&pieces, contacts(), dt);
        last.colliders.clear();
        last.colliders
            .extend(contacts().filter_map(|contact| contact.collider));
        last.colliders.sort_unstable();
        last.colliders.dedup();
        let mut effect = TrackGripEffect::default();
        #[cfg(feature = "bitprobe")]
        {
            effect.transmission_probe = tr.bitprobe;
        }
        for (si, report) in reports.into_iter().enumerate() {
            effect.belt_reaction[si] = report.belt_reaction;
            for contact in &report.contacts {
                effect.tangential_force += contact.tangential_force();
                effect.rolling_force += contact.rolling_force;
                effect.traction_capacity_n += contact.mu * contact.load_elastic;
                effect.tangential_torque +=
                    (contact.point - center_of_mass).cross(contact.tangential_force());
            }
            // Each owner retains left/right and force-emission order. The pass's verdict alone
            // restarts the rest timer, so a force leaves it alone, as a solver contact does.
            for app in &report.apps {
                forces
                    .non_waking()
                    .apply_force_at_point(app.force, app.point);
            }
            tank.contacts.0[si] = report.contacts;
        }
        #[cfg(any(test, feature = "bitprobe"))]
        {
            effect.field_digest = coarse_grip_digest(&tank.elements);
        }
        *tank.effect = effect;
    });
    #[cfg(feature = "bitprobe")]
    if let Some(capture) = bitprobe.as_deref_mut() {
        *capture = captured
            .into_inner()
            .expect("the selected dynamic capture source completed this tick");
    }
}

/// Each source's pass prepared its effects ([`LastPass`]); they commit by ascending source
/// entity, then side and contact emission order. Impulses on a dynamic recipient add, so no order
/// among them matters. Each source's sampled colliders replace its entries in
/// [`SampledSupports`].
fn commit_track_contacts(
    mut supports: ResMut<DriveSupports>,
    tanks: Query<(&TrackContacts, &LastPass), With<Tank>>,
    mut bodies: Query<PieceData>,
    mut transfers: ResMut<crate::contact_transfer::ContactTransfers>,
    mut sampled: ResMut<SampledSupports>,
) {
    let DriveSupports {
        pieces,
        sources,
        hard_contacts,
        ..
    } = &mut *supports;
    for &(source, _) in sources.iter() {
        let (contacts, last) = tanks
            .get(source)
            .expect("a gathered source remains alive through its same-tick commit");
        for side in &contacts.0 {
            pieces.record_static_contacts(source, side, &mut transfers);
        }
        apply_pushes(&last.pushes, &mut bodies);
        sampled.record(source, &last.colliders);
    }
    *pieces = RubbleBodies::default();
    sources.clear();
    hard_contacts.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(bevy::ecs::schedule::ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
    struct DriveTest;
    use crate::damage::Part;

    /// Fixture capabilities whose drive control the Driver alone grants.
    fn driver_staffed(
        requirements: std::collections::HashMap<
            crate::damage::Capability,
            crate::damage::Requirement,
        >,
    ) -> TankCapabilities {
        TankCapabilities {
            requirements,
            drive_controls: crate::damage::ControlStaffing::Any(vec![vec![
                crate::damage::CrewStation::Driver,
            ]]),
        }
    }

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
        .init_resource::<DriveSupports>()
        .init_resource::<SampledSupports>()
        .add_systems(DriveTest, driving_systems())
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
        #[cfg(feature = "bitprobe")]
        app.init_resource::<BitprobeCapture>();
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
        let sense = super::super::sense::DriverSense::for_cut(gear.trans().shift_ticks);
        let count = gear.count;
        let lateral_points = gear.columns[0].samples().len();
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
                TrackGripElements::for_links(count, lateral_points),
                DriveSignal::default(),
                trans,
                sense,
            ))
            .id()
    }

    /// Avian's tick-start velocity state on `body`, then the production adapter once.
    fn run_adapter(app: &mut App, bodies: &[(Entity, Vec3)]) {
        use avian3d::prelude::*;
        for &(body, velocity) in bodies {
            app.world_mut().entity_mut(body).insert((
                LinearVelocity(velocity),
                AngularVelocity::ZERO,
                avian3d::dynamics::integrator::VelocityIntegrationData::default(),
            ));
        }
        app.world_mut().run_schedule(DriveTest);
    }

    /// A hard contact's friction must enter the same joint transmission as the spring contacts.
    /// An airborne fixture isolates that path; omitting the solver reading makes both runs equal.
    #[test]
    fn hard_contact_friction_loads_the_joint_transmission() {
        use crate::contact_transfer::{ContactTransfer, ContactTransfers};
        let run = |impulse: f32| {
            let (mut app, _) = adapter_app();
            let root = fixture_tank(&mut app, Vec3::Y * 100.0); // Fixture: no spring support.
            let outline = vec![
                Vec2::new(-1.0, -1.0),
                Vec2::new(1.0, -1.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(-1.0, 1.0),
            ];
            let mut drive = TrackDrive::default();
            drive.sides.iter_mut().for_each(|side| side.speed = 2.0);
            app.world_mut().entity_mut(root).insert((
                super::super::stop_contacts::BeltFace::new(Side::Left, outline, false),
                drive,
                ParsedTankCommand::try_from(crate::command::TankCommand {
                    throttle: 1.0,
                    ..default()
                })
                .unwrap(),
                DriveSignal {
                    throttle: 1.0,
                    steer: 0.0,
                },
            ));
            app.update();
            app.update();
            let other = app.world_mut().spawn_empty().id();
            app.insert_resource(ContactTransfers::fixture(
                vec![vec![ContactTransfer {
                    reference: root,
                    reference_collider: root,
                    other,
                    other_collider: Some(other),
                    point: Vec3::Y * 99.0,
                    normal: Vec3::NEG_Y,
                    impulse: Vec3::Z * impulse,
                    effective_mass: 1.0,
                    closing_speed: 0.0,
                    friction: 0.0,
                }]],
                1.0 / 64.0,
            ));
            run_adapter(&mut app, &[(root, Vec3::ZERO)]);
            assert!(
                app.world()
                    .get::<TrackContacts>(root)
                    .unwrap()
                    .0
                    .iter()
                    .all(Vec::is_empty)
            );
            let reaction = app
                .world()
                .get::<TrackGripEffect>(root)
                .unwrap()
                .belt_reaction;
            assert_eq!(reaction, [impulse * 64.0, 0.0]);
            app.world().get::<TrackDrive>(root).unwrap().sides[0].speed
        };
        let (free, loaded) = (run(0.0), run(16.0)); // Fixture friction impulses, N s.
        assert!(
            loaded < free,
            "a delivered friction impulse must reduce belt speed: {loaded} vs {free}"
        );
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
            for v in [effect.tangential_force, effect.tangential_torque] {
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
            .map(|c| c.normal * c.load + c.tangential_force())
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
                .map(|c| (c.normal * c.load + c.tangential_force(), c.point, c.normal))
                .collect();
            let impulse = on_lower.iter().map(|c| c.0).sum::<Vec3>() * dt;
            assert!(impulse.y > 0.0, "the upper belt bears on the lower body");
            let momentum = app.world().get::<LinearVelocity>(lower).unwrap().0 * 30_000.0;
            println!("MEASURED driving {driving}: reaction {impulse} N s, momentum {momentum} N s");
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

    /// On open ground, a lone hull is never its own support. A sensor reference removes the
    /// hull from spatial admission entirely; making it solid must retain every driving bit.
    #[test]
    fn a_lone_tank_on_open_ground_retains_every_field_only_bit() {
        let (reference, a) = support_fixture(false, false, true, 0.0);
        let (solid, b) = support_fixture(false, false, false, 0.0);
        assert_eq!(support_bits(&reference, &a), support_bits(&solid, &b));
    }

    /// The game's damage model: the selector preserves authored control authority, the
    /// requirement's task rate and each source's own condition; a reduced engine supplies whole
    /// and a knocked-out one leaves no task. Fixture: ten-point pools, native and foreign
    /// operators, normalized input axes.
    #[test]
    fn drive_reads_staffing_task_rate_and_each_sources_condition() {
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
            (CrewStation::Driver, true, 1.0, 1.0, axes, 0.0),
        ] {
            let mut world = World::new();
            let tank = world
                .spawn(driver_staffed(std::collections::HashMap::from([(
                    Capability::Drive,
                    vec![
                        Group::Single(Part::Driver),
                        Group::Single(Part::Engine),
                        Group::Single(Part::Transmission),
                    ],
                )])))
                .id();
            let driver = world
                .spawn((CrewStation::Driver, Crewman { home }, VolumeOf(tank)))
                .id();
            if out {
                world.entity_mut(driver).insert(Dead);
            }
            for (role, supply, share) in [
                (
                    FunctionRole::Engine,
                    crate::damage::ModuleSupply::Whole,
                    engine,
                ),
                (
                    FunctionRole::Transmission,
                    crate::damage::ModuleSupply::Held,
                    gearbox,
                ),
            ] {
                let mut health = ComponentHealth::full(10);
                health.set_held((health.capacity() as f32 * share) as u32);
                world.spawn((role, supply, health, VolumeOf(tank)));
            }
            let engine_in_action = engine > 0.0;
            let supply = DriveSupply {
                control_authority: !out,
                task_rate: if engine_in_action {
                    competence.min(gearbox)
                } else {
                    0.0
                },
                engine_in_action,
                transmission_condition: gearbox,
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

    /// The game's damage model: an unavailable upstream station cannot restore an emptied
    /// engine or change the transmission's own held share. The idle and clutch consumers rely on it.
    #[test]
    fn unavailable_upstream_retains_each_modules_own_condition() {
        use crate::damage::{GradedGroup, Group, Part};
        // Fixture: an unavailable complete path with a reduced downstream module.
        let caps = driver_staffed(
            [(
                Capability::Drive,
                vec![Group::Graded(GradedGroup::Backup(vec![(
                    0.6,
                    vec![Part::Driver, Part::Engine, Part::Transmission],
                )]))],
            )]
            .into(),
        );
        let supply = supply_from_parts(&caps, &[(Part::Transmission, 0.25)].into(), &[].into());
        assert_eq!(
            supply,
            DriveSupply {
                control_authority: false,
                task_rate: 0.0,
                engine_in_action: false,
                transmission_condition: 0.25,
            }
        );
    }

    /// The game's damage model: the task rate used by Drive cannot be reconstructed from
    /// source conditions. Fixture: simultaneous reductions in one complete Backup path.
    #[test]
    fn drive_readout_matches_complete_path_rate() {
        use crate::damage::{CrewStation, GradedGroup, Group, Part};
        let req = vec![Group::Graded(GradedGroup::Backup(vec![(
            1.0,
            vec![Part::Driver, Part::Engine, Part::Transmission],
        )]))];
        let quality = [
            (Part::Driver, 0.6),
            (Part::Engine, 1.0),
            (Part::Transmission, 0.5),
        ]
        .into();
        let expected = crate::damage::evaluate(&req, &quality);
        let caps = driver_staffed([(Capability::Drive, req)].into());
        let supply = supply_from_parts(&caps, &quality, &[CrewStation::Driver].into());
        assert_eq!(supply.task_rate, expected);
        assert_eq!(supply.task_rate, 0.3);
        assert!(supply.control_authority);
    }

    /// The game's damage model: simultaneous reductions across required groups take one
    /// bottleneck; complete cooperative and alternative paths pay their coefficients once.
    /// Drive and the rendered HUD must read that same task rate, independently of source condition.
    #[test]
    fn required_groups_coefficients_and_readout_share_one_rate() {
        use crate::damage::{CrewStation, GradedGroup, Group, Part};
        // Fixture: a reduced operator and a reduced transmission, plus an alternative operator.
        let quality = [
            (Part::Driver, 0.6),
            (Part::Commander, 1.0),
            (Part::Engine, 1.0),
            (Part::Transmission, 0.5),
        ]
        .into();
        let members = vec![
            (0.4, vec![Part::Driver, Part::Engine]),
            (0.4, vec![Part::Commander, Part::Transmission]),
        ];
        for (req, expected) in [
            (
                vec![
                    Group::Single(Part::Driver),
                    Group::Single(Part::Engine),
                    Group::Single(Part::Transmission),
                ],
                0.5,
            ),
            (
                vec![
                    Group::Graded(GradedGroup::Pool(members.clone())),
                    Group::Single(Part::Engine),
                ],
                0.44,
            ),
            (
                vec![
                    Group::Graded(GradedGroup::Backup(members)),
                    Group::Single(Part::Engine),
                ],
                0.24,
            ),
        ] {
            let caps = driver_staffed([(Capability::Drive, req)].into());
            let supply = supply_from_parts(&caps, &quality, &[CrewStation::Driver].into());
            assert!((supply.task_rate - expected).abs() < 1e-6);
            assert!(supply.engine_in_action);
            assert_eq!(supply.transmission_condition, 0.5);
            assert!(supply.control_authority);
        }
    }

    /// The game's damage model: production alternatives cannot grant control authority.
    /// Fixture: an available alternative task path with its declared control station unavailable.
    #[test]
    fn an_available_task_path_cannot_grant_unstaffed_controls() {
        use crate::damage::{CrewStation, GradedGroup, Group, Part};
        let caps = driver_staffed(
            [(
                Capability::Drive,
                vec![Group::Graded(GradedGroup::Backup(vec![
                    (1.0, vec![Part::Driver, Part::Engine]),
                    (0.6, vec![Part::Commander, Part::Transmission]),
                ]))],
            )]
            .into(),
        );
        use crate::ballistics::ComponentHealth;
        use crate::damage::{Crewman, Dead, FunctionRole, VolumeOf};
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let tank = world.spawn(caps).id();
        world.spawn((
            CrewStation::Driver,
            Crewman {
                home: CrewStation::Driver,
            },
            Dead,
            VolumeOf(tank),
        ));
        world.spawn((
            CrewStation::Commander,
            Crewman {
                home: CrewStation::Commander,
            },
            VolumeOf(tank),
        ));
        for (function, supply, held) in [
            (FunctionRole::Engine, crate::damage::ModuleSupply::Whole, 5),
            (
                FunctionRole::Transmission,
                crate::damage::ModuleSupply::Held,
                10,
            ),
        ] {
            let mut health = ComponentHealth::full(10);
            health.set_held(held * crate::ballistics::POOL_UNITS_PER_HP);
            world.spawn((function, supply, health, VolumeOf(tank)));
        }
        let command = ParsedTankCommand::try_from(crate::command::TankCommand {
            throttle: 1.0,
            ..default()
        })
        .unwrap();
        world
            .run_system_once(
                move |tanks: Query<(&TankVolumes, &TankCapabilities)>,
                      facets: Query<VolumeFacets>| {
                    let (volumes, caps) = tanks.get(tank).unwrap();
                    let target = drive_target(&command, Some(volumes), Some(caps), &facets);
                    assert_eq!(
                        target.intent,
                        DriveIntent::Axes(DriveAxes {
                            throttle: command.throttle,
                            steer: command.steer
                        })
                    );
                    assert_eq!(target.supply.served_task_rate(), 0.0);
                    assert_eq!(target.supply.task_rate, 0.6);
                    assert!(!target.supply.control_authority);
                    assert!(target.supply.engine_in_action);
                },
            )
            .unwrap();
    }

    /// The game's damage model: a fixture hull driven through the production adapter, with
    /// its longitudinal speed integrated from the adapter's own traction at the vehicle mass the
    /// grip law carries (DERIVED: twice its side weight over g). The adapter lands each belt
    /// against its own prediction of the body's motion, so the body is that hull: it carries
    /// that mass, stands at the rig's rest height where the belts carry its weight, and feels
    /// the grade through the world's gravity. Drive requires `[Driver, Engine, Transmission]`;
    /// the 336 HP pools and the along-track `grade` force are fixture data, never an asset pass
    /// condition.
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
            use avian3d::prelude::{Gravity, Mass, Sensor};
            let (mut app, _) = adapter_app();
            let grip = app
                .world()
                .resource::<TrackGear>()
                .params
                .grip
                .expect("the shipped running gear carries a grip law");
            let mass = 2.0 * grip.side_weight / super::super::derive::G;
            let rest = app
                .world()
                .resource::<super::super::rig_geom::RigGeom>()
                .hull_rest_y;
            let root = fixture_tank(&mut app, Vec3::Y * rest);
            app.world_mut()
                .entity_mut(root)
                .insert((Sensor, Mass(mass)));
            // Let Avian publish body state and spatial bounds; the driving adapter is not scheduled.
            app.update();
            app.update();
            // Only the adapter runs from here, so the weight never moves the held body.
            app.insert_resource(Gravity(Vec3::NEG_Y * super::super::derive::G));
            run_adapter(&mut app, &[(root, Vec3::ZERO)]);
            app.world_mut().entity_mut(root).insert(driver_staffed(
                [(
                    Capability::Drive,
                    vec![
                        Group::Single(Part::Driver),
                        Group::Single(Part::Engine),
                        Group::Single(Part::Transmission),
                    ],
                )]
                .into(),
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
                    crate::damage::ModuleSupply::Whole,
                    ComponentHealth::full(Self::POOL),
                    VolumeOf(root),
                ))
                .id();
            app.world_mut().spawn((
                FunctionRole::Transmission,
                crate::damage::ModuleSupply::Held,
                ComponentHealth::full(Self::POOL),
                VolumeOf(root),
            ));
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

        /// One adapter tick under `throttle`, plus a forward along-track `grade` force (N), the
        /// weight's along-track share.
        fn tick(&mut self, throttle: f32, grade: f32) {
            use crate::command::TankCommand;
            self.app
                .world_mut()
                .resource_mut::<avian3d::prelude::Gravity>()
                .0
                .z = -grade / self.mass;
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
                .tangential_force
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

    /// The game's damage model: an engine delivers whole power while its HP pool holds anything
    /// (provisional, playtest fork F20), so a reduced engine drives bit for bit as a whole one,
    /// pulling away under full pedal and engine braking down a released descent. Scaling
    /// combustion or drag with the pool fails. Fixture: one HP held of the 336 HP pool, and a
    /// 10 degree along-track gravity share.
    #[test]
    fn a_reduced_engine_drives_exactly_as_a_whole_one() {
        let runs = [DrivenHull::POOL, 1].map(|held| {
            let mut hull = DrivenHull::new();
            hull.engine_held(held);
            let mut trace = Vec::new();
            for _ in 0..64 * 5 {
                hull.tick(1.0, 0.0);
                trace.push((hull.speed.to_bits(), hull.rpm().to_bits()));
            }
            let grade = hull.mass * super::super::derive::G * 10.0_f32.to_radians().sin();
            for _ in 0..64 * 10 {
                hull.tick(0.0, grade);
                trace.push((hull.speed.to_bits(), hull.rpm().to_bits()));
            }
            trace
        });
        assert!(runs[0] == runs[1], "a reduced engine drives differently");
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
        let engine = &hull.app.world().resource::<TrackGear>().trans.engine;
        // Fixture: reach the upper operating band on the sheet's own rpm scale.
        let target_rpm = engine.idle_rpm + 0.95 * (engine.governed_rpm - engine.idle_rpm);
        for _ in 0..64 * 60 {
            hull.tick(1.0, 0.0);
            if hull.rpm() >= target_rpm {
                break;
            }
        }
        let at = hull.rpm();
        assert!(at >= target_rpm, "the fixture reached only {at} rpm");
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
        let tp = &hull.app.world().resource::<TrackGear>().trans;
        let drag_base = transmission::motoring_drag_base_share();
        let mid_rpm = (tp.engine.idle_rpm + tp.engine.governed_rpm) * 0.5;
        let drag = tp.peak_torque_nm
            * tp.drag_fraction
            * (drag_base + (1.0 - drag_base) * run_down.abs() / mid_rpm);
        // DERIVED: gross fuel/idle torque <= curve peak + drag, loss <= drag and clutch
        // torque <= capacity. With no steering work, J*|delta omega| <= sum(|torque|)*dt.
        let max_rpm_step = (tp.peak_torque_nm + 2.0 * drag + tp.clutch_capacity)
            * hull.app.world().resource::<Time>().delta_secs()
            / tp.engine_inertia
            / transmission::RPM_TO_RAD;
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
        println!("DERIVED recovery angular-impulse bound {max_rpm_step} rpm per tick");
        assert!(
            (first - run_down).abs() <= max_rpm_step,
            "recovery exceeds available angular impulse: {run_down} -> {first}, bound {max_rpm_step} rpm"
        );
        assert!(hull.speed > slowest, "a recovered engine pulls away again");
    }

    /// The game's damage model: with no one at the Driver station, a free hull coasting up a
    /// grade turns and rolls back, and nobody can set the park lever at its turning point.
    #[test]
    fn an_unmanned_hull_rolls_back_off_a_grade_without_the_park_lever() {
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
    }

    /// The game's damage model: zero combustion and zero stored crank energy cannot power
    /// a pivot. Integrate the adapter's traction torque into a free hull's yaw velocity.
    #[test]
    fn stopped_engine_cannot_pivot_a_free_hull() {
        use avian3d::prelude::*;
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
            hull.app.world_mut().run_schedule(DriveTest);
            let effect = hull.app.world().get::<TrackGripEffect>(hull.root).unwrap();
            // Fixture yaw inertia is the 30,000 kg m² authored by fixture_tank. Translation
            // uses the same data-derived mass as the free longitudinal hull fixture.
            yaw += effect.tangential_torque.y / 30_000.0 / 64.0;
            velocity += effect.tangential_force / hull.mass / 64.0;
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
            .insert(driver_staffed(
                [(
                    Capability::Drive,
                    vec![Group::Graded(group(vec![
                        (1.0, vec![Part::Driver, Part::Engine]),
                        (1.0, vec![Part::Commander, Part::Transmission]),
                    ]))],
                )]
                .into(),
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
                    assert_eq!(supply.task_rate, 0.0, "no complete Backup path: {supply:?}");
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
        hull.app.world_mut().entity_mut(root).insert(driver_staffed(
            [(
                Capability::Drive,
                vec![Group::Graded(group(vec![(
                    1.0,
                    vec![Part::Driver, Part::Engine],
                )]))],
            )]
            .into(),
        ));
        hull.app
            .world_mut()
            .run_system_once(
                move |tanks: Query<(&TankVolumes, &TankCapabilities)>,
                      facets: Query<VolumeFacets>| {
                    let (volumes, caps) = tanks.get(root).unwrap();
                    let supply = drive_supply(Some(volumes), Some(caps), &facets);
                    assert!(supply.control_authority);
                    assert!(!supply.engine_in_action);
                    assert_eq!(supply.task_rate, 0.0);
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

    /// The game's damage model: repair brakes mask the player's notch while work is active;
    /// completing repair resumes propulsion from that same request, without another key edge.
    #[test]
    fn repair_brakes_release_onto_the_retained_notch() {
        let mut hull = DrivenHull::new();
        hull.drive_to(5.0);
        hull.engine_held(168); // Fixture: a reduced, repairable module HP pool.
        hull.app
            .world_mut()
            .entity_mut(hull.engine)
            .insert(crate::damage::repair::ModuleRepair { elapsed: Some(0.0) });
        for _ in 0..64 * 5 {
            hull.tick(0.5, 0.0);
        }
        let signal = hull.app.world().get::<DriveSignal>(hull.root).unwrap();
        assert_eq!(
            signal.throttle, 0.0,
            "repair must mask the retained request"
        );
        assert!(hull.speed.abs() < super::transmission::PARK_ENGAGE_SPEED);
        hull.app
            .world_mut()
            .get_mut::<crate::damage::repair::ModuleRepair>(hull.engine)
            .unwrap()
            .elapsed = None;
        hull.engine_held(DrivenHull::POOL);
        for _ in 0..64 * 5 {
            hull.tick(0.5, 0.0);
        }
        assert!(
            hull.app
                .world()
                .get::<DriveSignal>(hull.root)
                .unwrap()
                .throttle
                > 0.0
        );
        assert!(
            hull.speed > super::transmission::PARK_ENGAGE_SPEED,
            "ending repair must resume the player's notch"
        );
    }

    /// The game's damage model: staffing masks a retained player request; restoring the station
    /// resumes propulsion without another input edge. Clearing that request or retaining control
    /// authority while the Driver is out of action breaks this adapter contract.
    #[test]
    fn staffing_resumes_the_retained_notch() {
        let mut hull = DrivenHull::new();
        hull.app
            .world_mut()
            .entity_mut(hull.driver)
            .insert(crate::damage::Dead);
        let notch = super::super::drive::drive_feel().notch_step();
        for _ in 0..64 {
            hull.tick(notch, 0.0);
        }
        assert_eq!(
            hull.app
                .world()
                .get::<DriveSignal>(hull.root)
                .unwrap()
                .throttle,
            0.0
        );
        hull.app
            .world_mut()
            .entity_mut(hull.driver)
            .remove::<crate::damage::Dead>();
        for _ in 0..64 * 5 {
            hull.tick(notch, 0.0);
        }
        assert_eq!(
            hull.app
                .world()
                .get::<DriveSignal>(hull.root)
                .unwrap()
                .throttle,
            notch
        );
        assert!(hull.speed > super::transmission::PARK_ENGAGE_SPEED);
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
        let before = gear.braking_deceleration(0.1, -1.0, gear.construction.weight_n(), 0.0);
        let scale = 3.0; // Fixture scale, dimensionless.
        gear.construction.belt_inertia *= scale;
        gear.params.grip.as_mut().unwrap().side_weight *= scale;
        gear.construction = transmission::DriveConstruction::new(
            gear.params.grip.unwrap(),
            gear.construction.belt_inertia,
        );
        gear.trans.brake_capacity_n *= scale;
        assert!(
            (gear.braking_deceleration(0.1, -1.0, gear.construction.weight_n(), 0.0) - before)
                .abs()
                < 1e-5
        );
        assert!(
            gear.braking_deceleration(0.1, 1.0, gear.construction.weight_n(), 0.0)
                > gear.braking_deceleration(0.1, -1.0, gear.construction.weight_n(), 0.0)
        );
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
            let inertia = gear.construction.belt_inertia;
            let decel = gear.braking_deceleration(0.0, 1.0, gear.construction.weight_n(), 0.0);
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
    /// Spec construction, terrain observation and contact must carry a second drivetrain's
    /// own mass, curve and launch domain through the production belt tick. Fixture: fixed
    /// attitude/ride height on a loaded incline; only longitudinal hull motion is integrated.
    /// Replaces the match's single blueprint before constructing its running gear.
    #[test]
    fn second_spec_drives_and_settles_through_observation_and_contact() {
        use super::super::drive_fixture::{compact_spec, ground};
        use super::super::oracle::{BlockField, TerrainBlock};
        let spec = compact_spec();
        spec.validate().unwrap();
        let mass = spec.mass;
        let launch_limit = spec.track.powertrain.transmission.gearbox.launch_gears;
        let mut app = App::new();
        app.add_plugins((crate::bake::plugin, sim_plugin));
        app.world_mut().run_schedule(Startup);
        app.world_mut().resource_mut::<TankBlueprint>().spec = std::sync::Arc::new(spec);
        app.world_mut().run_schedule(PreUpdate);
        let gear = app.world().resource::<TrackGear>();
        let rest = app
            .world()
            .resource::<super::super::rig_geom::RigGeom>()
            .hull_rest_y;
        assert_eq!(gear.trans.launch_gears, launch_limit);
        assert_eq!(
            gear.params.grip.unwrap().side_weight * 2.0,
            mass * super::super::derive::G
        );
        assert!(gear.trans.engine.crank_limit_rpm > gear.trans.engine.governed_rpm);
        let angle = 10.0_f32.to_radians(); // Fixture: supported, loaded travel.
        let rotation = Quat::from_rotation_x(angle);
        let forward = rotation * Vec3::NEG_Z;
        let field = BlockField::new(vec![TerrainBlock::new(
            rotation * Vec3::NEG_Y * 0.5,
            rotation,
            Vec3::new(10_000.0, 1.0, 10_000.0),
        )])
        .with_ground(Some(crate::ground_field::GroundField::uniform(ground())));
        let mut drive = TrackDrive::default();
        let mut signal = DriveSignal::default();
        let mut grip = TrackGrip::default();
        let mut elements =
            TrackGripElements::for_links(gear.count, gear.columns[0].samples().len());
        let mut transmission = TransmissionState::from_spec(&gear.trans);
        let mut sense = super::super::sense::DriverSense::for_cut(gear.trans.shift_ticks);
        let pieces = RubbleBodies::default();
        let dt = 1.0 / 64.0; // Fixture tick.
        let weight = mass * super::super::derive::G;
        let mut speed = 0.0;
        let mut distance = 0.0;
        let mut final_lo = f32::INFINITY;
        let mut final_hi = f32::NEG_INFINITY;
        let (mut resisted, mut contact_load) = (Vec::new(), Vec::new());
        for tick in 0..64 * 60 {
            let (reports, ..) = belt_tick(
                gear,
                &field,
                &pieces,
                DriveTarget {
                    intent: DriveIntent::Axes(DriveAxes {
                        throttle: 1.0,
                        steer: 0.0,
                    }),
                    supply: DriveSupply::WHOLE,
                },
                Affine3A::from_rotation_translation(
                    rotation,
                    rotation * Vec3::Y * rest + forward * distance,
                ),
                dt,
                HullBody {
                    linear: forward * speed,
                    angular: Vec3::ZERO,
                    center_of_mass: rotation * Vec3::Y * rest + forward * distance,
                    inverse_mass: 1.0 / mass,
                    inverse_inertia: Mat3::ZERO,
                    gravity: Vec3::NEG_Y * super::super::derive::G,
                },
                BeltRig {
                    drive: &mut drive,
                    signal: &mut signal,
                    grip: &mut grip,
                    elements: &mut elements,
                    transmission: &mut transmission,
                    sense: &mut sense,
                    hard_reactions: [None; 2],
                },
            );
            assert!(reports.iter().any(|r| !r.contacts.is_empty()));
            let expected_mu = ground().grip;
            assert!(
                reports
                    .iter()
                    .flat_map(|r| &r.contacts)
                    .all(|c| c.mu == expected_mu)
            );
            if tick == 0 {
                assert!(
                    transmission.gear <= launch_limit,
                    "the spec's start domain reaches the Driver"
                );
            }
            let force: f32 = reports
                .iter()
                .flat_map(|r| &r.apps)
                .map(|a| a.force.dot(forward))
                .sum();
            speed += (force - weight * angle.sin()) / mass * dt;
            distance += speed * dt;
            if tick >= 64 * 59 {
                final_lo = final_lo.min(speed);
                final_hi = final_hi.max(speed);
                if let Some(sensed) = sense.reading {
                    resisted.push(sensed.resistance);
                }
                // What the ground resists with: the grade and the contacts' own rolling.
                let rolling: f32 = reports
                    .iter()
                    .flat_map(|r| &r.contacts)
                    .map(|c| -c.rolling_force.dot(forward))
                    .sum();
                contact_load.push(weight * angle.sin() + rolling);
            }
        }
        let mean = |values: &[f32]| values.iter().sum::<f32>() / values.len() as f32;
        let (resistance, expected) = (mean(&resisted), mean(&contact_load));
        assert!(
            expected > weight * angle.sin(),
            "the contact field must supply a real rolling load"
        );
        // Resolution: the stations' passing ripple on the belt push over one window.
        assert!(
            (resistance - expected).abs() < weight * 1e-3,
            "the ledger must feel the grade and rolling it climbs: {resistance} vs {expected}"
        );
        assert!(final_lo > super::super::transmission::PARK_ENGAGE_SPEED);
        // Resolution: one percent of construction's governed speed permits discrete contact ripple.
        assert!(
            final_hi - final_lo < 0.01 * gear.trans.governed_speed(false),
            "loaded travel must settle: [{final_lo}, {final_hi}]"
        );
        println!(
            "MEASURED second-spec resistance_n={resistance} contact_load_n={expected} speed_range=[{final_lo},{final_hi}] mass_kg={mass}"
        );
    }
    /// A strong engine cannot drive through drag above grip. Inside the numerical fade,
    /// crawl follows the smaller of engine pull and grip; an affordable launch leaves it.
    /// Fixture: compact drivetrain and fixed footprint, varied pressure and ground grip.
    #[test]
    fn a_pressure_heavy_construction_stalls_in_the_rolling_fade() {
        use super::super::forces::ROLLING_FADE_SPEED;
        use super::super::oracle::{BlockField, TerrainBlock};
        let construct = |spec: crate::spec::TankSpec| {
            spec.validate().unwrap();
            let mut app = App::new();
            app.add_plugins((crate::bake::plugin, sim_plugin));
            app.world_mut().run_schedule(Startup);
            app.world_mut().resource_mut::<TankBlueprint>().spec = std::sync::Arc::new(spec);
            app.world_mut().run_schedule(PreUpdate);
            app
        };
        let baseline = construct(super::super::drive_fixture::compact_spec());
        let baseline_pressure = baseline
            .world()
            .resource::<TrackGear>()
            .params
            .grip
            .unwrap()
            .nominal_pressure_kpa;
        drop(baseline);
        // Fixture: grip-limited, engine-limited and affordable full-drag travel, respectively.
        for (case, pressure, mu) in [
            ("grip", 40.0, 0.3),
            ("engine", 60.0, 0.9),
            ("launch", 15.0, 0.3),
        ] {
            let ground = crate::ground_field::ContactGround {
                grip: mu,
                rolling_resistance: 1.0,
                shear_length_m: 0.010,
            };
            let mut spec = super::super::drive_fixture::compact_spec();
            if case == "engine" {
                // Fixture: a flat net curve makes available pull independent of crank RPM.
                let curve = &mut spec.track.powertrain.transmission.engine.torque_curve;
                let torque = curve[0].1;
                for point in curve {
                    point.1 = torque;
                }
            }
            spec.mass *= pressure / baseline_pressure;
            let mass = spec.mass;
            let app = construct(spec);
            let gear = app.world().resource::<TrackGear>();
            let pressure = gear.params.grip.unwrap().nominal_pressure_kpa;
            let rest = app
                .world()
                .resource::<super::super::rig_geom::RigGeom>()
                .hull_rest_y;
            let field = BlockField::new(vec![TerrainBlock::new(
                Vec3::NEG_Y * 0.5,
                Quat::IDENTITY,
                Vec3::new(10_000.0, 1.0, 10_000.0),
            )])
            .with_ground(Some(crate::ground_field::GroundField::uniform(ground)));
            let mut drive = TrackDrive::default();
            let mut signal = DriveSignal::default();
            let mut grip = TrackGrip::default();
            let mut elements =
                TrackGripElements::for_links(gear.count, gear.columns[0].samples().len());
            let mut state = TransmissionState::from_spec(&gear.trans);
            let mut sense = super::super::sense::DriverSense::for_cut(gear.trans.shift_ticks);
            let pieces = RubbleBodies::default();
            let dt = 1.0 / 64.0; // Fixture fixed step.
            let (mut speed, mut distance) = (ROLLING_FADE_SPEED, 0.0_f32);
            drive.sides.iter_mut().for_each(|side| side.speed = speed);
            // The flat fixture's full potential drag and grip at its own pressure.
            let weight = mass * super::super::derive::G;
            let (rolling_n, grip_n) = (ground.rolling_at(pressure) * weight, mu * weight);
            let mut late_min = f32::INFINITY;
            let mut late_max = f32::NEG_INFINITY;
            let mut late_force = 0.0;
            let mut full_drag_force = 0.0;
            for tick in 0..64 * 60 {
                let (reports, ..) = belt_tick(
                    gear,
                    &field,
                    &pieces,
                    DriveTarget {
                        intent: DriveIntent::Axes(DriveAxes {
                            throttle: 1.0,
                            steer: 0.0,
                        }),
                        supply: DriveSupply::WHOLE,
                    },
                    Affine3A::from_translation(Vec3::Y * rest + Vec3::NEG_Z * distance),
                    dt,
                    HullBody {
                        linear: Vec3::NEG_Z * speed,
                        angular: Vec3::ZERO,
                        center_of_mass: Vec3::Y * rest + Vec3::NEG_Z * distance,
                        inverse_mass: 1.0 / mass,
                        inverse_inertia: Mat3::ZERO,
                        gravity: Vec3::NEG_Y * super::super::derive::G,
                    },
                    BeltRig {
                        drive: &mut drive,
                        signal: &mut signal,
                        grip: &mut grip,
                        elements: &mut elements,
                        transmission: &mut state,
                        sense: &mut sense,
                        hard_reactions: [None; 2],
                    },
                );
                let force: f32 = reports
                    .iter()
                    .flat_map(|r| &r.apps)
                    .map(|a| -a.force.z)
                    .sum();
                if tick >= 64 * 59 {
                    late_min = late_min.min(speed);
                    late_max = late_max.max(speed);
                    late_force += force / 64.0;
                    // DERIVED remove actual faded drag and apply the full potential at this
                    // same force state: a refused launch cannot deliver sustained surplus.
                    let rolling: f32 = reports
                        .iter()
                        .flat_map(|r| &r.contacts)
                        .map(|c| c.rolling_force.z)
                        .sum();
                    full_drag_force += (force + rolling - rolling_n) / 64.0;
                }
                speed += force / mass * dt;
                distance += speed * dt;
            }
            let engine_pull = gear.trans.peak_torque_nm.min(gear.trans.clutch_capacity)
                * gear.trans.gears_fwd.iter().copied().fold(0.0_f32, f32::max)
                / gear.trans.sprocket_radius;
            // Resolution: f32 support quadrature/force accumulation, 0.1% of the limiting force.
            let force_precision = engine_pull.min(grip_n) * 1e-3;
            match case {
                "grip" | "engine" => {
                    if case == "grip" {
                        assert!(engine_pull > rolling_n && rolling_n > grip_n);
                    } else {
                        assert!(grip_n > rolling_n && rolling_n > engine_pull);
                    }
                    assert!(
                        full_drag_force < -force_precision,
                        "a refused launch must have no full-drag surplus"
                    );
                    let expected = ROLLING_FADE_SPEED * engine_pull.min(grip_n) / rolling_n;
                    // Resolution: the stations' passing ripple on the crawl, MEASURED 0.26 % of the
                    // fade speed at 48 stations.
                    assert!(
                        (late_min - expected).abs() < ROLLING_FADE_SPEED * 5e-3
                            && (late_max - expected).abs() < ROLLING_FADE_SPEED * 5e-3,
                        "{case}: crawl [{late_min}, {late_max}] must follow limiting force, expected {expected}"
                    );
                    assert!(
                        late_force.abs() < force_precision,
                        "the crawl must stop gaining speed"
                    );
                }
                "launch" => {
                    assert!(
                        late_min > ROLLING_FADE_SPEED,
                        "an affordable launch must deliver through full drag"
                    );
                }
                _ => unreachable!("all fixture cases are declared inline"),
            }
            println!(
                "MEASURED pressure case={case} mass_kg={mass:.3} pressure_kpa={pressure:.6} drag_n={:.3} grip_n={:.3} engine_pull_n={engine_pull:.3} late_speed=[{late_min:.8},{late_max:.8}] late_force_n={late_force:.4} full_drag_force_n={full_drag_force:.4}",
                rolling_n, grip_n
            );
        }
    }

    /// Bot stopping must price the force the contact law delivers, including rolling.
    /// FIXTURE: the production adapter on flat mud, 4 m/s entry. Saturated grip makes
    /// delivered braking an independent oracle; transient mobilization is not forecast.
    #[test]
    fn service_stop_reads_contact_capacity_and_records_rollback() {
        let mut hull = DrivenHull::new();
        hull.drive_to(4.0);
        let ground = crate::ground_types::GroundTypes::load_shipped()
            .get("mud")
            .contact();
        {
            let mut field = hull.app.world_mut().resource_mut::<TrackField>();
            let oracle = field.field.take().unwrap();
            field.field =
                Some(oracle.with_ground(Some(crate::ground_field::GroundField::uniform(ground))));
        }
        hull.distance = 0.0;
        let mut peak = 0.0_f32;
        let mut reverse = 0.0_f32;
        let mut capacity = 0.0_f32;
        let mut saturated = false;
        for _ in 0..64 * 20 {
            hull.app
                .world_mut()
                .entity_mut(hull.root)
                .insert(ParsedTankCommand::hold());
            run_adapter(&mut hull.app, &[(hull.root, Vec3::NEG_Z * hull.speed)]);
            let effect = *hull.app.world().get::<TrackGripEffect>(hull.root).unwrap();
            let contacts = hull.app.world().get::<TrackContacts>(hull.root).unwrap();
            let expected: f32 = contacts
                .0
                .iter()
                .flatten()
                .map(|c| c.mu * c.load_elastic)
                .sum();
            assert!((effect.traction_capacity_n - expected).abs() < 1.0);
            capacity = expected;
            let gear = hull.app.world().resource::<TrackGear>();
            let available = 2.0 * gear.trans.brake_capacity_n;
            let decel = gear.braking_deceleration(
                0.0,
                1.0,
                effect.traction_capacity_n,
                effect.rolling_force.z,
            );
            let grip_force: f32 = contacts.0.iter().flatten().map(|c| c.traction.z).sum();
            let rolling: f32 = contacts.0.iter().flatten().map(|c| c.rolling_force.z).sum();
            assert!((effect.rolling_force.z - rolling).abs() < 1.0);
            // Resolution: saturation within 1 N; force assembly within 2 N.
            if hull.speed > super::super::forces::ROLLING_FADE_SPEED
                && (grip_force - expected).abs() < 1.0
            {
                assert!(
                    available > expected && rolling > 0.0,
                    "fixture must separate grip and drag"
                );
                assert!(
                    (decel * transmission::coupled_mass(&gear.construction)
                        - effect.tangential_force.z)
                        .abs()
                        < 2.0,
                    "forecast {} N must match delivered {} N, rolling {rolling} N",
                    decel * transmission::coupled_mass(&gear.construction),
                    effect.tangential_force.z
                );
                saturated = true;
            }
            hull.speed -= effect.tangential_force.z / hull.mass / 64.0;
            hull.distance += hull.speed / 64.0;
            peak = peak.max(hull.distance);
            reverse = reverse.min(hull.speed);
        }
        assert!(
            saturated,
            "the stop must reach saturated grip outside the rolling fade"
        );
        println!(
            "MEASURED mud service stop: capacity={capacity:.3} N peak={peak:.6} m rollback={:.6} m reverse={reverse:.6} m/s",
            peak - hull.distance
        );
    }

    /// Diagnostics distinguish recoverable shear states within one ground's shear length.
    #[test]
    fn diagnostic_digest_distinguishes_recoverable_shear() {
        let shear = super::super::drive_fixture::ground().shear_length_m;
        let mut elements = TrackGripElements::for_links(1, 3);
        elements.sides[0].strain[0] = Vec3::X * shear * 0.2; // FIXTURE capacity shares.
        let first = coarse_grip_digest(&elements);
        elements.sides[0].strain[0] = Vec3::X * shear * 0.4;
        assert_ne!(first, coarse_grip_digest(&elements));
    }

    /// A tank owns its drive result: adding unrelated tanks or moving it to another query batch
    /// cannot change its forces or carried state. The server relies on that ownership when workers
    /// receive different batches. Fixture: sensor hulls on a uniform field, varied belt/point speeds.
    #[test]
    fn one_tanks_drive_is_independent_of_population_and_query_order() {
        use avian3d::prelude::*;
        let mut reference = None;
        for (count, reverse) in [(1, false), (8, false), (8, true)] {
            let (mut app, upper_y) = adapter_app();
            let mut order: Vec<_> = (0..count).collect();
            if reverse {
                order.reverse();
            }
            let mut roots = vec![Entity::PLACEHOLDER; count];
            for index in order {
                let root = fixture_tank(
                    &mut app,
                    Vec3::new(index as f32 * 10.0, upper_y - 0.25, 0.0),
                );
                app.world_mut().entity_mut(root).insert((
                    Sensor,
                    TrackDrive {
                        sides: [
                            TrackDriveSide {
                                speed: 2.0,
                                phase: 0.01,
                            },
                            TrackDriveSide {
                                speed: -1.0,
                                phase: 0.02,
                            },
                        ],
                    },
                ));
                roots[index] = root;
            }
            app.update();
            app.update();
            let velocities: Vec<_> = roots
                .iter()
                .enumerate()
                .map(|(index, &root)| (root, Vec3::NEG_Z * (1.0 + index as f32 * 0.25)))
                .collect();
            let mut observed = Vec::new();
            for _ in 0..4 {
                run_adapter(&mut app, &velocities);
                observed.extend(support_bits(&app, &roots[..1]));
                let effect = app.world().get::<TrackGripEffect>(roots[0]).unwrap();
                observed.extend(
                    effect
                        .rolling_force
                        .to_array()
                        .map(|v| u64::from(v.to_bits())),
                );
                assert!(
                    effect.traction_capacity_n > 0.0,
                    "the comparison must exercise supported driving"
                );
                observed.push(u64::from(effect.traction_capacity_n.to_bits()));
                #[cfg(feature = "bitprobe")]
                {
                    let selected = *roots.iter().max().unwrap();
                    let capture = app.world().resource::<BitprobeCapture>();
                    assert_eq!(capture.tanks_seen as usize, count);
                    assert_eq!(
                        capture.transmission,
                        app.world()
                            .get::<TrackGripEffect>(selected)
                            .unwrap()
                            .transmission_probe,
                        "capture commits the selected source, regardless of its worker's completion order"
                    );
                }
            }
            if let Some(reference) = &reference {
                assert_eq!(
                    &observed, reference,
                    "population {count}, reversed {reverse}"
                );
            } else {
                reference = Some(observed);
            }
        }
    }

    /// Structural readers receive source-keyed contact staging after every tank has evaluated.
    /// A worker-completion-order append breaks this contract. Fixture: sensor hulls bearing on
    /// one fixed structural slab; entity order differs from their spatial order.
    #[test]
    fn shared_static_contacts_commit_by_source_then_side_and_contact() {
        use avian3d::prelude::*;
        let (mut app, upper_y) = adapter_app();
        let slab = app
            .world_mut()
            .spawn((
                Position(Vec3::ZERO),
                Rotation::default(),
                RigidBody::Static,
                Collider::cuboid(200.0, 0.5, 200.0),
                crate::contact_transfer::StructureBody,
            ))
            .id();
        let mut roots = Vec::new();
        for x in [20.0, 0.0, -20.0] {
            let root = fixture_tank(&mut app, Vec3::new(x, upper_y, 0.0));
            app.world_mut().entity_mut(root).insert(Sensor);
            roots.push(root);
        }
        app.update();
        app.update();
        let velocities: Vec<_> = roots
            .iter()
            .map(|&root| (root, Vec3::NEG_Y * 0.1))
            .collect();
        run_adapter(&mut app, &velocities);
        roots.sort_unstable();
        let mut expected = Vec::new();
        for root in roots {
            let contacts = app.world().get::<TrackContacts>(root).unwrap();
            let before = expected.len();
            expected.extend(
                contacts
                    .0
                    .iter()
                    .flatten()
                    .filter(|contact| contact.collider == Some(slab))
                    .map(|contact| (root, contact.point)),
            );
            assert!(
                expected.len() > before,
                "every source must bear on the shared slab"
            );
        }
        let transfers = app
            .world()
            .resource::<crate::contact_transfer::ContactTransfers>();
        let staged: Vec<_> = transfers
            .staged()
            .iter()
            .map(|contact| (contact.body, contact.point))
            .collect();
        assert_eq!(staged, expected);
    }

    /// A dynamic hull must never vanish from the worker query while its old contact report survives.
    /// Structural admission relies on a missing driving component failing before any force commit.
    #[test]
    #[should_panic(expected = "a dynamic tank carries its complete spawn-built driving state")]
    fn an_incomplete_dynamic_drive_bundle_is_a_bug_instead_of_a_skipped_tank() {
        let (mut app, roots) = support_fixture(false, false, true, 0.0);
        app.world_mut().entity_mut(roots[0]).remove::<Position>();
        run_adapter(&mut app, &[(roots[0], Vec3::ZERO)]);
    }
}
