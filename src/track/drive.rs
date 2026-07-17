//! The drive-command seam: raw two-axis intent → slewed axes → per-side belt commands. One pure
//! implementation shared by the game adapter (`track::sim`) and the sandbox, both running it on
//! the FIXED tick — so identical raw command scripts produce identical shaped axes and side
//! commands everywhere (the harness enters through raw edges and tests the slew as part of the
//! path, not around it).
//!
//! Deliberately NOT in [`super::forces`]: the force core is per-side physics; input feel
//! (slew), differential mixing, and the side clamp are vehicle command policy — a separate
//! seam, so testing one belt never requires understanding keyboard shaping (codex commit-E
//! review §2).

/// The two-axis drive intent/state in [−1, 1] per axis.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct DriveAxes {
    pub throttle: f32,
    pub steer: f32,
}

/// Command slew (per second): the vehicle's input shaping, SEPARATE from the belt governor —
/// folding them changes keyboard feel and damage-recovery semantics (codex phase-B #9).
/// Provenance: adopted into the sandbox reference untuned; a playtest feel dial, not a physics
/// constant.
pub const DRIVE_SLEW_PER_SECOND: f32 = 4.0;

/// Move `current` toward `target` by at most `step` — never overshoots, exact at the target.
fn approach(current: f32, target: f32, step: f32) -> f32 {
    if current < target {
        (current + step).min(target)
    } else {
        (current - step).max(target)
    }
}

/// One fixed tick of command slew: each axis approaches its target at
/// [`DRIVE_SLEW_PER_SECOND`], independently (a reversal −1→+1 takes twice the 0→1 time).
pub fn shape_drive(current: DriveAxes, target: DriveAxes, dt: f32) -> DriveAxes {
    let step = DRIVE_SLEW_PER_SECOND * dt;
    DriveAxes {
        throttle: approach(current.throttle, target.throttle, step),
        steer: approach(current.steer, target.steer, step),
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

    const DT: f32 = 1.0 / 64.0;

    #[test]
    fn slew_rises_exactly_and_never_overshoots() {
        let mut axes = DriveAxes::default();
        let target = DriveAxes {
            throttle: 1.0,
            steer: 0.0,
        };
        // 4.0/s at 64 Hz = 0.0625/tick → exactly 16 ticks from 0 to 1.
        for _ in 0..15 {
            axes = shape_drive(axes, target, DT);
            assert!(axes.throttle < 1.0);
        }
        axes = shape_drive(axes, target, DT);
        assert_eq!(axes.throttle, 1.0);
        // At the target, further ticks are exact no-ops (no oscillation).
        assert_eq!(shape_drive(axes, target, DT), axes);
    }

    #[test]
    fn reversal_takes_twice_the_rise_and_release_returns_to_zero() {
        let mut axes = DriveAxes {
            throttle: 1.0,
            steer: -1.0,
        };
        let target = DriveAxes {
            throttle: -1.0,
            steer: 1.0,
        };
        for _ in 0..32 {
            axes = shape_drive(axes, target, DT);
        }
        assert_eq!(axes, target);

        let mut axes = DriveAxes {
            throttle: 0.5,
            steer: 0.5,
        };
        for _ in 0..8 {
            axes = shape_drive(axes, DriveAxes::default(), DT);
        }
        assert_eq!(axes, DriveAxes::default());
    }

    #[test]
    fn zero_dt_is_identity() {
        let axes = DriveAxes {
            throttle: 0.3,
            steer: -0.7,
        };
        assert_eq!(
            shape_drive(
                axes,
                DriveAxes {
                    throttle: 1.0,
                    steer: 1.0
                },
                0.0
            ),
            axes
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
