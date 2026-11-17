//! The joint drivetrain integrates both belts and the crank from construction data.
//! `prepare_orders` runs the Driver once per tick; `advance` solves its orders per substep.
//! `TransmissionState` is its mechanical state. The Driver's
//! gear choice and the ledger it reads are [`super::driver`]; the pulls it judges gears by,
//! [`viable_pull`], [`landing_pull`] and [`candidate_pull`], are the drivetrain's and live here.
//! Forces are N, torques N m, belt speeds m/s and crank speed rad/s. The design narrative lives in
//! `transmission-design.md`, "Bounded drivetrain rows".
//!
//! The rejected `STEER_SERVO_BAND` proportional servo loses authority with vehicle scale;
//! steering instead spends a bounded row impulse in the joint block.

use std::num::NonZeroU8;

use bevy::ecs::error::BevyError;

use super::drive::DriveIntent;
use super::forces::GripParams;

mod advance;
#[cfg(test)]
pub(crate) use advance::prepare_orders;
pub(crate) use advance::{DriveState, DriveTick, Orders};

#[cfg(feature = "bitprobe")]
use crate::bitprobe::TransmissionProbe;

/// Construction countdowns use the shared simulation clock.
fn fixed_step_s() -> f32 {
    crate::sim_tick::sim_timestep().as_secs_f32()
}

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

pub(crate) fn dead() -> f32 {
    driver_actuation().dead.value
}
pub(crate) fn park_engage_speed() -> f32 {
    driver_actuation().park_engage_speed.value
}

