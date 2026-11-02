//! The joint drivetrain integrates both belts and the crank from construction data.
//! `step` owns Driver gear/clutch/brake decisions, then solves clutch and steering torque,
//! work, belt acceleration and crank settlement. `TransmissionState` is its complete memory.
//! Gear decisions read support geometry, requested pedal and crank rpm; contact reactions
//! belong only to the mechanical solve. Forces are N, torques N m, belt speeds m/s and
//! crank speed rad/s. The design narrative lives in track-model architecture section 5a
//! and `drive-control-design-2026-11-02.md`.
//!
//! The rejected `STEER_SERVO_BAND` proportional servo loses authority with vehicle scale;
//! the steering law instead solves its capacity-limited constraint semi-implicitly.

use bevy::ecs::error::BevyError;

use super::forces::ForceParams;

#[cfg(feature = "bitprobe")]
use crate::bitprobe::TransmissionProbe;

/// The fixed simulation tick rate the shift countdown quantizes against (the module operates
/// on fixed 64 Hz ticks). Resolution: shift times quantize to this solver clock.
const TICK_HZ: f32 = 64.0;

/// Resolution: the clutch's over-rev slip band avoids a hard speed discontinuity at the
/// curve endpoint. Gear choices themselves must land at or below that endpoint.
pub(crate) const OVERREV_MARGIN_RPM: f32 = 100.0;

/// Fuel-governor cut width (rpm): torque ramps linearly to zero over this band past the
/// governed rpm, so the top-speed equilibrium is a smooth root instead of a hard clip.
/// Resolution: a finite ramp avoids discontinuous torque forcing at the fixed tick.
const GOVERNOR_CUT_RPM: f32 = 100.0;

/// Driver actuation stand-ins, admitted before the drivetrain can be constructed.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EngineControl {
    idle_catch_droop_rpm: crate::provenance::Labelled<f32>,
    idle_guard_band_rpm: crate::provenance::Labelled<f32>,
    rev_match_band_rpm: crate::provenance::Labelled<f32>,
    drag_throttle_release: crate::provenance::Labelled<f32>,
    motoring_drag_base_share: crate::provenance::Labelled<f32>,
}

const ENGINE_CONTROL_PATH: &str = "assets/driving/engine.ron";

impl EngineControl {
    fn validate(&self) {
        for (field, number) in [
            ("idle_catch_droop_rpm", &self.idle_catch_droop_rpm),
            ("idle_guard_band_rpm", &self.idle_guard_band_rpm),
            ("rev_match_band_rpm", &self.rev_match_band_rpm),
            ("drag_throttle_release", &self.drag_throttle_release),
            ("motoring_drag_base_share", &self.motoring_drag_base_share),
        ] {
            assert!(
                number.value.is_finite()
                    && number.value > 0.0
                    && matches!(number.label, crate::provenance::Provenance::StandIn(_)),
                "{ENGINE_CONTROL_PATH}: {field} must be finite, positive and labelled StandIn; author the missing Driver actuation law's width"
            );
        }
        assert!(
            self.drag_throttle_release.value <= 1.0 && self.motoring_drag_base_share.value < 1.0,
            "{ENGINE_CONTROL_PATH}: drag_throttle_release and motoring_drag_base_share must be shares below one; fix their authored domains"
        );
        assert!(
            self.idle_guard_band_rpm.value >= self.idle_catch_droop_rpm.value,
            "{ENGINE_CONTROL_PATH}: idle_guard_band_rpm must be at least idle_catch_droop_rpm; widen the guard or narrow the catch so recovery saturates by the guard point"
        );
    }
}

fn engine_control() -> &'static EngineControl {
    static DATA: std::sync::LazyLock<EngineControl> = std::sync::LazyLock::new(|| {
        let data: EngineControl = crate::provenance::from_shipped_str(
            ENGINE_CONTROL_PATH,
            include_str!("../../assets/driving/engine.ron"),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        data.validate();
        data
    });
    &DATA
}

/// Resolution: near-zero axis tolerance prevents input quantization from moving direction
/// and parking controls at their zero crossing.
const DEAD: f32 = 0.05;

/// Resolution: at-rest belt-speed tolerance (m/s), below perceptible motion. The park latch
/// retains full brake capacity after a breach. Direction, launch and static capacity share it.
pub(crate) const PARK_ENGAGE_SPEED: f32 = 0.05;

/// Shared Driver actuation parameters, admitted before play. Labels live with their data.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DriverActuation {
    reversal_hold_s: crate::provenance::Labelled<f32>,
    neutral_throttle: crate::provenance::Labelled<f32>,
    neutral_speed_m_s: crate::provenance::Labelled<f32>,
}

fn driver_actuation() -> &'static DriverActuation {
    static DATA: std::sync::LazyLock<DriverActuation> = std::sync::LazyLock::new(|| {
        const PATH: &str = "assets/driving/actuation.ron";
        let data: DriverActuation = crate::provenance::from_shipped_str(
            PATH,
            include_str!("../../assets/driving/actuation.ron"),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        for (field, number) in [
            ("neutral_throttle", &data.neutral_throttle),
            ("neutral_speed_m_s", &data.neutral_speed_m_s),
        ] {
            assert!(
                number.value.is_finite()
                    && number.value > 0.0
                    && matches!(number.label, crate::provenance::Provenance::StandIn(_)),
                "{PATH}: {field} must be finite, positive and labelled StandIn; supply the missing actuator regime"
            );
        }
        assert!(
            data.neutral_throttle.value <= 1.0,
            "{PATH}: neutral_throttle must be a share; repair the actuator data"
        );
        assert!(
            data.reversal_hold_s.value.is_finite()
                && data.reversal_hold_s.value > 0.0
                && data.reversal_hold_s.value * TICK_HZ <= f32::from(u8::MAX)
                && matches!(
                    data.reversal_hold_s.label,
                    crate::provenance::Provenance::Taste(_)
                ),
            "{PATH}: reversal_hold_s must be positive, fit the fixed-tick countdown and be labelled Taste; author the stop-to-reverse pause"
        );
        data
    });
    &DATA
}

/// The engine's declared operating envelope: a piecewise-linear torque curve (N·m over rpm,
/// ascending, clamped at the ends) under a fuel governor at `governed_rpm`.
#[derive(Clone, Debug)]
pub struct EngineParams {
    pub idle_rpm: f32,
    pub governed_rpm: f32,
    /// `(rpm, N·m)` authoring points, ascending rpm.
    pub torque_nm: Vec<(f32, f32)>,
}

/// The joint transmission's declared data — everything vehicle spec, nothing tuned-to-feel.
/// Built from authored tables via [`TransmissionParams::from_authoring`] (the spec block /
/// the sandbox's T-34 lab values).
#[derive(Clone, Debug)]
pub struct TransmissionParams {
    pub engine: EngineParams,
    /// Total reduction (engine rev per sprocket rev) per forward gear, 1-based order. DERIVED
    /// from authored per-gear speeds (the anchors) against the spec's own sprocket radius —
    /// speed ratios are r_s-independent, so the ladder survives the open 19-vs-20-tooth
    /// sprocket discrepancy (tiger-transmission-data.md implementation rule).
    pub gears_fwd: Vec<f32>,
    pub gears_rev: Vec<f32>,
    /// Sprocket pitch radius (m): belt speed = engine speed / G × r_s.
    pub sprocket_radius: f32,
    /// Declared highest start gear and largest one-window gear step.
    pub launch_gears: u8,
    pub max_step: u8,
    /// Per FORWARD gear `(κ_tight, κ_wide)` where `κ = half_tread / R` — the L600 detents.
    /// Reverse gears index the same table (R1–R4 mirror F1–F4).
    pub steer_kappa: Vec<(f32, f32)>,
    /// The authored per-forward-gear `(R_tight, R_wide)` table (m), retained verbatim for
    /// presentation. The sim law consumes [`Self::steer_kappa`]; retaining the source table keeps
    /// the HUD from re-deriving and rounding away the authored radii.
    pub steer_radii_m: Vec<(f32, f32)>,
    /// Steering-member force capacity PER OUTPUT (N). The steering member drives the two
    /// outputs DIFFERENTIALLY — each output's steering share is bounded by its own
    /// gearing/grip-scale cap (this datum), so the belt-difference axis `F_s` carries up to
    /// 2× it (each side sees `F_s/2`), and the L600 constraint force λ is bounded by
    /// `capacity / max|jᵢ|`. Reading this datum as an `F_s` bound was the pivot-dead bug:
    /// it halves the yaw ceiling (Tiger: 373 kN·m < its ~478 kN·m footprint scrub — could
    /// not break away; the T-34 lab's 300 vs 224 kN·m masked it).
    pub steer_capacity_n: f32,
    /// Full neutral-turn belt-speed half-difference (m/s): `κ_tight(F1) × v(F1 @ governed)` —
    /// the L600's brake-gated neutral-turn target. DERIVED, and the correct emergent pivot
    /// scale for a fixed-radius box: the radii table's own invariant makes `κ_tight(g) ×
    /// v(g)` gear-independent (Tiger: ≈ 0.337 m/s @ 3000 rpm in every gear). The authored
    /// `neutral_fraction` feel scalar that used to shrink it was DELETED as unprovenanced.
    pub neutral_d_full: f32,
    /// Inner→outer recirculation efficiency η (mechanical ~0.9, INFERRED tag at the authoring
    /// site).
    pub recirculation: f32,
    /// Per-side service/parking brake capacity at the sprocket (N).
    pub brake_capacity_n: f32,
    /// Static breakaway capacity multiplier for a latched, at-rest belt. Dynamic dissipation,
    /// service braking, and every moving slide use [`Self::brake_capacity_n`] unchanged.
    pub brake_static_factor: f32,
    /// Zero-throttle engine drag (compression braking): the MID-BAND anchor of the rising
    /// motoring-torque curve, as a fraction of peak torque. [`engine_drag`] equals
    /// `drag_fraction × peak` at `ω_mid = (idle + governed)/2` and grows linearly with
    /// crank speed from the authored actuation threshold base. A downhill equilibrium exists
    /// only where this drag plus rolling resistance balances grade force before clutch guards
    /// cap transmitted drag.
    /// A drag TORQUE at the crank (design §3), never the negative half of rated power.
    /// Diesel motoring/compression braking runs ~20–30% of rated torque mid-band
    /// (INFERRED band, tagged at the authoring site).
    pub drag_fraction: f32,
    /// Crank + flywheel + clutch rotational inertia J (kg·m²) — the engine-side inertia the
    /// stage-B crank state integrates against. Vehicle data (INFERRED at the authoring
    /// sites: class scaling, flywheel-dominant).
    pub engine_inertia: f32,
    /// Main clutch torque capacity (N·m) — the coupling's clamp: the largest torque the
    /// engaged clutch transmits before slipping (≈ 1.3 × peak engine torque by the usual
    /// sizing rule; INFERRED at the authoring sites). THE COUPLING-LAW SLOT's one datum —
    /// a torque-converter characteristic replaces the clamp for modern automatics later.
    pub clutch_capacity: f32,
    /// Gear-shift torque-interruption window in fixed ticks — DERIVED from the authored
    /// `shift_secs` (a Tiger preselector and a crash box shift very differently: vehicle
    /// data, not module policy).
    pub shift_ticks: u8,
    /// Derived at construction: the torque curve's peak (the low-speed rev target).
    pub peak_torque_rpm: f32,
    pub peak_torque_nm: f32,
}

