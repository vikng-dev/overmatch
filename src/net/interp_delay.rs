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
//! headroom  = max(Q_p{l_i} - min{d_i}, 0) + one send interval
//! min_delay = rtt/2 + headroom
//! ```
//!
//! The buffer covers the full measured spread: `Q_p` of the lateness each packet had at the last
//! read that lacked it, over the sample ring, less the drain-stamped 2 s min (`net::sync_margin`).
//! Retention is nominally 60 s at full per-tick reception; the percentile excludes one sample at
//! capacity. These frame-stamped observations include sender pacing and receiver scheduling,
//! rather than measuring link delay independently. No positive extrapolation horizon has an error bound in the presence of unknown
//! impulses; the contract and rejected traction bound live in
//! `.agents/docs/design/net-motion-contract.md`.
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
//! The writer applies the full delay and relocates an already-synced cursor by the resulting
//! objective displacement before clock synchronization. This preserves clock error; cold cursors
//! still acquire their first objective through Lightyear's handshake. The relocation contract lives
//! in `.agents/docs/design/net-motion-contract.md`.
//!
//! The quantile contract and estimator parameters live in `net::sync_margin`'s module doc.

use core::time::Duration;

use bevy::prelude::*;
use lightyear::core::tick::TickDuration;
use lightyear::interpolation::timeline::{InterpolationConfig, InterpolationTimeline};
use lightyear::prelude::client::RemoteTimeline;
use lightyear::prelude::{Client, NetworkTimeline, PingManager, SyncSystems, TimelineSync};

use super::sync_margin::{ArrivalDelay, ArrivalStats};

/// Resolution: suppress sub-5 ms diagnostic changes to bound logging cost as the law moves.
const LOG_STEP: Duration = Duration::from_millis(5);

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
        "net: interpolation min_delay DERIVED = rtt/2 + (Qp − min) + one tick ({:.1} ms cold)",
        millis(initial)
    );
    app.add_systems(
        PostUpdate,
        derive_interpolation_delay.before(SyncSystems::Sync),
    );
    InterpolationConfig::default().with_min_delay(initial)
}

