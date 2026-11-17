//! Headless real-UDP view proofs; fixture link times are controlled independently of the clock.
use super::*;
use bevy::platform::time::Instant;
use bevy::time::TimeUpdateStrategy;
use lightyear::link::{LinkReceiveSystems, LinkSystems, RecvPayload};
use lightyear::prelude::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

// Fixture: symmetric 80 ms RTT, integer uniform 10 ms downlink jitter; 128 Hz render reads.
const FRAME: Duration = Duration::from_nanos(7_812_500);
const DOWN_S: f64 = 0.040;
const JITTER_S: f64 = 0.010;
// Fixture: 12 ms work exceeds one read quantum, so dropping compute cannot hide inside the
// derived one-read allowance. Ordinary work also has an exact clock-law proof.
const COMPUTE_S: f64 = 0.012;

#[derive(Component, Default)]
struct Delay {
    queue: VecDeque<(f64, RecvPayload)>,
    packets: u64,
    burst: bool,
    compute_s: f64,
    jitter_s: f64,
}
fn condition(real: Res<Time<Real>>, mut links: Query<(&mut Link, &mut Delay)>) {
    let now = real.elapsed_secs_f64();
    for (mut link, mut delay) in &mut links {
        for packet in link.recv.drain() {
            // Fixture: reproducible uniform integer jitter; coprime walk visits every bucket.
            let jitter =
                ((delay.packets.wrapping_mul(7) % 20) as f64 / 10.0 - 1.0) * delay.jitter_s;
            delay.packets += 1;
            let mut due = now + DOWN_S + jitter + delay.compute_s;
            if delay.burst {
                // Fixture: the tracked burst lever's 60 ms blackout every 33 server ticks.
                let period = 33.0 * super::super::server::TICK.as_secs_f64();
                let phase = due % period;
                if phase < 0.060 {
                    due += 0.060 - phase;
                }
            }
            delay.queue.push_back((due, packet));
        }
        // A later datagram can overtake an earlier one under jitter.
        let mut ready = Vec::new();
        let mut kept = VecDeque::new();
        for (due, packet) in delay.queue.drain(..) {
            if due <= now {
                ready.push((due, packet));
            } else {
                kept.push_back((due, packet));
            }
        }
        delay.queue = kept;
        ready.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, packet) in ready {
            link.recv.push_raw(packet);
        }
    }
}