/// The authored tables the params derive from — the shape the spec block and the sandbox lab
/// both author. Speeds, radii, and anchor rpm are the source data; reductions and curvatures
/// are derived here so two numbers that must agree stay one number.
pub struct TransmissionAuthoring<'a> {
    pub idle_rpm: f32,
    pub governed_rpm: f32,
    /// The rpm the per-gear speeds are anchored at (the Tiger's are quoted @ 3000).
    pub rated_rpm: f32,
    pub torque_nm: &'a [(f32, f32)],
    /// Per-gear top BELT speeds (km/h) at `rated_rpm`, 1st..top.
    pub forward_speeds_kmh: &'a [f32],
    pub reverse_speeds_kmh: &'a [f32],
    /// Declared highest start gear and largest one-window gear step.
    pub launch_gears: u8,
    pub max_step: u8,
    /// Per forward gear `(R_tight, R_wide)` turn radii (m).
    pub steer_radii_m: &'a [(f32, f32)],
    /// Steering-member force capacity PER OUTPUT (N) — see
    /// [`TransmissionParams::steer_capacity_n`] for the convention (the difference axis
    /// carries 2× this).
    pub steer_capacity_n: f32,
    pub recirculation: f32,
    pub brake_capacity_n: f32,
    pub brake_static_factor: f32,
    /// See [`TransmissionParams::drag_fraction`].
    pub drag_fraction: f32,
    /// See [`TransmissionParams::engine_inertia`] (kg·m²).
    pub engine_inertia_kgm2: f32,
    /// See [`TransmissionParams::clutch_capacity`] (N·m).
    pub clutch_capacity_nm: f32,
    /// DERIVED per-side belt translation + reflected wheel inertia (kg); the coupling uses twice this value.
    pub belt_inertia: f32,
    /// Gear-shift torque-interruption time (s) — see [`TransmissionParams::shift_ticks`].
    pub shift_secs: f32,
    pub sprocket_radius_m: f32,
    /// Track half-tread `b` (m) — the spec's `plane_x`.
    pub half_tread_m: f32,
}

pub(crate) const RPM_TO_RAD: f32 = std::f32::consts::TAU / 60.0;

impl TransmissionParams {
    /// Validate and derive runtime parameters from the one authored transmission shape shared by
    /// tank specs, the sandbox lab vehicle, and arithmetic tests.
    pub fn from_authoring(a: &TransmissionAuthoring) -> Result<Self, BevyError> {
        let _ = driver_actuation();
        if a.forward_speeds_kmh.is_empty() || a.reverse_speeds_kmh.is_empty() {
            return Err("transmission.gearbox ladders must be non-empty".into());
        }
        if a.reverse_speeds_kmh.len() > a.forward_speeds_kmh.len() {
            return Err("track.powertrain.transmission.gearbox.reverse_speeds_kmh (ladder shape): every reverse gear needs a forward steering-radii row; shorten the reverse ladder or supply the forward rows".into());
        }
        if a.steer_radii_m.len() != a.forward_speeds_kmh.len() {
            return Err(format!(
                "transmission.steering.radii must have one (tight, wide) pair per forward \
                 gear ({} pairs for {} gears)",
                a.steer_radii_m.len(),
                a.forward_speeds_kmh.len()
            )
            .into());
        }
        for (field, ok) in [
            (
                "sprocket radius and track half-tread",
                a.sprocket_radius_m.is_finite()
                    && a.sprocket_radius_m > 0.0
                    && a.half_tread_m.is_finite()
                    && a.half_tread_m > 0.0,
            ),
            (
                "gearbox speeds",
                a.forward_speeds_kmh
                    .iter()
                    .chain(a.reverse_speeds_kmh)
                    .all(|v| v.is_finite() && *v > 0.0),
            ),
            (
                "steering.radii",
                a.steer_radii_m.iter().all(|(tight, wide)| {
                    tight.is_finite() && wide.is_finite() && *tight > 0.0 && *wide > 0.0
                }),
            ),
            (
                // Sanity bounds beyond finiteness: an absurd finite torque can overflow
                // reflected torque and crank work. Refuse it before the physical solve.
                "engine.torque_curve",
                a.torque_nm.len() >= 2
                    && a.torque_nm.windows(2).all(|w| w[0].0 < w[1].0)
                    && a.torque_nm.iter().all(|(rpm, torque)| {
                        rpm.is_finite()
                            && torque.is_finite()
                            && *rpm > 0.0
                            && *rpm <= 20_000.0
                            && *torque >= 0.0
                            && *torque <= 100_000.0
                    }),
            ),
            (
                "engine rpms",
                [a.idle_rpm, a.governed_rpm, a.rated_rpm]
                    .iter()
                    .all(|v| v.is_finite() && *v > 0.0),
            ),
            (
                // The authored idle guard needs positive crank-speed headroom.
                // The game's damage model can still reach rest.
                "engine.idle_rpm floor",
                a.idle_rpm > engine_control().idle_guard_band_rpm.value,
            ),
            (
                "gearbox launch_gears/max_step (nonzero construction limits)",
                a.launch_gears > 0 && a.max_step > 0,
            ),
            (
                // Ladders ascend and fit the runtime's u8 gear index.
                "gearbox ladder shape (ascending, u8-indexable)",
                a.forward_speeds_kmh.len() <= u8::MAX as usize
                    && a.reverse_speeds_kmh.len() <= u8::MAX as usize
                    && a.forward_speeds_kmh.windows(2).all(|w| w[0] < w[1])
                    && a.reverse_speeds_kmh.windows(2).all(|w| w[0] < w[1]),
            ),
            (
                "steering capacity/efficiency + brake_force",
                a.steer_capacity_n.is_finite()
                    && a.steer_capacity_n > 0.0
                    && (0.0..=1.0).contains(&a.recirculation)
                    && a.brake_capacity_n.is_finite()
                    && a.brake_capacity_n > 0.0,
            ),
            (
                "brake_static_factor",
                a.brake_static_factor.is_finite()
                    && (1.0..=2.5).contains(&a.brake_static_factor)
                    && (a.brake_capacity_n * a.brake_static_factor).is_finite(),
            ),
            (
                "engine.drag_fraction",
                (0.0..=1.0).contains(&a.drag_fraction),
            ),
            (
                "engine.inertia_kgm2",
                a.engine_inertia_kgm2.is_finite() && (0.1..=100.0).contains(&a.engine_inertia_kgm2),
            ),
            (
                "engine.clutch_capacity_nm",
                a.clutch_capacity_nm.is_finite()
                    && (100.0..=50_000.0).contains(&a.clutch_capacity_nm),
            ),
            (
                // The coupling lock denominator includes `k^2 / (2 * belt_inertia)`; tiny
                // positive values overflow that term even though the generic belt law accepts
                // every finite positive inertia.
                "derived belt inertia floor (coupling divisor)",
                a.belt_inertia.is_finite() && a.belt_inertia >= 1.0,
            ),
            (
                // The u8 countdown represents at most 255 ticks (DERIVED 255 / 64 = 3.984375 s).
                "gearbox.shift_secs",
                (0.0..=3.0).contains(&a.shift_secs),
            ),
        ] {
            if !ok {
                return Err(format!("track.powertrain.transmission: invalid {field}").into());
            }
        }

        let omega_rated = a.rated_rpm * RPM_TO_RAD;
        let gear = |v_kmh: &f32| omega_rated * a.sprocket_radius_m / (v_kmh / 3.6);
        let gears_fwd: Vec<f32> = a.forward_speeds_kmh.iter().map(gear).collect();
        let gears_rev: Vec<f32> = a.reverse_speeds_kmh.iter().map(gear).collect();
        let steer_kappa: Vec<(f32, f32)> = a
            .steer_radii_m
            .iter()
            .map(|&(tight, wide)| (a.half_tread_m / tight, a.half_tread_m / wide))
            .collect();
        let (peak_torque_rpm, peak_torque_nm) =
            a.torque_nm
                .iter()
                .copied()
                .fold((a.idle_rpm, 0.0f32), |best, (rpm, t)| {
                    if t > best.1 { (rpm, t) } else { best }
                });
        // Genuine neutral-steer scale: 1st gear tight curvature × 1st gear governed speed.
        let v1_governed = a.forward_speeds_kmh[0] / 3.6 * (a.governed_rpm / a.rated_rpm);
        let neutral_d_full = steer_kappa[0].0 * v1_governed;
        let derived = Self {
            engine: EngineParams {
                idle_rpm: a.idle_rpm,
                governed_rpm: a.governed_rpm,
                torque_nm: a.torque_nm.to_vec(),
            },
            gears_fwd,
            gears_rev,
            sprocket_radius: a.sprocket_radius_m,
            launch_gears: a.launch_gears,
            max_step: a.max_step,
            steer_kappa,
            steer_radii_m: a.steer_radii_m.to_vec(),
            steer_capacity_n: a.steer_capacity_n,
            neutral_d_full,
            recirculation: a.recirculation,
            brake_capacity_n: a.brake_capacity_n,
            brake_static_factor: a.brake_static_factor,
            drag_fraction: a.drag_fraction,
            engine_inertia: a.engine_inertia_kgm2,
            clutch_capacity: a.clutch_capacity_nm,
            shift_ticks: (a.shift_secs * TICK_HZ).round().clamp(0.0, 255.0) as u8,
            peak_torque_rpm,
            peak_torque_nm,
        };
        derived.check_derived(a.belt_inertia)?;
        Ok(derived)
    }

