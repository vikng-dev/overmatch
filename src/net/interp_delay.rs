//! The interpolation delay, derived from the stream-measured arrival-delay distribution rather
//! than pinned.
//!
//! Lightyear places the interpolation clock at `remote_estimate − (delay + jitter_margin)` and
//! anchors `remote_estimate` RTT/2 AHEAD of the newest received keyframe (`lightyear_sync`
//! `remote.rs`: `network_delay = ping_manager.rtt() / 2`). A keyframe therefore exists ahead of the
//! cursor only while `delay ≥ rtt/2 + largest snapshot gap`; below that lightyear clamps to the
//! newest sample (`lightyear_interpolation` `registry.rs`).
//!
//! Lightyear's own `delay` term is `max(send_interval_ratio × remote_send_interval, min_delay)`,
//! which carries the gap half only, and our per-tick server advertises `send_interval = 0` — that
//! collapses the ratio term entirely (`src/net/tests/net_interp_delay.rs` pins the degenerate).
//! `min_delay` carries the delay objective:
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
//! `net::sync_margin`). Before arming applied headroom is held (one tick on the first link), while RTT/2
//! still cancels the anchor, so startup estimator pollution — connect/load stalls read as hundreds of ms
//! of arrival delay — never reaches the objective; arming then latches for the link
//! (`ArrivalDelay::link`).
//!
//! Applied spread delay and cursor pacing move together: `pace_interpolation_delay` changes
//! the frame's rate before Lightyear advances it, and the writer pays the same elapsed delay
//! before synchronization. Intentional motion therefore cancels in the controller's error.
//! RTT/2 is written separately, including during warmup: it cancels the remote anchor and is
//! never a coverage displacement. Only genuine clock error can trigger a steady-state resync.
//! The rate bound and coverage cost live in `.agents/docs/design/net-motion-contract.md`.
//!
//! The quantile contract and estimator parameters live in `net::sync_margin`'s module doc.

use core::time::Duration;

use bevy::prelude::*;
use lightyear::core::tick::TickDuration;
use lightyear::interpolation::timeline::{InterpolationConfig, InterpolationTimeline};
use lightyear::prelude::{Client, PingManager, SyncSystems, TimelineSync, TimelineSystems};

use super::sync_margin::{ArrivalDelay, ArrivalStats};

/// Coverage applied so far, excluding RTT anchor compensation. The saved controller rate exists
/// only around `TimelineSystems::Advance`; synchronization always sees its own uncomposed rate.
#[derive(Resource)]
pub(super) struct AppliedDelay {
    headroom: Duration,
    link: Option<u64>,
    controller_rate: Option<f32>,
}

/// Pace coverage at the controller's maximum rate correction. Its source algorithm bounds the
/// normal rate by `[1/M, M]`, `M = 1 + 2*(speedup_factor - 1)`. Using budget
/// `B = min(M - 1, 1/M)` gives a composed rate in `[1/M - B, M + B]`, never negative.
/// This is a Resolution bound on presentation pacing, not a limit on the delay law's magnitude.
fn pace_interpolation_delay(
    time: Res<Time<Virtual>>,
    tick: Res<TickDuration>,
    estimator: Res<ArrivalDelay>,
    config: Res<InterpolationConfig>,
    mut timeline: ResMut<InterpolationTimeline>,
    mut applied: ResMut<AppliedDelay>,
) {
    if applied.link != Some(estimator.link()) {
        if !estimator.stats.warmed() {
            return; // A cold distribution must not add startup-pollution coverage.
        }
        applied.link = Some(estimator.link());
        info!("net: interpolation delay law ARMED — estimator distribution valid");
    }
    if !timeline.is_synced() || time.relative_speed() == 0.0 {
        return; // Handshake acquires the cold cursor; a paused clock has no elapsed pacing.
    }
    let frame = time.delta().div_f32(time.relative_speed());
    if frame.is_zero() {
        return;
    }
    let target = derived_min_delay(Duration::ZERO, &estimator.stats, tick.0);
    let maximum = 1.0 + 2.0 * (config.sync.speedup_factor - 1.0);
    let budget = (maximum - 1.0).min(1.0 / maximum);
    let step = applied.headroom.abs_diff(target).min(frame.mul_f32(budget));
    let controller_rate = timeline.relative_speed();
    let rate_change = step.as_secs_f32() / frame.as_secs_f32();
    let rate = if target > applied.headroom {
        applied.headroom += step;
        controller_rate - rate_change
    } else {
        applied.headroom -= step;
        controller_rate + rate_change
    };
    applied.controller_rate = Some(controller_rate);
    timeline.set_relative_speed(rate);
}

