//! The joint drivetrain integrates both belts and the crank from construction data.
//! `step` owns Driver gear/clutch/brake decisions, then solves clutch and steering torque,
//! work, belt acceleration and crank settlement. `TransmissionState` is its complete memory.
//! Gear decisions read what the Driver felt of the hull (`track::sense`: speed over the ground
//! and resistance), requested pedal and crank rpm. Forces are N, torques N m, belt speeds m/s
//! and crank speed rad/s. The design narrative lives in track-model architecture section 5a.
//!
//! The rejected `STEER_SERVO_BAND` proportional servo loses authority with vehicle scale;
//! the steering law instead solves its capacity-limited constraint semi-implicitly.

use bevy::ecs::error::BevyError;

use super::drive::DriveIntent;
use super::forces::GripParams;

#[cfg(feature = "bitprobe")]
use crate::bitprobe::TransmissionProbe;

/// The fixed simulation tick rate the shift countdown quantizes against (the module operates
/// on fixed 64 Hz ticks). Resolution: shift times quantize to this solver clock.
const TICK_HZ: f32 = 64.0;

/// StandIn: the slip guard substitutes for the missing over-rev engine outcome. Its width
/// can set descent speed; the owner must choose that outcome before this guard is replaced.
pub(crate) fn overrev_margin_rpm() -> f32 {
    engine_control().overrev_margin_rpm.value
}

/// Engine and Driver stand-ins, admitted before the drivetrain can be constructed.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EngineControl {
    governor_cut_rpm: crate::provenance::Labelled<f32>,
    overrev_margin_rpm: crate::provenance::Labelled<f32>,
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
            ("governor_cut_rpm", &self.governor_cut_rpm),
            ("overrev_margin_rpm", &self.overrev_margin_rpm),
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
                "{ENGINE_CONTROL_PATH}: {field} must be finite, positive and labelled StandIn; author the missing engine or Driver law"
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
    /// Declared do-not-exceed crank speed, independent of torque-curve sampling.
    pub crank_limit_rpm: f32,
    /// `(rpm, N·m)` authoring points, ascending rpm.
    pub torque_nm: Vec<(f32, f32)>,
}

/// Delivered-torque polynomial in local rpm, after splitting at curve and fuel-cut knots.
struct TorqueSpan {
    start_rpm: f64,
    width_rpm: f64,
    torque: [f64; 3],
}

/// What holds the mean track speed in a neutral turn, a construction fact. A box that forces only
/// the speed difference leaves the mean to the load (`Free`); one whose mean path is held in a
/// neutral turn ties the mean at rest (`Held`), within the two-sided brake capacity.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize)]
pub enum NeutralMean {
    Free,
    Held,
}

/// The joint transmission's declared data — everything vehicle spec, nothing tuned-to-feel.
/// Built from authored tables via [`TransmissionParams::from_authoring`].
#[derive(Clone, Debug)]
pub struct TransmissionParams {
    pub engine: EngineParams,
    /// Total reduction (engine rev per sprocket rev), in 1-based forward order.
    /// DERIVED from each authored gear speed, anchor rpm and sprocket radius.
    pub gears_fwd: Vec<f32>,
    pub gears_rev: Vec<f32>,
    /// Sprocket pitch radius (m): belt speed = engine speed / G × r_s.
    pub sprocket_radius: f32,
    /// Declared highest start gear and largest one-window gear step.
    pub launch_gears: u8,
    pub max_step: u8,
    /// Per FORWARD gear `(κ_tight, κ_wide)` where `κ = half_tread / R` — the L600 detents.
    /// Each ladder declares its own radii.
    pub steer_kappa: Vec<(f32, f32)>,
    pub reverse_steer_kappa: Vec<(f32, f32)>,
    /// Authored per-forward-gear `(R_tight, R_wide)` (m), shown by the HUD. The steering
    /// constraint and the Driver's turn price use [`Self::steer_kappa`].
    pub steer_radii_m: Vec<(f32, f32)>,
    pub reverse_steer_radii_m: Vec<(f32, f32)>,
    /// Steering-member force capacity PER OUTPUT (N). The steering member drives the two
    /// outputs DIFFERENTIALLY — each output's steering share is bounded by its own
    /// gearing/grip-scale cap (this datum), so the belt-difference axis `F_s` carries up to
    /// 2× it (each side sees `F_s/2`), and the L600 constraint force λ is bounded by
    /// `capacity / max|jᵢ|`. Treating this as the total difference-axis bound halves
    /// the available steering force.
    pub steer_capacity_n: f32,
    /// DERIVED neutral-turn belt-speed half-difference (m/s):
    /// `κ_tight(F1) × v(F1 @ governed)`, from construction's fixed-radius steering.
    pub neutral_d_full: f32,
    pub neutral_mean: NeutralMean,
    /// Inner-to-outer recirculation efficiency, a share in [0, 1].
    pub recirculation: f32,
    /// Per-side service/parking brake capacity at the sprocket (N).
    pub brake_capacity_n: f32,
    /// Static breakaway capacity multiplier for a latched, at-rest belt. Dynamic dissipation,
    /// service braking, and every moving slide use [`Self::brake_capacity_n`] unchanged.
    pub brake_static_factor: f32,
    /// Zero-throttle engine drag (compression braking): the MID-BAND anchor of the rising
    /// motoring-torque curve, as a fraction of peak torque. [`engine_drag`] equals
    /// `drag_fraction × peak` at `ω_mid = (idle + governed)/2` and grows linearly with
    /// crank speed from its Coulomb base. A downhill equilibrium exists
    /// only where this drag plus rolling resistance balances grade force before clutch guards
    /// cap transmitted drag.
    /// A drag TORQUE at the crank (design §3), never the negative half of rated power.
    pub drag_fraction: f32,
    /// Crank + flywheel + clutch rotational inertia J (kg·m²).
    pub engine_inertia: f32,
    /// Main clutch torque capacity (N·m), the engaged coupling's transmitted-torque clamp.
    pub clutch_capacity: f32,
    /// DERIVED torque-interruption ticks from authored `shift_secs`.
    pub shift_ticks: u8,
    /// DERIVED rpm of maximum fuel-governed torque: the launch and steering rev target.
    pub peak_torque_rpm: f32,
    /// Raw curve peak torque, the declared motoring-drag anchor.
    pub peak_torque_nm: f32,
}