    /// The authored scalars can each be finite while their ratios, squared coupling terms or
    /// force capacities overflow/underflow. Refuse those coupled outputs before publication.
    fn check_derived(&self, belt_inertia: f32) -> Result<(), BevyError> {
        let fail = |field: &str| {
            BevyError::from(format!(
                "track.powertrain.transmission: non-finite or nonpositive derived {field}; correct gearbox speeds, steering radii, sprocket radius or inertia"
            ))
        };
        for &gear in self.gears_fwd.iter().chain(&self.gears_rev) {
            let coupling = gear / self.sprocket_radius;
            let inverse_inertia =
                1.0 / self.engine_inertia + coupling * coupling / (2.0 * belt_inertia);
            let force = self.peak_torque_nm.min(self.clutch_capacity) * coupling;
            if ![gear, coupling, inverse_inertia, force]
                .into_iter()
                .all(|value| value.is_finite() && value > 0.0)
            {
                return Err(fail("gear reduction, coupling divisor or drive force"));
            }
        }
        for &(tight, wide) in &self.steer_kappa {
            if ![tight, wide]
                .into_iter()
                .all(|value| value.is_finite() && value > 0.0 && (1.0 + value * value).is_finite())
            {
                return Err(fail("steering curvature"));
            }
        }
        if !self.roundoff_force_scale().is_finite()
            || self.roundoff_force_scale() <= 0.0
            || !(2.0 * belt_inertia).is_finite()
            || !self.neutral_d_full.is_finite()
            || self.neutral_d_full <= 0.0
            || !(2.0 * self.brake_capacity_n).is_finite()
            || !(2.0 * self.steer_capacity_n).is_finite()
            || ![self.governed_speed(false), self.governed_speed(true)]
                .into_iter()
                .all(|speed| speed.is_finite() && speed > 0.0)
        {
            return Err(fail(
                "neutral speed, governed speed, paired force capacity or crank arithmetic scale",
            ));
        }
        Ok(())
    }

    /// DERIVED top-gear speed (m/s) at the fuel governor's rpm for the selected ladder.
    /// External loads may drive the belts faster.
    pub fn governed_speed(&self, reverse: bool) -> f32 {
        let ladder = if reverse {
            &self.gears_rev
        } else {
            &self.gears_fwd
        };
        let top = ladder
            .last()
            .expect("authoring requires a nonempty gear ladder");
        self.engine.governed_rpm * RPM_TO_RAD * self.sprocket_radius / top
    }

    /// Engine torque (N·m) at `rpm`: piecewise-linear over the authored points (end-clamped),
    /// under the fuel-governor cut past `governed_rpm`.
    pub fn torque_at(&self, rpm: f32) -> f32 {
        let pts = &self.engine.torque_nm;
        let raw = if rpm <= pts[0].0 {
            pts[0].1
        } else if rpm >= pts[pts.len() - 1].0 {
            pts[pts.len() - 1].1
        } else {
            let mut t = pts[0].1;
            for w in pts.windows(2) {
                let ((r0, t0), (r1, t1)) = (w[0], w[1]);
                if rpm >= r0 && rpm <= r1 {
                    t = t0 + (t1 - t0) * ((rpm - r0) / (r1 - r0));
                    break;
                }
            }
            t
        };
        raw * self.fuel_cut(rpm)
    }

    fn fuel_cut(&self, rpm: f32) -> f32 {
        (1.0 - (rpm - self.engine.governed_rpm) / GOVERNOR_CUT_RPM).clamp(0.0, 1.0)
    }

    pub(crate) fn launch_omega(&self, command: f32) -> f32 {
        (self.engine.idle_rpm + command * (self.peak_torque_rpm - self.engine.idle_rpm))
            * RPM_TO_RAD
    }

    /// Force scale of the crank-acceleration subtraction, J*omega/dt times the largest
    /// admitted crank-to-belt ratio. Its f32 roundoff follows construction, not hull weight.
    pub(crate) fn roundoff_force_scale(&self) -> f32 {
        let ratio = self
            .gears_fwd
            .iter()
            .chain(&self.gears_rev)
            .copied()
            .fold(0.0, f32::max);
        self.engine_inertia
            * (self.max_curve_rpm() + OVERREV_MARGIN_RPM)
            * RPM_TO_RAD
            * TICK_HZ
            * ratio
            / self.sprocket_radius
    }

    /// Last authored curve rpm: the landing ceiling; the clutch guard adds its slip band.
    pub fn max_curve_rpm(&self) -> f32 {
        self.engine.torque_nm[self.engine.torque_nm.len() - 1].0
    }
}

/// Physical main-clutch regime. Slip retains a launch until its shaft reaches the command's
/// launch speed; Out carries no mean-axis torque.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum ClutchState {
    Out,
    Slip,
    In,
}

/// Complete path-dependent drivetrain state, built at spawn and replicated atomically.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct TransmissionState {
    /// 1-based gear in the active ladder.
    pub gear: u8,
    /// Remaining torque-interruption ticks of an in-flight shift.
    pub shift_ticks: u8,
    /// L600 steering detent: 0 straight (locked differential), 1 wide, 2 tight.
    pub steer_step: u8,
    /// Which ladder is engaged (reverse uses the R ladder).
    pub reverse: bool,
    /// Park lever: set at rest and by a released low-speed stop. It holds both ways during
    /// reversal pause/cut and without intent; launch carries only the rollback shortfall.
    /// A breached lever retains its dynamic brake capacity.
    pub park: bool,
    /// Engine crank speed ω_e (rad/s) — stage B's crank state. Initialized explicitly from
    /// vehicle data by [`Self::from_spec`]; the regenerative tick path has no in-band sentinel.
    pub omega_e: f32,
    /// Main clutch's physical regime, replicated with the crank and gear.
    pub clutch: ClutchState,
    /// Remaining stopped ticks before a held opposite request may change direction.
    /// Any motion refreshes it; release cancels it without releasing park.
    pub reversal_hold_ticks: u8,
}

/// One field in the authoritative canonical transmission projection. Float values retain their raw
/// bits.
#[derive(Clone, Copy, Debug)]
pub(crate) enum TransmissionProjectionValue {
    U8(u8),
    Bool(bool),
    F32(f32),
}

/// The exhaustive canonical transmission inventory in its canonical replication/hash/trace order.
/// Adding state fails this destructure until the field is classified exactly once here.
pub(crate) fn transmission_state_projection(
    state: &TransmissionState,
) -> [TransmissionProjectionValue; 8] {
    let TransmissionState {
        gear,
        shift_ticks,
        steer_step,
        reverse,
        park,
        omega_e,
        clutch,
        reversal_hold_ticks,
    } = *state;
    use TransmissionProjectionValue::{Bool, F32, U8};
    [
        U8(gear),
        U8(shift_ticks),
        U8(steer_step),
        Bool(reverse),
        Bool(park),
        F32(omega_e),
        U8(match clutch {
            ClutchState::Out => 0,
            ClutchState::Slip => 1,
            ClutchState::In => 2,
        }),
        U8(reversal_hold_ticks),
    ]
}

impl TransmissionState {
    /// Construct complete regenerative transmission state synchronously from validated vehicle
    /// data. The crank starts at the authored idle speed.
    pub fn from_spec(tp: &TransmissionParams) -> Self {
        Self::with_crank_speed(tp.engine.idle_rpm * RPM_TO_RAD)
    }

    /// A state at rest with its crank stopped: the fixture for tests whose subject is not the
    /// transmission (replication scope, probe layout, HUD formatting).
    #[cfg(test)]
    pub(crate) fn stopped() -> Self {
        Self::with_crank_speed(0.0)
    }

    fn with_crank_speed(omega_e: f32) -> Self {
        Self {
            gear: 1,
            shift_ticks: 0,
            steer_step: 0,
            reverse: false,
            park: true,
            omega_e,
            clutch: ClutchState::Out,
            reversal_hold_ticks: 0,
        }
    }
}

/// A compact operating-point readout of the joint drivetrain — the ONE place the HUD/legend
/// reads gear and rpm from, so the display never re-derives drivetrain math (the gear/rpm
/// relation lives here, beside the adapter that integrates on it).
#[derive(Clone, Debug, PartialEq)]
pub struct DriveReadout {
    /// Engine rpm — the crank state ω_e DIRECTLY (stage B: the state IS the display), including
    /// sub-idle lugging and, in the game's damage model, a crank at rest when combustion
    /// cannot sustain idle (`crank_guards`).
    pub rpm: f32,
    /// The engaged gear as a display label: `F1..Fn` forward, `R1..Rn` reverse.
    pub gear_label: String,
}

/// Read the drivetrain operating point THROUGH THE LAW: the engaged gear from
/// [`TransmissionState`] against the active ladder, and the engine rpm from the CRANK STATE
/// ω_e (stage B — no belt-derived re-derivation: the crank slips against the shaft at
/// launch, rev-matches through shifts, and can turn backward when back-driven; the display shows
/// exactly that state). Pure (no ECS), so the HUD and any legend share one implementation.
pub fn readout(st: &TransmissionState, tp: &TransmissionParams) -> DriveReadout {
    let ladder: &[f32] = if st.reverse {
        &tp.gears_rev
    } else {
        &tp.gears_fwd
    };
    let top = ladder.len() as u8;
    let gear = st.gear.clamp(1, top);
    let rpm = st.omega_e / RPM_TO_RAD;
    DriveReadout {
        rpm,
        gear_label: format!("{}{gear}", if st.reverse { 'R' } else { 'F' }),
    }
}

