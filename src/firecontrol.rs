//! Fire control: the per-weapon superelevation solution. The bore is laid on the line of sight by the
//! pure servos; to drop a shell onto a target at the dialed range we lob the *aim point* above that
//! line by a small angle — the superelevation — so the bore physically elevates and the shell arcs
//! back down onto the sight line. The servos and the shell never learn about it; only the aim commit
//! does (`aim::commit_aim`, `sight::drive_gunner_aim`), and the gunner optic depresses its view by the
//! same angle so the reticle holds the target while the barrel rides above it.
//!
//! With air drag there is no closed form, so this is the real-world *range table*: at spawn we fire the
//! weapon's trajectory — off `ballistics`' shared flight kernel, so the solution matches the actual
//! shell — at a sweep of launch angles, record where each lands, and invert that to a `range → angle`
//! lookup. Each weapon gets its own; the draggy coax needs far more lob than the 88 at equal range,
//! which *is* the per-ammo ballistic scale on a real sight.
//!
//! Deep module: a small interface (`RangeTable::superelevation`, `Ranging`, `lob`) over the
//! sweep / invert / interpolate inside.

use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;

use crate::ballistics::{drag_k, freeflight_step};
use crate::command::gather_commands;
use crate::sight::in_gunner;
use crate::state::{GameplaySet, PlayerInputSet};

/// The player-dialed range to the target (m). The Tiger has no rangefinder, so ranging is a *skill*:
/// the player estimates and scrolls this in the optic, and the gun lobs for it — dial wrong and the
/// shot falls short or long. One shared value drives the superelevation in both views.
#[derive(Resource)]
pub struct Ranging {
    pub range: f32,
}

impl Default for Ranging {
    fn default() -> Self {
        Self { range: 800.0 }
    }
}

/// A weapon's precomputed range table: ascending `(range_m, superelevation_rad)` rows. Built once per
/// weapon at spawn (the trajectory is fixed) and stored on its muzzle; the aim commit looks up the
/// dialed range to lob the aim point. Each weapon has its own — the coax out-lobs the 88 at equal
/// range, the per-ammo ballistic scale.
#[derive(Component)]
pub struct RangeTable {
    /// `(range, superelevation)`, ascending by range. Monotonic by construction: only the rising
    /// (direct-fire) branch is kept, so a binary-free linear scan interpolates cleanly.
    points: Vec<(f32, f32)>,
}

impl RangeTable {
    /// Build by sweeping launch angles, simulating each shot with `ballistics`' flight kernel, and
    /// keeping `(range, angle)` along the rising (direct-fire) branch — so the predicted range matches
    /// where the live shell lands. Stops at the weapon's max range (where range stops increasing) or
    /// the engagement envelope.
    pub fn for_weapon(speed: f32, caliber: f32, mass: f32) -> Self {
        const DT: f32 = 0.005;
        const ANGLE_STEP: f32 = 0.000_2; // ~0.01° — fine range resolution near flat fire
        const MAX_ANGLE: f32 = std::f32::consts::FRAC_PI_4; // past 45° range only falls
        const MAX_RANGE: f32 = 5_000.0; // engagement envelope; the 88 reaches it, the coax peaks first
        const MAX_TIME: f32 = 30.0; // flight-time guard against a degenerate near-vertical shot

        let k = drag_k(caliber, mass);
        let mut points = Vec::new();
        let mut angle = 0.0;
        let mut last_range = f32::NEG_INFINITY;
        while angle <= MAX_ANGLE {
            if let Some(range) = simulate_range(speed, angle, k, DT, MAX_TIME) {
                if range < last_range {
                    break; // past the max-range angle — keep only the direct-fire branch
                }
                points.push((range, angle));
                last_range = range;
                if range >= MAX_RANGE {
                    break;
                }
            }
            angle += ANGLE_STEP;
        }
        Self { points }
    }

    /// Superelevation (rad) to lob over the line of sight for a target at `range` (m). Linear
    /// interpolation between rows; clamped to the weapon's reach — a range past its max returns the
    /// max-range angle (the gun saturates rather than the lay jumping).
    pub fn superelevation(&self, range: f32) -> f32 {
        let Some(&(first_range, first_angle)) = self.points.first() else {
            return 0.0;
        };
        if range <= first_range {
            return first_angle;
        }
        for w in self.points.windows(2) {
            let (r0, a0) = w[0];
            let (r1, a1) = w[1];
            if range <= r1 {
                return a0 + (a1 - a0) * (range - r0) / (r1 - r0);
            }
        }
        self.points.last().map_or(0.0, |&(_, a)| a)
    }
}