/// The authored tables the params derive from — the shape the spec block and the sandbox lab
/// both author. Speeds, radii, and anchor rpm are the source data; reductions and curvatures
/// are derived here so two numbers that must agree stay one number.
pub struct TransmissionAuthoring<'a> {
    pub idle_rpm: f32,
    pub governed_rpm: f32,
    /// Declared do-not-exceed crank speed, independent of torque-curve sampling.
    pub crank_limit_rpm: f32,
    /// The rpm the per-gear speeds are anchored at.
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
    /// Per reverse gear `(R_tight, R_wide)` turn radii (m).
    pub reverse_steer_radii_m: &'a [(f32, f32)],
    /// Steering-member force capacity PER OUTPUT (N) — see
    /// [`TransmissionParams::steer_capacity_n`] for the convention (the difference axis
    /// carries 2× this).
    pub steer_capacity_n: f32,
    pub neutral_mean: NeutralMean,
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
        if a.reverse_steer_radii_m.len() != a.reverse_speeds_kmh.len() {
            return Err("transmission.steering.reverse_radii must have one (tight, wide) pair per reverse gear; author each reverse gear's radii".into());
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
                "steering.reverse_radii",
                a.reverse_steer_radii_m.iter().all(|(tight, wide)| {
                    tight.is_finite() && wide.is_finite() && *tight > 0.0 && *wide > 0.0
                }),
            ),
            (
                "engine.torque_curve",
                a.torque_nm.len() >= 2
                    && a.torque_nm.windows(2).all(|w| w[0].0 < w[1].0)
                    && a.torque_nm.iter().all(|(rpm, torque)| {
                        rpm.is_finite() && torque.is_finite() && *rpm > 0.0 && *torque >= 0.0
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
                    && a.brake_static_factor >= 1.0
                    && (a.brake_capacity_n * a.brake_static_factor).is_finite(),
            ),
            (
                "engine.drag_fraction",
                (0.0..=1.0).contains(&a.drag_fraction),
            ),
            (
                "engine.inertia_kgm2",
                a.engine_inertia_kgm2.is_finite() && a.engine_inertia_kgm2 > 0.0,
            ),
            (
                "engine.clutch_capacity_nm",
                a.clutch_capacity_nm.is_finite() && a.clutch_capacity_nm > 0.0,
            ),
            (
                "belt inertia (positive coupling divisor)",
                a.belt_inertia.is_finite() && a.belt_inertia > 0.0,
            ),
            (
                // Representation bound: the rounded countdown must fit its u8 state field.
                "gearbox.shift_secs",
                a.shift_secs.is_finite()
                    && a.shift_secs >= 0.0
                    && (a.shift_secs * TICK_HZ).round() <= f32::from(u8::MAX),
            ),
        ] {
            if !ok {
                return Err(format!("track.powertrain.transmission: invalid {field}").into());
            }
        }

        // The curve brackets operation; its end-clamped tail does not define the crank limit.
        if a.idle_rpm >= a.governed_rpm
            || a.governed_rpm > a.rated_rpm
            || a.torque_nm[0].0 >= a.governed_rpm
            || a.torque_nm[a.torque_nm.len() - 1].0 < a.governed_rpm
        {
            return Err("engine.torque_curve / governed_rpm / rated_rpm: bracket the governor with curve data and keep idle < governed <= rated".into());
        }
        if !a.crank_limit_rpm.is_finite()
            || a.crank_limit_rpm - a.governed_rpm < engine_control().governor_cut_rpm.value
        {
            return Err("engine.crank_limit_rpm: must be finite and at or beyond completed fuel cut (governed_rpm + governor_cut_rpm); raise the limit or lower governed_rpm so fuel is shut off at the limit".into());
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
        let peak_torque_nm = a
            .torque_nm
            .iter()
            .map(|point| point.1)
            .fold(0.0_f32, f32::max);
        let reverse_steer_kappa = a
            .reverse_steer_radii_m
            .iter()
            .map(|&(tight, wide)| (a.half_tread_m / tight, a.half_tread_m / wide))
            .collect();
        // Genuine neutral-steer scale: 1st gear tight curvature × 1st gear governed speed.
        let v1_governed = a.forward_speeds_kmh[0] / 3.6 * (a.governed_rpm / a.rated_rpm);
        let neutral_d_full = steer_kappa[0].0 * v1_governed;
        let mut derived = Self {
            engine: EngineParams {
                idle_rpm: a.idle_rpm,
                governed_rpm: a.governed_rpm,
                crank_limit_rpm: a.crank_limit_rpm,
                torque_nm: a.torque_nm.to_vec(),
            },
            gears_fwd,
            gears_rev,
            sprocket_radius: a.sprocket_radius_m,
            launch_gears: a.launch_gears,
            max_step: a.max_step,
            steer_kappa,
            reverse_steer_kappa,
            steer_radii_m: a.steer_radii_m.to_vec(),
            reverse_steer_radii_m: a.reverse_steer_radii_m.to_vec(),
            steer_capacity_n: a.steer_capacity_n,
            neutral_d_full,
            neutral_mean: a.neutral_mean,
            recirculation: a.recirculation,
            brake_capacity_n: a.brake_capacity_n,
            brake_static_factor: a.brake_static_factor,
            drag_fraction: a.drag_fraction,
            engine_inertia: a.engine_inertia_kgm2,
            clutch_capacity: a.clutch_capacity_nm,
            shift_ticks: (a.shift_secs * TICK_HZ).round() as u8,
            peak_torque_rpm: a.idle_rpm,
            peak_torque_nm,
        };
        derived.peak_torque_rpm = derived.delivered_peak_rpm();
        derived.check_derived(a.belt_inertia)?;
        Ok(derived)
    }

    /// The authored scalars can each be finite while their ratios, squared coupling terms or
    /// force capacities overflow/underflow. Refuse those coupled outputs before publication.
    fn check_derived(&self, belt_inertia: f32) -> Result<(), BevyError> {
        // Resolution: dt*max|dT/domega|/J <= 1 keeps a falling torque branch from reversing
        // a crank-speed perturbation in one explicit tick. Fuel cut makes each span quadratic;
        // its derivative is linear, so the largest magnitude occurs at a span endpoint.
        let slope = self
            .torque_spans()
            .into_iter()
            .map(|span| {
                let [_, b, c] = span.torque;
                b.abs().max((b + 2.0 * c * span.width_rpm).abs())
            })
            .fold(0.0_f64, f64::max);
        // The idle catch is another falling branch; construction losses add their viscous
        // slope when the pedal does not compensate them. Full task rate bounds the catch.
        let idle_slope = f64::from(self.torque_at(self.engine.idle_rpm))
            / f64::from(engine_control().idle_catch_droop_rpm.value);
        let drag_slope = f64::from(
            self.peak_torque_nm * self.drag_fraction * (1.0 - motoring_drag_base_share()),
        ) / f64::from((self.engine.idle_rpm + self.engine.governed_rpm) * 0.5);
        // Fuel cut removes reconstructed losses as well as net curve torque. Across any
        // pedal share, the falling gross branch is bounded by the net slope plus that loss cut.
        let cut_end = self.engine.governed_rpm + engine_control().governor_cut_rpm.value;
        let loss_cut_slope = f64::from(engine_drag(self, cut_end * RPM_TO_RAD).abs())
            / f64::from(engine_control().governor_cut_rpm.value);
        let minimum_inertia = ((slope + loss_cut_slope).max(idle_slope) + drag_slope)
            / (f64::from(TICK_HZ) * f64::from(RPM_TO_RAD));
        if !minimum_inertia.is_finite() || f64::from(self.engine_inertia) < minimum_inertia {
            return Err("engine.inertia_kgm2 / torque_curve: fixed-tick torque slope is unresolved (torque_curve with governor_cut_rpm, idle_catch_droop_rpm and motoring drag); correct the curve or crank inertia from construction data, widen the named assets/driving/engine.ron control widths, or increase solver tick resolution".into());
        }
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
        for &(tight, wide) in self.steer_kappa.iter().chain(&self.reverse_steer_kappa) {
            if ![tight, wide]
                .into_iter()
                .all(|value| value.is_finite() && value > 0.0 && (1.0 + value * value).is_finite())
            {
                return Err(fail("steering curvature"));
            }
        }
        // Full net torque bounds positive crank acceleration; a free tick can add T_peak*dt/J.
        let free_bound = (self.engine.crank_limit_rpm + overrev_margin_rpm()) * RPM_TO_RAD
            + self.peak_torque_nm / self.engine_inertia / TICK_HZ;
        let energy_bound = 0.5 * self.engine_inertia * free_bound * free_bound;
        if !energy_bound.is_finite() || energy_bound <= 0.0 {
            return Err(fail(
                "engine.inertia_kgm2 / engine.torque_curve crank energy",
            ));
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
                "neutral speed, governed speed, paired force capacity or arithmetic scale",
            ));
        }
        Ok(())
    }

    /// The active ladder owns its radii; equal gear indices do not imply equal construction.
    pub(crate) fn steering_radii(&self, reverse: bool, gear: u8) -> (f32, f32) {
        let table = if reverse {
            &self.reverse_steer_radii_m
        } else {
            &self.steer_radii_m
        };
        *table
            .get(
                usize::from(gear)
                    .checked_sub(1)
                    .expect("an active gear is one-based"),
            )
            .expect("an active gear has an admitted steering row on its own ladder")
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
        self.raw_torque_at(rpm) * self.fuel_cut(rpm)
    }

    fn raw_torque_at(&self, rpm: f32) -> f32 {
        let pts = &self.engine.torque_nm;
        if rpm <= pts[0].0 {
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
        }
    }

    /// Split at every slope change of the raw curve or fuel ramp. In local rpm x,
    /// delivered torque is a + b*x + c*x^2; the end-clamped raw tail needs no extra knots.
    fn torque_spans(&self) -> Vec<TorqueSpan> {
        let end = self.engine.governed_rpm + engine_control().governor_cut_rpm.value;
        let mut points: Vec<_> = self
            .engine
            .torque_nm
            .iter()
            .map(|&(rpm, _)| rpm)
            .chain([0.0, self.engine.governed_rpm, end])
            .filter(|rpm| *rpm <= end)
            .collect();
        points.sort_by(f32::total_cmp);
        points.dedup();
        points
            .windows(2)
            .map(|pair| {
                let [start, end] = [pair[0], pair[1]];
                let width = f64::from(end) - f64::from(start);
                let raw = f64::from(self.raw_torque_at(start));
                let raw_slope = (f64::from(self.raw_torque_at(end)) - raw) / width;
                let cut = f64::from(self.fuel_cut(start));
                let cut_slope = (f64::from(self.fuel_cut(end)) - cut) / width;
                TorqueSpan {
                    start_rpm: f64::from(start),
                    width_rpm: width,
                    torque: [
                        raw * cut,
                        raw_slope * cut + raw * cut_slope,
                        raw_slope * cut_slope,
                    ],
                }
            })
            .collect()
    }

    /// A fuel-ramp span can peak between knots. Check its endpoints and quadratic vertex;
    /// ties retain the lowest rpm at or above idle, without requesting unavailable fuel.
    fn delivered_peak_rpm(&self) -> f32 {
        let idle = f64::from(self.engine.idle_rpm);
        self.torque_spans()
            .into_iter()
            .filter(|span| span.start_rpm + span.width_rpm >= idle)
            .fold(
                (idle, f64::from(self.torque_at(self.engine.idle_rpm))),
                |best, span| {
                    let lo = (idle - span.start_rpm).clamp(0.0, span.width_rpm);
                    let [a, b, c] = span.torque;
                    let vertex = if c < 0.0 {
                        (-b / (2.0 * c)).clamp(lo, span.width_rpm)
                    } else {
                        lo
                    };
                    [lo, span.width_rpm, vertex]
                        .into_iter()
                        .fold(best, |best, x| {
                            let torque = a + x * (b + c * x);
                            if torque > best.1 {
                                (span.start_rpm + x, torque)
                            } else {
                                best
                            }
                        })
                },
            )
            .0 as f32
    }

    fn fuel_cut(&self, rpm: f32) -> f32 {
        (1.0 - (rpm - self.engine.governed_rpm) / engine_control().governor_cut_rpm.value)
            .clamp(0.0, 1.0)
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
            * (self.engine.crank_limit_rpm + overrev_margin_rpm())
            * RPM_TO_RAD
            * TICK_HZ
            * ratio
            / self.sprocket_radius
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
    /// Engine crank speed ω_e (rad/s). Initialized explicitly from
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
    /// Engine rpm from crank state ω_e, including
    /// sub-idle lugging and, in the game's damage model, a crank at rest when combustion
    /// cannot sustain idle (`crank_guards`).
    pub rpm: f32,
    /// The engaged gear as a display label: `F1..Fn` forward, `R1..Rn` reverse.
    pub gear_label: String,
}

