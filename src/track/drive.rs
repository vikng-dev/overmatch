//! The drive-command seam: raw two-axis intent → slewed axes → per-side belt commands. One pure
//! implementation shared by the game adapter (`track::sim`) and the sandbox, both running it on
//! the FIXED tick — so identical raw command scripts produce identical shaped axes and side
//! commands everywhere (the harness enters through raw edges and tests the slew as part of the
//! path, not around it).
//!
//! Deliberately NOT in [`super::forces`]: the force core is per-side physics; input feel
//! (slew), differential mixing, and the side clamp are vehicle command policy — a separate
//! seam, so testing one belt never requires understanding keyboard shaping.

/// The two-axis drive intent/state in [−1, 1] per axis.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct DriveAxes {
    pub throttle: f32,
    pub steer: f32,
}

/// The game's damage model: what happens at the controls this tick, and what the drivetrain
/// supplies to it. A brake hold changes the intent, never the supply.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DriveTarget {
    pub intent: DriveIntent,
    pub supply: super::transmission::DriveSupply,
}

/// Who acts on the controls this tick.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DriveIntent {
    /// The driver chases these axes. Zero is a released throttle: the tank coasts under engine
    /// drag.
    Axes(DriveAxes),
    /// Release both axes and hold the service brakes: the tank brakes to a stop, then the parking
    /// latch holds it (`transmission::TransmissionInput::brake`).
    Brake,
    /// The game's damage model: the authored controls are unstaffed, so no command exists.
    /// The controls release at once; the existing park lever retains its state.
    Unmanned,
}

/// Shared keyboard feel, admitted before play by the command core. The embedded data is also
/// used by the track sandbox, whose arrow keys supply raw full-axis intent without notches.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DriveFeel {
    pub(crate) tap_window_s: crate::provenance::Sourced<f64>,
    pub(crate) notch_step: crate::provenance::Sourced<f32>,
    pub(crate) rise_per_second: crate::provenance::Sourced<f32>,
    pub(crate) wide_on: crate::provenance::Sourced<f32>,
    pub(crate) wide_off: crate::provenance::Sourced<f32>,
    pub(crate) tight_on: crate::provenance::Sourced<f32>,
    pub(crate) tight_off: crate::provenance::Sourced<f32>,
}

impl DriveFeel {
    fn validate(&self) {
        assert!(
            self.tap_window_s.0.is_finite() && self.tap_window_s.0 > 0.0,
            "assets/input/drive.ron: tap_window_s must be finite and positive; author the tap/hold boundary"
        );
        assert!(
            self.notch_step.0.is_finite() && self.notch_step.0 > 0.0 && self.notch_step.0 <= 1.0,
            "assets/input/drive.ron: notch_step must be in (0, 1]; author a bounded lever step"
        );
        assert!(
            self.rise_per_second.0.is_finite() && self.rise_per_second.0 > 0.0,
            "assets/input/drive.ron: rise_per_second must be finite and positive; author the keyboard rise rate"
        );
        assert!(
            0.0 < self.wide_off.0
                && self.wide_off.0 < self.wide_on.0
                && self.wide_on.0 < self.tight_off.0
                && self.tight_off.0 < self.tight_on.0
                && self.tight_on.0 <= 1.0,
            "assets/input/drive.ron: author ordered wide_off < wide_on < tight_off < tight_on within [0, 1]"
        );
        for (name, off, on) in [
            ("wide", self.wide_off.0, self.wide_on.0),
            ("tight", self.tight_off.0, self.tight_on.0),
        ] {
            // The lever can start stepping from zero or either clamped endpoint.
            for origin in [-1.0, 0.0, 1.0] {
                let first_notch =
                    origin + ((off - origin) / self.notch_step.0).ceil() * self.notch_step.0;
                assert!(
                    first_notch >= on || first_notch > 1.0,
                    "assets/input/drive.ron: {name} detent hysteresis contains a persistent notch; move its thresholds or notch_step so steps from zero and clamped endpoints avoid the band"
                );
            }
        }
    }
}

pub(crate) fn drive_feel() -> &'static DriveFeel {
    static DATA: std::sync::LazyLock<DriveFeel> = std::sync::LazyLock::new(|| {
        let data: DriveFeel = crate::provenance::from_shipped_str(
            "assets/input/drive.ron",
            include_str!("../../assets/input/drive.ron"),
        )
        .expect("assets/input/drive.ron: author tap_window_s, notch_step and rise_per_second with provenance");
        data.validate();
        data
    });
    &DATA
}

/// Move `current` toward `target` by at most `step` — never overshoots, exact at the target.
fn approach(current: f32, target: f32, step: f32) -> f32 {
    if current < target {
        (current + step).min(target)
    } else {
        (current - step).max(target)
    }
}