/// The game's damage model: independent staffing, task throughput and local module condition.
/// The drivetrain consumes the task rate once; source condition also remains available when
/// the task has no throughput. Brakes and construction losses retain their own laws.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DriveSupply {
    pub control_authority: bool,
    pub task_rate: f32,
    /// The engine's held HP share; the power readout reads its own source condition.
    pub engine_condition: f32,
    /// The transmission's held HP share; clutch and steering capacity read this source.
    pub transmission_condition: f32,
}

impl DriveSupply {
    /// The game's damage model: the sandbox lab's staffed, full-rate, whole-source fixture.
    pub const WHOLE: Self = Self {
        control_authority: true,
        task_rate: 1.0,
        engine_condition: 1.0,
        transmission_condition: 1.0,
    };
}

/// Shaped drive axes, requested intent, pre-tick belt speeds and ground reactions.
/// The mixed side commands are probe telemetry only.
pub struct TransmissionInput {
    /// The game's damage model: what the crew, engine and transmission each supply this tick.
    pub supply: DriveSupply,
    /// Requested pedal before shaping; gear choices must not read the actuator ramp.
    pub command: f32,
    pub throttle: f32,
    pub steer: f32,
    pub side_commands: [f32; 2],
    pub speeds: [f32; 2],
    pub reactions: [f32; 2],
    /// Unfiltered support load and grip seen by the Driver.
    pub perceived_load: super::load::PerceivedLoad,
    pub dt: f32,
    /// The driver holds the service brakes: no drive at any throttle, the full service brakes while
    /// the tank moves, and the parking latch once it is at rest (`driver_intent`).
    pub brake: bool,
}

/// What the joint solve produced: the integrated next belt speeds, the per-side sprocket
/// forces actually applied (telemetry — the `engine` slot of the harness rows), and the
/// operating point for HUD/legend display.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransmissionReport {
    pub next_speeds: [f32; 2],
    pub forces: [f32; 2],
    pub rpm: f32,
    pub gear: u8,
    pub reverse: bool,
    pub steer_step: u8,
    pub shifting: bool,
    /// Power-conservation scale applied to the drive/steer forces this tick (1 = unconstrained).
    pub power_scale: f32,
    /// Combustion power capacity at the pre-tick operating point (W). The work gate also
    /// accounts for crank inertia and the speeds reached during this tick.
    pub power_available: f32,
    #[cfg(feature = "bitprobe")]
    pub(crate) bitprobe: TransmissionProbe,
}

/// Authored Coulomb share of the motoring-drag anchor, shared with the law's instruments.
pub(crate) fn motoring_drag_base_share() -> f32 {
    engine_control().motoring_drag_base_share.value
}

/// The game's damage model: construction motoring drag at the crank (N m), independent of
/// pedal and combustion. Its authored mid-band anchor is `drag_fraction × peak torque`;
/// the affine Coulomb/viscous shape rises with speed. The discrete free-crank stop bounds drag at zero.
/// Combustion reconstructs the gross torque behind the authored net curve (`engine_torques`).
fn engine_drag(tp: &TransmissionParams, omega_e: f32) -> f32 {
    let omega_mid = (tp.engine.idle_rpm + tp.engine.governed_rpm) * 0.5 * RPM_TO_RAD;
    let rise = motoring_drag_base_share()
        + (1.0 - motoring_drag_base_share()) * (omega_e.abs() / omega_mid);
    tp.peak_torque_nm * tp.drag_fraction * rise * omega_e.signum()
}

/// The game's damage model: the full-load curve is net of construction losses. Reconstruct
/// the combustion torque it implies, scale only that source, and charge drag whole. The pedal
/// blend retains the healthy net curve at every command. The crank and gear selector share
/// these three torques (N m), so an emptied engine keeps friction even under full command.
fn engine_torques(
    tp: &TransmissionParams,
    supply: DriveSupply,
    omega: f32,
    command: f32,
) -> [f32; 3] {
    let drag = engine_drag(tp, omega);
    if omega < 0.0 {
        return [0.0, 0.0, drag]; // A backward crank cannot fire.
    }
    if !supply.control_authority {
        return [0.0, 0.0, drag]; // No Driver fuel request or independent idle actuator.
    }
    let pedal = (command / engine_control().drag_throttle_release.value).clamp(0.0, 1.0);
    let released = pedal * pedal * (3.0 - 2.0 * pedal);
    let rpm = omega / RPM_TO_RAD;
    let curve = tp.torque_at(rpm);
    let induced = supply.task_rate * (command * curve + released * drag * tp.fuel_cut(rpm));
    let idle_capacity = idle_catch_capacity(tp, supply);
    let catch = (idle_capacity * (tp.engine.idle_rpm * RPM_TO_RAD - omega)
        / (engine_control().idle_catch_droop_rpm.value * RPM_TO_RAD))
        .clamp(0.0, idle_capacity)
        .min(curve + drag);
    // Both requests move one rack; the catch supplies only what the command has not supplied.
    [induced, (catch - induced).max(0.0), drag]
}

/// The game's driving and damage model: the Driver's idle catch is work by the Drive task,
/// requiring control authority and consuming its throughput once, like commanded fueling.
fn idle_catch_capacity(tp: &TransmissionParams, supply: DriveSupply) -> f32 {
    if supply.control_authority {
        supply.task_rate * tp.torque_at(tp.engine.idle_rpm)
    } else {
        0.0
    }
}

/// Instrument for the accepted Driver law: price the crank at the landing shaft speed,
/// or the commanded launch speed while slipping, bounded by clutch and contact grip.
/// A cut cannot reverse a free crank, so pricing uses the saturated catch floor.
/// This shared price does not select a gear or change delivery.
pub(crate) fn price_driver_force(
    tp: &TransmissionParams,
    supply: DriveSupply,
    shaft: f32,
    gear: f32,
    command: f32,
    grip_n: f32,
    clutch: ClutchState,
) -> f32 {
    let capacity = supply.transmission_condition * tp.clutch_capacity * gear / tp.sprocket_radius;
    free_driver_force(tp, supply, shaft, gear, command, clutch)
        .clamp(-capacity, capacity)
        .clamp(-grip_n, grip_n)
}

fn free_driver_force(
    tp: &TransmissionParams,
    supply: DriveSupply,
    shaft: f32,
    gear: f32,
    command: f32,
    clutch: ClutchState,
) -> f32 {
    let launch_rpm = tp.launch_omega(command) / RPM_TO_RAD;
    let floor = idle_floor(tp, supply, 0.0).unwrap_or(f32::NEG_INFINITY);
    let shaft_rpm = (shaft * gear / tp.sprocket_radius / RPM_TO_RAD).max(floor / RPM_TO_RAD);
    // A moving In clutch below idle is a lug, not a fresh launch.
    let rpm = if clutch == ClutchState::Slip {
        launch_rpm
    } else {
        shaft_rpm
    };
    let [induced, idle, drag] = engine_torques(tp, supply, rpm * RPM_TO_RAD, command);
    (induced + idle - drag) * gear / tp.sprocket_radius
}

/// Every candidate uses its own detent radius; the support moment is independent of the
/// currently selected gear and remains available on the tick the steer detent changes.
fn candidate_load(
    tp: &TransmissionParams,
    load: super::load::PerceivedLoad,
    gear: u8,
    detent: u8,
) -> super::load::PerceivedLoad {
    let (tight, wide) = tp.steer_radii_m[usize::from(gear - 1)];
    let turn_n = match detent {
        0 => 0.0,
        1 => load.turn_moment_nm / wide,
        2 => load.turn_moment_nm / tight,
        _ => panic!("the transmission admits only straight, wide and tight detents"),
    };
    super::load::PerceivedLoad { turn_n, ..load }
}

/// Coupled translation mass (kg): nominal hull mass and both belt inertias.
pub(crate) fn coupled_mass(fp: &ForceParams) -> f32 {
    let hull = fp
        .grip
        .expect("a drivetrain requires grip construction")
        .side_weight
        * 2.0
        / super::derive::G;
    hull + 2.0 * fp.inertia
}

fn force_margin(fp: &ForceParams, tp: &TransmissionParams) -> f32 {
    let weight = 2.0
        * fp.grip
            .expect("a drivetrain requires grip construction")
            .side_weight;
    super::load::DriverResolution::load().margin_n(weight, tp)
}

/// A coupled crank accelerates with the vehicle: J*k^2 adds reflected mass. Solving
/// M*a = F-L and F = k*tau-J*k^2*a gives this force. Slip holds its launch rpm.
#[expect(
    clippy::too_many_arguments,
    reason = "the force law reads construction, contact, supply, landing, ratio, command, load and clutch"
)]
fn gear_force(
    tp: &TransmissionParams,
    fp: &ForceParams,
    supply: DriveSupply,
    shaft: f32,
    ratio: f32,
    command: f32,
    load: super::load::PerceivedLoad,
    clutch: ClutchState,
) -> f32 {
    let force = free_driver_force(tp, supply, shaft, ratio, command, clutch);
    let force = if clutch == ClutchState::Slip {
        force
    } else {
        let reflected = tp.engine_inertia * (ratio / tp.sprocket_radius).powi(2) / coupled_mass(fp);
        (force + reflected * load.total_n()) / (1.0 + reflected)
    };
    let capacity = supply.transmission_condition * tp.clutch_capacity * ratio / tp.sprocket_radius;
    force
        .clamp(-capacity, capacity)
        .clamp(-load.grip_n, load.grip_n)
}

