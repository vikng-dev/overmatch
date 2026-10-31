//! The interpolation delay, derived from the stream-measured arrival-delay distribution rather
//! than pinned.
//!
//! Lightyear places the interpolation clock at `remote_estimate − (delay + jitter_margin)` and
//! anchors `remote_estimate` RTT/2 AHEAD of the newest received keyframe (`lightyear_sync`
//! `remote.rs`: `network_delay = ping_manager.rtt() / 2`). A keyframe therefore exists ahead of the
//! cursor only while `delay ≥ rtt/2 + largest snapshot gap`; below that lightyear clamps to the
//! newest sample (`lightyear_interpolation` `registry.rs`) — the buffer edge `net::frontier`
//! instruments.
//!
//! Lightyear's own `delay` term is `max(send_interval_ratio × remote_send_interval, min_delay)`,
//! which carries the gap half only, and our per-tick server advertises `send_interval = 0` — that
//! collapses the ratio term entirely (`src/net/tests/net_interp_delay.rs` pins the degenerate).
//! `min_delay` is the only writable slot, so it carries the whole law:
//!
//! ```text
//! headroom  = Q_p{d_i} - min{d_i} + one send interval
//! min_delay = rtt/2 + headroom
//! ```
//!
//! The buffer covers the full measured receipt spread. No positive extrapolation horizon has an
//! error bound in the presence of unknown impulses; the contract and rejected traction bound live
//! in `.agents/docs/design/net-motion-contract.md`.
//!
//! The rtt/2 term stays on the ping EWMA (it cancels the anchor, which is built from the same
//! EWMA); every jitter term now comes from the measured stream. The Gaussian margin chain on top
//! is zeroed by `net::sync_margin` — the quantile subsumes it — keeping the half-tick floor.
//!
//! The law ARMS only once the estimator distribution is valid — the spike window holds a full
//! 2 s of samples AND the ring holds one window's worth (`ArrivalStats::warmed`, derivation in
//! `net::sync_margin`). Before arming the installed default rides unchanged (the latch pattern of
//! the uplink law), so startup estimator pollution — connect/load stalls read as hundreds of ms
//! of arrival delay — never reaches the objective; arming then latches for the link
//! (`ArrivalDelay::link`).
//!
//! Lightyear's `sync_timeline` re-reads `Res<InterpolationConfig>` every frame with no caching, so
//! writing the resource is the whole mechanism. Lightyear converges the timeline by ±5% clock speed, and a
//! stepped target is an error step the controller escalates to `Resync` (the live gauge showed
//! `steady=2` from exactly this). The written value therefore FOLLOWS the law through a slewed
//! follower — at most [`MAX_SLEW_PER_FRAME_TICKS`] per Update frame — and the steady-state
//! resync count is ZERO (`net::frontier`'s FRONTIER `steady=` counter is the live gauge).
//!
//! Derivation and the contract constants behind `Q_p`:
//! `net::sync_margin`'s module doc and `.agents/scratch/adaptive-cursor-frontier-2026-08-15.md` §1.

use core::time::Duration;

use bevy::prelude::*;
use lightyear::core::tick::TickDuration;
use lightyear::interpolation::timeline::InterpolationConfig;
use lightyear::prelude::{Client, PingManager, SyncSystems};

use super::sync_margin::{ArrivalDelay, ArrivalStats};

/// Log-rate guard, not a term of the law: `rtt()` and the quantile spread move continuously, so an
/// unconditional log is per-frame spam.
const LOG_STEP: Duration = Duration::from_millis(5);

/// The written `min_delay` moves at most this far per Update frame toward the law's value: half
/// the sync controller's ±1-tick deadband (`SyncConfig::error_margin` = 1.0 tick, which
/// `net::sync_margin::derived_interp_sync` keeps), so target motion alone can never exit the
/// proportional regime and reach `Resync`.
const MAX_SLEW_PER_FRAME_TICKS: f64 = 0.5;

/// One step of the slewed follower: from `current` at most `step` toward `target`, landing
/// exactly on it inside the step — a rate-limited pursuit, never the target's step.
fn slewed(current: Duration, target: Duration, step: Duration) -> Duration {
    if target > current {
        (current + step).min(target)
    } else {
        current.saturating_sub(step).max(target)
    }
}

/// The law. The send interval is one tick — the server replicates every tick (`send_interval = 0`,
/// the degenerate `src/net/tests/net_interp_delay.rs` pins), so the gap to the next keyframe is
/// one tick.
/// The whole measured receipt spread is covered; unknown impulses permit no projection discount.
fn derived_min_delay(rtt: Duration, stats: &ArrivalStats, tick: Duration) -> Duration {
    rtt / 2 + stats.spread() + tick
}