struct Pair {
    server: App,
    client: App,
    server_epoch: Instant,
    departure: Arc<Mutex<Instant>>,
    at_s: f64,
    next_server_s: f64,
    compute_samples: Vec<f64>,
    server_ticks: usize,
}
impl Pair {
    fn new(burst: bool) -> Self {
        let (mut server, mut client) = super::super::client::match_exit::in_match();
        super::super::server_runner::install_tick_schedule(&mut server);
        install(&mut client);
        super::super::frontier::install(&mut client);
        client.insert_resource(TimeUpdateStrategy::ManualDuration(FRAME));
        server.add_systems(
            PreUpdate,
            condition
                .in_set(LinkSystems::Receive)
                .after(LinkReceiveSystems::ApplyConditioner),
        );
        let up = server.world_mut().query_filtered::<Entity, (With<lightyear::prelude::server::ClientOf>, With<Connected>)>().single(server.world()).unwrap();
        server.world_mut().entity_mut(up).insert(Delay::default());
        client.add_systems(
            PreUpdate,
            condition
                .in_set(LinkSystems::Receive)
                .after(LinkReceiveSystems::ApplyConditioner),
        );
        let link = client
            .world_mut()
            .query_filtered::<Entity, With<Client>>()
            .single(client.world())
            .unwrap();
        client.world_mut().entity_mut(link).insert(Delay {
            burst,
            compute_s: COMPUTE_S,
            jitter_s: JITTER_S,
            ..default()
        });
        let server_epoch = server
            .world()
            .resource::<Time<Real>>()
            .last_update()
            .unwrap();
        let departure = Arc::new(Mutex::new(server_epoch));
        let source = departure.clone();
        server.insert_resource(super::super::tick_stamp::DepartureClock::new(move || {
            *source.lock().unwrap()
        }));
        server.world_mut().spawn((
            TickStamp::new(0.0, 0.0).unwrap(),
            Replicate::to_clients(NetworkTarget::All),
        ));
        Self {
            server,
            client,
            server_epoch,
            departure,
            at_s: 0.0,
            next_server_s: super::super::server::TICK.as_secs_f64(),
            compute_samples: vec![COMPUTE_S],
            server_ticks: 0,
        }
    }
    fn server_at(&mut self, at_s: f64) {
        let instant = self.server_epoch + Duration::from_secs_f64(at_s);
        let compute = self.compute_samples[self.server_ticks % self.compute_samples.len()];
        self.server_ticks += 1;
        *self.departure.lock().unwrap() = instant + Duration::from_secs_f64(compute);
        for mut delay in self
            .client
            .world_mut()
            .query::<&mut Delay>()
            .iter_mut(self.client.world_mut())
        {
            delay.compute_s = compute;
        }
        self.server
            .insert_resource(TimeUpdateStrategy::ManualInstant(instant));
        self.server.update();
        // Fixture: let loopback enqueue before advancing the independent manual clock. OS
        // scheduling is outside this controlled clean-link proof, not extra simulated latency.
        std::thread::sleep(Duration::from_millis(1));
    }
    fn frame(&mut self, run_server: bool) {
        self.at_s += FRAME.as_secs_f64();
        self.client
            .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME));
        while run_server && self.at_s >= self.next_server_s {
            self.server_at(self.next_server_s);
            self.next_server_s += super::super::server::TICK.as_secs_f64();
        }
        self.client.update();
    }
    fn run(&mut self, seconds: f64) -> (u64, f64, u64) {
        let mut holds = 0;
        let mut held_s = 0.0;
        let before = self.client.world().resource::<ViewClock>().cuts;
        let mut was_held = self.client.world().resource::<ViewClock>().held;
        for _ in 0..(seconds / FRAME.as_secs_f64()).ceil() as usize {
            self.frame(true);
            let clock = self.client.world().resource::<ViewClock>();
            if clock.held {
                held_s += FRAME.as_secs_f64();
                holds += u64::from(!was_held);
            }
            was_held = clock.held;
            if let Some(cursor) = clock.now() {
                let timeline = self.client.world().resource::<InterpolationTimeline>();
                assert_eq!(
                    timeline.now(),
                    cursor,
                    "the shipped controller must not overwrite the application cursor in PostUpdate"
                );
                assert!(
                    cursor <= TickInstant::from(clock.newest().unwrap()),
                    "presentation never extrapolates"
                );
            }
        }
        (
            holds,
            held_s,
            self.client.world().resource::<ViewClock>().cuts - before,
        )
    }
}

