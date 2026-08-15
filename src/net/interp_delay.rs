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
//! headroom  = max(Q_p{d_i} − min{d_i} − g*, 0)           (the arrival-delay distribution's
//!             + one send interval                         covered spread — `net::sync_margin`'s
//!                                                         estimator, empirical, no Gaussian —
//!                                                         less the extrapolation horizon, plus
//!                                                         the gap to the next keyframe)
//! min_delay = rtt/2 + headroom                           (rtt/2 cancels the anchor, as above)
//! ```
//!
//! `g*` is `net::extrapolate`'s horizon (the single source — imported, never restated): the
//! extrapolator covers tail excursions up to g* invisibly (ε-bounded), so the buffer pays only
//! for the excess beyond it. On a clean link the subtraction floors at zero and the law reduces
//! to `rtt/2 + one send interval`.
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
/// The spread pays only its excess beyond the extrapolation horizon (module doc); `saturating_sub`
/// is the floor at zero.
fn derived_min_delay(rtt: Duration, stats: &ArrivalStats, tick: Duration) -> Duration {
    rtt / 2 + stats.spread().saturating_sub(super::extrapolate::horizon()) + tick
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
            "net: interpolation min_delay DERIVED = rtt/2 + max(Qp − min − g*, 0) + one tick \
             ({:.1} ms cold) [OVERMATCH_INTERP_DELAY_MS unset]",
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

    /// `g*` recomputed here from the raw constants (μ = 0.9, g = 9.81, ε_vis = 12.08 mm)
    /// independently of `net::extrapolate`'s expression — the same independence discipline as
    /// `extrapolate`'s own horizon test.
    fn g_star() -> Duration {
        Duration::from_secs_f64((2.0 * 0.012_08_f64 / (0.9 * 9.81)).sqrt())
    }

    /// Absolute values at three operating points. Dropping `rtt/2` fails the 100 ms cases;
    /// dropping the excess-spread term fails the burst case; dropping the one-interval gap term
    /// fails all three; using `rtt` instead of `rtt/2` fails the 100 ms cases.
    #[test]
    fn law_is_half_rtt_plus_excess_spread_plus_the_interval_gap() {
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
        // rtt 100 ms over the measured burst link (Qp − min = 60 ms): 50 + (60 − g*) + 15.625 —
        // the extrapolator absorbs the first g* ≈ 52.3 ms of the burst invisibly, so the buffer
        // pays only the ~7.7 ms excess.
        assert_close(
            derived_min_delay(Duration::from_millis(100), &spread(60.0), TICK),
            Duration::from_millis(100) / 2 + (Duration::from_millis(60) - g_star()) + TICK,
            "droplet burst",
        );
    }

    /// THE SUBTRACTION FLOORS AT ZERO: any spread at or under the horizon is fully absorbed by
    /// the extrapolator, so the law reduces to the clean form `rtt/2 + one tick` exactly.
    /// Dropping the `− g*` subtraction reds this (a 30 ms spread would leak into the delay);
    /// dropping the floor (a signed subtraction) reds the sub-horizon case by underflow.
    #[test]
    fn a_spread_under_the_horizon_floors_to_the_clean_law() {
        let clean = derived_min_delay(Duration::from_millis(100), &spread(0.0), TICK);
        for ms in [10.0, 30.0, 52.0] {
            assert_close(
                derived_min_delay(Duration::from_millis(100), &spread(ms), TICK),
                clean,
                "sub-horizon spread is the extrapolator's to cover",
            );
        }
    }

    /// The slope in RTT is exactly 1/2, independent of the other terms. Fails if the `rtt/2` term
    /// is dropped (slope 0) or left undivided (slope 1).
    #[test]
    fn law_slope_in_rtt_is_one_half() {
        let low = derived_min_delay(Duration::from_millis(40), &spread(20.0), TICK);
        let high = derived_min_delay(Duration::from_millis(240), &spread(20.0), TICK);
        assert_close(high - low, Duration::from_millis(100), "rtt slope");
    }

    /// Beyond the horizon the spread passes through at slope 1, independent of RTT: the headroom
    /// IS the measured excess, not a multiple of it. A Gaussian-style multiplier on the excess
    /// fails this; so does subtracting g* from only one of the pair.
    #[test]
    fn the_excess_spread_passes_through_at_slope_one() {
        let rtt = Duration::from_millis(60);
        let narrow = derived_min_delay(rtt, &spread(60.0), TICK);
        let wide = derived_min_delay(rtt, &spread(95.0), TICK);
        assert_close(wide - narrow, Duration::from_millis(35), "excess slope");
    }
}