/// Simulate one shot in the vertical plane: launch at `angle` above horizontal at `speed`, step the
/// shared flight kernel until it falls back to firing height (descending through `y = 0`), and return
/// the horizontal distance at that crossing (interpolated across the final step). `None` if it never
/// comes down within `max_time` (a degenerate near-vertical or zero-speed shot).
fn simulate_range(speed: f32, angle: f32, k: f32, dt: f32, max_time: f32) -> Option<f32> {
    let (s, c) = angle.sin_cos();
    let mut vel = Vec3::new(speed * c, speed * s, 0.0);
    let mut pos = Vec3::ZERO;
    let mut t = 0.0;
    while t < max_time {
        let next_vel = freeflight_step(vel, k, dt);
        let next_pos = pos + next_vel * dt;
        if next_pos.y <= 0.0 && next_vel.y < 0.0 {
            // Interpolate the exact horizontal distance where the path crossed firing height.
            let span = pos.y - next_pos.y;
            let frac = if span > 0.0 { pos.y / span } else { 0.0 };
            return Some(pos.x + (next_pos.x - pos.x) * frac);
        }
        vel = next_vel;
        pos = next_pos;
        t += dt;
    }
    None
}

/// Rotate a vector by `theta` radians in its own vertical plane (about the horizontal axis
/// perpendicular to it): positive lobs it up, negative depresses it. The aim commit lobs the aim point
/// up by the superelevation so the bore elevates; length is preserved, so the point keeps its range.
pub fn lob(v: Vec3, theta: f32) -> Vec3 {
    match Dir3::new(v.cross(Vec3::Y)) {
        Ok(right) => Quat::from_axis_angle(right.into(), theta) * v,
        Err(_) => v, // vertical — no well-defined vertical plane
    }
}

/// The player's range dial — client-side control state; its value rides to the sim inside the
/// command (`gather_commands` copies it absolute each frame).
pub fn client_plugin(app: &mut App) {
    app.init_resource::<Ranging>()
        // Scroll dials the range in the optic (mutually exclusive with `orbit_camera`'s third-person
        // zoom, which also reads scroll — they're gated on opposite view modes, so no contention).
        //
        // In `BeforeFixedMainLoop`, right before `gather_commands`, so the dial the sim reads is
        // *this* frame's scroll: `gather_commands` copies `Ranging::range` into the command, so a
        // dial left in `Update` would only reach the sim one render frame later. `.before` makes the
        // read-after-write ordering explicit (both touch the `Ranging` resource).
        .add_systems(
            RunFixedMainLoop,
            adjust_range
                .run_if(in_gunner)
                .before(gather_commands)
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .in_set(PlayerInputSet)
                .in_set(GameplaySet),
        );
}

/// Scroll dials the range in the optic (range, not zoom — the optic's magnification is fixed). The
/// dialed value persists into third-person, where scroll is the camera dolly.
fn adjust_range(scroll: Res<AccumulatedMouseScroll>, mut ranging: ResMut<Ranging>) {
    const STEP: f32 = 50.0;
    const MIN: f32 = 50.0;
    const MAX: f32 = 4000.0;
    ranging.range = (ranging.range + scroll.delta.y * STEP).clamp(MIN, MAX);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_gun() -> RangeTable {
        RangeTable::for_weapon(773.0, 0.088, 10.2) // 88 mm, 10.2 kg
    }
    fn coax() -> RangeTable {
        RangeTable::for_weapon(755.0, 0.0079, 0.0118) // 7.9 mm, 11.8 g
    }

    /// The table inverts its own forward simulation: a shell fired at `superelevation(R)` lands at R.
    /// Built from the same flight kernel the live shell uses, so the aim solution can't drift from the
    /// trajectory.
    #[test]
    fn superelevation_round_trips_to_range() {
        let table = main_gun();
        let k = drag_k(0.088, 10.2);
        for &range in &[400.0_f32, 800.0, 1500.0, 2000.0] {
            let theta = table.superelevation(range);
            let landed = simulate_range(773.0, theta, k, 0.005, 30.0).unwrap();
            assert!(
                (landed - range).abs() < range * 0.02 + 5.0,
                "dialed {range} m, shell landed {landed} m"
            );
        }
    }

    /// The whole reason laying matters later: the light-for-bore coax bleeds speed fast, so it needs
    /// markedly more lob than the 88 at the same range.
    #[test]
    fn coax_out_lobs_the_main_gun() {
        let (main, coax) = (main_gun(), coax());
        for &range in &[400.0_f32, 800.0, 1200.0] {
            assert!(
                coax.superelevation(range) > main.superelevation(range),
                "coax should need more lob than the 88 at {range} m"
            );
        }
    }

    /// `lob` raises a forward vector and preserves its length (so the lobbed aim point keeps its range).
    #[test]
    fn lob_raises_and_preserves_length() {
        let raised = lob(Vec3::NEG_Z, 0.1);
        assert!(raised.y > 0.0, "lobbing a forward vector should raise it");
        assert!(
            (raised.length() - 1.0).abs() < 1.0e-5,
            "lob must preserve length"
        );
    }
}