/// Highest declared launch gear affordable at requested pedal. Released park preselects
/// at full pedal; a live request reprices before actuator travel, with no paid drive cut.
fn launch_pick(
    tp: &TransmissionParams,
    fp: &ForceParams,
    supply: DriveSupply,
    ladder: &[f32],
    load: super::load::PerceivedLoad,
    detent: u8,
    command: f32,
) -> Option<u8> {
    (1..=tp.launch_gears.min(ladder.len() as u8))
        .rev()
        .find(|&gear| {
            let load = candidate_load(tp, load, gear, detent);
            gear_force(
                tp,
                fp,
                supply,
                0.0,
                ladder[usize::from(gear - 1)],
                command,
                load,
                ClutchState::Slip,
            ) - load.total_n()
                > force_margin(fp, tp)
        })
}

/// Candidate gears pay their own cut; holding the current gear pays none. Pricing a cut
/// for holding would keep low gears at their pre-governor force and prevent the ladder walk.
pub(crate) fn landed_spare(
    tp: &TransmissionParams,
    fp: &ForceParams,
    inp: &TransmissionInput,
    state: &TransmissionState,
    candidate: u8,
    command: f32,
) -> f32 {
    let ladder = if state.reverse {
        &tp.gears_rev
    } else {
        &tp.gears_fwd
    };
    let shaft = (inp.speeds[0] + inp.speeds[1]) * 0.5 * if state.reverse { -1.0 } else { 1.0 };
    let load = candidate_load(tp, inp.perceived_load, candidate, state.steer_step);
    let landing = if candidate == state.gear {
        shaft
    } else {
        shaft - load.total_n() * f32::from(tp.shift_ticks) / TICK_HZ / coupled_mass(fp)
    };
    let ratio = ladder[usize::from(candidate - 1)];
    // C1 corrects an engaged crank below the curve floor. An upshift into that same lug
    // would immediately undo itself and price the idle catch as sustained pedal power.
    let floor = tp.engine.torque_nm[0].0 * RPM_TO_RAD;
    if candidate > state.gear && landing * ratio / tp.sprocket_radius < floor {
        return f32::NEG_INFINITY;
    }
    if (landing * ratio / tp.sprocket_radius / RPM_TO_RAD).abs() > tp.max_curve_rpm() {
        return f32::NEG_INFINITY;
    }
    gear_force(
        tp,
        fp,
        inp.supply,
        landing,
        ratio,
        command,
        load,
        if candidate == state.gear {
            state.clutch
        } else {
            ClutchState::In
        },
    ) - load.total_n()
}

/// One Driver decision: C3 preselects at rest; C2 preserves overrun; C1 admits a lower
/// gear only on a deficit beyond M or a lugging crank. A detent turn cannot upshift.
fn choose_gear(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    fp: &ForceParams,
    inp: &TransmissionInput,
    command: f32,
    shaft: f32,
) {
    if !inp.supply.control_authority
        || inp.supply.task_rate == 0.0
        || st.shift_ticks != 0
        || inp.perceived_load.grip_n == 0.0
    {
        return; // No Driver work, paid cut, or airborne: no gear decision.
    }
    let ladder = if st.reverse {
        &tp.gears_rev
    } else {
        &tp.gears_fwd
    };
    if shaft.abs() < PARK_ENGAGE_SPEED {
        if let Some(gear) = launch_pick(
            tp,
            fp,
            inp.supply,
            ladder,
            inp.perceived_load,
            st.steer_step,
            if command > DEAD { command } else { 1.0 },
        ) {
            st.gear = gear;
        }
        return;
    }
    let [fuel, catch, drag] = engine_torques(tp, inp.supply, st.omega_e, command);
    if command <= 0.0 || fuel + catch <= drag.abs() {
        return;
    }
    let current = landed_spare(tp, fp, inp, st, st.gear, command);
    let margin = force_margin(fp, tp);
    let lower = current < -margin
        || (st.clutch == ClutchState::In && st.omega_e / RPM_TO_RAD < tp.engine.torque_nm[0].0);
    let mut best = (st.gear, current);
    for candidate in st.gear.saturating_sub(tp.max_step).max(1)
        ..=st.gear.saturating_add(tp.max_step).min(ladder.len() as u8)
    {
        if candidate == st.gear
            || (candidate < st.gear && !lower)
            || (candidate > st.gear && st.steer_step != 0)
        {
            continue;
        }
        let spare = landed_spare(tp, fp, inp, st, candidate, command);
        if spare > best.1 + margin {
            best = (candidate, spare);
        }
    }
    if best.0 != st.gear {
        st.gear = best.0;
        st.shift_ticks = tp.shift_ticks;
    }
}

/// The crank's side of one tick, fixed before the coupling solve and shared by
/// [`clutch_coupling`] and [`settle_crank`]: the pre-tick crank speed `omega_e` and free torque
/// `tau_free` (rad/s, N·m), the crank-to-belt ratio `k` (gear / sprocket radius) and direction sign
/// `dir`, the guard band `[omega_floor, omega_over]` (rad/s) and the step `dt` (s).
#[derive(Clone, Copy)]
struct CrankTick {
    omega_e: f32,
    tau_free: f32,
    k: f32,
    dir: f32,
    omega_floor: f32,
    omega_over: f32,
    dt: f32,
}

/// The game's driving model: capacity-limited clutch torque, solved semi-implicitly.
/// The lock torque makes the crank and mean belt integration agree:
/// `tau* = [(omega_e-k*dir*m)/dt + tau_free/J - k*dir*F_other/I_m] / [1/J+k^2/I_m]`.
/// F_other includes ground reactions. Brakes and steering act later, so `settle_crank`
/// independently checks the final lock's feasibility rather than trusting this predictor.
///
/// The lower guard reduces transmission to preserve a freely reachable crank floor;
/// the upper guard raises it to bound over-rev. Both stay inside clutch capacity.
/// In straight overrun both sprocket powers are regenerative: the work gate cannot reduce
/// torque, so the guard-point motoring drag reaches the belts in full. Powered steering may
/// scale all engine forces together; the final crank guard remains unconditional.
fn clutch_coupling(
    crank: CrankTick,
    j_e: f32,
    capacity: f32,
    m: f32,
    i_m: f32,
    f_other: f32,
    #[cfg(feature = "bitprobe")] probe: &mut TransmissionProbe,
) -> f32 {
    let CrankTick {
        omega_e,
        tau_free,
        k,
        dir: s,
        omega_floor,
        omega_over,
        dt,
    } = crank;
    let tau_star = ((omega_e - k * s * m) / dt + tau_free / j_e - k * s * f_other / i_m)
        / (1.0 / j_e + k * k / i_m);
    let mut tau_c = tau_star.clamp(-capacity, capacity);
    let omega_next = omega_e + (tau_free - tau_c) * dt / j_e;
    #[cfg(feature = "bitprobe")]
    {
        probe.tau_star = tau_star;
        probe.tau_clamped = tau_c;
        probe.omega_coupled = omega_next;
    }
    if omega_next < omega_floor {
        // Land exactly at the floor: τ_guard = τ_free − J·(ω_floor − ω_e)/dt. By
        // construction τ_guard < τ_c (less torque = higher landing), so the guard only
        // ever reduces — one-sided.
        tau_c = (tau_free - (omega_floor - omega_e) * j_e / dt).clamp(-capacity, capacity);
    } else if omega_next > omega_over {
        // Land exactly at the over-rev guard point: same formula, other side. By
        // construction τ_guard > τ_c (more torque = lower landing), so this side only
        // ever increases — one-sided, and mutually exclusive with the stall branch.
        tau_c = (tau_free - (omega_over - omega_e) * j_e / dt).clamp(-capacity, capacity);
    }
    #[cfg(feature = "bitprobe")]
    {
        probe.tau_c = tau_c;
    }
    tau_c
}

/// Per-side brake capacity for the current regime. Static breakaway is a zero-work latch property,
/// not a stronger moving brake: every missing predicate returns the authored dynamic capacity.
pub(crate) fn brake_capacity_for_regime(
    tp: &TransmissionParams,
    latch_active: bool,
    service: bool,
    belt_speed: f32,
) -> f32 {
    if latch_active && !service && belt_speed.abs() < PARK_ENGAGE_SPEED {
        tp.brake_capacity_n * tp.brake_static_factor
    } else {
        tp.brake_capacity_n
    }
}

/// A moving reversal refreshes the stopped countdown. Release cancels the countdown;
/// only a complete at-rest hold permits one paid direction cut.
fn swap_ladder_direction(
    tp: &TransmissionParams,
    st: &mut TransmissionState,
    inp: &TransmissionInput,
) {
    if !inp.supply.control_authority || inp.supply.task_rate == 0.0 {
        return;
    }
    let opposite = inp.command.abs() >= DEAD && (inp.command < 0.0) != st.reverse;
    if !opposite || inp.brake {
        st.reversal_hold_ticks = 0;
        return;
    }
    if inp
        .speeds
        .iter()
        .any(|speed| speed.abs() >= PARK_ENGAGE_SPEED)
    {
        st.reversal_hold_ticks = (driver_actuation().reversal_hold_s.value * TICK_HZ).ceil() as u8;
        return;
    }
    if st.reversal_hold_ticks > 0 {
        st.reversal_hold_ticks -= 1;
        return;
    }
    if st.shift_ticks == 0 {
        st.reverse = inp.command < 0.0;
        st.gear = 1;
        st.shift_ticks = tp.shift_ticks;
    }
}

struct DriverIntent {
    dir: f32,
    propulsive: f32,
    service: bool,
}

/// Opposing motion gets the full service brake at every nonzero notch. The engaged ladder
/// alone determines propulsion; releasing the request coasts above the data-derived stop speed.
fn driver_intent(inp: &TransmissionInput, reverse: bool, parked: bool) -> DriverIntent {
    let dir = if reverse { -1.0 } else { 1.0 };
    let opposing = inp.command * dir < -DEAD;
    let against_motion = inp.command.abs() >= DEAD
        && inp
            .speeds
            .iter()
            .any(|v| v * inp.command.signum() < -PARK_ENGAGE_SPEED);
    DriverIntent {
        dir,
        propulsive: if inp.brake || opposing {
            0.0
        } else {
            (inp.throttle * dir).clamp(0.0, 1.0)
        },
        service: against_motion || ((inp.brake || opposing) && !parked),
    }
}

