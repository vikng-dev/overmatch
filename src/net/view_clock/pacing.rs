//! Installer-mounted pacing proofs. Fixture production stamps reach the shipped view clock
//! through the replication completion path; production and link times are fixture data.
use super::*;
use bevy::time::TimeUpdateStrategy;
use std::f64::consts::E;

// Fixture: a 120 Hz display, incommensurate with the 64 Hz authority.
const FRAME: Duration = Duration::from_nanos(8_333_333);
// Fixture: constant one-way completion delay.
const LINK_S: f64 = 0.040;
// Fixture: production phase spread, a quarter tick peak to peak, late like an oversleep.
const SPREAD_TICKS: f64 = 0.25;
// Fixture: a minute of settled playback; recoveries get the time their derived bound needs.
const MEASURE_S: f64 = 60.0;

fn tick_s() -> f64 {
    super::super::server::TICK.as_secs_f64()
}
fn frame_s() -> f64 {
    FRAME.as_secs_f64()
}
/// DERIVED: both endpoint oversteps of a frame's advance round to U0F16 quanta.
fn quantization() -> f64 {
    2.0 / (f64::from(u16::MAX) + 1.0) * tick_s() / frame_s()
}

/// Fixture production: tick k starts at k T plus a phase within `SPREAD_TICKS` of a tick,
/// alternately on schedule and late, or late by a per-tick uniform draw.
fn alternating(k: u32) -> f64 {
    (f64::from(k) + SPREAD_TICKS * f64::from(k % 2)) * tick_s()
}
fn jittered(k: u32) -> f64 {
    // Fixture: deterministic xorshift phase per tick, uniform over the spread.
    let mut x = u64::from(k).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (f64::from(k) + SPREAD_TICKS * (x >> 11) as f64 / (1u64 << 53) as f64) * tick_s()
}
fn regular(k: u32) -> f64 {
    f64::from(k) * tick_s()
}

struct Frame {
    at_s: f64,
    /// Simulation seconds per Real second over the frame.
    rate: f64,
    /// Cursor minus its inverse-map target, in simulation seconds.
    lead_s: f64,
    applied_s: f64,
    delay_target_s: f64,
    held: bool,
    cut: bool,
}

struct Stream {
    app: App,
    flight: Vec<(f64, u32, f64)>,
    next: u32,
}

impl Stream {
    fn new() -> Self {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ReplicationStorage>()
            .init_resource::<ReplicationCheckpointMap>()
            .init_resource::<InterpolationTimeline>()
            .insert_resource(lightyear::core::tick::TickDuration(
                super::super::server::TICK,
            ))
            .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
            .add_message::<MutateTickReceived>();
        install(&mut app);
        app.update(); // Real's first sample has zero delta.
        Self {
            app,
            flight: Vec::new(),
            next: 1,
        }
    }

    fn now_s(&self) -> f64 {
        self.app.world().resource::<Time<Real>>().elapsed_secs_f64()
    }