/// Apply measured coverage before clock sync, preserving its residual error under relocation.
pub(super) fn derive_interpolation_delay(
    tick: Res<TickDuration>,
    estimator: Res<ArrivalDelay>,
    mut config: ResMut<InterpolationConfig>,
    mut timeline: ResMut<InterpolationTimeline>,
    peers: Query<(&RemoteTimeline, &PingManager), With<Client>>,
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
    // Before a session link exists there is no remote clock to follow.
    for (remote, pings) in &peers {
        let target = derived_min_delay(pings.rtt(), &estimator.stats, tick.0);
        if config.min_delay == target {
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
        let before = timeline.sync_objective(remote, &config, pings, tick.0);
        config.min_delay = target;
        if timeline.is_synced() {
            let after = timeline.sync_objective(remote, &config, pings, tick.0);
            timeline.apply_delta(after - before);
        }
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;
    use lightyear::core::tick::{Tick, TickDuration};
    use lightyear::core::time::TickDelta;
    use lightyear::interpolation::timeline::{InterpolationConfig, InterpolationTimeline};
    use lightyear::prelude::client::RemoteTimeline;
    use lightyear::prelude::{Client, NetworkTimeline, PingManager, TimelineSync};

    use super::{ArrivalDelay, ArrivalStats, derived_min_delay};

    // Fixture interval for independent arithmetic checks, not a gate on the shipped tick rate.
    const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);
    // Resolution: tolerate duration rounding in the fixed-point timeline conversion.
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

    fn writer_world(stats: ArrivalStats) -> World {
        let mut world = World::new();
        world.insert_resource(TickDuration(TICK));
        let mut estimator = ArrivalDelay::default();
        estimator.stats = stats;
        world.insert_resource(estimator);
        let mut config = InterpolationConfig::default().with_min_delay(TICK);
        config.sync.handshake_pings = 0; // Fixture: sample without a ping handshake.
        let pings = PingManager::default();
        let mut remote = RemoteTimeline::default();
        remote.set_now(Tick(1000).into()); // Fixture epoch, beyond every tested delay.
        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(timeline.sync_objective(&remote, &config, &pings, TICK));
        timeline.set_synced(true);
        world.insert_resource(config);
        world.insert_resource(timeline);
        world.spawn((Client, pings, remote));
        world
    }

    fn sample_controller(world: &mut World) -> (f32, Option<i32>) {
        world
            .run_system_once(
                |config: Res<InterpolationConfig>,
                 mut timeline: ResMut<InterpolationTimeline>,
                 peers: Query<(&RemoteTimeline, &PingManager)>| {
                    let (remote, pings) = peers.single().unwrap();
                    let now = timeline.now();
                    let objective = timeline.sync_objective(remote, &config, pings, TICK);
                    (
                        (now - objective).to_f32(),
                        timeline.sync(now, remote, &config, pings, TICK),
                    )
                },
            )
            .unwrap()
    }

    /// Presentation delay changes are known objective relocations, not clock drift. The clock
    /// controller must see the same residual error while the full coverage law applies at once.
    #[test]
    fn delay_changes_preserve_clock_error_and_apply_full_coverage_immediately() {
        let mut world = writer_world(spread(0.0));
        let writer = world.register_system(super::derive_interpolation_delay);
        // Fixture: both directions and sustained motion, larger than the resync bound.
        for ms in [200.0, 500.0, 1000.0, 800.0, 0.0, 300.0] {
            world.resource_mut::<ArrivalDelay>().stats = spread(ms);
            world.run_system(writer).unwrap();
            let (error, resync) = sample_controller(&mut world);
            assert!(
                error.abs() < f32::EPSILON,
                "delay movement invented {error} ticks of clock error"
            );
            assert_eq!(
                resync, None,
                "a deliberate relocation is not a clock resync"
            );
            assert_eq!(
                world.resource::<InterpolationConfig>().min_delay,
                derived_min_delay(Duration::ZERO, &spread(ms), TICK),
                "new coverage must apply in the same frame"
            );
        }
    }

    /// Relocation must preserve genuine drift, so clients still steer small errors and resync
    /// errors beyond the installed bound. Resetting or snapping the cursor hides these defects.
    #[test]
    fn delay_relocation_preserves_small_drift_and_does_not_hide_a_clock_fault() {
        let bound = InterpolationConfig::default().sync.max_error_margin;
        // Fixture: a sub-deadband error, then an error beyond the controller's own resync bound.
        for drift in [0.25, bound + 1.0] {
            let mut world = writer_world(spread(0.0));
            let writer = world.register_system(super::derive_interpolation_delay);
            let drift_delta = TickDelta::from_duration(TICK.mul_f32(drift), TICK);
            world
                .resource_mut::<InterpolationTimeline>()
                .apply_delta(drift_delta);
            world.resource_mut::<ArrivalDelay>().stats = spread(500.0);
            world.run_system(writer).unwrap();
            let (error, resync) = sample_controller(&mut world);
            assert_eq!(
                error,
                drift_delta.to_f32(),
                "relocation must preserve the injected clock error (requested {drift} ticks)"
            );
            assert_eq!(resync.is_some(), drift > bound);
        }
    }

    /// A cold cursor has no established clock error to preserve. Clients still need the first
    /// synchronization to acquire the entire objective after coverage changes.
    #[test]
    fn an_unsynced_cursor_acquires_the_new_objective_at_handshake() {
        let mut world = writer_world(spread(500.0));
        world
            .resource_mut::<InterpolationTimeline>()
            .set_synced(false);
        let before = world.resource::<InterpolationTimeline>().now();
        world
            .run_system_once(super::derive_interpolation_delay)
            .unwrap();
        assert_eq!(world.resource::<InterpolationTimeline>().now(), before);
        assert!(sample_controller(&mut world).1.is_some());
        assert!(sample_controller(&mut world).0.abs() < f32::EPSILON);
    }

    /// Clients retain the installed delay until the distribution warms. Arming persists through
    /// a cold digest on the same link, while a new link must warm independently.
    #[test]
    fn the_writer_holds_the_installed_default_until_the_estimator_warms() {
        let mut world = writer_world(ArrivalStats::test_cold_spread(0.243)); // Fixture pollution.
        let installed = TICK;
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
        world.resource_mut::<ArrivalDelay>().stats = ArrivalStats::test_cold_spread(0.2);
        world.run_system(writer).expect("writer runs");
        assert!(
            read(&world) > after_arm,
            "arming latches — a regressing digest must not freeze the law"
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
