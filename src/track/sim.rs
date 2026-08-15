//! The locomotion sim (phase B): the track model's belt forces ARE how tanks drive. The ECS
//! adapter over [`super::forces`] — one deep boundary; support, traction, and belt dynamics
//! live behind `contact_side` + `transmission::step` (one joint form for every architecture),
//! this module owns queries, scheduling, capability gating, and the netcode-visible
//! [`TrackDrive`] state.
//!
//! Sim discipline (hard rules, each bought with a measured MP failure in the raycast sim this
//! replaces):
//! - Pose from tick-truth `Position`/`Rotation`, never `GlobalTransform` (render lag differs
//!   per machine).
//! - Terrain from the analytic [`TrackField`] — pure closed-form arithmetic, no spatial
//!   queries.
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
    BeltContact, ForceParams, GripElements, SideInput, SideReport, SideState, contact_side,
    grip_stiffness,
};
use super::oracle::TerrainOracle;
use super::route::build_route;
use super::side::Side;
use super::terrain::TrackField;
use super::transmission::{
    self, TransmissionInput, TransmissionMode, TransmissionParams, TransmissionReport,
    TransmissionState,
};

// Surface friction policy (ADR-0007 bucket 3: a property of the track–ground PAIR, destined
// for the terrain/ground-type mechanic — deliberately not vehicle spec). `pub(crate)`: the
// traction ceiling `μg` bounds hull acceleration, so `net::extrapolate` derives its horizon
// from this same constant.
pub(crate) const MU: f32 = 0.9;
const SLIP_SATURATION: f32 = 0.4;

/// Per-tank tracked-drivetrain sim state: server-authoritative, replicated to every client for
/// the track view. Hashed into the determinism trace (`hblt`).
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

/// The OFFLINE transmission feel-test override: which drivetrain adapter [`apply_track_forces`]
/// runs instead of the vehicle's declared architecture. Inserted ONLY by the `--offline`
/// composition; MP has no dial and follows the spec. Unlike the element gate this one is live (the
/// offline `T` key cycles governor → hybrid → L600): [`TankTransmission`] resets on every flip, so
/// a mid-session mode change cannot poison hidden state the way an element-regime flip would.
#[derive(Resource)]
pub struct TransmissionFeelTest(pub TransmissionMode);

/// The joint transmission's path-dependent state (gear/window/detent/direction/crank plus the
/// stage-C demand/filter/target/hill-hold scheduler state). REV 14 replicates this one atomic root
/// component: server-authoritative, advanced only where the body is dynamic through the
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
}