/// Resolve the detent before gear choice and steering: both must read this tick's detent,
/// or a newly engaged turn could permit an upshift that changes its radius.
fn update_steer_detent(st: &mut TransmissionState, steer: f32) {
    let a = steer.abs();
    let feel = super::drive::drive_feel();
    st.steer_step = match st.steer_step {
        0 => u8::from(a >= feel.wide_on.0),
        1 => {
            if a >= feel.tight_on.0 {
                2
            } else {
                u8::from(a >= feel.wide_off.0)
            }
        }
        _ => {
            if a < feel.tight_off.0 {
                1
            } else {
                2
            }
        }
    };
}

/// At rest the lever is on. Drive lets it carry only rollback; steering releases it for
/// a neutral turn. A released tank below first gear's idle speed stops and parks.
fn update_park_latch(st: &mut TransmissionState, tp: &TransmissionParams, inp: &TransmissionInput) {
    // The game's damage model: an unstaffed station retains the player's lever position.
    if !inp.supply.control_authority {
        return;
    }
    let speed = inp.speeds[0].abs().max(inp.speeds[1].abs());
    let requested = inp.command.abs() >= DEAD;
    let steering = inp.steer.abs() >= DEAD;
    let first = if st.reverse {
        tp.gears_rev[0]
    } else {
        tp.gears_fwd[0]
    };
    let stop_speed = tp.engine.idle_rpm * RPM_TO_RAD * tp.sprocket_radius / first;
    if !inp.brake && !requested && steering {
        st.park = false;
    } else if speed < PARK_ENGAGE_SPEED || (!requested && !steering && speed < stop_speed) {
        st.park = true;
    } else if requested && inp.command * (inp.speeds[0] + inp.speeds[1]) > 0.0 {
        st.park = false;
    }
}

/// The neutral steering member needs the main clutch open even while its counter-rotating
/// belts carry unequal speeds. Mean-axis drift must not close it and remove its rev-match fuel.
fn neutral_turn(inp: &TransmissionInput, mean: f32) -> bool {
    inp.throttle.abs() < driver_actuation().neutral_throttle.value
        && mean.abs() < driver_actuation().neutral_speed_m_s.value
}

/// Shift windows, a two-sided parked hold and neutral turns open
/// the clutch. Intent at rest below idle starts Slip regardless of the prior regime,
/// retaining it until the shaft reaches its commanded launch speed.
/// Without task work the clutch springs In, including at rest. A paid interruption still
/// expires on its mechanical clock; it cannot be renewed without a Driver.
fn update_clutch_state(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    inp: &TransmissionInput,
    shifting: bool,
    propulsive: f32,
    shaft_omega: f32,
) {
    let mean = (inp.speeds[0] + inp.speeds[1]) * 0.5;
    st.clutch = if shifting {
        ClutchState::Out
    } else if !inp.supply.control_authority || inp.supply.task_rate == 0.0 {
        ClutchState::In // Without opening work, the clutch springs engaged.
    } else if (st.park && (inp.brake || propulsive < DEAD))
        || (propulsive < DEAD
            && (mean.abs() < PARK_ENGAGE_SPEED
                || (inp.steer.abs() >= DEAD && neutral_turn(inp, mean))))
    {
        ClutchState::Out
    } else if propulsive >= DEAD
        && ((mean.abs() < PARK_ENGAGE_SPEED && shaft_omega < tp.engine.idle_rpm * RPM_TO_RAD)
            || (st.clutch == ClutchState::Slip && shaft_omega < tp.launch_omega(propulsive)))
    {
        ClutchState::Slip
    } else {
        ClutchState::In
    };
}

/// Free crank torque is commanded fuel or the Driver catch, less construction drag, all
/// evaluated at the pre-tick crank speed. Engaged fueling follows propulsive throttle only.
/// With the clutch open, authority plus a paid window or active steering admits a proportional
/// rev governor. A paid window targets `|m|*k` in the already selected landing gear, so the
/// clutch re-engages near-synchronous; without a window that target is idle. Steering targets
/// `idle + (peak - idle)*|steer|`; the governor takes the larger target. Open rest requests no
/// fuel. The catch and induced combustion consume task throughput; drag stays unscaled.
/// `engine_work` charges sprocket work against combustion and stored crank energy, including
/// steering work with the main clutch open. The governor width lives in assets/driving/engine.ron.
#[expect(
    clippy::too_many_arguments,
    reason = "the free-torque law's inputs: params, input, engagement, throttle, belt speed, ratio and both crank speeds"
)]
fn crank_free_torque(
    tp: &TransmissionParams,
    inp: &TransmissionInput,
    engaged: bool,
    shifting: bool,
    propulsive: f32,
    m: f32,
    k: f32,
    omega_e: f32,
    omega_idle: f32,
    #[cfg(feature = "bitprobe")] probe: &mut TransmissionProbe,
) -> (f32, f32) {
    let u_fuel = if engaged {
        propulsive
    } else if inp.supply.control_authority && (inp.steer.abs() >= DEAD || shifting) {
        let omega_match = if shifting { m.abs() * k } else { omega_idle };
        let omega_steer =
            omega_idle + (tp.peak_torque_rpm * RPM_TO_RAD - omega_idle) * inp.steer.abs().min(1.0);
        let omega_target = omega_match.max(omega_steer);
        ((omega_target - omega_e) / (engine_control().rev_match_band_rpm.value * RPM_TO_RAD))
            .clamp(0.0, 1.0)
    } else {
        0.0
    };

    let supply = inp.supply;
    let rpm = omega_e / RPM_TO_RAD;
    let [tau_ind, tau_idle, tau_drag] = engine_torques(tp, supply, omega_e, u_fuel);
    // A discrete loss can stop the free crank, never drive it through zero. Clutch work
    // may still back-drive the settled crank in either direction.
    let stop_drag = tp.engine_inertia * omega_e.abs() / inp.dt;
    let tau_free = tau_ind + tau_idle - tau_drag.clamp(-stop_drag, stop_drag);

    let p_avail = supply.engine_condition * tp.torque_at(rpm) * omega_e.max(0.0);
    #[cfg(feature = "bitprobe")]
    {
        probe.u_fuel = u_fuel * supply.task_rate;
        probe.rpm = rpm;
        probe.tau_idle = tau_idle;
        probe.tau_induced = tau_ind;
        probe.tau_drag = tau_drag;
        probe.tau_free = tau_free;
        probe.power_available = p_avail;
    }
    (tau_free, p_avail)
}

/// The steering member's contribution to this tick: the difference-axis force `F_s`, the L600
/// constraint force λ, and λ's per-output Jacobian `j`. Returned as `(f_s, lambda, j)`; a tick
/// that commands neither leaves all three at zero.
///
/// κ is table-indexed by the ACTIVE gear (reverse mirrors the low forward gears); `d` follows the
/// steer SIGN regardless of travel direction — the superimposed steering shaft is independent of
/// the gear's direction, historically and mechanically.
///
/// The servo is a capacity-limited KINEMATIC one, semi-implicit like the brakes and λ: the `F_s`
/// that lands `d` exactly on target after this tick's integration (`d` dynamics:
/// `d_next = d + (F_s/2 − R_d)/I·dt`), reaction-compensated, clamped to the per-output
/// convention's bound. Exact inside capacity, honest slip beyond it — and no proportional band:
/// the old P law's steady-state droop let the ground reaction eat the command (the Tiger's whole
/// neutral target, 0.21 m/s, sat INSIDE the 0.25 m/s band, so a sustained pivot ran at ≤ half
/// capacity and crawled at 0.03 rad/s — a vehicle-scaling defect the T-34's 0.46 m/s target
/// masked). The difference-axis bound is 2× the per-output capacity: `F_s` splits `±F_s/2` onto
/// the outputs, and each output's share is what the per-output datum caps (see
/// [`TransmissionParams::steer_capacity_n`] — the pivot-dead convention fix).
fn steering_force(
    tp: &TransmissionParams,
    fp: &ForceParams,
    st: &TransmissionState,
    inp: &TransmissionInput,
    m: f32,
    d: f32,
    f_c: f32,
) -> (f32, f32, [f32; 2]) {
    let dt = inp.dt;
    let [vl, vr] = inp.speeds;
    let kappa_idx = ((st.gear - 1) as usize).min(tp.steer_kappa.len() - 1);
    let (k_tight, k_wide) = tp.steer_kappa[kappa_idx];
    let mut f_s = 0.0;
    let mut lambda = 0.0;
    let mut j = [0.0f32; 2];
    let steer_capacity = inp.supply.transmission_condition * tp.steer_capacity_n;
    let f_s_max = 2.0 * steer_capacity;
    let r_d = (inp.reactions[0] - inp.reactions[1]) / 2.0;
    let servo =
        |target_d: f32| (2.0 * ((target_d - d) * fp.inertia / dt + r_d)).clamp(-f_s_max, f_s_max);
    // The steering detent (`st.steer_step`) was already updated for THIS tick
    // before gear selection — one detent truth per tick; this law only
    // consumes it.
    let neutral = neutral_turn(inp, m);
    if neutral {
        // The marginal brake-gated neutral turn: a slow capacity-limited servo
        // toward the DERIVED pivot scale `neutral_d_full = κ_tight(F1) ×
        // v1_governed` — the radii table's own gear-independent invariant (the
        // unprovenanced `neutral_fraction` that used to shrink it was deleted).
        f_s = servo(inp.steer * tp.neutral_d_full);
    } else {
        // The geared-radius constraint g = d − s·κ·|m| = 0, solved semi-implicitly:
        // λ is the force that lands g at zero after this tick's integration, clamped
        // so each output's share stays inside the per-output capacity (beyond it the
        // constraint slips). Zero ideal work: Q_c·v = λ·g, which the solve drives to
        // zero.
        let s = if inp.steer > 0.0 {
            1.0
        } else if inp.steer < 0.0 {
            -1.0
        } else {
            0.0
        };
        let kappa = match st.steer_step {
            0 => 0.0,
            1 => k_wide,
            _ => k_tight,
        };
        let a_l = f_c / 2.0 - inp.reactions[0];
        let a_r = f_c / 2.0 - inp.reactions[1];
        // One |m| branch of the solve: on branch b, |m| linearizes as `b·m`, so
        // `g = jl·v_L + jr·v_R` with the branch's Jacobian. Returns λ (per-output
        // capacity clamp — straight-gear `max|j| = 1/2` gives the same 2× bound
        // the servo uses), the Jacobian, and the pre-brake landing m. Brakes act later.
        let solve = |branch: f32| -> (f32, [f32; 2], f32) {
            let e = s * kappa * branch;
            let jl = (1.0 - e) / 2.0;
            let jr = -(1.0 + e) / 2.0;
            let g_now = jl * vl + jr * vr;
            let denom = jl * jl + jr * jr;
            let lambda_max = steer_capacity / jl.abs().max(jr.abs());
            let l = (-(g_now * fp.inertia / dt + jl * a_l + jr * a_r) / denom)
                .clamp(-lambda_max, lambda_max);
            let m_next = m + (a_l + l * jl + a_r + l * jr) / (2.0 * fp.inertia) * dt;
            (l, [jl, jr], m_next)
        };
        let b0 = if m > 0.0 {
            1.0
        } else if m < 0.0 {
            -1.0
        } else {
            0.0
        };
        let (l0, j0, m_next) = solve(b0);
        if b0 != 0.0 && m_next * b0 < 0.0 {
            // The tick crosses m = 0: the pre-tick branch would project onto
            // `d = s·κ·m` on the WRONG side of the |m| cusp — a one-tick
            // steering-sign reversal. Re-solve on the branch the belt actually
            // lands on; if the branches disagree about the landing side (the
            // genuine cusp), the constraint takes the tick off — λ = 0 is stable
            // and passive there.
            let (l1, j1, m1) = solve(-b0);
            if m1 * b0 <= 0.0 {
                lambda = l1;
                j = j1;
            }
        } else {
            lambda = l0;
            j = j0;
        }
    }
    (f_s, lambda, j)
}