    /// Client frames until `until_s`. Each frame sends every fixture tick started by its end
    /// and delivers those completed by its end; `start` gives a tick's frame-start stamp and
    /// `complete` its client completion time. Frames before a view epoch are not recorded.
    fn run(
        &mut self,
        until_s: f64,
        start: &impl Fn(u32) -> f64,
        complete: &impl Fn(u32, f64) -> f64,
    ) -> Vec<Frame> {
        let mut frames = Vec::new();
        while self.now_s() + frame_s() <= until_s {
            let due = self.now_s() + frame_s();
            while start(self.next) <= due {
                let s = start(self.next);
                self.flight.push((complete(self.next, s), self.next, s));
                self.next += 1;
            }
            self.flight.sort_by(|a, b| a.0.total_cmp(&b.0));
            let ready = self.flight.partition_point(|&(at, _, _)| at <= due);
            let world = self.app.world_mut();
            for (_, tick, s) in self.flight.drain(..ready) {
                let checkpoint = RepliconTick::new(tick);
                world
                    .resource_mut::<ReplicationCheckpointMap>()
                    .record(checkpoint, Tick(tick));
                world
                    .resource_mut::<ReplicationStorage>()
                    .global
                    .get_or_default::<ReceivedStamps>()
                    .queue
                    .push_back((checkpoint, TickStamp::new(s, 0.0).unwrap()));
                world.write_message(MutateTickReceived { tick: checkpoint });
            }
            let (before, cuts) = {
                let clock = self.app.world().resource::<ViewClock>();
                (clock.now(), clock.cuts)
            };
            self.app.update();
            let at_s = self.now_s();
            let clock = self.app.world().resource::<ViewClock>();
            let (Some(before), Some(now)) = (before, clock.now()) else {
                continue; // Waiting for the first completed stamp pair.
            };
            assert!(
                now <= TickInstant::from(clock.newest().unwrap()),
                "presentation never passes completed facts"
            );
            let applied_s = clock.delta_s.unwrap();
            let target = clock.inverse(at_s - applied_s).unwrap();
            frames.push(Frame {
                at_s,
                rate: (number(now) - number(before)) * tick_s() / frame_s(),
                lead_s: (number(now) - number(target)) * tick_s(),
                applied_s,
                delay_target_s: clock.target(&mut clock.intervals()).unwrap(),
                held: clock.held,
                cut: clock.cuts > cuts,
            });
        }
        frames
    }
}

fn summary(frames: &[Frame], v: f64) -> String {
    let mut rates: Vec<f64> = frames.iter().map(|f| f.rate).collect();
    rates.sort_by(f64::total_cmp);
    let n = rates.len() as f64;
    let rank = |p: f64| rates[((p * n).ceil() as usize).clamp(1, rates.len()) - 1];
    let share = |f: &dyn Fn(f64) -> bool| rates.iter().filter(|&&r| f(r)).count() as f64 / n;
    format!(
        "n={} min={:.4} p1={:.4} p5={:.4} p50={:.4} p95={:.4} p99={:.4} max={:.4} at_1+v={:.4} outside_1pct={:.4} held_frames={}",
        rates.len(),
        rank(0.0),
        rank(0.01),
        rank(0.05),
        rank(0.5),
        rank(0.95),
        rank(0.99),
        rank(1.0),
        share(&|r| r >= 1.0 + v - 1e-3),
        share(&|r| (r - 1.0).abs() > 0.01),
        frames.iter().filter(|f| f.held).count(),
    )
}

/// Fixture: every 61st tick completes a tick late, so the window's lateness maximum leaves
/// every other tick a tick of slack.
fn late(k: u32) -> bool {
    k.is_multiple_of(61)
}
fn slack_link(k: u32, s: f64) -> f64 {
    s + LINK_S + if late(k) { tick_s() } else { 0.0 }
}
fn constant_link(_: u32, s: f64) -> f64 {
    s + LINK_S
}

/// Remote hull motion relies on the cursor playing irregular production at a regular rate.
/// The pacing law bounds the rate's deviation by the L1 gain of its rate response, omega / e,
/// times the target's peak-to-peak phase: the fixture's production spread plus the applied
/// delay's own range. Following the stamp map segment by segment leaves this band.
#[test]
fn irregular_production_plays_at_a_regular_rate() {
    let timing = Timing::shipped();
    let v = timing.view_rate;
    let omega = bandwidth(v, tick_s());
    // Fixture: the late ticks start on schedule. The delay covers the tick that sets its
    // lateness maximum only from a production phase below the mean; the observations report
    // the other cases.
    let start = |k: u32| if late(k) { regular(k) } else { jittered(k) };
    let mut stream = Stream::new();
    stream.run(timing.view_window_s * 2.0, &start, &slack_link);
    // DERIVED: the rate kernel decays as exp(-omega t); a second of history covers it.
    let lead_in = stream.run(timing.view_window_s * 2.0 + 1.0, &start, &slack_link);
    let frames = stream.run(
        timing.view_window_s * 2.0 + 1.0 + MEASURE_S,
        &start,
        &slack_link,
    );
    let (low, high) = lead_in
        .iter()
        .chain(&frames)
        .fold((f64::MAX, f64::MIN), |(low, high), f| {
            (low.min(f.applied_s), high.max(f.applied_s))
        });
    let band = omega / E * (SPREAD_TICKS * tick_s() + high - low) + quantization();
    println!(
        "MEASURED jittered production, lateness slack: {} applied_delta_range_ms={:.3} DERIVED band={band:.4}",
        summary(&frames, v),
        (high - low) * 1e3
    );
    for frame in &frames {
        assert!(
            !frame.held && !frame.cut,
            "a delay covering production never holds or cuts"
        );
        assert!(
            (frame.rate - 1.0).abs() <= band,
            "rate {} outside the pacing bound 1 +/- {band}",
            frame.rate
        );
    }
}