/// Apply an explicit hull impulse at a world point (retaining its torque arm) and notify the grip
/// rest detector as one operation.
pub(crate) fn apply_explicit_impulse(
    mut forces: ForcesItem<'_, '_>,
    wake: Option<Mut<'_, TrackGripWake>>,
    impulse: Vec3,
    point: Vec3,
) {
    forces.apply_linear_impulse_at_point(impulse, point);
    if let Some(mut wake) = wake {
        wake.record_impulse(impulse);
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
    /// The contact envelope's per-position reach profile ((z, travel) knots — 0 at the
    /// unsprung sprocket/idler, full droop at the road wheels), ridden via
    /// [`super::forces::TravelField`] every tick.
    travel_knots: Vec<(f32, f32)>,
    /// Per-side collocation columns ([`SideInput::columns`], indexed by `Side::index()`):
    /// the MEASURED shoe faces + centre from
    /// [`RigGeom::grip_column_offsets`](super::rig_geom::RigGeom::grip_column_offsets) — signed
    /// per-side because the shoe is not centred on its pins (~17 mm outboard on the Tiger).
    columns: [[(f32, f32); 3]; 2],
    params: ForceParams,
    /// The declared joint transmission tables. `None` ⇔ the spec explicitly selects
    /// `architecture: Governor` (the tableless adapter) — never "the block was absent"
    /// (validation rejects that at load).
    trans: Option<TransmissionParams>,
    /// Spec-selected adapter — the RON's explicit `transmission.architecture`, mapped through
    /// [`crate::spec::TransmissionArchitecture::mode`]. Always an explicit authored choice.
    mode: TransmissionMode,
}

impl TrackGear {
    /// The declared joint transmission params, if the spec authored one. Read-only accessor for
    /// the shared drive HUD; the field stays private so only the drive step and HUD consume it.
    pub fn trans(&self) -> Option<&TransmissionParams> {
        self.trans.as_ref()
    }

    /// Spec-selected adapter for the shared client HUD. Read-only view access; the drive step remains
    /// the sole owner of how this choice affects simulation.
    pub(crate) fn mode(&self) -> TransmissionMode {
        self.mode
    }

    /// The chain-clamped droop (m): the maximum wheel-travel knot — how far below its rest line a
    /// road wheel may drop. `0.0` when the profile is empty. The track view spans its cosmetic
    /// wheels this far below rest so a fully-drooped wheel's drawn belt wrap stays feasible at the
    /// chain-limited link count (the knots' peak IS `RigGeom::droop_travel(..).effective`, floored).
    pub(crate) fn max_droop(&self) -> f32 {
        self.travel_knots
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
        out.f32("track_gear.plane_x", self.plane_x);
        out.u32(
            "track_gear.mode",
            match self.mode {
                TransmissionMode::Governor => 0,
                TransmissionMode::Hybrid => 1,
                TransmissionMode::FixedRadii => 2,
            },
        );
        for (index, point) in self.loop_pts.iter().enumerate() {
            out.vec2(&format!("track_gear.route[{index}]"), *point);
        }
        let p = &self.params;
        out.f32("force.face_offset", p.face_offset);
        out.f32("force.free_travel", p.free_travel);
        for (index, (z, travel)) in self.travel_knots.iter().copied().enumerate() {
            out.f32(&format!("force.travel_knots[{index}].z"), z);
            out.f32(&format!("force.travel_knots[{index}].travel"), travel);
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
        out.f32("force.mu", p.mu);
        out.f32("force.slip_saturation", p.slip_saturation);
        out.f32("force.max_speed", p.max_speed);
        out.f32("force.engine_power", p.engine_power);
        out.f32("force.engine_force", p.engine_force);
        out.f32("force.governor_gain", p.governor_gain);
        out.f32("force.inertia", p.inertia);
        out.f32("force.grip_stiffness", p.grip_stiffness);

        let Some(tp) = self.trans.as_ref() else {
            out.bool("transmission.present", false);
            return;
        };
        out.bool("transmission.present", true);
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
        &crate::assets::asset_root().join(crate::tank::TIGER_SIM_GLB_PATH),
        spec.sprocket.teeth,
        spec.link_count,
        &sus,
    );
    // ONE shared loop for both sides (the shipped asset's sides measure within ~60 µm; the
    // Right side is the `hull_rest_y` datum side). Per-side loops land with per-side gear.
    let circles = geom.rest.get(Side::Right).clone();
    let belt_len = geom.belt_len();
    let route = build_route(&circles, belt_len);
    let mut loop_pts = route.pts.clone();
    if loop_pts.first() != loop_pts.last()
        && let Some(&first) = loop_pts.first()
    {
        loop_pts.push(first);
    }

    // The envelope law, calibrated against the exact loop/columns/engage the live law rides.
    // Columns are the MEASURED shoe faces + centre per side (`grip_column_offsets`), not a
    // symmetric ±width/2 about the pin plane — the shoe rides ~17 mm outboard of its pins.
    let droop = geom.droop_travel(&sus);
    let columns = [Side::Left, Side::Right].map(|s| geom.grip_column_offsets(s));
    let travel_knots = super::envelope::wheel_travel_knots(
        &circles,
        droop.effective.max(super::envelope::MIN_ENVELOPE_TRAVEL),
    );
    let cal = super::envelope::calibrate(
        &[Side::Left, Side::Right].map(|s| super::envelope::EnvelopeSide {
            loop_pts: &loop_pts,
            plane_x: s.plane_x(geom.plane_x),
            knots: &travel_knots,
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

    // The spec's EXPLICIT architecture selection. There is no fallback here any more: a spec
    // without a transmission block fails validation at load (spec.rs), so Governor runs only
    // when a sheet NAMES it (`architecture: Governor`) — never because a block was forgotten.
    let declared = spec
        .powertrain
        .transmission
        .as_ref()
        .expect("TankSpec transmission was validated before TrackGear construction");
    // The declared transmission, derived from the authored tables against the MEASURED sprocket
    // radius and half-tread (tiger-transmission-data.md rule: speeds are the anchors, reductions
    // derive, so the ladder survives the 19-vs-20-tooth discrepancy). Governor is tableless
    // by contract (params() rejects a Governor block carrying tables).
    match declared.gearbox.as_ref() {
        Some(gearbox) => info!(
            "declared transmission: {:?}, {}F/{}R",
            declared.architecture,
            gearbox.forward_speeds_kmh.len(),
            gearbox.reverse_speeds_kmh.len()
        ),
        None => info!(
            "declared transmission: {:?} (tableless)",
            declared.architecture
        ),
    }
    // Measured geometry: the sprocket pin-line pitch radius is the rest running gear's first
    // circle (chord-exact `derive::sprocket_pitch_radius(geom.pitch, teeth)` by construction), and
    // the half-tread is `geom.plane_x` — the same measured values the belt/envelope ride.
    let trans = spec
        .transmission_params(circles[0].1, geom.plane_x)
        .expect("TankSpec transmission was validated before TrackGear construction");
    let mode = declared.architecture.mode();

    commands.insert_resource(TrackGear {
        loop_pts,
        count: spec.link_count,
        plane_x: geom.plane_x,
        travel_knots,
        columns,
        trans,
        mode,
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
            mu: MU,
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

    // Keep the marker-derived running gear alive as the game's single geometry source: the track
    // VIEW (`track::view`) reads its rest circles / pitch / plate / plane directly, and the bit
    // probe echoes the measured scalars. Built once here alongside `TrackGear`, never re-derived.
    commands.insert_resource(geom);
}

/// One tank's mutable belt/drivetrain state for [`belt_tick`] — the caller owns the storage
/// (ECS components for the live sim, plain fields for the recoil microsim).
pub(crate) struct BeltRig<'a> {
    pub drive: &'a mut TrackDrive,
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
pub(crate) fn belt_tick<O: TerrainOracle>(
    gear: &TrackGear,
    oracle: &O,
    mode: TransmissionMode,
    target: DriveAxes,
    affine: Affine3A,
    dt: f32,
    vel_at: impl Fn(Vec3) -> Vec3,
    rig: BeltRig<'_>,
) -> ([SideReport; 2], TransmissionReport) {
    let shaped = shape_drive(
        DriveAxes {
            throttle: rig.drive.throttle,
            steer: rig.drive.steer,
        },
        target,
        dt,
    );
    rig.drive.throttle = shaped.throttle;
    rig.drive.steer = shaped.steer;
    let side_commands = shaped.side_commands();

    // THE joint drivetrain form — every architecture, one path. An explicit Governor selection
    // runs through [`transmission::step`] like the others: its Governor arm is the exact
    // per-side [`super::forces::governor_belt`] law, bit-identical to the `step_side` tail this
    // module no longer calls (pinned by `transmission::tests::governor_adapter_matches_legacy_belt`),
    // and it never touches the transmission state. A regenerative override on a vehicle whose
    // spec declares `architecture: Governor` has no tables to run — say so instead of silently
    // pretending (dev-time loudness; the spec path can never hit this arm because `mode` and
    // `trans` come from the same validated block).
    let (mode, tp) = match (mode, gear.trans.as_ref()) {
        (TransmissionMode::Governor, _) => (TransmissionMode::Governor, None),
        (m, Some(tp)) => (m, Some(tp)),
        (m, None) => {
            bevy::log::warn_once!(
                "transmission override {m:?} needs declared tables, but this vehicle's \
                 spec selects architecture: Governor (tableless) — running the governor"
            );
            (TransmissionMode::Governor, None)
        }
    };
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
            loop_pts: &gear.loop_pts,
            count: gear.count,
            plane_x: side.plane_x(gear.plane_x),
            columns: gear.columns[si],
            command: side_commands[si],
            travel: super::forces::TravelField {
                knots: &gear.travel_knots,
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
            &vel_at,
            &mut rig.elements.sides[si],
        );
        reports[si] = report;
        live[si] = ok;
    }
    let tr = transmission::step(
        mode,
        &gear.params,
        tp,
        rig.transmission,
        &TransmissionInput {
            throttle: rig.drive.throttle,
            steer: rig.drive.steer,
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
                // stations there, and the retired governor tail advected the same way.
                phase: rig.drive.sides[si].phase + f64::from(pre_speed * dt),
            };
        }
        rig.grip.sides[si] = [report.state.grip.x, report.state.grip.y];
    }
    (reports, tr)
}

/// The drive step: shape the command, run each side's belt force model at the tick-truth
/// pose, apply the returned forces in report order, commit the new belt state.
fn apply_track_forces(
    time: Res<Time>,
    field: Res<TrackField>,
    gear: Option<Res<TrackGear>>,
    // Offline-only adapter override. MP leaves it absent and follows `TrackGear::mode` — the
    // spec's EXPLICIT `transmission.architecture` (a missing block is a load-time validation
    // error now, not a silent Governor).
    trans_feel: Option<Res<TransmissionFeelTest>>,
    #[cfg(feature = "bitprobe")] mut bitprobe: Option<ResMut<BitprobeCapture>>,
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
    #[cfg(feature = "bitprobe")]
    if let Some(capture) = bitprobe.as_deref_mut() {
        capture.clear_tick();
    }
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
        #[cfg(feature = "bitprobe")]
        if let Some(capture) = bitprobe.as_deref_mut() {
            capture.tanks_seen += 1;
            capture.command = [command.throttle, command.steer];
        }
        // Only dynamic bodies simulate the belt — and the guard sits BEFORE command shaping:
        // `TrackDrive` is replicated state, and a client tick must not locally slew the
        // replicated throttle/steer. Forces are no-ops on kinematic/static bodies, replicated
        // tanks' `TrackDrive` arrives for their track view, and clients neither receive nor
        // simulate the private element field (ADR-0027 disclosure). Skipping them entirely
        // also keeps their replicated belt state from being fought by a locally-integrated
        // one.
        if !matches!(*body, RigidBody::Dynamic) {
            continue;
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
        // avian3d 0.7 `ForcesItem` keeps this helper private; this is its version-pinned source
        // expression (`query_data.rs`): position + rotation * local computed COM.
        let center_of_mass = pos.0 + rot.0 * center_of_mass.0;
        let mut effect = TrackGripEffect::default();

        // The shared per-tank drivetrain tick — contact evaluation, joint transmission, belt
        // commit — with this adapter supplying Avian's live velocity field. MP runs the declared
        // architecture; the offline-only [`TransmissionFeelTest`] can override it.
        #[cfg_attr(
            not(feature = "bitprobe"),
            expect(
                unused_mut,
                unused_variables,
                reason = "only the bitprobe capture consumes"
            )
        )]
        let (mut reports, tr) = belt_tick(
            &gear,
            oracle,
            mode,
            target,
            affine,
            dt,
            |p| forces.velocity_at_point(p),
            BeltRig {
                drive: &mut drive,
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