/// Read the drivetrain operating point THROUGH THE LAW: the engaged gear from
/// [`TransmissionState`] against the active ladder, and the engine rpm from the CRANK STATE
/// ω_e: the crank slips against the shaft at
/// launch, rev-matches through shifts, and can turn backward when back-driven; the display shows
/// exactly that state. Pure (no ECS), so the HUD and any legend share one implementation.
pub fn readout(st: &TransmissionState, tp: &TransmissionParams) -> DriveReadout {
    let ladder: &[f32] = if st.reverse {
        &tp.gears_rev
    } else {
        &tp.gears_fwd
    };
    assert!(
        (1..=ladder.len() as u8).contains(&st.gear),
        "an active gear belongs to its admitted ladder"
    );
    let gear = st.gear;
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
    /// The game's damage model: only staffed controls can serve the available Drive task.
    /// The server's control work and the HUD consume this same rate.
    pub(crate) fn served_task_rate(self) -> f32 {
        if self.control_authority {
            self.task_rate
        } else {
            0.0
        }
    }

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
#[derive(Clone, Copy)]
pub struct TransmissionInput {
    /// The game's damage model: what the crew, engine and transmission each supply this tick.
    pub supply: DriveSupply,
    /// Requested pedal before shaping; gear choices must not read the actuator ramp.
    pub intent: DriveIntent,
    pub throttle: f32,
    pub steer: f32,
    pub side_commands: [f32; 2],
    pub speeds: [f32; 2],
    /// The ground reaction each belt meets if it holds its speed through the tick, the hull's own
    /// motion over the tick included (N).
    pub reactions: [f32; 2],
    /// Each reaction's slope against its own belt's speed over the tick (N s/m); the belt solves
    /// against it implicitly.
    pub reaction_slopes: [f32; 2],
    /// What the Driver feels of the hull this tick; `None` until the ledger has closed a tick.
    pub sensed: Option<super::sense::Sensed>,
    pub dt: f32,
}

/// What the joint solve produced: the integrated next belt speeds, the per-side sprocket
/// forces actually applied (telemetry — the `engine` slot of the harness rows), and the
/// operating point for HUD/legend display.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransmissionReport {
    pub next_speeds: [f32; 2],
    /// Each side's capacity-limited brake supplied the exact stop force: a held constraint.
    pub held: [bool; 2],
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
    let pedal = (command / engine_control().drag_throttle_release.value).clamp(0.0, 1.0);
    // Cubic smoothstep has zero endpoint slopes: 3p² - 2p³.
    let released = pedal * pedal * (3.0 - 2.0 * pedal);
    let rpm = omega / RPM_TO_RAD;
    let curve = tp.torque_at(rpm);
    let induced =
        supply.served_task_rate() * (command * curve + released * drag * tp.fuel_cut(rpm));
    let idle_capacity = idle_catch_capacity(tp, supply);
    let catch = (idle_capacity * (tp.engine.idle_rpm * RPM_TO_RAD - omega)
        / (engine_control().idle_catch_droop_rpm.value * RPM_TO_RAD))
        .clamp(0.0, idle_capacity)
        .min(curve + drag);
    // Both requests move one rack; the catch supplies only what the command has not supplied.
    [induced, (catch - induced).max(0.0), drag]
}