/// The delay target covers lateness, not production phase: its one-interval margin leaves a
/// smooth cursor short by the production phase above its mean at the tick that sets the
/// lateness maximum, and the nearest-rank median of alternating intervals is the short one
/// about half the time. Observations only; `Stream::run` asserts the class invariants.
#[test]
fn irregular_production_coverage_observations() {
    let timing = Timing::shipped();
    let link = constant_link as fn(u32, f64) -> f64;
    for (name, start, complete) in [
        ("regular, constant link", regular as fn(u32) -> f64, link),
        ("alternating, constant link", alternating, link),
        ("jittered, constant link", jittered, link),
        ("alternating, lateness slack", alternating, slack_link),
        (
            "jittered, lateness slack at any phase",
            jittered,
            slack_link,
        ),
    ] {
        let mut stream = Stream::new();
        stream.run(timing.view_window_s * 2.0, &start, &complete);
        let frames = stream.run(timing.view_window_s * 2.0 + MEASURE_S, &start, &complete);
        let (low, high) = frames.iter().fold((f64::MAX, f64::MIN), |(low, high), f| {
            (low.min(f.delay_target_s), high.max(f.delay_target_s))
        });
        println!(
            "MEASURED {name}: {} delay_target_ms=[{:.3},{:.3}]",
            summary(&frames, timing.view_rate),
            low * 1e3,
            high * 1e3
        );
    }
}

/// Hull and event consumers rely on a released server stall's backlog being spent at no more
/// than (1+v) and closed within the slew-limited time. DERIVED: a backlog B saturates the rate
/// at v until 2 v / omega remains, and the critically damped exit leaves 4 e^-2 v / omega, under
/// v / omega, at B / v; one frame to leave rest after the hold and one of held target follow.
#[test]
fn a_released_server_stall_holds_then_catches_up_at_the_slew_limit() {
    let timing = Timing::shipped();
    let v = timing.view_rate;
    let omega = bandwidth(v, tick_s());
    // Fixture: regular production, a 200 ms server silence inside the admission cap, then the
    // admitted debt spent one tick per 1 ms server frame.
    let (stall_s, pause_s, compute_s) = (timing.view_window_s * 2.0, 0.200, 0.001);
    let first = (stall_s / tick_s()).ceil() as u32;
    let start = move |k: u32| {
        let due = regular(k);
        if k < first {
            due
        } else {
            due.max(stall_s + pause_s + f64::from(k - first) * compute_s)
        }
    };
    let mut stream = Stream::new();
    stream.run(stall_s, &start, &constant_link);
    // Fixture: three seconds outlast the derived catch-up of this backlog.
    let frames = stream.run(stall_s + pause_s + 3.0, &start, &constant_link);
    assert!(
        frames.iter().any(|f| f.held),
        "the silence exhausts the buffer"
    );
    assert!(
        frames.iter().all(|f| !f.cut),
        "a backlog inside the admission cap is paced, not cut"
    );
    let (peak, backlog_s) = frames
        .iter()
        .enumerate()
        .map(|(i, f)| (i, -f.lead_s))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    let deadline_s = frames[peak].at_s + backlog_s / v + 2.0 * frame_s();
    let caught = frames[peak..]
        .iter()
        .find(|f| f.lead_s.abs() <= v / omega)
        .expect("the backlog closes");
    println!(
        "MEASURED released 200 ms stall: held_ms={} backlog_ms={} caught_up_after_peak_ms={} DERIVED deadline_ms={}",
        frames.iter().filter(|f| f.held).count() as f64 * frame_s() * 1e3,
        backlog_s * 1e3,
        (caught.at_s - frames[peak].at_s) * 1e3,
        (deadline_s - frames[peak].at_s) * 1e3
    );
    for frame in &frames {
        assert!(
            frame.rate <= 1.0 + v + quantization(),
            "catch-up respects the slew limit"
        );
        if frame.at_s >= deadline_s {
            assert!(
                frame.lead_s.abs() <= v / omega,
                "the cursor is {} ms from its target after the derived catch-up time",
                frame.lead_s * 1e3
            );
        }
    }
}

