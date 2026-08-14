//! The interpolation delay, derived from the measured link rather than pinned.
//!
//! Lightyear places the interpolation clock at `remote_estimate − (delay + jitter_margin)` and
//! anchors `remote_estimate` RTT/2 AHEAD of the newest received keyframe (`lightyear_sync`
//! `remote.rs`: `network_delay = ping_manager.rtt() / 2`). A keyframe therefore exists ahead of the
//! cursor only while `delay ≥ rtt/2 + largest snapshot gap`; below that lightyear clamps to the
//! newest sample (`lightyear_interpolation` `registry.rs`) — freeze, then step, with no counter, no
//! event and no warning on the path.
//!
//! Lightyear's own `delay` term is `max(send_interval_ratio × remote_send_interval, min_delay)`,
//! which carries the gap half only, and our per-tick server advertises `send_interval = 0` — that
//! collapses the ratio term entirely (`tests/net_interp_delay.rs` pins the degenerate). `min_delay`
//! is the only writable slot, so it carries both terms:
//!
//! ```text
//! min_delay = rtt/2 + send_interval_ratio × tick
//! ```
//!
//! `sync_timelines` re-reads `&InterpolationConfig` every frame with no caching, so writing the
//! component is the whole mechanism. `rtt()` is already an EWMA (α = 1/12 over 100 ms pings) behind
//! an outlier clamp, and lightyear converges the timeline by ±5% clock speed rather than by a step,
//! so the law needs no smoothing and no hysteresis of its own.
//!
//! Derivation, rejected alternatives and the certification protocol:
//! `.agents/scratch/interp-delay-derivation-2026-08-14.md`.

use core::time::Duration;

use bevy::prelude::*;
use lightyear::core::tick::TickDuration;
use lightyear::interpolation::timeline::InterpolationConfig;
use lightyear::prelude::{PingManager, SyncSystems};

/// Log-rate guard, not a term of the law: `rtt()` is a continuous EWMA, so the derived value moves
/// every frame and an unconditional log is per-frame spam.
const LOG_STEP: Duration = Duration::from_millis(5);

/// `OVERMATCH_INTERP_DELAY_MS`: SET pins `min_delay` for the session — the certification instrument
/// needs deliberately-undersized runs (report §5.3) and the A/B against the retired 100 ms pin.
/// UNSET runs the derived law.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
enum DelayMode {
    Fixed(Duration),
    Derived,
}

/// The law. `send_interval_ratio` is read back from the config being written, so a lightyear default
/// change moves the gap term without a second copy of the constant here.
fn derived_min_delay(rtt: Duration, tick: Duration, send_interval_ratio: f32) -> Duration {
    rtt / 2 + tick.mul_f32(send_interval_ratio)
}

/// Resolve the mode, mount the deriving system, and hand back the client entity's initial
/// `InterpolationConfig`. Single call site (`net::client`), so the env var is read exactly once.
pub(super) fn install(app: &mut App, tick: Duration) -> InterpolationConfig {
    let mode = match super::harness::env_parse::<u64>("OVERMATCH_INTERP_DELAY_MS") {
        Some(ms) => DelayMode::Fixed(Duration::from_millis(ms)),
        None => DelayMode::Derived,
    };
    let defaults = InterpolationConfig::default();
    let initial = match mode {
        DelayMode::Fixed(delay) => delay,
        // No RTT sample exists before the first pong, and the law at rtt = 0 is the gap term alone;
        // `derive_interpolation_delay` overwrites this before the first `SyncSystems::Sync`.
        DelayMode::Derived => derived_min_delay(Duration::ZERO, tick, defaults.send_interval_ratio),
    };
    match mode {
        DelayMode::Fixed(delay) => info!(
            "net: interpolation min_delay FIXED {} ms [OVERMATCH_INTERP_DELAY_MS] — derived law off",
            delay.as_millis()
        ),
        DelayMode::Derived => info!(
            "net: interpolation min_delay DERIVED = rtt/2 + {} x tick ({:.1} ms at rtt 0) \
             [OVERMATCH_INTERP_DELAY_MS unset]",
            defaults.send_interval_ratio,
            millis(initial)
        ),
    }
    app.insert_resource(mode);
    app.add_systems(
        PostUpdate,
        derive_interpolation_delay.before(SyncSystems::Sync),
    );
    defaults.with_min_delay(initial)
}