/// The game's driving and damage model: the Driver's idle catch is work by the Drive task,
/// consuming the served task throughput, like commanded fueling.
fn idle_catch_capacity(tp: &TransmissionParams, supply: DriveSupply) -> f32 {
    supply.served_task_rate() * tp.torque_at(tp.engine.idle_rpm)
}

/// Instrument for the accepted Driver law: price the crank at the shaft speed, or the
/// commanded launch speed while slipping, bounded by the clutch. A cut cannot reverse a free
/// crank, so pricing uses the saturated catch floor. This shared price does not select a gear
/// or change delivery.
#[cfg(any(test, feature = "bitprobe"))]
pub(crate) fn price_driver_force(
    tp: &TransmissionParams,
    supply: DriveSupply,
    shaft: f32,
    gear: f32,
    command: f32,
    clutch: ClutchState,
) -> f32 {
    let capacity = supply.transmission_condition * tp.clutch_capacity * gear / tp.sprocket_radius;
    free_driver_force(tp, supply, shaft, gear, command, clutch).clamp(-capacity, capacity)
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

/// Required driving weight and belt inertia. Support-only contact parameters cannot enter
/// the Driver without grip construction.
#[derive(Clone, Copy)]
pub(crate) struct DriveConstruction {
    grip: GripParams,
    pub(crate) belt_inertia: f32,
}

impl DriveConstruction {
    pub(crate) fn new(grip: GripParams, belt_inertia: f32) -> Self {
        Self { grip, belt_inertia }
    }

    pub(crate) fn weight_n(self) -> f32 {
        2.0 * self.grip.side_weight
    }

    pub(crate) fn grip(self) -> GripParams {
        self.grip
    }
}

/// Coupled translation mass (kg): nominal hull mass and both belt inertias.
pub(crate) fn coupled_mass(fp: &DriveConstruction) -> f32 {
    let hull = fp.weight_n() / super::derive::G;
    hull + 2.0 * fp.belt_inertia
}

/// The Driver's force comparison resolution: cancellation in the crank acceleration follows
/// `J·ω/dt` reflected through the gearbox, independently of weight.
#[derive(bevy::prelude::Resource, Clone, Copy)]
pub(crate) struct DriverResolution {
    pub(crate) crank_roundoff_share: f32,
}

impl DriverResolution {
    /// The margin `M` a gear's spare must exceed (N).
    pub(crate) fn margin_n(&self, tp: &TransmissionParams) -> f32 {
        self.crank_roundoff_share * tp.roundoff_force_scale()
    }

    pub(crate) fn load() -> Self {
        static DATA: std::sync::LazyLock<DriverResolution> = std::sync::LazyLock::new(|| {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Source {
                crank_roundoff_share: crate::provenance::Labelled<f32>,
            }
            const PATH: &str = "assets/driving/driver.ron";
            let source: Source = crate::provenance::from_shipped_str(
                PATH,
                include_str!("../../assets/driving/driver.ron"),
            )
            .unwrap_or_else(|error| panic!("{error}"));
            let share = source.crank_roundoff_share;
            assert!(
                matches!(share.label, crate::provenance::Provenance::Resolution(_))
                    && share.value.is_finite()
                    && share.value > 0.0,
                "{PATH}: crank_roundoff_share must be finite, positive and Resolution; remeasure the same-instant crank residual"
            );
            DriverResolution {
                crank_roundoff_share: share.value,
            }
        });
        *DATA
    }
}

fn force_margin(tp: &TransmissionParams) -> f32 {
    DriverResolution::load().margin_n(tp)
}

/// A coupled crank accelerates with the vehicle: J*k^2 adds reflected mass. Solving
/// M*a = F-L and F = k*tau-J*k^2*a gives this force against load `load` (N). Slip holds its
/// launch rpm, so its crank inertia loads nothing.
#[expect(
    clippy::too_many_arguments,
    reason = "the force law reads construction, supply, shaft speed, ratio, command, load and clutch"
)]
fn gear_force(
    tp: &TransmissionParams,
    fp: &DriveConstruction,
    supply: DriveSupply,
    shaft: f32,
    ratio: f32,
    command: f32,
    load: f32,
    clutch: ClutchState,
) -> f32 {
    let force = free_driver_force(tp, supply, shaft, ratio, command, clutch);
    let force = if clutch == ClutchState::Slip {
        force
    } else {
        let reflected = tp.engine_inertia * (ratio / tp.sprocket_radius).powi(2) / coupled_mass(fp);
        (force + reflected * load) / (1.0 + reflected)
    };
    let capacity = supply.transmission_condition * tp.clutch_capacity * ratio / tp.sprocket_radius;
    force.clamp(-capacity, capacity)
}

/// The detent's steering ratio `κ` for `gear` on the engaged ladder: 0 straight.
fn detent_kappa(tp: &TransmissionParams, st: &TransmissionState, gear: u8) -> f32 {
    let table = if st.reverse {
        &tp.reverse_steer_kappa
    } else {
        &tp.steer_kappa
    };
    let (tight, wide) = table[usize::from(gear - 1)];
    match st.steer_step {
        0 => 0.0,
        1 => wide,
        2 => tight,
        _ => panic!("the transmission admits only straight, wide and tight detents"),
    }
}

/// One Driver decision on what it felt, at the requested pedal `command`. Speed `v` and
/// resistance `R` read along the engaged ladder, `v` floored at rest. A gear carries `R` plus the
/// turn's share in its own detent radius, `s·κ·ΔR` (the steering tie's load on the mean axis,
/// [`BeltTick::tied_mean`]), and *holds* when its spare over that load exceeds M. Below its launch
/// speed a gear is priced as a launch slipping at the launch rpm, so every gear is priced alike:
/// priced as it would turn locked to the hull, a gear crawling there (or riding belts that slip
/// on soft ground) reads a lug, and a higher gear's launch from a stopped landing outbids it,
/// cut after cut. A candidate lands at `v − R·t_cut/M_c`; the current gear pays no cut.
/// 1. Clutch out at hull rest: the highest launch gear that holds, else first; free.
/// 2. The current gear does not hold, judged at no more than its torque-peak speed: the highest
///    lower gear within `max_step` that holds where it lands; with none the gear stays.
/// 3. The current gear holds, straight: the highest higher gear within `max_step` that holds
///    where it lands and has more spare there than the current gear has now.
///
/// Clauses 2 and 3 pay the cut. A cut in flight, no pull asked or fuel that cannot beat drag
/// choose nothing.
fn choose_gear(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    fp: &DriveConstruction,
    inp: &TransmissionInput,
    command: f32,
    sensed: super::sense::Sensed,
) {
    if st.shift_ticks != 0 || command <= 0.0 {
        return;
    }
    let [fuel, catch, drag] = engine_torques(tp, inp.supply, st.omega_e, command);
    if fuel + catch <= drag.abs() {
        return;
    }
    let (ladder, dir) = if st.reverse {
        (&tp.gears_rev, -1.0)
    } else {
        (&tp.gears_fwd, 1.0)
    };
    let top = ladder.len() as u8;
    let speed = (dir * sensed.speed).max(0.0);
    let resistance = dir * sensed.resistance;
    let margin = force_margin(tp);
    let spare = |gear: u8, at: f32| {
        let load =
            resistance + branch(inp.steer) * detent_kappa(tp, st, gear) * sensed.push_difference;
        let ratio = ladder[usize::from(gear - 1)];
        let clutch = if at * ratio / tp.sprocket_radius < tp.launch_omega(command) {
            ClutchState::Slip
        } else {
            ClutchState::In
        };
        gear_force(tp, fp, inp.supply, at, ratio, command, load, clutch) - load
    };
    if st.clutch == ClutchState::Out && speed < PARK_ENGAGE_SPEED {
        st.gear = (1..=tp.launch_gears.min(top))
            .rev()
            .find(|&gear| spare(gear, speed) > margin)
            .unwrap_or(1); // No launch gear holds: first gear is the most the ladder has.
        return;
    }
    let landing = speed - resistance * f32::from(tp.shift_ticks) / TICK_HZ / coupled_mass(fp);
    let peak =
        tp.peak_torque_rpm * RPM_TO_RAD * tp.sprocket_radius / ladder[usize::from(st.gear - 1)];
    let chosen = if spare(st.gear, speed.min(peak)) > margin {
        let now = spare(st.gear, speed);
        // A detent turn's radius belongs to its gear: no upshift changes it.
        (st.steer_step == 0)
            .then(|| {
                (st.gear + 1..=st.gear.saturating_add(tp.max_step).min(top))
                    .rev()
                    .find(|&gear| {
                        let landed = spare(gear, landing);
                        landed > margin && landed > now + margin
                    })
            })
            .flatten()
    } else {
        (st.gear.saturating_sub(tp.max_step).max(1)..st.gear)
            .rev()
            .find(|&gear| spare(gear, landing) > margin)
    };
    // No gear qualifies: the current one stays.
    if let Some(gear) = chosen {
        st.gear = gear;
        st.shift_ticks = tp.shift_ticks;
    }
}