/// A delay-target step reaches the cursor through the applied delay's slew: playback speeds up
/// to no more than (1+v), and its rate changes by at most 3 v^2 F / T per frame. DERIVED: the
/// critically damped response to a slope change v accelerates at most v omega / e = v^2 / T per
/// second, and the slew's per-frame staircase differs from that ramp by a phase of v F, worth at
/// most v^2 F / T of rate in each frame. The target settles within the slew time plus 2 / omega,
/// where the exit from a slope v leaves 4 e^-2 v / omega, plus a frame of held target and one of
/// the slew's own step. Moving at the slewed target's own rate jumps by v in one frame.
#[test]
fn a_delay_target_step_is_absorbed_smoothly_within_the_slew_limit() {
    let timing = Timing::shipped();
    let v = timing.view_rate;
    let omega = bandwidth(v, tick_s());
    // Fixture: a link 100 ms slower for two windows; expiry of its lateness steps the target.
    let (switch_s, slow_s) = (timing.view_window_s * 2.0, 0.100);
    let complete = move |_: u32, s: f64| s + LINK_S + if s < switch_s { slow_s } else { 0.0 };
    let mut stream = Stream::new();
    let settled = stream.run(switch_s, &regular, &complete);
    let before = settled.last().unwrap().delay_target_s;
    // Fixture: three seconds past the window outlast the derived absorption of this step.
    let frames = stream.run(switch_s + timing.view_window_s + 3.0, &regular, &complete);
    let drop = frames
        .iter()
        .position(|f| f.delay_target_s < before - slow_s / 2.0)
        .expect("the slow link's lateness expires from the window");
    let step_s = before - frames.last().unwrap().delay_target_s;
    let deadline_s = frames[drop].at_s + step_s / v + 2.0 / omega + 2.0 * frame_s();
    let jerk = 3.0 * v * v * frame_s() / tick_s() + 2.0 * quantization();
    let worst = frames
        .windows(2)
        .map(|pair| (pair[1].rate - pair[0].rate).abs())
        .fold(0.0, f64::max);
    println!(
        "MEASURED delay step {:.3} ms: rate=[{:.4},{:.4}] worst_rate_change_per_frame={worst:.5} DERIVED limit={jerk:.5}",
        step_s * 1e3,
        frames.iter().map(|f| f.rate).fold(f64::MAX, f64::min),
        frames.iter().map(|f| f.rate).fold(f64::MIN, f64::max)
    );
    assert!(worst <= jerk, "a delay step never becomes a rate jump");
    for frame in &frames {
        assert!(!frame.held && !frame.cut, "a falling delay never starves");
        assert!(
            frame.rate >= 1.0 - quantization() && frame.rate <= 1.0 + v + quantization(),
            "a falling delay is absorbed by speeding up within the slew limit: {}",
            frame.rate
        );
        if frame.at_s >= deadline_s {
            assert!(
                frame.lead_s.abs() <= v / omega,
                "the step is absorbed by the derived time"
            );
        }
    }
}