/// Shared Driver actuation parameters, admitted before play. Labels live with their data.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DriverActuation {
    reversal_hold_s: crate::provenance::Labelled<f32>,
    dead: crate::provenance::Labelled<f32>,
    park_engage_speed: crate::provenance::Labelled<f32>,
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
            ("dead", &data.dead),
            ("park_engage_speed", &data.park_engage_speed),
            ("neutral_speed_m_s", &data.neutral_speed_m_s),
        ] {
            assert!(
                number.value.is_finite()
                    && number.value > 0.0
                    && matches!(number.label, crate::provenance::Provenance::Taste(_)),
                "{PATH}: {field} must be finite, positive and labelled Taste; author the Driver perception"
            );
        }
        assert!(
            data.dead.value <= 1.0,
            "{PATH}: dead must be a share; repair the actuator data"
        );
        assert!(
            data.reversal_hold_s.value.is_finite()
                && data.reversal_hold_s.value > 0.0
                && data.reversal_hold_s.value * (1.0 / fixed_step_s()) <= f32::from(u8::MAX)
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
    pub half_tread_m: f32,
    /// Declared highest start gear and largest one-window gear step.
    pub launch_gears: u8,
    pub max_step: u8,
    /// Per forward gear `(κ_tight, κ_wide)` from the admitted crank ratios: κ = c·G/r.
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
    pub couplings: Vec<Coupling>,
    /// DERIVED belt half-difference per rad/s, tight and wide; independent of the mean clutch.
    pub steer_crank_ratio: (f32, f32),
    pub steering_radius_rounding_share: f32,
    /// Per-side service/parking brake capacity at the sprocket (N).
    pub brake_capacity_n: f32,
    /// Mean-stop force rating (N); None means this construction has no mean stop.
    pub mean_hold_capacity_n: Option<f32>,
    /// Static/dynamic multiplier for a latched, at-rest stop. Sliding uses that stop's own
    /// force rating; service braking uses the side stops' dynamic rating.
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
    /// DERIVED torque-interruption ticks from authored `shift_secs`: at least one, so every gear
    /// change opens the clutch.
    pub shift_ticks: NonZeroU8,
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
    pub couplings: &'a [Coupling],
    pub steering_radius_rounding_share: f32,
    pub brake_capacity_n: f32,
    /// Mean-stop force rating (N); None means this construction has no mean stop.
    pub mean_hold_capacity_n: Option<f32>,
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
                "steering capacity + derived brake capacity",
                (!a.couplings.contains(&Coupling::Steering)
                    || (a.steer_capacity_n.is_finite() && a.steer_capacity_n > 0.0))
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
        ] {
            if !ok {
                return Err(format!("track.powertrain.transmission: invalid {field}").into());
            }
        }

        // The rounded countdown must fit its u8 state field.
        let shift_ticks = Some(a.shift_secs * (1.0 / fixed_step_s()))
            .filter(|ticks| ticks.is_finite() && ticks.round() <= f32::from(u8::MAX))
            .and_then(|ticks| NonZeroU8::new(ticks.round() as u8))
            .ok_or_else(|| -> BevyError {
                "track.powertrain.transmission: invalid gearbox.shift_secs: author a cut of 1 to \
                 255 fixed ticks, so every gear change opens the clutch"
                    .into()
            })?;

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

        let peak_torque_nm = a
            .torque_nm
            .iter()
            .map(|point| point.1)
            .fold(0.0_f32, f32::max);
        let steering = a.couplings.contains(&Coupling::Steering);
        let steer_crank_ratio = if steering {
            let (tight, wide) = a.steer_radii_m[0];
            let scale = a.half_tread_m * a.sprocket_radius_m / gears_fwd[0];
            (scale / tight, scale / wide)
        } else {
            (0.0, 0.0)
        };
        let kappas = |gears: &[f32]| -> Vec<(f32, f32)> {
            if !steering {
                return Vec::new();
            }
            gears
                .iter()
                .map(|g| {
                    (
                        steer_crank_ratio.0 * g / a.sprocket_radius_m,
                        steer_crank_ratio.1 * g / a.sprocket_radius_m,
                    )
                })
                .collect()
        };
        let steer_kappa = kappas(&gears_fwd);
        let reverse_steer_kappa = kappas(&gears_rev);
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
            half_tread_m: a.half_tread_m,
            launch_gears: a.launch_gears,
            max_step: a.max_step,
            steer_kappa,
            reverse_steer_kappa,
            steer_radii_m: a.steer_radii_m.to_vec(),
            reverse_steer_radii_m: a.reverse_steer_radii_m.to_vec(),
            steer_capacity_n: a.steer_capacity_n,
            couplings: a.couplings.to_vec(),
            steer_crank_ratio,
            steering_radius_rounding_share: a.steering_radius_rounding_share,
            brake_capacity_n: a.brake_capacity_n,
            mean_hold_capacity_n: a.mean_hold_capacity_n,
            brake_static_factor: a.brake_static_factor,
            drag_fraction: a.drag_fraction,
            engine_inertia: a.engine_inertia_kgm2,
            clutch_capacity: a.clutch_capacity_nm,
            shift_ticks,
            peak_torque_rpm: a.idle_rpm,
            peak_torque_nm,
        };
        derived.peak_torque_rpm = derived.delivered_peak_rpm();
        derived.check_derived(a.belt_inertia)?;
        derived.check_couplings_and_radii(a.half_tread_m)?;
        derived.check_row_conditioning(a.belt_inertia)?;
        Ok(derived)
    }

    /// Construction data becomes the full row family here; runtime removes inactive members.
    /// Certifies the unloaded metric only: both effective belt inertias equal `belt_inertia`.
    /// Runtime requires a resolvable optimal independent KKT basis, including any crank-limit
    /// border, in the actual metric M_i = belt_inertia + reaction_slopes[i] * dt. There is no authored
    /// upper bound on those contact slopes. For the mean clutch/steering pair at equal M, the
    /// Gram determinant is (1/(2M) + k²c²/(2M)) / engine_inertia + k²/(4M²): increasing M
    /// can reduce rank separation, so this admission is not a loaded-metric certificate.
    /// Sign reflections preserve conditioning; positive magnitudes cover both ladders and signs.
    fn check_row_conditioning(&self, belt_inertia: f32) -> Result<(), BevyError> {
        for gear in self.gears_fwd.iter().chain(&self.gears_rev) {
            let k = f64::from(*gear / self.sprocket_radius);
            for c in [
                0.0,
                f64::from(self.steer_crank_ratio.0),
                f64::from(self.steer_crank_ratio.1),
            ] {
                let mut rows = Vec::new();
                for coupling in &self.couplings {
                    rows.push(coupling.jacobian(k, c));
                }
                for output in self.couplings.iter().filter_map(|c| {
                    if let Coupling::Clutch(o) = c {
                        Some(o)
                    } else {
                        None
                    }
                }) {
                    let w = output.weights();
                    rows.push([0.0, w[0], w[1]]);
                }
                if !super::block::conditioning_resolvable(
                    &[
                        f64::from(self.engine_inertia),
                        f64::from(belt_inertia),
                        f64::from(belt_inertia),
                    ],
                    &rows,
                    0, // This construction's crank coordinate.
                ) {
                    return Err("transmission.couplings / steering.radii / steering.reverse_radii / gearbox.forward_speeds_kmh / gearbox.reverse_speeds_kmh / engine.rated_rpm / engine.inertia_kgm2 / track.link_mass / track.link_count / roadwheels[].spin_inertia_kgm2 / roadwheels[].tread.radius / track.sprocket.spin_inertia_kgm2 / track.idler.spin_inertia_kgm2 / track.sprocket.teeth / track.idler.radius: row conditioning falls below the solver rank resolution; separate near-dependent ratios or correct the inertias".into());
                }
            }
        }
        Ok(())
    }

    /// Construction readout, not a pivot target: the tight member's belt speed at the governor.
    pub(crate) fn steering_speed_at_governor(&self) -> f32 {
        self.steer_crank_ratio.0 * self.engine.governed_rpm * RPM_TO_RAD
    }

    fn has_mean_hold(&self) -> bool {
        self.couplings.contains(&Coupling::Stop(Output::Mean))
    }

    pub(crate) fn ratio(&self, reverse: bool, gear: u8) -> f32 {
        let ladder = if reverse {
            &self.gears_rev
        } else {
            &self.gears_fwd
        };
        *ladder
            .get(usize::from(
                gear.checked_sub(1).expect("an engaged gear is one-based"),
            ))
            .expect("an engaged gear belongs to its admitted ladder")
    }

    fn check_couplings_and_radii(&self, half_tread: f32) -> Result<(), BevyError> {
        let clutches: Vec<_> = self
            .couplings
            .iter()
            .filter_map(|c| match c {
                Coupling::Clutch(o) => Some(*o),
                Coupling::Steering | Coupling::Stop(_) => None,
            })
            .collect();
        if clutches.is_empty()
            || self
                .couplings
                .iter()
                .enumerate()
                .any(|(i, c)| self.couplings[..i].contains(c))
            || (clutches.contains(&Output::Mean) && clutches.len() != 1)
            || (!clutches.contains(&Output::Mean)
                && !(clutches.contains(&Output::Left) && clutches.contains(&Output::Right)))
        {
            return Err("transmission.couplings: author one mean clutch or one clutch for each side, with no duplicate coupling".into());
        }
        if ![Output::Left, Output::Right]
            .into_iter()
            .all(|o| self.couplings.contains(&Coupling::Stop(o)))
        {
            return Err("transmission.couplings: author one side stop per belt; park, service and direction handover require both brakes".into());
        }
        if self.has_mean_hold() && !self.couplings.contains(&Coupling::Steering) {
            return Err("transmission.couplings: a mean stop needs a steering tie to arrest differential motion before side-stop handover; author that tie or remove the mean stop".into());
        }
        if self.has_mean_hold() != self.mean_hold_capacity_n.is_some()
            || self.mean_hold_capacity_n.is_some_and(|rating| {
                !rating.is_finite()
                    || rating <= 0.0
                    || !(rating * self.brake_static_factor).is_finite()
            })
        {
            return Err("transmission.mean_hold_capacity_n: derived capacity must be finite and positive exactly when Stop(Mean) is constructed; admit transmission.mean_hold as StandIn through the construction seam".into());
        }
        if !self.couplings.contains(&Coupling::Steering) {
            return Ok(());
        }
        let bound = self.steering_radius_rounding_share;
        if !bound.is_finite() || !(0.0..1.0).contains(&bound) {
            return Err("transmission.steering.radius_rounding_share: author a finite rounding bound in [0,1)".into());
        }
        for reverse in [false, true] {
            let (gears, radii) = if reverse {
                (&self.gears_rev, &self.reverse_steer_radii_m)
            } else {
                (&self.gears_fwd, &self.steer_radii_m)
            };
            for (i, (&g, &(tight, wide))) in gears.iter().zip(radii).enumerate() {
                for (detent, radius, c) in [
                    ("tight", tight, self.steer_crank_ratio.0),
                    ("wide", wide, self.steer_crank_ratio.1),
                ] {
                    let predicted = half_tread * self.sprocket_radius / (g * c);
                    if !predicted.is_finite() || (predicted - radius).abs() > radius * bound {
                        return Err(format!("transmission.steering.{}radii[{i}].{detent}: table exceeds radius_rounding_share {bound}; author radii consistent with one crank ratio per detent", if reverse { "reverse_" } else { "" }).into());
                    }
                }
            }
        }
        Ok(())
    }

    /// The authored scalars can each be finite while their ratios, squared coupling terms or
    /// force capacities overflow/underflow. Refuse those coupled outputs before publication.
    fn check_derived(&self, belt_inertia: f32) -> Result<(), BevyError> {
        // Resolution: dt*max|dT/domega|/J <= 1 keeps a falling torque branch from reversing
        // a crank-speed perturbation in one explicit interval. Certifying one whole tick
        // admits every subdivision without tying construction to a particular substep count. Fuel cut makes each span quadratic;
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
            / (f64::from(1.0 / fixed_step_s()) * f64::from(RPM_TO_RAD));
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
            + self.peak_torque_nm / self.engine_inertia / (1.0 / fixed_step_s());
        let energy_bound = 0.5 * self.engine_inertia * free_bound * free_bound;
        if !energy_bound.is_finite() || energy_bound <= 0.0 {
            return Err(fail(
                "engine.inertia_kgm2 / engine.torque_curve crank energy",
            ));
        }
        if !self.roundoff_force_scale().is_finite()
            || self.roundoff_force_scale() <= 0.0
            || !(2.0 * belt_inertia).is_finite()
            || (self.couplings.contains(&Coupling::Steering)
                && ![self.steer_crank_ratio.0, self.steer_crank_ratio.1]
                    .into_iter()
                    .all(|c| c.is_finite() && c > 0.0 && (c * self.steer_capacity_n).is_finite()))
            || !(2.0 * self.brake_capacity_n).is_finite()
            || (self.couplings.contains(&Coupling::Steering)
                && !(2.0 * self.steer_capacity_n).is_finite())
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

    /// Torque scale of the crank-acceleration subtraction, J*omega/dt at the over-rev bound. Its
    /// f32 roundoff follows construction, not hull weight.
    pub(crate) fn roundoff_torque_scale(&self) -> f32 {
        self.engine_inertia
            * (self.engine.crank_limit_rpm + overrev_margin_rpm())
            * RPM_TO_RAD
            * (1.0 / fixed_step_s())
    }

    /// [`Self::roundoff_torque_scale`] at the belt, through the largest admitted crank-to-belt
    /// ratio.
    pub(crate) fn roundoff_force_scale(&self) -> f32 {
        let ratio = self
            .gears_fwd
            .iter()
            .chain(&self.gears_rev)
            .copied()
            .fold(0.0, f32::max);
        self.roundoff_torque_scale() * ratio / self.sprocket_radius
    }
}