/// Both belts over one tick. A belt lands at `speed + (force − reaction)·mobility`, its ground
/// reaction solved implicitly: the mobility `dt/(I + G·dt)` carries the reaction's slope `G`
/// against belt speed beside the belt's inertia `I`, so a stiff ground cannot ring a belt lighter
/// than the hull at the fixed step. Every force this tick solves lands through it.
#[derive(Clone, Copy)]
struct BeltTick {
    speeds: [f32; 2],
    reactions: [f32; 2],
    /// Speed per newton of net force over the tick (s/kg).
    mobility: [f32; 2],
}

impl BeltTick {
    fn new(fp: &DriveConstruction, inp: &TransmissionInput) -> Self {
        let mobility = [0, 1].map(|i| {
            let inertia = fp.belt_inertia + inp.reaction_slopes[i] * inp.dt;
            assert!(
                inertia.is_finite() && inertia > 0.0,
                "a ground reaction's slope never cancels its belt's inertia within a tick"
            );
            inp.dt / inertia
        });
        Self {
            speeds: inp.speeds,
            reactions: inp.reactions,
            mobility,
        }
    }

    /// Side `i`'s speed after the tick under sprocket force `force` (N).
    fn land(&self, i: usize, force: f32) -> f32 {
        self.speeds[i] + (force - self.reactions[i]) * self.mobility[i]
    }

    /// The force along per-side weights `j` that, added to sprocket forces `base`, lands
    /// `j·speeds` at `target`.
    fn constraint_force(&self, j: [f32; 2], base: [f32; 2], target: f32) -> f32 {
        let free = j[0] * self.land(0, base[0]) + j[1] * self.land(1, base[1]) - target;
        -free / (j[0] * j[0] * self.mobility[0] + j[1] * j[1] * self.mobility[1])
    }

    /// The mean axis as the tie `j·v = target` holds the belts under an even clutch split, in the
    /// form [`clutch_coupling`] reads: its inertia (kg) and the force on it besides the clutch's
    /// (N), so that `m_next = m + (f_c + f_other)·dt/i_m`. DERIVED by eliminating the tie's force:
    /// the tied mean lands affine in `f_c`, and `per_newton` is that line's slope. With equal
    /// mobilities `dt/I` the geared tie gives `i_m = 2I·(1 + e²)` and
    /// `f_other = −(1 + e)·R_l − (1 − e)·R_r + e·(d − e·m)·2I/dt`.
    fn tied_mean(&self, j: [f32; 2], target: f32, dt: f32) -> (f32, f32) {
        let [w0, w1] = self.mobility;
        let tie = self.constraint_force(j, [0.0; 2], target);
        let landed = (self.land(0, tie * j[0]) + self.land(1, tie * j[1])) / 2.0;
        let pull = j[0] * w0 + j[1] * w1;
        let per_newton =
            (w0 + w1) / 4.0 - pull * pull / (4.0 * (j[0] * j[0] * w0 + j[1] * j[1] * w1));
        let mean = (self.speeds[0] + self.speeds[1]) / 2.0;
        (dt / per_newton, (landed - mean) / per_newton)
    }