/// Signed sprocket work after the declared recirculation loss. The split is physical:
/// one sprocket can absorb work while both modal axes report positive work.
fn recirculated_work(tp: &TransmissionParams, sides: [f32; 2]) -> f32 {
    let positive = sides[0].max(0.0) + sides[1].max(0.0);
    let negative = (-sides[0]).max(0.0) + (-sides[1]).max(0.0);
    if positive == 0.0 {
        return -negative;
    }
    // Only work transferred to the positive output recirculates. Pure engine braking
    // returns work through the main clutch without an inner-to-outer transfer loss.
    positive - negative + (1.0 - tp.recirculation) * negative.min(positive / tp.recirculation)
}

/// The engine's work allocation for one tick: a common force scale and the remaining crank
/// energy (J). The game's damage model has one source budget: combustion's free-crank step
/// plus stored crank inertia. Positive/recirculated work uses each sprocket's midpoint speed,
/// so acceleration from rest costs energy on the tick it happens.
struct EngineWork {
    scale: f32,
    crank_energy: f32,
}

fn engine_work(
    tp: &TransmissionParams,
    fp: &ForceParams,
    inp: &TransmissionInput,
    crank: CrankTick,
    motor: [f32; 2],
    #[cfg(feature = "bitprobe")] probe: &mut TransmissionProbe,
) -> EngineWork {
    let omega_free = (crank.omega_e + crank.tau_free * crank.dt / tp.engine_inertia)
        .clamp(-crank.omega_over, crank.omega_over);
    // DERIVED: the unclamped constant-torque work is tau_free * dt times mean crank
    // speed, including construction losses. Guard clipping discards excess work.
    let energy = 0.5 * tp.engine_inertia * omega_free * omega_free;
    let spendable =
        (energy - 0.5 * tp.engine_inertia * crank.omega_floor.max(0.0).powi(2)).max(0.0);
    let sides_at = |scale: f32| {
        [0, 1].map(|i| {
            let force = motor[i] * scale;
            let next = inp.speeds[i] + (force - inp.reactions[i]) * inp.dt / fp.inertia;
            force * (inp.speeds[i] + next) * 0.5 * inp.dt
        })
    };
    let work = |scale| recirculated_work(tp, sides_at(scale));
    let scale = if work(1.0) <= spendable {
        1.0
    } else {
        let (mut lo, mut hi) = (0.0, 1.0);
        // Resolution: 24 bisections resolve the unit force scale to f32's mantissa. Positive
        // work is convex in scale; zero is feasible even with an empty engine and stopped crank.
        for _ in 0..24 {
            let mid = (lo + hi) * 0.5;
            if work(mid) <= spendable {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    };
    #[cfg(feature = "bitprobe")]
    {
        let [left, right] = sides_at(scale).map(|joules| joules / inp.dt);
        probe.power_left = left;
        probe.power_right = right;
        probe.power_positive = left.max(0.0) + right.max(0.0);
        probe.power_negative = (-left).max(0.0) + (-right).max(0.0);
        probe.power_net = work(scale) / inp.dt;
        probe.power_scale = scale;
    }
    EngineWork {
        scale,
        crank_energy: (energy - work(scale)).max(0.0),
    }
}

/// The capacity-limited stop force is R-Q-vI/dt. Under intent the lever takes only its
/// rollback-opposing half after the reversal pause and cut, so useful launch motion is free.
/// No temporal release decision or command/speed blend participates in that balance.
fn apply_brake_stop_forces(
    q: &mut [f32; 2],
    tp: &TransmissionParams,
    fp: &ForceParams,
    st: &TransmissionState,
    inp: &TransmissionInput,
    service: bool,
) {
    // Opposite intent owns the complete stop/pause/cut. Only the launch is one-way.
    let one_way = !inp.brake
        && st.clutch != ClutchState::Out
        && inp.command.abs() >= DEAD
        && inp.supply.control_authority
        && inp.supply.task_rate > 0.0;
    let lever = st.park || one_way;
    for (i, qi) in q.iter_mut().enumerate() {
        if !lever && !service {
            continue;
        }
        let cap = brake_capacity_for_regime(tp, lever, service, inp.speeds[i]);
        let stop = inp.reactions[i] - *qi - inp.speeds[i] * fp.inertia / inp.dt;
        let brake = if one_way && !service {
            inp.command.signum() * (stop * inp.command.signum()).clamp(0.0, cap)
        } else {
            stop.clamp(-cap, cap)
        };
        *qi += brake;
    }
}

/// The Driver holds the crank ceiling with service brakes. Below it no brake acts; beyond
/// capacity the clutch's independent slip guard still bounds the crank. No gear change is paid.
#[expect(
    clippy::too_many_arguments,
    reason = "ceiling service reads construction, contact inertia, Driver state and the tick's crank/brake regime"
)]
fn apply_ceiling_brakes(
    q: &mut [f32; 2],
    tp: &TransmissionParams,
    fp: &ForceParams,
    st: &TransmissionState,
    inp: &TransmissionInput,
    crank: CrankTick,
    shifting: bool,
    service: bool,
) {
    if !inp.supply.control_authority
        || inp.supply.task_rate == 0.0
        || shifting
        || st.clutch == ClutchState::Out
        || service
        || st.park
        || crank.omega_e < tp.max_curve_rpm() * RPM_TO_RAD
    {
        return;
    }
    let mean = (inp.speeds[0] + inp.speeds[1]) * 0.5;
    let direction = if st.reverse { -1.0 } else { 1.0 };
    let next =
        mean + (q[0] + q[1] - inp.reactions[0] - inp.reactions[1]) * inp.dt / (2.0 * fp.inertia);
    let ceiling = tp.max_curve_rpm() * RPM_TO_RAD / crank.k;
    let overspeed = (next * direction - ceiling).max(0.0);
    let brake = (overspeed * fp.inertia / inp.dt).min(tp.brake_capacity_n);
    for force in q {
        *force -= direction * brake;
    }
}

/// Re-anchor only when the final belt speed implies a within-capacity clutch torque,
/// `tau = tau_free - (k*dir*m_next - omega_e)*J/dt`, inside both guards. Brakes and
/// steering act after the coupling predictor, so its earlier clamp cannot certify this snap.
/// An infeasible snap leaves the integrated slipping crank. The caller then caps its energy
/// by the source budget left after sprocket work; a feasible clutch cannot create energy.
/// `None` names an open clutch; without a Driver catch the crank may turn backward.
fn settle_crank(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    engaged: Option<f32>,
    next: [f32; 2],
    crank: CrankTick,
    #[cfg(feature = "bitprobe")] probe: &mut TransmissionProbe,
) {
    let CrankTick {
        omega_e,
        tau_free,
        k,
        dir,
        omega_floor,
        omega_over,
        dt,
    } = crank;
    if let Some(capacity) = engaged {
        let m_next = (next[0] + next[1]) / 2.0;
        let locked = k * dir * m_next;
        let tau_impl = tau_free - (locked - omega_e) * tp.engine_inertia / dt;
        #[cfg(feature = "bitprobe")]
        {
            let feasible =
                tau_impl.abs() <= capacity && locked >= omega_floor && locked <= omega_over;
            probe.reanchor_attempted = true;
            probe.reanchor_locked = locked;
            probe.reanchor_tau_impl = tau_impl;
            probe.reanchor_feasible = feasible;
        }
        if tau_impl.abs() <= capacity && locked >= omega_floor && locked <= omega_over {
            st.omega_e = locked;
        }
    }

    st.omega_e = st.omega_e.max(omega_floor).min(omega_over);
    #[cfg(feature = "bitprobe")]
    {
        probe.omega_end = st.omega_e;
    }
}