/// Solved clutch feedback for the Driver and displays: ordered open, slipping or locked.
/// A guard or reduced capacity never turns a closed order into free gear selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum ClutchState {
    Out,
    Slip,
    In,
}

/// A free initial choice is distinct from take-up of an already selected ratio. A paid
/// shift keeps its selected gear through closing; the first solved lock ends its launch floor.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum LaunchState {
    Unchosen,
    TakingUp,
    Locked,
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
    /// Park lever: set at rest and while braking released motion against the gear outside a
    /// neutral pivot, or by a released sub-idle stop along it. It holds both ways during
    /// reversal pause/cut and without intent; launch carries only the rollback shortfall.
    /// A breached lever retains its dynamic brake capacity.
    pub park: bool,
    /// Engine crank speed ω_e (rad/s). Initialized explicitly from
    /// vehicle data by [`Self::from_spec`]; the regenerative tick path has no in-band sentinel.
    pub omega_e: f32,
    /// Clutch row outcome, replicated with the crank and gear.
    pub clutch: ClutchState,
    /// Remaining stopped ticks before a held opposite request may change direction.
    /// Any motion refreshes it; release cancels it without releasing park.
    pub reversal_hold_ticks: u8,
    /// Passive mean-stop latch; a side-stop handover waits for exact rest.
    pub mean_hold: bool,
    pub launch: LaunchState,
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
) -> [TransmissionProjectionValue; 10] {
    let TransmissionState {
        gear,
        shift_ticks,
        steer_step,
        reverse,
        park,
        omega_e,
        clutch,
        reversal_hold_ticks,
        mean_hold,
        launch,
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
        Bool(mean_hold),
        U8(match launch {
            LaunchState::Unchosen => 0,
            LaunchState::TakingUp => 1,
            LaunchState::Locked => 2,
        }),
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
            mean_hold: false,
            launch: LaunchState::Unchosen,
        }
    }
}

/// Shared gear/rpm readout for displays; the drivetrain owns their interpretation.
#[derive(Clone, Debug, PartialEq)]
pub struct DriveReadout {
    /// Engine rpm from crank state ω_e, including
    /// sub-idle lugging and, in the game's damage model, a crank at rest when combustion
    /// cannot sustain idle.
    pub rpm: f32,
    /// The engaged gear as a display label: `F1..Fn` forward, `R1..Rn` reverse.
    pub gear_label: String,
}

/// Read actual crank rpm, including slip, shift matching and backward back-drive, and the
/// engaged ladder's gear label.
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
    /// The engine's HP pool holds anything ([`crate::damage::module_quality`]); the power
    /// readout reads this source even when the task has no throughput.
    pub engine_in_action: bool,
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
        engine_in_action: true,
        transmission_condition: 1.0,
    };
}

/// Shaped drive axes, requested intent, pre-tick belt speeds and ground reactions.
/// The mixed side commands are probe telemetry only.
#[derive(Clone, Copy, Debug)]
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
    /// What the Driver feels of the hull this tick; `None` until the ledger has closed two ticks.
    pub sensed: Option<super::driver::Sensed>,
    pub dt: f32,
}

/// What the joint solve produced: the integrated next belt speeds, the per-side sprocket
/// forces actually applied (telemetry — the `engine` slot of the harness rows), and the
/// operating point for HUD/legend display.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransmissionReport {
    pub next_speeds: [f32; 2],
    /// Assignment trials in the mechanical solve (candidate pricing reports its own work).
    pub assignments_tried: usize,
    /// Mechanical rows evaluated; candidate pricing is counted separately.
    pub rows: usize,
    /// Each side's capacity-limited brake supplied the exact stop force: a held constraint.
    pub held: [bool; 2],
    /// A holding brake (a side's stop brake or the held mean path) met a stop beyond its
    /// capacity: what it holds slides under it.
    pub brakes_slide: bool,
    pub forces: [f32; 2],
    pub rpm: f32,
    pub gear: u8,
    pub reverse: bool,
    pub steer_step: u8,
    pub shifting: bool,
    /// This tick asked the engaged ladder for drive: a propulsive pedal past the dead band.
    pub pulling: bool,
    /// The clutch sent drive to the belts this tick: torque along the engaged ladder. A crank
    /// spooling to its launch rpm, an open clutch and overrun send none.
    pub driving: bool,
    /// The clutch held the crank to the belts throughout the reported interval. A tick report
    /// requires every substep locked before the Driver counts it as that gear's held sample.
    pub locked: bool,
    /// Combustion power capacity at the pre-tick operating point (W).
    pub power_available: f32,
    /// The last mechanical interval; its named work counters cover the enclosing tick.
    #[cfg(feature = "bitprobe")]
    pub(crate) bitprobe: TransmissionProbe,
}