    /// The even-split force (N) that lands the mean at rest while the tie `j·v = target`'s own
    /// force lands the tie, the two solved together on top of sprocket forces `base`. With equal
    /// mobilities an even split leaves an antisymmetric tie alone and this is the plain stop
    /// force; with unequal ones the split moves the tie, and its force answers.
    fn held_mean_force(&self, j: [f32; 2], target: f32, base: [f32; 2]) -> f32 {
        let even = [0.5; 2];
        let landed = [self.land(0, base[0]), self.land(1, base[1])];
        let dot = |a: [f32; 2], b: [f32; 2]| a[0] * b[0] + a[1] * b[1];
        let weigh = |a: [f32; 2], b: [f32; 2]| {
            a[0] * self.mobility[0] * b[0] + a[1] * self.mobility[1] * b[1]
        };
        let (jj, je, ee) = (weigh(j, j), weigh(j, even), weigh(even, even));
        let (tie_gap, mean_gap) = (target - dot(j, landed), -dot(even, landed));
        (jj * mean_gap - je * tie_gap) / (jj * ee - je * je)
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
/// The caller passes `m`'s inertia and `F_other` as the steering member's tie and each belt's
/// implicit ground reaction land the belts ([`BeltTick::tied_mean`]). Brakes, a steering member
/// past its capacity and the work scale act later, so `settle_crank` independently checks the
/// final lock's feasibility rather than trusting this predictor.
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
    command: f32,
    brake: bool,
) {
    let opposite = command.abs() >= DEAD && (command < 0.0) != st.reverse;
    if !opposite || brake {
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
        st.reverse = command < 0.0;
        st.gear = 1;
        st.shift_ticks = tp.shift_ticks;
    }
}

/// Driver decisions for one mechanical tick. The physical laws also read served task
/// throughput when turning these requests into fuel and mechanical capacity.
struct Controls {
    input: TransmissionInput,
    command: f32,
    dir: f32,
    propulsive: f32,
    service: bool,
    one_way: bool,
    ceiling: bool,
    shifting: bool,
    /// A held mean path ties the mean belt speed at rest during this tick's neutral pivot.
    mean_hold: bool,
}

/// The game's driving and damage model: policy reads the requested action, staffing and served work.
/// Absence releases fuel and steering, retains gear and park, and lets the clutch spring In
/// after any paid cut. Staffed input works the brakes; a phase hold also works them unstaffed.
/// Served task throughput works gear and clutch.
fn driver(
    fp: &DriveConstruction,
    tp: &TransmissionParams,
    st: &mut TransmissionState,
    inp: &TransmissionInput,
) -> Controls {
    let mut input = *inp;
    let staffed = input.supply.control_authority;
    if !staffed {
        input.throttle = 0.0;
    }
    let (command, brake) = match input.intent {
        DriveIntent::Axes(axes) => (if staffed { axes.throttle } else { 0.0 }, false),
        DriveIntent::Brake => (0.0, true),
    };
    let work = input.supply.served_task_rate() > 0.0;
    if work {
        swap_ladder_direction(tp, st, &input, command, brake);
    } else {
        input.steer = 0.0;
    }
    if staffed {
        update_park_latch(st, tp, &input, command, brake);
    }
    let dir = if st.reverse { -1.0 } else { 1.0 };
    let mean = (input.speeds[0] + input.speeds[1]) * 0.5;
    let ladder = if st.reverse {
        &tp.gears_rev
    } else {
        &tp.gears_fwd
    };
    assert!(
        (1..=ladder.len() as u8).contains(&st.gear),
        "a direction swap selects first gear on its admitted ladder"
    );
    let opposing = command * dir < -DEAD;
    let against_motion = command.abs() >= DEAD
        && input
            .speeds
            .iter()
            .any(|v| v * command.signum() < -PARK_ENGAGE_SPEED);
    let service = against_motion || ((brake || opposing) && !st.park);
    let propulsive = if brake || opposing {
        0.0
    } else {
        (input.throttle * dir).clamp(0.0, 1.0)
    };
    update_steer_detent(st, input.steer);
    // Before the ledger closes its first tick the Driver has felt nothing, so it chooses nothing.
    // A request against the engaged ladder asks no pull of it: it brakes, then waits for the swap.
    if work && let Some(sensed) = input.sensed {
        let price_command = if service || opposing {
            0.0
        } else {
            command.abs().min(1.0)
        };
        choose_gear(st, tp, fp, &input, price_command, sensed);
    }
    let shifting = advance_shift_window(st);
    if work {
        let ratio = ladder[usize::from(st.gear - 1)] / tp.sprocket_radius;
        // A Driver that has felt nothing has chosen no gear, so it engages none from rest.
        let unchosen = input.sensed.is_none() && mean.abs() < PARK_ENGAGE_SPEED;
        update_clutch_state(
            st,
            tp,
            &input,
            shifting || unchosen,
            propulsive,
            dir * mean * ratio,
            brake,
        );
    } else {
        // Without task work the clutch springs In after the already-paid mechanical cut.
        st.clutch = if shifting {
            ClutchState::Out
        } else {
            ClutchState::In
        };
    }
    let one_way = !brake && st.clutch != ClutchState::Out && command.abs() >= DEAD && work;
    // The pivot's own clutch clause, not every open clutch: a shift window is no pivot. A tick
    // whose per-side brakes act (park lever, service or Brake intent) keeps their capacity whole.
    let mean_hold = tp.neutral_mean == NeutralMean::Held
        && propulsive < DEAD
        && neutral_pivot(&input, mean)
        && !st.park
        && !service;
    Controls {
        input,
        command,
        dir,
        propulsive,
        service,
        one_way,
        ceiling: staffed,
        shifting,
        mean_hold,
    }
}

/// Resolve the detent before gear choice and steering: both must read this tick's detent,
/// or a newly engaged turn could permit an upshift that changes its radius.
fn update_steer_detent(st: &mut TransmissionState, steer: f32) {
    let a = steer.abs();
    let feel = super::drive::drive_feel();
    st.steer_step = if a >= feel.tight_on.0 {
        2
    } else {
        u8::from(a >= feel.wide_on.0)
    };
}

/// At rest the lever is on. Drive lets it carry only rollback; steering releases it for
/// a neutral turn. A released tank below first gear's idle speed stops and parks.
fn update_park_latch(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    inp: &TransmissionInput,
    command: f32,
    brake: bool,
) {
    let speed = inp.speeds[0].abs().max(inp.speeds[1].abs());
    let requested = command.abs() >= DEAD;
    let steering = inp.steer.abs() >= DEAD;
    let first = if st.reverse {
        tp.gears_rev[0]
    } else {
        tp.gears_fwd[0]
    };
    let stop_speed = tp.engine.idle_rpm * RPM_TO_RAD * tp.sprocket_radius / first;
    if !brake && !requested && steering {
        st.park = false;
    } else if speed < PARK_ENGAGE_SPEED || (!requested && !steering && speed < stop_speed) {
        st.park = true;
    } else if requested && command * (inp.speeds[0] + inp.speeds[1]) > 0.0 {
        st.park = false;
    }
}

/// The neutral steering member needs the main clutch open even while its counter-rotating
/// belts carry unequal speeds. Mean-axis drift must not close it and remove its rev-match fuel.
fn neutral_turn(inp: &TransmissionInput, mean: f32) -> bool {
    inp.throttle.abs() < driver_actuation().neutral_throttle.value
        && mean.abs() < driver_actuation().neutral_speed_m_s.value
}

/// Steering applied in the neutral regime: the pivot that opens the main clutch.
fn neutral_pivot(inp: &TransmissionInput, mean: f32) -> bool {
    inp.steer.abs() >= DEAD && neutral_turn(inp, mean)
}

/// The Driver holding it `open` (a shift window, or no gear chosen yet), a two-sided parked
/// hold and neutral turns open the clutch. Intent at rest below idle starts Slip regardless of
/// the prior regime, retaining it until the shaft reaches its commanded launch speed.
fn update_clutch_state(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    inp: &TransmissionInput,
    open: bool,
    propulsive: f32,
    shaft_omega: f32,
    brake: bool,
) {
    let mean = (inp.speeds[0] + inp.speeds[1]) * 0.5;
    st.clutch = if open
        || (st.park && (brake || propulsive < DEAD))
        || (propulsive < DEAD && (mean.abs() < PARK_ENGAGE_SPEED || neutral_pivot(inp, mean)))
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
/// With the clutch open, a paid window or active steering requests a proportional rev
/// governor. `DriveSupply::served_task_rate` gates throughput for all fueling. A paid window
/// targets `|m|*k` in the selected landing gear, so the clutch re-engages near-synchronous; without a window that target is idle. Steering targets
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
    } else if inp.steer.abs() >= DEAD || shifting {
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
        probe.u_fuel = u_fuel * supply.served_task_rate();
        probe.rpm = rpm;
        probe.tau_idle = tau_idle;
        probe.tau_induced = tau_ind;
        probe.tau_drag = tau_drag;
        probe.tau_free = tau_free;
        probe.power_available = p_avail;
    }
    (tau_free, p_avail)
}

/// The geared steering member's signed ratio `s·κ` of belt half-difference to |mean| this tick;
/// zero when straight.
///
/// κ is indexed by the active gear on its own forward or reverse ladder; `d` follows the
/// steer SIGN regardless of travel direction — the superimposed steering shaft is independent of
/// the gear's direction, historically and mechanically. The detent (`st.steer_step`) was updated
/// for this tick before gear selection: one detent truth per tick.
fn steer_ratio(tp: &TransmissionParams, st: &TransmissionState, inp: &TransmissionInput) -> f32 {
    branch(inp.steer) * detent_kappa(tp, st, st.gear)
}