/// Hull and event consumers rely on a settled jitter buffer covering this bounded link without
/// holds. The delay expectation is derived from link latency and one production interval, not a
/// recorded result. Mounting the shipped clock also catches a live Lightyear controller overwrite.
#[test]
fn bounded_eighty_ten_udp_has_no_settled_holds() {
    let _udp = super::super::test_harness::lock_real_udp_test();
    let timing = Timing::shipped();
    let mut pair = Pair::new(false);
    pair.run(timing.view_window_s * 2.0);
    let result = pair.run(60.0); // Fixture: a minute of settled presentation.
    let clock = pair.client.world().resource::<ViewClock>();
    // The fixture knows both clock origins. Production never measures or estimates this offset.
    let client_s = pair
        .client
        .world()
        .resource::<Time<Real>>()
        .elapsed_secs_f64();
    let server_s = pair
        .server
        .world()
        .resource::<Time<Real>>()
        .elapsed_secs_f64();
    let applied = clock.delta_s.unwrap() - (client_s - server_s);
    // DERIVED: the coprime jitter walk reaches +9 ms, not +10. Delivery is read at
    // ceil((work + down + jitter)/FRAME)*FRAME; x uses the preceding read. The socket
    // may slip one read phase. There is no extra server-tick or symmetric allowance.
    let peak_jitter = JITTER_S * (19.0 / 10.0 - 1.0);
    let expected = super::super::server::TICK.as_secs_f64()
        + ((COMPUTE_S + DOWN_S + peak_jitter) / FRAME.as_secs_f64()).ceil() * FRAME.as_secs_f64()
        - FRAME.as_secs_f64();
    let allowance = FRAME.as_secs_f64();
    assert!(
        applied >= expected - 1e-9 && applied <= expected + allowance + 1e-9,
        "{applied} against derived [{expected}, {}]",
        expected + allowance
    );
    assert_eq!(
        result.0, 0,
        "bounded jitter needs zero settled holds per minute: {result:?}"
    );
    assert_eq!(result.2, 0, "a live bounded link never needs a cut");
    println!(
        "MEASURED 80/10 UDP: holds/min={} held_s={} applied_ms={} DERIVED expected_ms={} sampling_allowance_ms={}",
        result.0,
        result.1,
        applied * 1e3,
        expected * 1e3,
        allowance * 1e3
    );
}

/// The burst lever's repeatable blackout must enter the measured absence window and cover later
/// repetitions. A percentile or a window that forgets the burst between repetitions fails this.
#[test]
fn sixty_ms_udp_bursts_are_covered_after_the_window_settles() {
    let _udp = super::super::test_harness::lock_real_udp_test();
    let timing = Timing::shipped();
    let mut pair = Pair::new(true);
    pair.run(timing.view_window_s * 2.0);
    let result = pair.run(timing.view_window_s * 2.0);
    assert_eq!(
        result.0, 0,
        "periodic bounded bursts must be covered once settled: {result:?}"
    );
    assert_eq!(result.2, 0);
    println!("MEASURED 60ms UDP bursts: {result:?}");
}

/// A clean link under variable server work must have zero settled holds. The distribution is
/// Fixture data spanning cheap and near-budget frames, not a claim of a measured server profile.
#[test]
fn loaded_server_compute_distribution_has_no_settled_holds() {
    let _udp = super::super::test_harness::lock_real_udp_test();
    let mut pair = Pair::new(false);
    // Fixture: deterministic load distribution, milliseconds, all inside the authority budget.
    pair.compute_samples = vec![0.0015, 0.002, 0.0036, 0.008, 0.0143];
    for mut delay in pair
        .client
        .world_mut()
        .query::<&mut Delay>()
        .iter_mut(pair.client.world_mut())
    {
        delay.jitter_s = 0.0; // Fixture: clean link with fixed propagation.
    }
    pair.run(Timing::shipped().view_window_s * 2.0);
    let result = pair.run(60.0); // Fixture: one settled minute.
    assert_eq!(
        result.0, 0,
        "frame-start lateness must cover loaded-server work: {result:?}"
    );
    assert_eq!(result.2, 0);
    println!("MEASURED loaded-server clean UDP: {result:?}");
}