/// Authored Coulomb share of the motoring-drag anchor, shared with the law's instruments.
pub(crate) fn motoring_drag_base_share() -> f32 {
    engine_control().motoring_drag_base_share.value
}

/// Authored command share at which the pedal blend recovers the whole net curve, shared with the
/// law's fixtures.
pub(super) fn drag_throttle_release() -> f32 {
    engine_control().drag_throttle_release.value
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
    let pedal = (command / drag_throttle_release()).clamp(0.0, 1.0);
    // Cubic smoothstep has zero endpoint slopes: 3p² - 2p³.
    let released = pedal * pedal * (3.0 - 2.0 * pedal);
    let rpm = omega / RPM_TO_RAD;
    let curve = tp.torque_at(rpm);
    let cut_drag = drag * tp.fuel_cut(rpm);
    let rate = if supply.engine_in_action {
        supply.served_task_rate()
    } else {
        0.0
    };
    let induced = rate * (command * curve + released * cut_drag);
    let idle_capacity = idle_catch_capacity(tp, supply);
    // Both requests move one rack: the catch asks no more than the served rack gives at full
    // command, so full pedal leaves it nothing to add at any task rate, and it supplies only what
    // the command has not supplied.
    let catch = (idle_capacity * (tp.engine.idle_rpm * RPM_TO_RAD - omega)
        / (engine_control().idle_catch_droop_rpm.value * RPM_TO_RAD))
        .clamp(0.0, idle_capacity)
        .min(rate * (curve + cut_drag));
    [induced, (catch - induced).max(0.0), drag]
}

/// The game's driving and damage model: the Driver's idle catch is work by the Drive task,
/// consuming the served task throughput, like commanded fueling.
fn idle_catch_capacity(tp: &TransmissionParams, supply: DriveSupply) -> f32 {
    if supply.engine_in_action {
        supply.served_task_rate() * tp.torque_at(tp.engine.idle_rpm)
    } else {
        0.0
    }
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

#[cfg(any(test, feature = "bitprobe"))]
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
    // A take-up price starts at its launch floor; a locked price follows the shaft.
    let rpm = if clutch != ClutchState::In {
        launch_rpm.max(shaft_rpm)
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

pub(super) fn force_margin(tp: &TransmissionParams) -> f32 {
    DriverResolution::load().margin_n(tp)
}

/// Candidate acceleration price with the hull carried in the belt inertia. The physical row
/// assembly owns delivery; candidate pricing supplies a hypothetical starting operating point.
#[expect(
    clippy::too_many_arguments,
    reason = "the force law reads construction, supply, shaft speed, ratio, command, load and clutch"
)]
pub(super) fn candidate_pull(
    tp: &TransmissionParams,
    fp: &DriveConstruction,
    supply: DriveSupply,
    shaft: f32,
    ratio: f32,
    command: f32,
    load: f32,
    clutch: ClutchState,
) -> f32 {
    // An unlocked candidate starts at its launch floor; a locked candidate starts on its shaft.
    let mut price_construction = *fp;
    price_construction.belt_inertia = coupled_mass(fp) * 0.5;
    let shaft_omega = shaft * ratio / tp.sprocket_radius;
    let omega = if clutch == ClutchState::In {
        shaft_omega
    } else {
        shaft_omega.max(tp.launch_omega(command))
    };
    let mut state = TransmissionState {
        omega_e: omega,
        park: false,
        launch: if clutch == ClutchState::In {
            LaunchState::Locked
        } else {
            LaunchState::TakingUp
        },
        ..TransmissionState::from_spec(tp)
    };
    let input = TransmissionInput {
        supply,
        intent: DriveIntent::Axes(super::drive::DriveAxes {
            throttle: command,
            steer: 0.0,
        }),
        throttle: command,
        steer: 0.0,
        side_commands: [command; 2],
        speeds: [shaft; 2],
        reactions: [load * 0.5; 2],
        reaction_slopes: [0.0; 2],
        sensed: None,
        dt: fixed_step_s(),
    };
    let controls = Controls {
        input,
        command,
        dir: 1.0,
        propulsive: command,
        service: false,
        one_way: false,
        ceiling: false,
        shifting: false,
        mean_hold: false,
        clutch_open: false,
        ratio,
    };
    let [fuel, catch, drag] = engine_torques(tp, supply, omega, command);
    let drag = bounded_drag(tp, omega, input.dt, drag);
    let (report, _, _) = advance_rows(
        &price_construction,
        tp,
        &mut state,
        &controls,
        fuel + catch - drag,
    );
    report.forces.iter().sum()
}

/// Sustained pedal pull: a locked crank may lug; a take-up is priced at its launch floor.
pub(crate) fn viable_pull(
    tp: &TransmissionParams,
    supply: DriveSupply,
    at: f32,
    ratio: f32,
    command: f32,
    prior: ClutchState,
) -> f32 {
    let shaft = at * ratio / tp.sprocket_radius;
    let crank = if prior == ClutchState::In {
        let floor = idle_floor(tp, supply, 0.0).unwrap_or(0.0);
        floor..=shaft.max(floor)
    } else {
        let launch = tp.launch_omega(command).max(shaft);
        launch..=launch
    };
    pedal_pull(tp, supply, command, crank, ratio)
}

/// Free gear price at the signed shaft speed and launch floor; overspeed landings are refused.
pub(crate) fn landing_pull(
    tp: &TransmissionParams,
    supply: DriveSupply,
    at: f32,
    ratio: f32,
    command: f32,
) -> Option<f32> {
    let shaft = at * ratio / tp.sprocket_radius;
    if shaft > tp.engine.crank_limit_rpm * RPM_TO_RAD {
        return None;
    }
    let crank = shaft.max(tp.launch_omega(command));
    Some(pedal_pull(tp, supply, command, crank..=crank, ratio))
}

/// The pull (N) through `ratio` of the most the pedal's own fuel nets over crank speeds `crank`
/// (rad/s), the clutch capping the torque either way.
fn pedal_pull(
    tp: &TransmissionParams,
    supply: DriveSupply,
    command: f32,
    crank: std::ops::RangeInclusive<f32>,
    ratio: f32,
) -> f32 {
    let capacity = supply.transmission_condition * tp.clutch_capacity;
    pedal_peak(tp, supply, command, crank).clamp(-capacity, capacity) * ratio / tp.sprocket_radius
}