/// Mount the deriving system and hand back the initial `InterpolationConfig` resource.
pub(super) fn install(app: &mut App, tick: Duration) -> InterpolationConfig {
    // No RTT or arrival sample exists before the link opens; the law at rtt = spread = 0 is the
    // one-interval gap term alone, and `derive_interpolation_delay` overwrites this before the
    // first `SyncSystems::Sync`.
    let initial = derived_min_delay(Duration::ZERO, &ArrivalStats::default(), tick);
    info!(
        "net: interpolation min_delay DERIVED = rtt/2 + max(Qp − min − g*, 0) + one tick \
         ({:.1} ms cold)",
        millis(initial)
    );
    app.add_systems(
        PostUpdate,
        derive_interpolation_delay.before(SyncSystems::Sync),
    );
    InterpolationConfig::default().with_min_delay(initial)
}

/// Follow the measured link every frame, ahead of the frame's timeline sync (and after
/// `net::sync_margin` refreshes the estimator digest): the written `min_delay` slews at most
/// [`MAX_SLEW_PER_FRAME_TICKS`] per frame toward the law's value. The `!=` guard keeps change
/// detection quiet while the follower sits on its target.
pub(super) fn derive_interpolation_delay(
    tick: Res<TickDuration>,
    estimator: Res<ArrivalDelay>,
    config: Option<ResMut<InterpolationConfig>>,
    pings: Query<&PingManager, With<Client>>,
    // The link the law is armed for: the latch holds for one link and re-arms on the next.
    mut armed: Local<Option<u64>>,
    mut logged: Local<Option<Duration>>,
) {
    if *armed != Some(estimator.link()) {
        if !estimator.stats.warmed() {
            // Pre-arm the installed default rides unchanged (module doc: warmup arming).
            return;
        }
        *armed = Some(estimator.link());
        info!(
            "net: interpolation delay law ARMED — estimator distribution valid ({})",
            estimator.describe()
        );
    }
    let Some(mut config) = config else {
        return;
    };
    for pings in &pings {
        let target = derived_min_delay(pings.rtt(), &estimator.stats, tick.0);
        let next = slewed(
            config.min_delay,
            target,
            tick.0.mul_f64(MAX_SLEW_PER_FRAME_TICKS),
        );
        if config.min_delay == next {
            continue;
        }
        if logged.is_none_or(|last| last.abs_diff(target) >= LOG_STEP) {
            info!(
                "net: interpolation min_delay → {:.1} ms (rtt {:.1} ms, {})",
                millis(target),
                millis(pings.rtt()),
                estimator.describe(),
            );
            *logged = Some(target);
        }
        config.min_delay = next;
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::World;
    use lightyear::core::tick::TickDuration;
    use lightyear::interpolation::timeline::InterpolationConfig;
    use lightyear::prelude::{Client, PingManager};

    use super::{ArrivalDelay, ArrivalStats, derived_min_delay, slewed};

    /// The game's fixed tick (64 Hz), spelled out rather than read from the config —
    /// `src/net/tests/net_interp_delay.rs` is what fires when lightyear moves its config shape, and
    /// this test must pin the arithmetic independently of it.
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

    /// Absolute values at three operating points. Dropping `rtt/2` fails the 100 ms cases;
    /// dropping the excess-spread term fails the burst case; dropping the one-interval gap term
    /// fails all three; using `rtt` instead of `rtt/2` fails the 100 ms cases.
    #[test]
    fn law_is_half_rtt_plus_full_spread_plus_the_interval_gap() {
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
            "rtt 100 ms clean",
        );
        // DERIVED fixture: 100 ms RTT, 60 ms receipt spread: 50 + 60 + 15.625 ms.
        assert_close(
            derived_min_delay(Duration::from_millis(100), &spread(60.0), TICK),
            Duration::from_micros(125_625),
            "rtt 100 ms burst",
        );
    }

    /// Every nonzero receipt spread costs buffer time. A traction-derived discount would hide
    /// small gaps from the law despite unbounded impulse error; clients rely on full coverage.
    #[test]
    fn every_receipt_spread_is_paid_in_full() {
        let clean = derived_min_delay(Duration::from_millis(100), &spread(0.0), TICK);
        for ms in [1.0, 10.0, 30.0, 52.0] {
            assert_close(
                derived_min_delay(Duration::from_millis(100), &spread(ms), TICK),
                clean + Duration::from_secs_f64(ms / 1000.0),
                "full receipt spread",
            );
        }
    }

    /// THE FOLLOWER IS RATE-LIMITED IN BOTH DIRECTIONS AND LANDS EXACTLY: every step is at most
    /// `step`, monotone toward the target, and the final step is the remainder — no overshoot, no
    /// dither around the target. A follower that writes the target directly reds the per-step
    /// bound; one that always steps the full `step` reds the exact landing.
    #[test]
    fn the_follower_steps_at_most_the_bound_and_lands_exactly() {
        let step = Duration::from_micros(7_812);
        for (from, to) in [
            (Duration::from_millis(15), Duration::from_millis(160)),
            (Duration::from_millis(160), Duration::from_millis(15)),
        ] {
            let mut current = from;
            let mut steps = 0;
            while current != to {
                let next = slewed(current, to, step);
                assert!(
                    next.abs_diff(current) <= step,
                    "one frame moves at most one step"
                );
                assert!(
                    next.abs_diff(to) < current.abs_diff(to),
                    "every step is toward the target"
                );
                current = next;
                steps += 1;
                assert!(
                    steps <= 20,
                    "145 ms at 7.812 ms/frame lands within 19 frames"
                );
            }
            assert_eq!(steps, 19, "⌈145 / 7.812⌉ frames, the last one partial");
        }
        // On target the follower is a fixed point — the `!=` write guard stays quiet.
        let settled = Duration::from_millis(60);
        assert_eq!(slewed(settled, settled, step), settled);
    }

    /// A STEP TARGET NEVER STEPS THE WRITTEN CONFIG: the writer walks `min_delay` toward a
    /// suddenly-large law value at most half a tick per frame (half the sync controller's ±1-tick
    /// deadband), and converges exactly. A writer that assigns the law's value directly — the
    /// steady=2 Resync defect this law retires — reds the per-frame bound on its first frame.
    #[test]
    fn a_step_target_walks_to_the_law_inside_the_deadband() {
        let mut world = World::new();
        world.insert_resource(TickDuration(TICK));
        let mut estimator = ArrivalDelay::default();
        // DERIVED fixture: a 100 ms receipt spread raises the objective by 100 ms.
        estimator.stats = spread(100.0);
        world.insert_resource(estimator);
        let installed = Duration::from_nanos(15_625_000);
        world.insert_resource(InterpolationConfig::default().with_min_delay(installed));
        world.spawn((Client, PingManager::default()));
        let target = derived_min_delay(Duration::ZERO, &spread(100.0), TICK);
        let step = TICK / 2;
        let mut last = installed;
        for _ in 0..64 {
            world
                .run_system_once(super::derive_interpolation_delay)
                .expect("writer runs");
            let now = world.resource::<InterpolationConfig>().min_delay;
            assert!(
                now.abs_diff(last) <= step,
                "one frame moves min_delay at most half a tick (moved {:?})",
                now.abs_diff(last)
            );
            last = now;
            if now == target {
                break;
            }
        }
        assert_eq!(last, target, "the follower converges on the law exactly");
    }

    /// THE LAW ARMS ONLY ON A VALID DISTRIBUTION, AND ARMING LATCHES. A cold digest — even the
    /// observed 243 ms startup-pollution Qp — leaves the installed default untouched; the first
    /// warmed digest arms the writer; a digest later regressing below the warmup bound must not
    /// re-freeze it. Dropping the arming gate reds the pre-arm half; dropping the latch (gating
    /// on `warmed()` every frame) reds the regression half.
    #[test]
    fn the_writer_holds_the_installed_default_until_the_estimator_warms() {
        let mut world = World::new();
        world.insert_resource(TickDuration(TICK));
        let mut estimator = ArrivalDelay::default();
        // The observed startup pollution: Qp 243 ms before the distribution is valid.
        estimator.stats = ArrivalStats::test_cold_spread(0.243);
        world.insert_resource(estimator);
        let installed = Duration::from_nanos(15_625_000);
        world.insert_resource(InterpolationConfig::default().with_min_delay(installed));
        world.spawn((Client, PingManager::default()));
        // A registered system keeps its `Local` state across runs — the latch under test.
        let writer = world.register_system(super::derive_interpolation_delay);
        let read = |world: &World| world.resource::<InterpolationConfig>().min_delay;
        for _ in 0..8 {
            world.run_system(writer).expect("writer runs");
        }
        assert_eq!(
            read(&world),
            installed,
            "startup pollution must never reach the objective"
        );
        world.resource_mut::<ArrivalDelay>().stats = ArrivalStats::test_spread(0.1);
        world.run_system(writer).expect("writer runs");
        let after_arm = read(&world);
        assert_ne!(after_arm, installed, "a valid distribution arms the law");
        world.resource_mut::<ArrivalDelay>().stats = ArrivalStats::test_cold_spread(0.1);
        world.run_system(writer).expect("writer runs");
        assert!(
            read(&world) > after_arm,
            "arming latches — a regressing digest must not re-freeze the follower"
        );
        // A new link re-arms: its cold digest (the startup pollution again) moves nothing.
        let latched = read(&world);
        world.resource_mut::<ArrivalDelay>().reset_for_new_link();
        world.resource_mut::<ArrivalDelay>().stats = ArrivalStats::test_cold_spread(0.243);
        world.run_system(writer).expect("writer runs");
        assert_eq!(
            read(&world),
            latched,
            "the latch is the link's: a new link's cold digest must not reach the objective"
        );
    }
}