/// The sign of `x`, zero at zero: the |m| branch a geared steering solve linearizes on.
fn branch(x: f32) -> f32 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// The steering member's tie this tick on the |m| branch `side`: per-belt weights `j` and the
/// value `j·v` it lands the belts on. The neutral servo lands the half-difference on
/// construction's `neutral_d_full` share of the steer; a geared detent lands `d − e·m` at zero
/// with `e = s·κ·side` (straight: `e = 0`, the locked differential). The clutch solve and the
/// steering solve read this one tie.
fn steering_tie(
    tp: &TransmissionParams,
    st: &TransmissionState,
    inp: &TransmissionInput,
    m: f32,
    side: f32,
) -> ([f32; 2], f32) {
    if neutral_turn(inp, m) {
        ([0.5, -0.5], inp.steer * tp.neutral_d_full)
    } else {
        let e = steer_ratio(tp, st, inp) * side;
        ([(1.0 - e) / 2.0, -(1.0 + e) / 2.0], 0.0)
    }
}

/// The steering member's contribution to this tick: the difference-axis force `F_s`, the L600
/// constraint force λ, and λ's per-output Jacobian `j`. Returned as `(f_s, lambda, j)`; a tick
/// that commands neither leaves all three at zero.
///
/// The servo is a capacity-limited KINEMATIC one, semi-implicit like the brakes and λ: the `F_s`
/// that lands `d` exactly on target after this tick's integration (each side landing through
/// [`BeltTick`], `F_s/2` on the left and `−F_s/2` on the right), clamped to the per-output
/// convention's bound. Exact inside capacity, honest slip beyond it — and no proportional band:
/// a proportional band permits steady droop when the target lies inside that band.
/// The difference-axis bound is twice the per-output capacity because each output receives
/// half of `F_s`; see [`TransmissionParams::steer_capacity_n`]. `even_stop` is the capacity of
/// the even mean-axis stop that acts after this solve ([`even_stop_capacity`]): its split moves
/// the difference when the belts' mobilities differ, so the servo lands the difference with that
/// stop's force solved alongside it, and the stop carries the mean evenly as it does on equal
/// mobilities.
fn steering_force(
    tp: &TransmissionParams,
    belts: &BeltTick,
    st: &TransmissionState,
    inp: &TransmissionInput,
    m: f32,
    f_c: f32,
    even_stop: Option<f32>,
) -> (f32, f32, [f32; 2]) {
    let mut f_s = 0.0;
    let mut lambda = 0.0;
    let mut j = [0.0f32; 2];
    let steer_capacity = inp.supply.transmission_condition * tp.steer_capacity_n;
    let clutch = [f_c / 2.0; 2];
    let b0 = branch(m);
    if neutral_turn(inp, m) {
        let (tie, target) = steering_tie(tp, st, inp, m, b0);
        let base = even_stop.map_or(clutch, |cap| {
            let held = belts.held_mean_force(tie, target, clutch).clamp(-cap, cap);
            clutch.map(|force| force + held / 2.0)
        });
        f_s = belts
            .constraint_force(tie, base, target)
            .clamp(-2.0 * steer_capacity, 2.0 * steer_capacity);
    } else {
        // The geared-radius constraint g = d − s·κ·|m| = 0, solved semi-implicitly:
        // λ is the force that lands g at zero after this tick's integration, clamped
        // so each output's share stays inside the per-output capacity (beyond it the
        // constraint slips). Zero ideal work: Q_c·v = λ·g, which the solve drives to
        // zero.
        //
        // One |m| branch of the solve: on branch b, |m| linearizes as `b·m`, so
        // `g = jl·v_L + jr·v_R` with the branch's Jacobian. Returns λ (per-output
        // capacity clamp — straight-gear `max|j| = 1/2` gives the same 2× bound
        // the servo uses), the Jacobian, and the pre-brake landing m. Brakes act later.
        let solve = |side: f32| -> (f32, [f32; 2], f32) {
            let (tie, target) = steering_tie(tp, st, inp, m, side);
            let lambda_max = steer_capacity / tie[0].abs().max(tie[1].abs());
            let l = belts
                .constraint_force(tie, clutch, target)
                .clamp(-lambda_max, lambda_max);
            let m_next = (belts.land(0, clutch[0] + l * tie[0])
                + belts.land(1, clutch[1] + l * tie[1]))
                / 2.0;
            (l, tie, m_next)
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
    belts: &BeltTick,
    crank: CrankTick,
    motor: [f32; 2],
    #[cfg(feature = "bitprobe")] probe: &mut TransmissionProbe,
) -> EngineWork {
    let omega_free = crank.omega_e + crank.tau_free * crank.dt / tp.engine_inertia;
    // DERIVED: the unclamped constant-torque work is tau_free * dt times mean crank
    // speed, including construction losses. This virtual free crank is not the guarded
    // physical crank: clipping it would discard curve work before the clutch can transmit it.
    let energy = 0.5 * tp.engine_inertia * omega_free * omega_free;
    let spendable =
        (energy - 0.5 * tp.engine_inertia * crank.omega_floor.max(0.0).powi(2)).max(0.0);
    let sides_at = |scale: f32| {
        [0, 1].map(|i| {
            let force = motor[i] * scale;
            force * (belts.speeds[i] + belts.land(i, force)) * 0.5 * crank.dt
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
        let [left, right] = sides_at(scale).map(|joules| joules / crank.dt);
        probe.power_left = left;
        probe.power_right = right;
        probe.power_positive = left.max(0.0) + right.max(0.0);
        probe.power_negative = (-left).max(0.0) + (-right).max(0.0);
        probe.power_net = work(scale) / crank.dt;
        probe.power_scale = scale;
    }
    EngineWork {
        scale,
        crank_energy: (energy - work(scale)).max(0.0),
    }
}

/// The capacity-limited stop force lands the belt at rest through [`BeltTick`]: R-Q-v/mobility.
/// Under intent the lever takes only its rollback-opposing half after the reversal pause and cut,
/// so useful launch motion is free.
/// No temporal release decision or command/speed blend participates in that balance.
fn apply_brake_stop_forces(
    q: &mut [f32; 2],
    tp: &TransmissionParams,
    belts: &BeltTick,
    st: &TransmissionState,
    controls: &Controls,
) -> [bool; 2] {
    let service = controls.service;
    let one_way = controls.one_way;
    let lever = st.park || one_way;
    let mut held = [false; 2];
    for (i, qi) in q.iter_mut().enumerate() {
        if !lever && !service {
            continue;
        }
        let cap = brake_capacity_for_regime(tp, lever, service, belts.speeds[i]);
        let stop = belts.reactions[i] - *qi - belts.speeds[i] / belts.mobility[i];
        let brake = if one_way && !service {
            controls.command.signum() * (stop * controls.command.signum()).clamp(0.0, cap)
        } else {
            stop.clamp(-cap, cap)
        };
        *qi += brake;
        held[i] = brake == stop;
    }
    held
}

/// The held mean path's capacity this tick when it acts (N): twice the per-side brake capacity
/// at the mean speed (static while the mean is at rest, dynamic once breached).
/// StandIn: the parking hold's capacity stands for the holding element's own (a gearbox brake
/// through the engaged gear), which construction does not author.
fn mean_hold_capacity(
    tp: &TransmissionParams,
    belts: &BeltTick,
    controls: &Controls,
) -> Option<f32> {
    let mean = (belts.speeds[0] + belts.speeds[1]) * 0.5;
    controls
        .mean_hold
        .then(|| 2.0 * brake_capacity_for_regime(tp, true, false, mean))
}

/// The even mean-axis stop acting after the steering solve this tick, which the neutral servo
/// solves alongside ([`steering_force`]): its capacity (N, both belts together), or `None` when
/// nothing stops both belts. The held mean path is one; both belts' two-sided stop brakes (the
/// park lever or service) are the other, their even share bounded by the weaker side's capacity.
fn even_stop_capacity(
    tp: &TransmissionParams,
    belts: &BeltTick,
    st: &TransmissionState,
    controls: &Controls,
    hold: Option<f32>,
) -> Option<f32> {
    let two_sided = controls.service || (st.park && !controls.one_way);
    hold.or_else(|| {
        two_sided.then(|| {
            let lever = st.park || controls.one_way;
            let [left, right] = belts
                .speeds
                .map(|speed| brake_capacity_for_regime(tp, lever, controls.service, speed));
            2.0 * left.min(right)
        })
    })
}

/// A held mean path supplies the mean-axis stop force that lands the mean at rest through
/// [`BeltTick`], split evenly, within its capacity `hold`; the neutral servo has solved alongside
/// the split ([`steering_force`]), so the steering member's difference still lands. The hold is
/// passive, so it is not charged to the engine. Beyond capacity the mean slides and the neutral
/// regime may end.
fn apply_mean_hold(q: &mut [f32; 2], belts: &BeltTick, hold: Option<f32>) {
    let Some(cap) = hold else {
        return; // No held mean path this tick.
    };
    // A total force split evenly moves the mean by `force·(w_l + w_r)/4`.
    let [w0, w1] = belts.mobility;
    let stop = -2.0 * (belts.land(0, q[0]) + belts.land(1, q[1])) / (w0 + w1);
    let hold = stop.clamp(-cap, cap) * 0.5;
    for force in q {
        *force += hold;
    }
}

/// The Driver holds the crank ceiling with service brakes. Below it no brake acts; beyond
/// capacity the clutch's independent slip guard still bounds the crank. No gear change is paid.
fn apply_ceiling_brakes(
    q: &mut [f32; 2],
    tp: &TransmissionParams,
    belts: &BeltTick,
    st: &TransmissionState,
    controls: &Controls,
    crank: CrankTick,
) {
    if !controls.ceiling
        || controls.shifting
        || st.clutch == ClutchState::Out
        || controls.service
        || st.park
        || crank.omega_e < tp.engine.crank_limit_rpm * RPM_TO_RAD
    {
        return;
    }
    let direction = if st.reverse { -1.0 } else { 1.0 };
    let next = (belts.land(0, q[0]) + belts.land(1, q[1])) * 0.5;
    let ceiling = tp.engine.crank_limit_rpm * RPM_TO_RAD / crank.k;
    let overspeed = (next * direction - ceiling).max(0.0);
    // One brake force on both sides lowers the mean by force·(w_l + w_r)/2.
    let brake =
        (2.0 * overspeed / (belts.mobility[0] + belts.mobility[1])).min(tp.brake_capacity_n);
    for force in q {
        *force -= direction * brake;
    }
}

/// Re-anchor only when the final belt speed implies a within-capacity clutch torque,
/// `tau = tau_free - (k*dir*m_next - omega_e)*J/dt`, inside both guards. Brakes and a steering
/// member past its capacity act after the coupling predictor, so its earlier clamp cannot certify
/// this snap.
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
/// A slipping launch holds its launch floor, capped at the speed the crank reaches freely: with
/// no free torque to spool, the crank stays where it is and the clutch takes none of its stored
/// energy. The floor is continuous in free torque, so a crank settled at its idle balance (free
/// torque zero to rounding on the first pedal tick) launches the same whichever way that rounds.
///
/// The symmetric over-rev slip guard is independent of gear choice.
fn crank_guards(
    tp: &TransmissionParams,
    inp: &TransmissionInput,
    st: &TransmissionState,
    propulsive: f32,
    tau_free: f32,
) -> (f32, f32) {
    let over = (tp.engine.crank_limit_rpm + overrev_margin_rpm()) * RPM_TO_RAD;
    let floor = if st.clutch == ClutchState::Slip {
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
        sensed: inp.sensed.is_some(),
        sensed_speed: inp.sensed.map_or(0.0, |s| s.speed),
        sensed_resistance_n: inp.sensed.map_or(0.0, |s| s.resistance),
        sensed_push_difference_n: inp.sensed.map_or(0.0, |s| s.push_difference),
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
    probe.priced_force_n = price_driver_force(tp, inp.supply, shaft, reduction, command, clutch);
    probe.delivered_force_n = delivered;
}

/// Consume one tick of the paid drive interruption.
fn advance_shift_window(st: &mut TransmissionState) -> bool {
    let shifting = st.shift_ticks > 0;
    st.shift_ticks = st.shift_ticks.saturating_sub(1);
    shifting
}

/// Advance the joint drivetrain one fixed tick and integrate both belt speeds: the m/d
/// superimposed model documented at module level.
pub(crate) fn step(
    fp: &DriveConstruction,
    tp: &TransmissionParams,
    st: &mut TransmissionState,
    inp: &TransmissionInput,
) -> TransmissionReport {
    let dt = inp.dt;
    let [vl, vr] = inp.speeds;
    let m = (vl + vr) / 2.0;
    #[cfg(feature = "bitprobe")]
    let mut bitprobe = input_probe(st, inp, m, (vl - vr) / 2.0);

    let controls = driver(fp, tp, st, inp);
    let inp = &controls.input;
    let Controls {
        dir,
        propulsive,
        shifting,
        ..
    } = controls;
    let ladder = if st.reverse {
        &tp.gears_rev
    } else {
        &tp.gears_fwd
    };
    let g = ladder[usize::from(st.gear - 1)];

    // The crank carries real inertia; launch slip limits transmitted torque while it spools.
    let omega_idle = tp.engine.idle_rpm * RPM_TO_RAD;
    let omega_e = st.omega_e;
    let k = g / tp.sprocket_radius;

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

    // Mean-axis force is clutch torque reflected through the gear. An open main clutch
    // transmits neither propulsion nor engine drag; steering work is charged separately.
    let belts = BeltTick::new(fp, inp);
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
    // The steering member ties the belts, so the clutch drives them as one motion: the lock is
    // solved on the mean that tie and each belt's implicit ground reaction land, on the pre-tick
    // |m| branch the steering solve starts from.
    let (tie, target) = steering_tie(tp, st, inp, m, branch(m));
    let (tied_inertia, tied_force) = belts.tied_mean(tie, target, dt);
    let tau_c = if engaged {
        clutch_coupling(
            crank,
            tp.engine_inertia,
            inp.supply.transmission_condition * tp.clutch_capacity,
            m,
            tied_inertia,
            tied_force,
            #[cfg(feature = "bitprobe")]
            &mut bitprobe,
        )
    } else {
        0.0
    };
    let f_c_pre_scale = k * dir * tau_c;

    let hold = mean_hold_capacity(tp, &belts, &controls);
    let even_stop = even_stop_capacity(tp, &belts, st, &controls, hold);
    let (f_s_pre_scale, lambda, j) =
        steering_force(tp, &belts, st, inp, m, f_c_pre_scale, even_stop);

    let work = engine_work(
        tp,
        &belts,
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

    apply_ceiling_brakes(&mut q, tp, &belts, st, &controls, crank);
    let held = apply_brake_stop_forces(&mut q, tp, &belts, st, &controls);
    apply_mean_hold(&mut q, &belts, hold);

    // --- Integrate both sides simultaneously against their implicit reactions.
    let next = [belts.land(0, q[0]), belts.land(1, q[1])];

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
        shaft_speed: dir * m,
        gear_reduction: g,
        k,
        omega_idle,
        omega_floor,
        shifting,
        engaged,
        i_mean: tied_inertia,
        f_other: tied_force,
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
        held,
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