fn restore_controller_rate(
    mut timeline: ResMut<InterpolationTimeline>,
    mut applied: ResMut<AppliedDelay>,
) {
    if let Some(rate) = applied.controller_rate.take() {
        timeline.set_relative_speed(rate);
    }
}

/// The law. The send interval is one tick — the server replicates every tick (`send_interval = 0`,
/// the degenerate `src/net/tests/net_interp_delay.rs` pins), so the gap to the next keyframe is
/// one tick.
/// The whole measured receipt spread is covered; unknown impulses permit no projection discount.
pub(super) fn derived_min_delay(rtt: Duration, stats: &ArrivalStats, tick: Duration) -> Duration {
    rtt / 2 + stats.spread() + tick
}

/// Mount the deriving system and hand back the initial `InterpolationConfig` resource.
pub(super) fn install(app: &mut App, tick: Duration) -> InterpolationConfig {
    // No RTT or arrival sample exists before the link opens; the law at rtt = spread = 0 is the
    // one-interval gap term alone, and `derive_interpolation_delay` overwrites this before the
    // first `SyncSystems::Sync`; RTT anchor compensation is written even before warmup.
    let initial = derived_min_delay(Duration::ZERO, &ArrivalStats::default(), tick);
    app.insert_resource(AppliedDelay {
        headroom: initial,
        link: None,
        controller_rate: None,
    });
    app.add_systems(
        PreUpdate,
        (
            pace_interpolation_delay.before(TimelineSystems::Advance),
            restore_controller_rate.after(TimelineSystems::Advance),
        ),
    );
    app.add_systems(
        PostUpdate,
        derive_interpolation_delay.before(SyncSystems::Sync),
    );
    InterpolationConfig::default().with_min_delay(initial)
}