/// Rewrite `min_delay` from the measured RTT every frame, ahead of the frame's timeline sync. The
/// `!=` guard keeps change detection quiet while the estimate is settled.
fn derive_interpolation_delay(
    mode: Res<DelayMode>,
    tick: Res<TickDuration>,
    mut clients: Query<(&mut InterpolationConfig, &PingManager)>,
    mut logged: Local<Option<Duration>>,
) {
    if matches!(*mode, DelayMode::Fixed(_)) {
        return;
    }
    for (mut config, pings) in &mut clients {
        let derived = derived_min_delay(pings.rtt(), tick.0, config.send_interval_ratio);
        if config.min_delay == derived {
            continue;
        }
        if logged.is_none_or(|last| last.abs_diff(derived) >= LOG_STEP) {
            info!(
                "net: interpolation min_delay {:.1} ms (rtt {:.1} ms, jitter {:.1} ms)",
                millis(derived),
                millis(pings.rtt()),
                millis(pings.jitter())
            );
            *logged = Some(derived);
        }
        config.min_delay = derived;
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::derived_min_delay;

    /// The game's fixed tick (64 Hz) and lightyear's `send_interval_ratio`, spelled out rather than
    /// read from the config — `tests/net_interp_delay.rs` is what fires when lightyear moves the
    /// ratio, and this test must pin the arithmetic independently of it.
    const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);
    const RATIO: f32 = 1.7;

    /// `tick.mul_f32` routes through `f32 → f64`, so the gap term lands ~1 ns off exact. Any
    /// structural change to the law moves the result by ≥ 26 ms.
    const EPSILON: Duration = Duration::from_micros(10);

    fn assert_close(actual: Duration, expected: Duration, what: &str) {
        assert!(
            actual.abs_diff(expected) < EPSILON,
            "{what}: derived {actual:?}, expected {expected:?}"
        );
    }

    /// Absolute values at three operating points. Dropping `rtt/2` fails the 100/250 cases;
    /// dropping the gap term fails all three; using `rtt` instead of `rtt/2` fails 100/250; a ratio
    /// of 1.0 or 2.0 fails all three.
    #[test]
    fn law_is_half_rtt_plus_the_ratio_gap() {
        // rtt 0 (loopback): the gap term alone — 15.625 ms x 1.7.
        assert_close(
            derived_min_delay(Duration::ZERO, TICK, RATIO),
            Duration::from_nanos(26_562_500),
            "loopback",
        );
        // rtt 100 ms (droplet, typical): 50 + 26.5625.
        assert_close(
            derived_min_delay(Duration::from_millis(100), TICK, RATIO),
            Duration::from_nanos(76_562_500),
            "droplet",
        );
        // rtt 250 ms (the long clean path the 100 ms pin froze on): 125 + 26.5625.
        assert_close(
            derived_min_delay(Duration::from_millis(250), TICK, RATIO),
            Duration::from_nanos(151_562_500),
            "long path",
        );
    }

    /// The slope in RTT is exactly 1/2, independent of the gap term: 200 ms more RTT buys 100 ms
    /// more delay. Fails if the `rtt/2` term is dropped (slope 0) or left undivided (slope 1).
    #[test]
    fn law_slope_in_rtt_is_one_half() {
        let low = derived_min_delay(Duration::from_millis(40), TICK, RATIO);
        let high = derived_min_delay(Duration::from_millis(240), TICK, RATIO);
        assert_close(high - low, Duration::from_millis(100), "rtt slope");
    }

    /// The gap term is exactly `send_interval_ratio × tick`, independent of RTT: the same RTT under
    /// two ratios differs by exactly one tick. Fails if the gap term is dropped or is not scaled by
    /// the ratio.
    #[test]
    fn gap_term_scales_with_the_send_interval_ratio() {
        let rtt = Duration::from_millis(60);
        let narrow = derived_min_delay(rtt, TICK, RATIO);
        let wide = derived_min_delay(rtt, TICK, RATIO + 1.0);
        assert_close(wide - narrow, TICK, "ratio scaling");
    }
}