/// Stalls are observations of feel, not pinned output obligations. The invariants are monotonic
/// presentation, no extrapolation and bounded ordinary steps; cuts are deliberate discontinuities.
#[test]
fn server_stall_and_delay_spike_playback_observations() {
    let _udp = super::super::test_harness::lock_real_udp_test();
    let timing = Timing::shipped();
    // Fixture: short server work, capped server work, and a link-only blackout.
    for (server_stall, pause) in [(true, 0.200_f64), (true, 1.0), (false, 0.500)] {
        let mut pair = Pair::new(false);
        pair.run(timing.view_window_s * 2.0);
        let baseline = pair.client.world().resource::<ViewClock>().delta_s.unwrap();
        let start = pair.at_s;
        let client_start = pair
            .client
            .world()
            .resource::<Time<Real>>()
            .elapsed_secs_f64();
        // Fixture: a 200 ms server computation, or a 500 ms downlink blackout.
        let mut last = pair.client.world().resource::<ViewClock>().now().unwrap();
        let mut holds = 0.0;
        let mut peak_delta = baseline;
        let mut last_above = 0.0;
        let mut first_above = None;
        let mut above_s = 0.0;
        let mut slow_s = 0.0;
        let mut fast_s = 0.0;
        let mut cut_times = Vec::new();
        let mut previous_cuts = pair.client.world().resource::<ViewClock>().cuts;
        // Fixture: enough time for acquisition, window expiry and release of the extra delay.
        let frames = ((timing.view_window_s * 2.0 + pause / timing.view_rate) / FRAME.as_secs_f64())
            .ceil() as usize;
        for frame in 0..frames {
            let elapsed = pair.at_s - start;
            let paused = elapsed < pause;
            if !server_stall {
                // Queue opaque datagrams, then release them together; no packet bytes are edited.
                for mut delay in pair
                    .client
                    .world_mut()
                    .query::<&mut Delay>()
                    .iter_mut(pair.client.world_mut())
                {
                    if paused {
                        for (due, _) in &mut delay.queue {
                            *due = (*due).max(client_start + pause);
                        }
                    }
                }
            }
            if server_stall && paused {
                pair.frame(false);
            } else {
                if server_stall && elapsed < pause + FRAME.as_secs_f64() {
                    pair.compute_samples = vec![pause];
                    pair.server_at(start + super::super::server::TICK.as_secs_f64());
                    pair.compute_samples = vec![COMPUTE_S];
                    // Spend only admitted simulation debt, each tick gets its own send.
                    let ticks = (pause.min(timing.admission_s)
                        / super::super::server::TICK.as_secs_f64())
                    .floor() as usize;
                    for _ in 1..ticks {
                        pair.server_at(pair.at_s);
                    }
                    // The admitted remainder, not a render phase, determines the next deadline.
                    let debt = pair.server.world().resource::<Time<Fixed>>().overstep();
                    pair.next_server_s = pair.at_s
                        + super::super::server::TICK
                            .saturating_sub(debt)
                            .as_secs_f64();
                }
                pair.frame(true);
            }
            let clock = pair.client.world().resource::<ViewClock>();
            let now = clock.now().unwrap();
            assert!(now >= last, "presentation never rewinds");
            assert!(
                now <= TickInstant::from(clock.newest().unwrap()),
                "no extrapolation"
            );
            let rate = f64::from((now - last).to_f32()) * super::super::server::TICK.as_secs_f64()
                / FRAME.as_secs_f64();
            if clock.cuts > previous_cuts {
                cut_times.push((pair.at_s - start, number(now) - number(last)));
                previous_cuts = clock.cuts;
            } else if frame > 0 {
                assert!(
                    rate <= 1.0 + timing.view_rate + 1e-3,
                    "ordinary playback respects pace"
                );
                if rate > 0.0 && rate < 0.99 {
                    slow_s += FRAME.as_secs_f64();
                }
                if rate > 1.01 {
                    fast_s += FRAME.as_secs_f64();
                }
            }
            if clock.held {
                holds += FRAME.as_secs_f64();
            }
            let delta = clock.delta_s.unwrap();
            peak_delta = peak_delta.max(delta);
            if delta > baseline + 1e-6 {
                last_above = pair.at_s - start;
                first_above.get_or_insert(last_above);
                above_s += FRAME.as_secs_f64();
            }
            last = now;
        }
        println!(
            "MEASURED {}ms {}: held_ms={} cuts={cut_times:?} baseline_ms={} peak_ms={} delta_above_baseline_from_s={first_above:?} until_s={} duration_s={above_s} slow_s={slow_s} fast_s={fast_s}",
            pause * 1e3,
            if server_stall {
                "compute stall"
            } else {
                "downlink spike"
            },
            holds * 1e3,
            baseline * 1e3,
            peak_delta * 1e3,
            last_above
        );
    }
}