/// Pay exactly the coverage already paced this frame, plus the current RTT anchor compensation.
/// The estimator refresh precedes this writer; pacing reads that digest on the next frame.
pub(super) fn derive_interpolation_delay(
    applied: Res<AppliedDelay>,
    mut config: ResMut<InterpolationConfig>,
    pings: Query<&PingManager, With<Client>>,
) {
    // No client link exists before connection setup or after disconnect cleanup.
    for pings in &pings {
        let next = pings.rtt() / 2 + applied.headroom;
        if config.min_delay != next {
            config.min_delay = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use bevy::prelude::*;
    use lightyear::connection::network_topology::{NetworkTopology, NetworkingMetadata};
    use lightyear::core::tick::Tick;
    use lightyear::core::tick::TickDuration;
    use lightyear::interpolation::timeline::InterpolationConfig;
    use lightyear::interpolation::timeline::{InterpolationTimeline, TimelinePlugin};
    use lightyear::prelude::client::RemoteTimeline;
    use lightyear::prelude::{Client, PingManager};
    use lightyear::prelude::{NetworkTimeline, SyncSystems, TimelineSync, TimelineSystems};

    use super::{ArrivalDelay, ArrivalStats, derived_min_delay};

    // Fixture interval for independent arithmetic checks, not a gate on the shipped tick rate.
    const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);

    // Resolution: tolerate duration rounding without masking a term of the law.
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

    #[derive(Resource, Default)]
    struct Snaps(Vec<i32>);

    // This replaces packet receipt only: the actual Lightyear advance and controller, and our
    // shipped installer, still run. The fixture remote is a perfect clock with controlled RTT.
    fn advance_remote(
        time: Res<Time<Virtual>>,
        tick: Res<TickDuration>,
        mut peers: Query<&mut RemoteTimeline>,
    ) {
        for mut remote in &mut peers {
            remote.apply_duration(time.delta().div_f32(time.relative_speed()), tick.0);
        }
    }

    fn sync_clock(
        tick: Res<TickDuration>,
        config: Res<InterpolationConfig>,
        peers: Query<(&RemoteTimeline, &PingManager), With<Client>>,
        mut cursor: ResMut<InterpolationTimeline>,
        mut snaps: ResMut<Snaps>,
    ) {
        for (remote, pings) in &peers {
            let now = cursor.now();
            if let Some(delta) = cursor.sync(now, remote, &config, pings, tick.0) {
                snaps.0.push(delta);
            }
        }
    }

    fn fixture(frame: Duration, rtt: Duration) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(frame))
            .insert_resource(TickDuration(TICK))
            .init_resource::<ArrivalDelay>()
            .init_resource::<NetworkingMetadata>()
            .init_resource::<Snaps>()
            .add_plugins(TimelinePlugin)
            .add_systems(PreUpdate, advance_remote.in_set(TimelineSystems::Advance))
            .add_systems(PostUpdate, sync_clock.after(SyncSystems::Sync));
        let mut config = super::install(&mut app, TICK);
        config.sync.handshake_pings = 0; // Fixture bypasses packet handshake, not speed steering.
        config.sync.jitter_multiple = 0;
        config.sync.jitter_margin = 0.5;
        app.insert_resource(config);
        let mut remote = RemoteTimeline::default();
        remote.set_now(Tick(1000).into());
        let mut pings = PingManager::default();
        pings.rtt_estimator_ewma.final_stats.rtt = rtt;
        remote.apply_duration(rtt / 2, TICK);
        let link = app.world_mut().spawn((Client, remote, pings)).id();
        app.world_mut().resource_mut::<NetworkingMetadata>().mode = NetworkTopology::Client(link);
        app.update();
        app.world_mut().resource_mut::<Snaps>().0.clear();
        (app, link)
    }

    fn frame(app: &mut App, spread_ms: f64) -> f32 {
        let before = app.world().resource::<InterpolationTimeline>().now();
        app.world_mut().resource_mut::<ArrivalDelay>().stats = spread(spread_ms);
        app.update();
        (app.world().resource::<InterpolationTimeline>().now() - before).to_f32()
    }

    /// A sustained intentional objective move must never resync remote hulls, however long it
    /// outruns coverage. Fixture movement exceeds the controller's snap threshold many times.
    #[test]
    fn sustained_delay_motion_never_becomes_clock_error() {
        let (mut app, _) = fixture(TICK, Duration::ZERO);
        for index in 0..1000 {
            frame(&mut app, f64::from(index) * 5.0);
        }
        assert!(
            app.world().resource::<Snaps>().0.is_empty(),
            "intentional delay is not clock drift: {:?}",
            app.world().resource::<Snaps>().0
        );
    }

    /// Hulls and discrete piece births require a monotone cursor, with each frame's travel bounded
    /// by the configured speed range, across both rising and falling delay steps.
    #[test]
    fn delay_steps_only_change_the_presented_rate() {
        for millis in [8, 16, 33] {
            // Fixture render partitions.
            let dt = Duration::from_millis(millis);
            let (mut app, _) = fixture(dt, Duration::ZERO);
            let config = app.world().resource::<InterpolationConfig>();
            let maximum = 1.0 + 2.0 * (config.sync.speedup_factor - 1.0);
            let budget = (maximum - 1.0).min(1.0 / maximum);
            let (lo, hi) = (1.0 / maximum - budget, maximum + budget);
            for spread in [600.0, 0.0, 112.5, 0.0] {
                for _ in 0..500 {
                    let step = f64::from(frame(&mut app, spread)) * TICK.as_secs_f64();
                    assert!(
                        step >= dt.as_secs_f64() * f64::from(lo) - 1e-6,
                        "clock rewound or exceeded the slow-rate bound: {step}"
                    );
                    assert!(
                        step <= dt.as_secs_f64() * f64::from(hi) + 1e-6,
                        "clock stepped beyond the fast-rate bound: {step}"
                    );
                }
            }
        }
    }

    /// The game's damage model: a flying round older than the cosmetic catch-up horizon retains
    /// its free-flight cache through a delay rise. A cursor rewind would reset it and refuse it.
    #[test]
    fn an_old_round_survives_a_delay_rise_with_a_forward_path() {
        use crate::ballistics::path::Replica;
        let (mut app, _) = fixture(TICK, Duration::ZERO);
        let start = app.world().resource::<InterpolationTimeline>().now();
        let mut round = Replica::born(0, Vec3::ZERO, Vec3::Z * 800.0); // Fixture speed.
        let mut previous = 0.0;
        let step_at = crate::ballistics::MAX_COSMETIC_CATCH_UP_TICKS * 2;
        for index in 0..step_at + 600 {
            let spread = if index > step_at { 600.0 } else { 0.0 }; // Fixture spread step.
            frame(&mut app, spread);
            let age = (app.world().resource::<InterpolationTimeline>().now() - start).to_f32();
            let (at, _, _) = round
                .sample(f64::from(age), TICK.as_secs_f32(), 0.0)
                .expect("a delay rise cannot retire a flying round");
            assert!(at.z >= previous, "the presented path never runs backward");
            previous = at.z;
        }
    }

    /// RTT/2 cancels the remote estimate's RTT/2 anchor; it must not pace the cursor as though
    /// coverage changed. Fixture RTT changes together with that anchor, keeping the true objective.
    #[test]
    fn rtt_anchor_compensation_is_not_delay_displacement() {
        let (mut app, link) = fixture(TICK, Duration::from_millis(40));
        for _ in 0..100 {
            frame(&mut app, 0.0);
        }
        for rtt in [80, 40, 120, 40] {
            let old = app.world().get::<PingManager>(link).unwrap().rtt();
            let new = Duration::from_millis(rtt);
            app.world_mut()
                .get_mut::<PingManager>(link)
                .unwrap()
                .rtt_estimator_ewma
                .final_stats
                .rtt = new;
            let mut remote = app.world_mut().get_mut::<RemoteTimeline>(link).unwrap();
            use lightyear::core::time::TickDelta;
            remote.apply_delta(
                TickDelta::from_duration(new / 2, TICK) - TickDelta::from_duration(old / 2, TICK),
            );
            let step = frame(&mut app, 0.0);
            assert!(
                (step - 1.0).abs() < 0.001,
                "anchor compensation must not change presentation rate: {step}"
            );
            assert_eq!(
                app.world().resource::<InterpolationConfig>().min_delay,
                new / 2 + TICK
            );
        }
        assert!(app.world().resource::<Snaps>().0.is_empty());
    }

    /// Coverage is eventually paid in full, rather than counted as clock error. Fixture delay
    /// values are the review's measured step; elapsed time here is the test's controlled clock.
    #[test]
    fn measure_coverage_arrival_for_the_review_step() {
        let dt = Duration::from_nanos(16_666_667); // Fixture 60 Hz render cadence.
        let rtt = Duration::from_millis(10);
        let (mut app, _) = fixture(dt, rtt);
        let before = Duration::from_micros(35_100);
        let after = Duration::from_micros(147_600);
        for _ in 0..100 {
            frame(&mut app, (before - rtt / 2 - TICK).as_secs_f64() * 1000.0);
        }
        assert_eq!(
            app.world().resource::<InterpolationConfig>().min_delay,
            before
        );
        let mut frames = 0;
        while app.world().resource::<InterpolationConfig>().min_delay != after {
            frame(&mut app, (after - rtt / 2 - TICK).as_secs_f64() * 1000.0);
            frames += 1;
            assert!(frames < 1000, "coverage must arrive");
        }
        println!(
            "MEASURED controlled-clock coverage: 35.1 -> 147.6 ms in {frames} frames, {:.6} s at fixture 60 Hz",
            (dt * frames).as_secs_f64()
        );
        assert!(app.world().resource::<Snaps>().0.is_empty());
    }
    /// Cold startup and reconnect distributions cannot become coverage; RTT cancellation remains
    /// live. Arming latches for a link, so a temporarily cold digest cannot freeze an active law.
    #[test]
    fn warmup_excludes_pollution_and_rearms_per_link() {
        let (mut app, _) = fixture(TICK, Duration::from_millis(40));
        let cold = Duration::from_millis(20) + TICK;
        app.world_mut().resource_mut::<ArrivalDelay>().stats =
            ArrivalStats::test_cold_spread(0.243);
        app.update();
        assert_eq!(
            app.world().resource::<InterpolationConfig>().min_delay,
            cold
        );
        frame(&mut app, 100.0);
        let armed = app.world().resource::<InterpolationConfig>().min_delay;
        assert!(armed > cold);
        app.world_mut().resource_mut::<ArrivalDelay>().stats = ArrivalStats::test_cold_spread(0.1);
        app.update();
        let latched = app.world().resource::<InterpolationConfig>().min_delay;
        assert!(latched > armed);
        app.world_mut()
            .resource_mut::<ArrivalDelay>()
            .reset_for_new_link();
        app.world_mut().resource_mut::<ArrivalDelay>().stats =
            ArrivalStats::test_cold_spread(0.243);
        app.update();
        assert_eq!(
            app.world().resource::<InterpolationConfig>().min_delay,
            latched
        );
    }

    /// Genuine remote-clock error still snaps through Lightyear; the frontier gauge's contract
    /// remains that every nonzero steady count means every remote hull jumped.
    #[test]
    fn genuine_clock_displacement_still_resyncs() {
        let (mut app, link) = fixture(TICK, Duration::ZERO);
        let margin = app
            .world()
            .resource::<InterpolationConfig>()
            .sync
            .max_error_margin;
        app.world_mut()
            .get_mut::<RemoteTimeline>(link)
            .unwrap()
            .apply_duration(TICK.mul_f32(margin + 2.0), TICK);
        frame(&mut app, 0.0);
        assert_eq!(app.world().resource::<Snaps>().0.len(), 1);
    }
}