/// The crank's two guard points this tick (rad/s): the stall floor and the over-rev bound.
///
/// The game's damage model: the stall guard protects an engine that can idle, whose saturated
/// Driver-operated catch exceeds its motoring drag at the guard point. An engine short of that (no
/// Driver catch, or too little) has no positive floor: the crank runs down
/// under drag or follows the belt through the clutch, and the guard never pulls the belt to
/// hold a crank speed the engine cannot.
///
/// The symmetric over-rev slip guard is independent of gear choice.
fn crank_guards(
    tp: &TransmissionParams,
    inp: &TransmissionInput,
    st: &TransmissionState,
    propulsive: f32,
    tau_free: f32,
) -> (f32, f32) {
    let over = (tp.max_curve_rpm() + OVERREV_MARGIN_RPM) * RPM_TO_RAD;
    let floor = if st.clutch == ClutchState::Slip && tau_free > 0.0 {
        tp.launch_omega(propulsive)
    } else {
        idle_floor(tp, inp.supply, st.omega_e).unwrap_or(-over)
    };
    // Recovery cannot create crank energy: below the floor, spool before transmitting torque.
    let reachable = st.omega_e + tau_free * inp.dt / tp.engine_inertia;
    (floor.min(reachable), over)
}

/// The saturated catch protects only a nonnegative live crank. A cut's free crank cannot cross
/// zero under drag, so a backward vehicle landing does not revoke this admission in pricing.
fn idle_floor(tp: &TransmissionParams, supply: DriveSupply, omega_e: f32) -> Option<f32> {
    let point = (tp.engine.idle_rpm - engine_control().idle_guard_band_rpm.value) * RPM_TO_RAD;
    (omega_e >= 0.0 && idle_catch_capacity(tp, supply) > engine_drag(tp, point)).then_some(point)
}

/// The input half of the optional drivetrain trace, kept out of the physical solve.
#[cfg(feature = "bitprobe")]
fn input_probe(
    st: &TransmissionState,
    inp: &TransmissionInput,
    m: f32,
    d: f32,
) -> TransmissionProbe {
    TransmissionProbe {
        throttle: inp.throttle,
        steer: inp.steer,
        side_commands: inp.side_commands,
        speeds: inp.speeds,
        reactions: inp.reactions,
        dt: inp.dt,
        mean_speed: m,
        difference_speed: d,
        perceived_load_n: inp.perceived_load.total_n(),
        grip_limit_n: inp.perceived_load.grip_n,
        turn_moment_nm: inp.perceived_load.turn_moment_nm,
        omega_pre: st.omega_e,
        ..Default::default()
    }
}

/// The output half of the optional force trace. The operating point is signed shaft speed,
/// reduction and delivered mean-axis force; recording it cannot mutate the physical solve.
#[cfg(feature = "bitprobe")]
fn force_probe(
    tp: &TransmissionParams,
    inp: &TransmissionInput,
    (shaft, reduction, delivered, command): (f32, f32, f32, f32),
    clutch: ClutchState,
    probe: &mut TransmissionProbe,
) {
    probe.priced_force_n = price_driver_force(
        tp,
        inp.supply,
        shaft,
        reduction,
        command,
        inp.perceived_load.grip_n,
        clutch,
    );
    probe.delivered_force_n = delivered.min(inp.perceived_load.grip_n);
}

/// Consume one tick of the paid drive interruption.
fn advance_shift_window(st: &mut TransmissionState) -> bool {
    let shifting = st.shift_ticks > 0;
    st.shift_ticks = st.shift_ticks.saturating_sub(1);
    shifting
}

/// Advance the joint drivetrain one fixed tick and integrate both belt speeds: the m/d
/// superimposed model documented at module level.
pub fn step(
    fp: &ForceParams,
    tp: &TransmissionParams,
    st: &mut TransmissionState,
    inp: &TransmissionInput,
) -> TransmissionReport {
    fp.grip.expect("a drivetrain requires grip construction");
    let dt = inp.dt;
    let [vl, vr] = inp.speeds;
    let m = (vl + vr) / 2.0;
    let d = (vl - vr) / 2.0;
    #[cfg(feature = "bitprobe")]
    let mut bitprobe = input_probe(st, inp, m, d);

    swap_ladder_direction(tp, st, inp);
    update_park_latch(st, tp, inp);

    let ladder: &[f32] = if st.reverse {
        &tp.gears_rev
    } else {
        &tp.gears_fwd
    };
    let top = ladder.len() as u8;
    st.gear = st.gear.clamp(1, top);

    let DriverIntent {
        dir,
        propulsive,
        service,
    } = driver_intent(inp, st.reverse, st.park);

    let shaft = dir * m;
    let command = if service {
        0.0
    } else {
        inp.command.abs().min(1.0)
    };
    update_steer_detent(st, inp.steer);
    choose_gear(st, tp, fp, inp, command, shaft);
    let shifting = advance_shift_window(st);
    let g = ladder[(st.gear - 1) as usize];

    // The crank carries real inertia; launch slip limits transmitted torque while it spools.
    let omega_idle = tp.engine.idle_rpm * RPM_TO_RAD;
    let omega_e = st.omega_e;
    let k = g / tp.sprocket_radius;

    update_clutch_state(st, tp, inp, shifting, propulsive, shaft * k);
    let engaged = st.clutch != ClutchState::Out;

    let (tau_free, p_avail) = crank_free_torque(
        tp,
        inp,
        engaged,
        shifting,
        propulsive,
        m,
        k,
        omega_e,
        omega_idle,
        #[cfg(feature = "bitprobe")]
        &mut bitprobe,
    );

    // The engine force on the mean belt axis: the coupling's transmitted torque reflected
    // through the gear (in place of the old f_p + f_drag — drag reaches the belt only
    // through the coupling now). Declutched, the belt gets NOTHING from the engine.
    let i_m = 2.0 * fp.inertia;
    let (omega_floor, omega_over) = crank_guards(tp, inp, st, propulsive, tau_free);
    let crank = CrankTick {
        omega_e,
        tau_free,
        k,
        dir,
        omega_floor,
        omega_over,
        dt,
    };
    let tau_c = if engaged {
        clutch_coupling(
            crank,
            tp.engine_inertia,
            inp.supply.transmission_condition * tp.clutch_capacity,
            m,
            i_m,
            -(inp.reactions[0] + inp.reactions[1]),
            #[cfg(feature = "bitprobe")]
            &mut bitprobe,
        )
    } else {
        0.0
    };
    let f_c_pre_scale = k * dir * tau_c;

    let (f_s_pre_scale, lambda, j) = steering_force(tp, fp, st, inp, m, d, f_c_pre_scale);

    let work = engine_work(
        tp,
        fp,
        inp,
        crank,
        [
            (f_c_pre_scale + f_s_pre_scale) * 0.5 + lambda * j[0],
            (f_c_pre_scale - f_s_pre_scale) * 0.5 + lambda * j[1],
        ],
        #[cfg(feature = "bitprobe")]
        &mut bitprobe,
    );
    let power_scale = work.scale;
    let f_c = f_c_pre_scale * power_scale;
    let f_s = f_s_pre_scale * power_scale;
    let lambda = lambda * power_scale;

    // --- Integrate the crank: J·ω̇_e = τ_free − τ_c (the transmitted torque scaled by the
    // power gate exactly as the belt-side force was — one bookkeeping for both ends of the
    // clutch). The work allocation below also charges steering and caps any clutch re-anchor
    // at the crank energy left after both sprockets have done their work.
    let omega_integrated = omega_e + (tau_free - tau_c * power_scale) * dt / tp.engine_inertia;
    st.omega_e = omega_integrated;

    // --- Assemble per-side sprocket forces.
    let mut q = [
        f_c / 2.0 + f_s / 2.0 + lambda * j[0],
        f_c / 2.0 - f_s / 2.0 + lambda * j[1],
    ];

    apply_ceiling_brakes(&mut q, tp, fp, st, inp, crank, shifting, service);
    apply_brake_stop_forces(&mut q, tp, fp, st, inp, service);

    // --- Integrate both sides simultaneously: I·v̇ = Q − R (the reaction ALWAYS applies).
    let next = [
        inp.speeds[0] + (q[0] - inp.reactions[0]) / fp.inertia * dt,
        inp.speeds[1] + (q[1] - inp.reactions[1]) / fp.inertia * dt,
    ];

    settle_crank(
        st,
        tp,
        engaged.then_some(inp.supply.transmission_condition * tp.clutch_capacity),
        next,
        crank,
        #[cfg(feature = "bitprobe")]
        &mut bitprobe,
    );
    st.omega_e = st.omega_e.signum()
        * st.omega_e
            .abs()
            .min((2.0 * work.crank_energy / tp.engine_inertia).sqrt());
    #[cfg(feature = "bitprobe")]
    {
        let sample = (dir * m, g, dir * f_c, propulsive);
        force_probe(tp, inp, sample, st.clutch, &mut bitprobe);
        bitprobe.power_scale = power_scale;
        bitprobe.omega_end = st.omega_e;
    }
    // The tick's own values; the stages that take the probe filled what they compute.
    #[cfg(feature = "bitprobe")]
    let bitprobe = TransmissionProbe {
        direction: dir,
        shaft_speed: shaft,
        gear_reduction: g,
        k,
        omega_idle,
        omega_floor,
        shifting,
        engaged,
        i_mean: i_m,
        f_other: -(inp.reactions[0] + inp.reactions[1]),
        f_c_pre_scale,
        f_s_pre_scale,
        lambda,
        j,
        omega_integrated,
        forces: q,
        next_speeds: next,
        ..bitprobe
    };

    TransmissionReport {
        next_speeds: next,
        forces: q,
        // The crank state, post-tick — the report shows the same truth `readout` does.
        rpm: st.omega_e / RPM_TO_RAD,
        gear: st.gear,
        reverse: st.reverse,
        steer_step: st.steer_step,
        shifting,
        power_scale,
        power_available: p_avail,
        #[cfg(feature = "bitprobe")]
        bitprobe,
    }
}

#[cfg(test)]
#[path = "transmission/tests.rs"]
mod tests;
