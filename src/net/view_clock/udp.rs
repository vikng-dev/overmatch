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
const COMPUTE_S: f64 = 0.003; // Fixture: the design example's ordinary frame-to-departure work.

#[derive(Component, Default)]
struct Delay {
    queue: VecDeque<(f64, RecvPayload)>,
    packets: u64,
    burst: bool,
    compute_s: f64,
}
fn condition(real: Res<Time<Real>>, mut links: Query<(&mut Link, &mut Delay)>) {
    let now = real.elapsed_secs_f64();
    for (mut link, mut delay) in &mut links {
        for packet in link.recv.drain() {
            // Fixture: reproducible uniform integer jitter; coprime walk visits every bucket.
            let jitter = (delay.packets.wrapping_mul(7) % 20) as f64 * 0.001 - JITTER_S;
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
    frame: u32,
    at_s: f64,
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
            frame: 0,
            at_s: 0.0,
        }
    }
    fn server_at(&mut self, at_s: f64) {
        let instant = self.server_epoch + Duration::from_secs_f64(at_s);
        *self.departure.lock().unwrap() = instant + Duration::from_secs_f64(COMPUTE_S);
        self.server
            .insert_resource(TimeUpdateStrategy::ManualInstant(instant));
        self.server.update();
    }
    fn frame(&mut self, run_server: bool) {
        self.frame_for(FRAME, run_server);
    }
    fn frame_for(&mut self, duration: Duration, run_server: bool) {
        self.frame += 1;
        self.at_s += duration.as_secs_f64();
        self.client
            .insert_resource(TimeUpdateStrategy::ManualDuration(duration));
        if run_server && self.frame.is_multiple_of(2) {
            self.server_at(self.at_s);
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
    assert_eq!(
        result.0, 0,
        "bounded jitter needs zero settled holds per minute: {result:?}"
    );
    assert_eq!(result.2, 0, "a live bounded link never needs a cut");
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
    let expected = super::super::server::TICK.as_secs_f64() + COMPUTE_S + DOWN_S + JITTER_S;
    // DERIVED allowance: one client sampling phase plus one server tick/socket-drain phase.
    let allowance = FRAME.as_secs_f64() + super::super::server::TICK.as_secs_f64();
    assert!(
        (applied - expected).abs() <= allowance,
        "{applied} against derived {expected} +/- {allowance}"
    );
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

/// The player sees a hold at the last completed tick through server silence, never extrapolation
/// or a cut. Only admitted simulation backlog is replayed; discarded wall time is not a debt.
#[test]
fn udp_server_stalls_hold_without_raising_network_delay_or_cutting() {
    let _udp = super::super::test_harness::lock_real_udp_test();
    let timing = Timing::shipped();
    for stall_s in [0.200, 1.0] {
        // Fixture: requested short and capped server stalls.
        let mut pair = Pair::new(false);
        pair.run(timing.view_window_s * 2.0);
        let before_delta = pair.client.world().resource::<ViewClock>().delta_s.unwrap();
        let mut last = pair.client.world().resource::<ViewClock>().now().unwrap();
        let mut held_s = 0.0;
        let mut held_cursor = None;
        let mut remaining = Duration::from_secs_f64(stall_s);
        while !remaining.is_zero() {
            let step = remaining.min(FRAME);
            pair.frame_for(step, false);
            remaining -= step;
            let clock = pair.client.world().resource::<ViewClock>();
            let now = clock.now().unwrap();
            assert!(now >= last);
            if clock.held {
                held_s += step.as_secs_f64();
                if let Some(held) = held_cursor {
                    assert_eq!(now, held, "empty buffer holds its newest completed tick");
                }
                held_cursor = Some(now);
            }
            last = now;
        }
        assert!(
            held_s > 0.0,
            "both stalls exhaust a buffer sized for network lateness"
        );
        // Fixture recovery spends the production runner's admission cap, one tick/send per frame.
        let cap_s = pair
            .server
            .world()
            .resource::<Time<Virtual>>()
            .max_delta()
            .as_secs_f64();
        let ticks = (stall_s.min(cap_s) / super::super::server::TICK.as_secs_f64()).floor() as u32;
        for i in 0..ticks {
            pair.server_at(pair.at_s + f64::from(i) * 1e-6);
        }
        let result = pair.run(timing.view_window_s);
        let clock = pair.client.world().resource::<ViewClock>();
        assert_eq!(
            result.2, 0,
            "admitted server debt fits the continuity floor"
        );
        assert!(
            clock.now().unwrap() > last,
            "presentation resumes after completed ticks return"
        );
        // DERIVED: server silence changes S; the same fixed link cannot add that silence to x.
        assert!(
            (clock.delta_s.unwrap() - before_delta).abs()
                < super::super::server::TICK.as_secs_f64(),
            "server work must not become a W-long network delay"
        );
        println!(
            "MEASURED server stall_ms={} held_ms={} recovery={result:?} delta_change_ms={}",
            stall_s * 1e3,
            held_s * 1e3,
            (clock.delta_s.unwrap() - before_delta) * 1e3
        );
    }
}