/// The most torque the pedal's own fuel nets at the crank over crank speeds `crank` (rad/s, N m).
/// Between the curve's points and the governor ramp's ends the net torque is quadratic in crank
/// speed (a linear curve under a linear fuel ramp, less affine drag), so each span's ends and its
/// parabola's vertex bound it.
fn pedal_peak(
    tp: &TransmissionParams,
    supply: DriveSupply,
    command: f32,
    crank: std::ops::RangeInclusive<f32>,
) -> f32 {
    let (low, high) = crank.into_inner();
    let net = |omega: f32| {
        let [fuel, _, drag] = engine_torques(tp, supply, omega, command);
        fuel - drag
    };
    let governed = tp.engine.governed_rpm;
    // The curve's points and the governor ramp's ends, each ascending, merged in order.
    let mut curve = tp.engine.torque_nm.iter().map(|&(rpm, _)| rpm).peekable();
    let mut ramp = [governed, governed + engine_control().governor_cut_rpm.value]
        .into_iter()
        .peekable();
    let knots = std::iter::from_fn(|| match (curve.peek(), ramp.peek()) {
        (Some(c), Some(r)) if r < c => ramp.next(),
        (Some(_), _) => curve.next(),
        (None, _) => ramp.next(),
    });
    knots
        .map(|rpm| rpm * RPM_TO_RAD)
        .filter(|&omega| omega > low && omega < high)
        .chain([high])
        .scan(low, |from, to| Some([std::mem::replace(from, to), to]))
        .flat_map(|span| {
            let [ends @ .., mid] = [span[0], span[1], 0.5 * (span[0] + span[1])].map(net);
            // The span's parabola in `t` from -1 to 1 across it: `mid + slope t + bend t²`.
            let (slope, bend) = (0.5 * (ends[1] - ends[0]), 0.5 * (ends[0] + ends[1]) - mid);
            let vertex = (bend < 0.0 && slope.abs() < -2.0 * bend)
                .then(|| mid - slope * slope / (4.0 * bend));
            ends.into_iter().chain(vertex)
        })
        .fold(f32::NEG_INFINITY, f32::max)
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

/// Drive paths are construction data. A side clutch releases before its inner brake applies;
/// a mean clutch leaves steering to the independently constructed steering member.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum Output {
    Mean,
    Left,
    Right,
}

impl Output {
    fn weights(self) -> [f64; 2] {
        match self {
            Self::Mean => [0.5, 0.5],
            Self::Left => [1.0, 0.0],
            Self::Right => [0.0, 1.0],
        }
    }

