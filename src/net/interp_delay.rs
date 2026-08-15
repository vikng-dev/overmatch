//! The interpolation delay, derived from the stream-measured arrival-delay distribution rather
//! than pinned.
//!
//! Lightyear places the interpolation clock at `remote_estimate − (delay + jitter_margin)` and
//! anchors `remote_estimate` RTT/2 AHEAD of the newest received keyframe (`lightyear_sync`
//! `remote.rs`: `network_delay = ping_manager.rtt() / 2`). A keyframe therefore exists ahead of the
//! cursor only while `delay ≥ rtt/2 + largest snapshot gap`; below that lightyear clamps to the
//! newest sample (`lightyear_interpolation` `registry.rs`) — the buffer edge `net::extrapolate`
//! instruments and softens.
//!
//! Lightyear's own `delay` term is `max(send_interval_ratio × remote_send_interval, min_delay)`,
//! which carries the gap half only, and our per-tick server advertises `send_interval = 0` — that
//! collapses the ratio term entirely (`tests/net_interp_delay.rs` pins the degenerate). `min_delay`
//! is the only writable slot, so it carries the whole law:
//!
//! ```text
//! headroom  = Q_p{d_i} − min{d_i} + one send interval    (the arrival-delay distribution's
//!                                                         covered spread — `net::sync_margin`'s
//!                                                         estimator, empirical, no Gaussian —
//!                                                         plus the gap to the next keyframe)
//! min_delay = rtt/2 + headroom                           (rtt/2 cancels the anchor, as above)
//! ```
//!
//! The rtt/2 term stays on the ping EWMA (it cancels the anchor, which is built from the same
//! EWMA); every jitter term now comes from the measured stream. The Gaussian margin chain on top
//! is zeroed by `net::sync_margin` — the quantile subsumes it — keeping the half-tick floor.
//!
//! `sync_timelines` re-reads `&InterpolationConfig` every frame with no caching, so writing the
//! component is the whole mechanism. Lightyear converges the timeline by ±5% clock speed rather
//! than by a step, so the law needs no smoothing and no hysteresis of its own.
//!
//! Derivation and the contract constants behind `Q_p`:
//! `net::sync_margin`'s module doc and `.agents/scratch/adaptive-cursor-frontier-2026-08-15.md` §1.

use core::time::Duration;

use bevy::prelude::*;
use lightyear::core::tick::TickDuration;
use lightyear::interpolation::timeline::InterpolationConfig;
use lightyear::prelude::{PingManager, SyncSystems};

use super::sync_margin::{ArrivalDelay, ArrivalStats};

/// Log-rate guard, not a term of the law: `rtt()` and the quantile spread move continuously, so an
/// unconditional log is per-frame spam.
const LOG_STEP: Duration = Duration::from_millis(5);

/// `OVERMATCH_INTERP_DELAY_MS`: SET pins `min_delay` for the session — the certification instrument
/// needs deliberately-undersized runs (report §5.3) and the A/B against the retired 100 ms pin.
/// UNSET runs the derived law.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DelayMode {
    Fixed(Duration),
    Derived,
}