/// One fixed tick of pedal travel. Opposing nonzero signs snap to neutral for this tick;
/// subsequent ticks approach the target at the authored keyboard rise rate. Release and
/// same-sign notch changes use that same rate without overshoot.
pub fn shape_drive(current: DriveAxes, target: DriveAxes, dt: f32) -> DriveAxes {
    let step = drive_feel().rise_per_second.0 * dt;
    let shape = |current: f32, target: f32| {
        if current * target < 0.0 {
            0.0
        } else {
            approach(current, target, step)
        }
    };
    DriveAxes {
        throttle: shape(current.throttle, target.throttle),
        steer: shape(current.steer, target.steer),
    }
}

impl DriveAxes {
    /// The additive differential mix, `[left, right]`: steer adds to the left track and
    /// subtracts from the right, each side clamped to [−1, 1] independently. The clamp is
    /// command policy and lives HERE, not in the adapters — at full throttle it saturates the
    /// outer track and only slows the inner one (expected nonlinearity near full throttle;
    /// radius-monotonicity gates must use unsaturated commands).
    pub fn side_commands(self) -> [f32; 2] {
        [
            (self.throttle + self.steer).clamp(-1.0, 1.0),
            (self.throttle - self.steer).clamp(-1.0, 1.0),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixture: the simulation tick used to exercise pedal travel.
    const DT: f32 = 1.0 / 64.0;

    /// Fixture: stepping back from a clamped endpoint reaches a notch inside the tight
    /// detent band. Admission must refuse it so persistent steering has no approach history.
    #[test]
    #[should_panic(expected = "notch_step")]
    fn notch_admission_checks_clamped_endpoint_steps() {
        let mut feel: DriveFeel = crate::provenance::from_shipped_str(
            "assets/input/drive.ron",
            include_str!("../../assets/input/drive.ron"),
        )
        .unwrap();
        feel.notch_step.0 = 0.4;
        feel.wide_off.0 = 0.05;
        feel.wide_on.0 = 0.15;
        feel.tight_off.0 = 0.55;
        feel.tight_on.0 = 0.65;
        let persistent = 1.0 - feel.notch_step.0;
        assert!(persistent >= feel.tight_off.0 && persistent < feel.tight_on.0);
        feel.validate();
    }

    #[test]
    fn slew_rises_exactly_and_never_overshoots() {
        let mut axes = DriveAxes::default();
        let target = DriveAxes {
            throttle: 1.0,
            steer: 0.0,
        };
        let ticks = (1.0 / (drive_feel().rise_per_second.0 * DT)).ceil() as usize;
        for _ in 1..ticks {
            axes = shape_drive(axes, target, DT);
            assert!(axes.throttle < 1.0);
        }
        axes = shape_drive(axes, target, DT);
        assert_eq!(axes.throttle, 1.0);
        // At the target, further ticks are exact no-ops (no oscillation).
        assert_eq!(shape_drive(axes, target, DT), axes);

        // Descending travel obeys the same authored rate.
        for _ in 1..ticks {
            axes = shape_drive(axes, DriveAxes::default(), DT);
            assert!(axes.throttle > 0.0);
        }
        axes = shape_drive(axes, DriveAxes::default(), DT);
        assert_eq!(axes, DriveAxes::default());
    }

    /// The Driver must see neutral immediately on reversal; a slew through the previous sign
    /// would keep asking for the old direction. Each axis reverses independently.
    #[test]
    fn reversal_snaps_to_zero_before_rising_in_the_new_direction() {
        let current = DriveAxes {
            throttle: 0.5,
            steer: -0.75,
        }; // Fixture.
        let target = DriveAxes {
            throttle: -1.0,
            steer: 1.0,
        };
        let snapped = shape_drive(current, target, DT);
        assert_eq!(snapped, DriveAxes::default());
        let next = shape_drive(snapped, target, DT);
        let step = drive_feel().rise_per_second.0 * DT;
        assert_eq!(
            next,
            DriveAxes {
                throttle: -step,
                steer: step
            }
        );
    }

    #[test]
    fn side_mixer_signs_and_clamp() {
        // Positive steer speeds the LEFT track (index 0) — the sign convention the yaw-gate
        // tests key on (left-fast → right turn).
        let cmds = DriveAxes {
            throttle: 0.5,
            steer: 0.2,
        }
        .side_commands();
        assert_eq!(cmds, [0.7, 0.3]);
        // Saturation: the outer side clamps, the inner keeps moving.
        let cmds = DriveAxes {
            throttle: 1.0,
            steer: 0.4,
        }
        .side_commands();
        assert_eq!(cmds, [1.0, 0.6]);
        let cmds = DriveAxes {
            throttle: 0.0,
            steer: 1.0,
        }
        .side_commands();
        assert_eq!(cmds, [1.0, -1.0]);
    }
}