    fn inner(self, steer: f32) -> bool {
        match self {
            Self::Mean => false,
            Self::Left => steer < -dead(),
            Self::Right => steer > dead(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum Coupling {
    Clutch(Output),
    Steering,
    Stop(Output),
}

impl Coupling {
    /// Geometry in crank/left/right speed coordinates; signed ratios select runtime direction.
    fn jacobian(self, shaft_ratio: f64, steer_ratio: f64) -> [f64; 3] {
        match self {
            Self::Clutch(output) => {
                let w = output.weights();
                [1.0, -shaft_ratio * w[0], -shaft_ratio * w[1]]
            }
            Self::Steering => [-steer_ratio, 0.5, -0.5],
            Self::Stop(output) => {
                let w = output.weights();
                [0.0, w[0], w[1]]
            }
        }
    }
}

/// Passive couplings alone own static/dynamic capacity and make-before-break stop transfer.
fn passive_rows(
    tp: &TransmissionParams,
    st: &TransmissionState,
    controls: &Controls,
    capacity: f64,
) -> Vec<super::block::Row> {
    use super::block::{Bounds, Row};
    let inp = &controls.input;
    let symmetric = |capacity: f64| Bounds {
        lower: -capacity,
        upper: capacity,
    };
    let mut rows = Vec::new();
    if controls.mean_hold {
        assert!(
            tp.has_mean_hold(),
            "a mean-stop latch belongs to a constructed mean stop"
        );
        let rating = f64::from(
            tp.mean_hold_capacity_n
                .expect("a constructed mean stop has its own admitted rating"),
        ) * f64::from(inp.dt);
        rows.push(Row {
            jacobian: Coupling::Stop(Output::Mean).jacobian(0.0, 0.0).to_vec(),
            target: 0.0,
            bounds: symmetric(rating * f64::from(tp.brake_static_factor)),
            dynamic: Some(symmetric(rating)),
        });
    } else {
        for (coupling, side) in tp.couplings.iter().filter_map(|c| match c {
            Coupling::Stop(Output::Left) => Some((*c, 0)),
            Coupling::Stop(Output::Right) => Some((*c, 1)),
            _ => None, // Drive paths and the inactive mean stop do not supply side brakes.
        }) {
            let inner = tp.couplings.iter().any(|c| matches!(c, Coupling::Clutch(output)
                if *output != Output::Mean && output.weights()[side] != 0.0 && output.inner(inp.steer)));
            if st.park || controls.service || controls.one_way || inner {
                let bounds = |capacity| {
                    if controls.one_way && !controls.service && !inner {
                        if controls.command > 0.0 {
                            Bounds {
                                lower: 0.0,
                                upper: capacity,
                            }
                        } else {
                            Bounds {
                                lower: -capacity,
                                upper: 0.0,
                            }
                        }
                    } else {
                        symmetric(capacity)
                    }
                };
                let lever = st.park || (!controls.service && !inner);
                rows.push(Row {
                    jacobian: coupling.jacobian(0.0, 0.0).to_vec(),
                    target: 0.0,
                    bounds: bounds(if lever {
                        capacity * f64::from(tp.brake_static_factor)
                    } else {
                        capacity
                    }),
                    dynamic: lever.then(|| bounds(capacity)),
                });
            }
        }
    }
    rows
}

/// Crank guards share one impulse budget across every constructed drive path.
struct CouplingBudget {
    floor_impulse: f64,
    over_impulse: f64,
    signed_steer: f64,
    steering_capacity: f64,
}

fn coupling_budget(
    tp: &TransmissionParams,
    st: &TransmissionState,
    controls: &Controls,
    tau_free: f32,
) -> CouplingBudget {
    let inp = &controls.input;
    let dt = f64::from(inp.dt);
    let crank_free =
        f64::from(st.omega_e) + f64::from(tau_free) * dt / f64::from(tp.engine_inertia);
    let floor = if st.launch != LaunchState::Locked && controls.propulsive >= dead() {
        tp.launch_omega(controls.propulsive)
    } else {
        idle_floor(tp, inp.supply, st.omega_e)
            .unwrap_or(-((tp.engine.crank_limit_rpm + overrev_margin_rpm()) * RPM_TO_RAD))
    };
    // The block bounds the actual sum of crank reactions, never reserved path ratings.
    let over = f64::from((tp.engine.crank_limit_rpm + overrev_margin_rpm()) * RPM_TO_RAD);
    let down_budget = f64::from(tp.engine_inertia) * (crank_free - f64::from(floor)).max(0.0);
    let up_budget = f64::from(tp.engine_inertia) * (over - crank_free).max(0.0);
    let steer_ratio = match st.steer_step {
        0 => 0.0,
        1 => tp.steer_crank_ratio.1,
        2 => tp.steer_crank_ratio.0,
        _ => panic!("the Driver admits only straight, wide and tight detents"),
    };
    let signed_steer = f64::from(branch(inp.steer) * steer_ratio);
    let steering_capacity =
        2.0 * f64::from(tp.steer_capacity_n * inp.supply.transmission_condition) * dt;
    let floor_impulse = -down_budget;
    let over_impulse = up_budget;
    CouplingBudget {
        floor_impulse,
        over_impulse,
        signed_steer,
        steering_capacity,
    }
}

/// One block owns the crank and both belts. Ground slopes augment belt inertia; their affine
/// offsets preserve the supplied starting reactions. Every row impulse is billed to every
/// coordinate of its Jacobian, including the crank's steering work.
fn advance_rows(
    fp: &DriveConstruction,
    tp: &TransmissionParams,
    st: &mut TransmissionState,
    controls: &Controls,
    tau_free: f32,
) -> (TransmissionReport, f32, f32) {
    use super::block::{Bounds, ImpulseLimit, Row, Speed, solve_limited};
    let inp = &controls.input;
    let dt = f64::from(inp.dt);
    let k = f64::from(controls.ratio / tp.sprocket_radius);
    let dir = f64::from(controls.dir);
    let CouplingBudget {
        floor_impulse,
        over_impulse,
        signed_steer,
        steering_capacity,
    } = coupling_budget(tp, st, controls, tau_free);
    let mut speeds = vec![Speed {
        value: f64::from(st.omega_e),
        inertia: f64::from(tp.engine_inertia),
        free_impulse: f64::from(tau_free) * dt,
    }];
    for i in 0..2 {
        let inertia = f64::from(fp.belt_inertia) + f64::from(inp.reaction_slopes[i]) * dt;
        assert!(
            inertia.is_finite() && inertia > 0.0,
            "a sampled ground slope cannot cancel its belt inertia"
        );
        speeds.push(Speed {
            value: f64::from(inp.speeds[i]),
            inertia,
            free_impulse: -f64::from(inp.reactions[i]) * dt,
        });
    }
    let symmetric = |capacity: f64| Bounds {
        lower: -capacity,
        upper: capacity,
    };
    let mut rows = Vec::new();
    let mut clutches = Vec::new();
    let mut steering = None;
    for coupling in &tp.couplings {
        match *coupling {
            Coupling::Clutch(output) => {
                let open = controls.clutch_open || output.inner(inp.steer);
                let capacity = if open {
                    0.0
                } else {
                    f64::from(tp.clutch_capacity * inp.supply.transmission_condition) * dt
                };
                // Without an operator the spring clutch is passive and closed both ways:
                // served fuel and idle catch are then zero. An operated released pedal cannot drive.
                let lower =
                    if open || (inp.supply.control_authority && controls.propulsive < dead()) {
                        0.0
                    } else {
                        -capacity
                    };
                let upper = if open { 0.0 } else { capacity };
                clutches.push(rows.len());
                rows.push(Row {
                    jacobian: coupling.jacobian(dir * k, signed_steer).to_vec(),
                    target: 0.0,
                    bounds: Bounds { lower, upper },
                    dynamic: None,
                });
            }
            Coupling::Stop(_) => {}
            Coupling::Steering => {
                steering = Some(rows.len());
                rows.push(Row {
                    jacobian: coupling.jacobian(dir * k, signed_steer).to_vec(),
                    target: 0.0,
                    bounds: symmetric(steering_capacity),
                    dynamic: None,
                });
            }
        }
    }
    let capacity = f64::from(tp.brake_capacity_n) * dt;
    let first_stop = rows.len();
    rows.extend(passive_rows(tp, st, controls, capacity));
    let mut stops = first_stop..rows.len();
    if controls.ceiling
        && !controls.clutch_open
        && !controls.mean_hold
        && !st.park
        && !controls.service
    {
        for output in tp.couplings.iter().filter_map(|c| match c {
            Coupling::Clutch(output) if !output.inner(inp.steer) => Some(*output),
            _ => None, // Only a connected drive output has a crank-speed ceiling to protect.
        }) {
            let weights = output.weights();
            rows.push(Row {
                jacobian: vec![0.0, dir * weights[0], dir * weights[1]],
                target: f64::from(tp.engine.crank_limit_rpm * RPM_TO_RAD) / k,
                bounds: Bounds {
                    lower: -capacity / weights[0].max(weights[1]),
                    upper: 0.0,
                },
                dynamic: None,
            });
        }
    }

    let solved = solve_limited(
        &speeds,
        &rows,
        &[ImpulseLimit {
            coefficients: rows.iter().map(|r| r.jacobian[0]).collect(),
            bounds: Bounds {
                lower: floor_impulse,
                upper: over_impulse,
            },
        }],
    );
    let mut forces = [0.0_f64; 2];
    for (row, outcome) in rows.iter().zip(&solved.rows) {
        for (force, coefficient) in forces.iter_mut().zip(&row.jacobian[1..]) {
            *force += coefficient * outcome.impulse / dt;
        }
    }
    let active = clutches
        .iter()
        .any(|&i| rows[i].bounds.lower != rows[i].bounds.upper);
    let locked = active
        && clutches
            .iter()
            .filter(|&&i| rows[i].bounds.lower != rows[i].bounds.upper)
            .all(|&i| {
                (rows[i]
                    .jacobian
                    .iter()
                    .zip(&solved.speeds)
                    .map(|(a, v)| a * v)
                    .sum::<f64>()
                    - rows[i].target)
                    .abs()
                    <= rows[i].slip_tolerance(&solved.speeds)
            });
    st.clutch = if controls.clutch_open {
        ClutchState::Out
    } else if locked {
        ClutchState::In
    } else {
        ClutchState::Slip
    };
    if locked {
        st.launch = LaunchState::Locked;
    }
    st.omega_e = solved.speeds[0] as f32;
    let driving = clutches.iter().any(|&i| solved.rows[i].impulse < 0.0);
    let next_speeds = [solved.speeds[1] as f32, solved.speeds[2] as f32];
    let held = [0, 1].map(|side| {
        !controls.mean_hold
            && next_speeds[side] == 0.0
            && stops.clone().any(|i| rows[i].jacobian[side + 1] != 0.0)
    });
    let brakes_slide = stops.any(|i| {
        solved.rows[i].impulse != 0.0
            && rows[i]
                .jacobian
                .iter()
                .zip(&solved.speeds)
                .map(|(a, v)| a * v)
                .sum::<f64>()
                != 0.0
    });
    let steer_force = steering.map_or(0.0, |i| (solved.rows[i].impulse / dt) as f32);
    let clutch_torque = -clutches
        .iter()
        .map(|&i| solved.rows[i].impulse)
        .sum::<f64>()
        / dt;
    (
        TransmissionReport {
            next_speeds,
            assignments_tried: solved.assignments_tried,
            rows: rows.len(),
            held,
            brakes_slide,
            forces: forces.map(|f| f as f32),
            rpm: st.omega_e / RPM_TO_RAD,
            gear: st.gear,
            reverse: st.reverse,
            steer_step: st.steer_step,
            shifting: controls.shifting,
            pulling: controls.propulsive >= dead(),
            driving,
            locked,
            ..TransmissionReport::default()
        },
        clutch_torque as f32,
        steer_force,
    )
}

fn requests_other_ladder(command: f32, reverse: bool) -> bool {
    command.abs() >= dead() && (command < 0.0) != reverse
}

/// Both side stops must have landed exactly at rest before the stopped reversal countdown runs.
/// Motion refreshes the countdown; release cancels it without releasing the stop.
fn swap_ladder_direction(
    tp: &TransmissionParams,
    st: &mut TransmissionState,
    inp: &TransmissionInput,
    command: f32,
    brake: bool,
) {
    if !requests_other_ladder(command, st.reverse) || brake {
        st.reversal_hold_ticks = 0;
    } else if inp.speeds != [0.0; 2] || st.mean_hold {
        st.reversal_hold_ticks = (driver_actuation().reversal_hold_s.value / inp.dt).ceil() as u8;
    } else if st.reversal_hold_ticks > 0 {
        st.reversal_hold_ticks -= 1;
    } else if st.shift_ticks == 0 {
        st.reverse = command < 0.0;
        st.gear = 1;
        st.shift_ticks = tp.shift_ticks.get();
        st.launch = LaunchState::Unchosen;
    }
}

#[derive(Clone, Copy, Debug)]
struct Controls {
    input: TransmissionInput,
    command: f32,
    dir: f32,
    propulsive: f32,
    service: bool,
    one_way: bool,
    ceiling: bool,
    shifting: bool,
    mean_hold: bool,
    clutch_open: bool,
    ratio: f32,
}

/// Orders depend on intent. Static mean support survives until side stops can latch at rest;
/// a hold starting with nonzero mean slip has no static support to preserve, so every exit is immediate.
fn driver(
    fp: &DriveConstruction,
    tp: &TransmissionParams,
    st: &mut TransmissionState,
    inp: &TransmissionInput,
) -> Controls {
    let mut input = *inp;
    let staffed = input.supply.control_authority;
    let work = input.supply.served_task_rate() > 0.0;
    if !staffed {
        input.throttle = 0.0;
    }
    if !work {
        input.steer = 0.0;
    }
    let (command, brake) = match input.intent {
        DriveIntent::Axes(axes) => (if staffed { axes.throttle } else { 0.0 }, false),
        DriveIntent::Brake => (0.0, true),
    };
    let requested = command.abs() >= dead();
    let mean = (input.speeds[0] + input.speeds[1]) * 0.5;
    if brake
        || requests_other_ladder(command, st.reverse)
        || (st.mean_hold && mean == 0.0 && requested)
    {
        input.steer = 0.0;
    }
    let steer_only = !brake && !requested && input.steer.abs() >= dead();
    let dir = if st.reverse { -1.0 } else { 1.0 };
    // Keep a retiring mean hold only while it has static support to preserve. The straight
    // steering tie arrests differential motion before exact-rest transfer to both side discs.
    let retiring = requested || brake || input.steer.abs() < dead();
    if st.mean_hold && retiring && (mean != 0.0 || input.speeds == [0.0; 2]) {
        st.park = true;
        st.mean_hold = false;
    }
    if staffed {
        if requested {
            if requests_other_ladder(command, st.reverse) && input.speeds == [0.0; 2] {
                st.park = true;
            }
            if !requests_other_ladder(command, st.reverse) && st.shift_ticks == 0 {
                st.park = false;
            }
        } else if steer_only {
            if tp.has_mean_hold()
                && (mean.abs() < driver_actuation().neutral_speed_m_s.value || mean * dir < 0.0)
            {
                st.mean_hold = true;
            }
            // Steering intent releases the side stops; a constructed hold takes their mean.
            st.park = false;
        } else if !st.mean_hold {
            let speed = input.speeds[0].abs().max(input.speeds[1].abs());
            let stop_speed =
                tp.engine.idle_rpm * RPM_TO_RAD * tp.sprocket_radius / tp.ratio(st.reverse, 1);
            if input.speeds == [0.0; 2]
                || mean * dir < 0.0
                || (input.steer.abs() < dead() && speed < stop_speed)
            {
                st.park = true;
            }
        }
    }
    if work {
        swap_ladder_direction(tp, st, &input, command, brake);
    }
    let opposing = requests_other_ladder(command, st.reverse);
    let dir = if st.reverse { -1.0 } else { 1.0 };
    let service = brake || opposing;
    // A pending mean-stop handover serves no propulsion against its own stop. The exact-rest
    // transfer above admits the replacement lever's static capacity before the pedal can drive.
    let propulsive = if brake || opposing || st.mean_hold {
        0.0
    } else {
        (input.throttle * dir).clamp(0.0, 1.0)
    };
    update_steer_detent(st, input.steer);
    if work && let Some(sensed) = input.sensed {
        super::driver::choose(
            st,
            tp,
            fp,
            &input,
            // A backward crank cannot combust; a paid cut cannot buy that unavailable source.
            if service || st.omega_e < 0.0 {
                0.0
            } else {
                command.abs().min(1.0)
            },
            sensed,
        );
    }
    let shifting = advance_shift_window(st);
    // No ledger yet is a real initial state. Once a gear is chosen, motion never reopens it.
    let clutch_open = shifting || (st.launch == LaunchState::Unchosen && propulsive >= dead());
    Controls {
        input,
        command,
        dir,
        propulsive,
        service,
        one_way: !brake && requested && !opposing && !shifting,
        ceiling: staffed,
        shifting,
        mean_hold: st.mean_hold,
        clutch_open,
        ratio: tp.ratio(st.reverse, st.gear),
    }
}

fn update_steer_detent(st: &mut TransmissionState, steer: f32) {
    let a = steer.abs();
    let feel = super::drive::drive_feel();
    st.steer_step = if a >= feel.tight_on.0 {
        2
    } else {
        u8::from(a >= feel.wide_on.0)
    };
}

/// Construction drag alone cannot reverse a free crank within one tick; clutch work may
/// still back-drive the settled crank in either direction.
fn bounded_drag(tp: &TransmissionParams, omega: f32, dt: f32, drag: f32) -> f32 {
    let bound = tp.engine_inertia * omega.abs() / dt;
    drag.clamp(-bound, bound)
}

/// Engine force law at the starting crank speed. Pedal, steering and shift matching request
/// one fuel rack; idle catch and construction drag retain their own data-owned laws.
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
    let omega_match = if shifting {
        (m * k).max(tp.launch_omega(propulsive))
    } else {
        omega_idle
    };
    let omega_steer =
        omega_idle + (tp.peak_torque_rpm * RPM_TO_RAD - omega_idle) * inp.steer.abs().min(1.0);
    let steer_fuel = if inp.steer.abs() >= dead() || shifting {
        ((omega_match.max(omega_steer) - omega_e)
            / (engine_control().rev_match_band_rpm.value * RPM_TO_RAD))
            .clamp(0.0, 1.0)
    } else {
        0.0
    };
    let u_fuel = if engaged {
        propulsive.max(steer_fuel)
    } else {
        steer_fuel
    };

    let supply = inp.supply;
    let rpm = omega_e / RPM_TO_RAD;
    let [tau_ind, tau_idle, tau_drag] = engine_torques(tp, supply, omega_e, u_fuel);
    let tau_free = tau_ind + tau_idle - bounded_drag(tp, omega_e, inp.dt, tau_drag);

    let p_avail = if supply.engine_in_action {
        tp.torque_at(rpm) * omega_e.max(0.0)
    } else {
        0.0
    };
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

/// The locked-clutch turn price: signed belt half-difference per mean speed this tick
/// with `gear` engaged; zero when straight.
///
/// κ is indexed by `gear` on the engaged forward or reverse ladder; `d` follows the
/// steer SIGN regardless of travel direction — the superimposed steering shaft is independent of
/// the gear's direction, historically and mechanically. The detent (`st.steer_step`) was updated
/// for this tick before gear selection: one detent truth per tick.
pub(super) fn steer_ratio(
    tp: &TransmissionParams,
    st: &TransmissionState,
    inp: &TransmissionInput,
    gear: u8,
) -> f32 {
    if !tp.couplings.contains(&Coupling::Steering) {
        return 0.0;
    }
    branch(inp.steer) * detent_kappa(tp, st, gear)
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

/// The saturated catch protects only a nonnegative live crank. A cut's free crank cannot cross
/// zero under drag, so a backward vehicle landing does not revoke this admission in pricing.
/// The test reads the catch the law gives at the guard point, released, so a catch the served rack
/// caps below drag there guards nothing.
fn idle_floor(tp: &TransmissionParams, supply: DriveSupply, omega_e: f32) -> Option<f32> {
    let point = (tp.engine.idle_rpm - engine_control().idle_guard_band_rpm.value) * RPM_TO_RAD;
    let [_, catch, drag] = engine_torques(tp, supply, point, 0.0);
    (omega_e >= 0.0 && catch > drag).then_some(point)
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

/// Advance one owner's mechanical state under tick-constant orders and affine sampled loads.
/// The same operation serves live substeps, resting trials and the sandbox.
pub(crate) fn advance(
    fp: &DriveConstruction,
    tp: &TransmissionParams,
    state: &mut DriveState,
    orders: &Orders,
    h: f32,
) -> TransmissionReport {
    assert!(
        h.is_finite() && h > 0.0,
        "a mechanical substep has a positive finite duration"
    );
    let mut controls = orders.controls;
    let reference = controls.input;
    controls.input.dt = h;
    controls.input.speeds = state.speeds;
    // R(v) = R(v_ref) + slope * (v - v_ref); the sampled law stays fixed for this tick.
    controls.input.reactions = [0, 1].map(|i| {
        reference.reactions[i]
            + reference.reaction_slopes[i] * (state.speeds[i] - reference.speeds[i])
    });
    let inp = &controls.input;
    let st = &mut state.transmission;
    let mean = (inp.speeds[0] + inp.speeds[1]) * 0.5;
    #[cfg(feature = "bitprobe")]
    let mut probe = input_probe(st, inp, mean, (inp.speeds[0] - inp.speeds[1]) * 0.5);
    let ratio = tp.ratio(st.reverse, st.gear);
    let k = ratio / tp.sprocket_radius;
    let omega_pre = st.omega_e;
    let (torque, power) = crank_free_torque(
        tp,
        &controls.input,
        !controls.clutch_open,
        controls.shifting,
        controls.propulsive,
        controls.dir * mean,
        k,
        st.omega_e,
        tp.engine.idle_rpm * RPM_TO_RAD,
        #[cfg(feature = "bitprobe")]
        &mut probe,
    );
    let (mut report, clutch_torque, steer_force) = advance_rows(fp, tp, st, &controls, torque);
    report.power_available = power;
    #[cfg(feature = "bitprobe")]
    {
        force_probe(
            tp,
            inp,
            (
                controls.dir * mean,
                ratio,
                controls.dir * (report.forces[0] + report.forces[1]),
                controls.propulsive,
            ),
            st.clutch,
            &mut probe,
        );
        probe.assignments_tried = report.assignments_tried;
        probe.mechanical_assignments = report.assignments_tried;
        probe.mechanical_rows = report.rows;
        probe.mechanical_advances = 1;
        probe.max_substep_assignments = report.assignments_tried;
        probe.max_substep_rows = report.rows;
        probe.direction = controls.dir;
        probe.shaft_speed = controls.dir * mean;
        probe.gear_reduction = ratio;
        probe.k = k;
        probe.omega_idle = tp.engine.idle_rpm * RPM_TO_RAD;
        probe.omega_floor = if st.launch != LaunchState::Locked {
            tp.launch_omega(controls.propulsive)
        } else {
            idle_floor(tp, inp.supply, omega_pre).unwrap_or(0.0)
        };
        probe.locked = report.locked;
        probe.shifting = controls.shifting;
        probe.engaged = st.clutch != ClutchState::Out;
        probe.tau_c = clutch_torque;
        probe.clutch_force_n = controls.dir * k * clutch_torque;
        probe.steering_force_n = steer_force;
        probe.lambda = steer_force;
        probe.j = [0.5, -0.5];
        probe.omega_integrated = st.omega_e;
        probe.omega_end = st.omega_e;
        probe.forces = report.forces;
        probe.next_speeds = report.next_speeds;
        let powers =
            [0, 1].map(|i| report.forces[i] * (inp.speeds[i] + report.next_speeds[i]) * 0.5);
        probe.power_left = powers[0];
        probe.power_right = powers[1];
        probe.power_positive = powers.iter().map(|p| p.max(0.0)).sum();
        probe.power_negative = powers.iter().map(|p| (-p).max(0.0)).sum();
        probe.power_net = powers.iter().sum();
        report.bitprobe = probe;
    }
    #[cfg(not(feature = "bitprobe"))]
    let _ = (omega_pre, clutch_torque, steer_force);
    for (phase, speed) in state.phases.iter_mut().zip(state.speeds) {
        *phase += f64::from(speed * h);
    }
    state.speeds = report.next_speeds;
    report
}

#[cfg(test)]
#[path = "transmission/tests.rs"]
mod tests;