/// The law. The send interval is one tick — the server replicates every tick (`send_interval = 0`,
/// the degenerate `tests/net_interp_delay.rs` pins), so the gap to the next keyframe is one tick.
fn derived_min_delay(rtt: Duration, stats: &ArrivalStats, tick: Duration) -> Duration {
    rtt / 2 + stats.spread() + tick
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
        // No RTT or arrival sample exists before the link opens; the law at rtt = spread = 0 is
        // the one-interval gap term alone, and `derive_interpolation_delay` overwrites this before
        // the first `SyncSystems::Sync`.
        DelayMode::Derived => derived_min_delay(Duration::ZERO, &ArrivalStats::default(), tick),
    };
    match mode {
        DelayMode::Fixed(delay) => info!(
            "net: interpolation min_delay FIXED {} ms [OVERMATCH_INTERP_DELAY_MS] — derived law off",
            delay.as_millis()
        ),
        DelayMode::Derived => info!(
            "net: interpolation min_delay DERIVED = rtt/2 + (Qp − min) + one tick ({:.1} ms cold) \
             [OVERMATCH_INTERP_DELAY_MS unset]",
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

/// Rewrite `min_delay` from the measured link every frame, ahead of the frame's timeline sync
/// (and after `net::sync_margin` refreshes the estimator digest). The `!=` guard keeps change
/// detection quiet while the estimate is settled.
pub(super) fn derive_interpolation_delay(
    mode: Res<DelayMode>,
    tick: Res<TickDuration>,
    estimator: Res<ArrivalDelay>,
    mut clients: Query<(&mut InterpolationConfig, &PingManager)>,
    mut logged: Local<Option<Duration>>,
) {
    if matches!(*mode, DelayMode::Fixed(_)) {
        return;
    }
    for (mut config, pings) in &mut clients {
        let derived = derived_min_delay(pings.rtt(), &estimator.stats, tick.0);
        if config.min_delay == derived {
            continue;
        }
        if logged.is_none_or(|last| last.abs_diff(derived) >= LOG_STEP) {
            info!(
                "net: interpolation min_delay {:.1} ms (rtt {:.1} ms, {})",
                millis(derived),
                millis(pings.rtt()),
                estimator.describe(),
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

    use super::{ArrivalStats, derived_min_delay};

    /// The game's fixed tick (64 Hz), spelled out rather than read from the config —
    /// `tests/net_interp_delay.rs` is what fires when lightyear moves its config shape, and this
    /// test must pin the arithmetic independently of it.
    const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);

    const EPSILON: Duration = Duration::from_micros(10);

    fn assert_close(actual: Duration, expected: Duration, what: &str) {
        assert!(
            actual.abs_diff(expected) < EPSILON,
            "{what}: derived {actual:?}, expected {expected:?}"
        );
    }

    fn spread(ms: f64) -> ArrivalStats {
        ArrivalStats::test_spread(ms / 1000.0)
    }

    /// Absolute values at three operating points. Dropping `rtt/2` fails the 100/250 cases;
    /// dropping the quantile spread fails the burst case; dropping the one-interval gap term
    /// fails all three; using `rtt` instead of `rtt/2` fails 100/250.
    #[test]
    fn law_is_half_rtt_plus_spread_plus_the_interval_gap() {
        // Loopback, cold estimator: the gap term alone — 15.625 ms.
        assert_close(
            derived_min_delay(Duration::ZERO, &ArrivalStats::default(), TICK),
            Duration::from_nanos(15_625_000),
            "loopback cold",
        );
        // rtt 100 ms, clean path (spread ~0): 50 + 15.625.
        assert_close(
            derived_min_delay(Duration::from_millis(100), &spread(0.0), TICK),
            Duration::from_nanos(65_625_000),
            "droplet clean",
        );
        // rtt 100 ms over the measured burst link (Qp − min = 60 ms): 50 + 60 + 15.625 — the
        // headroom the ping-EWMA law (which read this link as ~3 ms jitter) never carried.
        assert_close(
            derived_min_delay(Duration::from_millis(100), &spread(60.0), TICK),
            Duration::from_nanos(125_625_000),
            "droplet burst",
        );
    }

    /// The slope in RTT is exactly 1/2, independent of the other terms. Fails if the `rtt/2` term
    /// is dropped (slope 0) or left undivided (slope 1).
    #[test]
    fn law_slope_in_rtt_is_one_half() {
        let low = derived_min_delay(Duration::from_millis(40), &spread(20.0), TICK);
        let high = derived_min_delay(Duration::from_millis(240), &spread(20.0), TICK);
        assert_close(high - low, Duration::from_millis(100), "rtt slope");
    }

    /// The spread term passes through at slope 1, independent of RTT: the headroom IS the measured
    /// distribution's covered spread, not a multiple of it. A Gaussian-style multiplier on the
    /// spread fails this.
    #[test]
    fn the_spread_term_passes_through_at_slope_one() {
        let rtt = Duration::from_millis(60);
        let narrow = derived_min_delay(rtt, &spread(10.0), TICK);
        let wide = derived_min_delay(rtt, &spread(45.0), TICK);
        assert_close(wide - narrow, Duration::from_millis(35), "spread slope");
    }
}
