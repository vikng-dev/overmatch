//! Installer-mounted pacing proofs. Fixture production stamps reach the shipped view clock
//! through the replication completion path; production and link times are fixture data. The
//! law and its derivations: `.agents/docs/design/net-motion-contract.md`.
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
/// DERIVED, independently of the implementation: the widest critically damped band whose rate
/// response, of L1 norm 2 omega / e, keeps a tick of target phase within the slew limit v.
fn expected_bandwidth(v: f64) -> f64 {
    E * v / tick_s()
}
/// DERIVED: beyond 20 / omega the loop's rate and phase kernels keep below 1e-7 of their L1
/// norms, so this much history bounds the disturbances that reach a measured frame.
fn memory_s(v: f64) -> f64 {
    20.0 / expected_bandwidth(v)
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
/// Fixture: a regular server at `speed` times real time.
fn at_speed(speed: f64) -> impl Fn(u32) -> f64 {
    move |k| f64::from(k) * tick_s() / speed
}
/// Fixture: a regular server that changes speed at `at_s`.
fn speed_step(from: f64, to: f64, at_s: f64) -> impl Fn(u32) -> f64 {
    let first = (at_s * from / tick_s()).ceil() as u32;
    move |k| {
        if k < first {
            f64::from(k) * tick_s() / from
        } else {
            f64::from(first) * tick_s() / from + f64::from(k - first) * tick_s() / to
        }
    }
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
    /// The cursor moved onto the newest completed tick from behind it.
    contact: bool,
    /// The newest completed tick minus the inverse-map target.
    behind_ticks: f64,
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
            let last = TickInstant::from(clock.newest().unwrap());
            assert!(now <= last, "presentation never passes completed facts");
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
                contact: before < last && now == last,
                behind_ticks: number(last) - number(target),
            });
        }
        frames
    }
}

fn range(frames: &[Frame], value: impl Fn(&Frame) -> f64) -> f64 {
    let (low, high) = frames
        .iter()
        .map(value)
        .fold((f64::MAX, f64::MIN), |(low, high), x| {
            (low.min(x), high.max(x))
        });
    high - low
}

/// Hold episodes: frames whose hold begins.
fn contacts(frames: &[Frame]) -> usize {
    frames
        .iter()
        .scan(false, |was, f| {
            let onset = f.held && !*was;
            *was = f.held;
            Some(onset)
        })
        .filter(|&onset| onset)
        .count()
}

fn summary(frames: &[Frame], v: f64) -> String {
    let mut rates: Vec<f64> = frames.iter().map(|f| f.rate).collect();
    rates.sort_by(f64::total_cmp);
    let n = rates.len() as f64;
    let rank = |p: f64| rates[((p * n).ceil() as usize).clamp(1, rates.len()) - 1];
    let share = |f: &dyn Fn(f64) -> bool| rates.iter().filter(|&&r| f(r)).count() as f64 / n;
    format!(
        "n={} min={:.4} p1={:.4} p5={:.4} p50={:.4} p95={:.4} p99={:.4} max={:.4} at_1+v={:.4} outside_1pct={:.4} held_frames={} contacts={}",
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
        contacts(frames),
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
/// DERIVED: the rate's response to target phase has L1 gain v / T, so its deviation is at most
/// v / T times the target's peak-to-peak phase (the production spread plus the applied delay's
/// range). The window's rate reads a phase within the spread over a settled span above W / 2,
/// so it moves by at most 4 spread / W, and reaches the rate high-passed by a kernel of unit
/// area. Following the stamp map segment by segment leaves this band.
#[test]
fn irregular_production_plays_at_a_regular_rate() {
    let timing = Timing::shipped();
    let v = timing.view_rate;
    // Fixture: the late ticks start on schedule. The delay covers the tick that sets its
    // lateness maximum only from a production phase below the mean; the observations report
    // the other cases.
    let start = |k: u32| if late(k) { regular(k) } else { jittered(k) };
    let settle_s = timing.view_window_s * 2.0;
    let mut stream = Stream::new();
    stream.run(settle_s, &start, &slack_link);
    let mut frames = stream.run(settle_s + memory_s(v), &start, &slack_link);
    let measured = frames.len();
    frames.extend(stream.run(settle_s + memory_s(v) + MEASURE_S, &start, &slack_link));
    let applied = range(&frames, |f| f.applied_s);
    let band = v * (SPREAD_TICKS + applied / tick_s())
        + 4.0 * SPREAD_TICKS * tick_s() / timing.view_window_s
        + quantization();
    let frames = &frames[measured..];
    assert!(frames.last().unwrap().at_s >= settle_s + memory_s(v) + MEASURE_S - frame_s());
    println!(
        "MEASURED jittered production, lateness slack: {} applied_delta_range_ms={:.3} DERIVED band={band:.4}",
        summary(frames, v),
        applied * 1e3
    );
    for frame in frames {
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

/// Observations only, for the estimator-coverage section of the motion contract; `Stream::run`
/// asserts the class invariants.
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

/// A slow server shows as smooth slow motion (#328); hull and event consumers rely on a regular
/// stream at any speed the cursor can follow playing without stops. DERIVED: the window's rate
/// of a regular stream is its speed, so once the window holds only this speed the cursor is the
/// target's phase through a positive kernel of unit area. Its lead is then bounded by the
/// target's phase range, which a regular stream draws only from the applied delay, and its rate
/// by v / T times that range, both within quantization.
#[test]
fn a_regular_server_at_any_followable_speed_plays_without_holds_or_lead() {
    let timing = Timing::shipped();
    let v = timing.view_rate;
    let settle_s = timing.view_window_s * 2.0;
    // Fixture: server speeds from half to just above real time, inside the 1 + v cap.
    for speed in [0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1] {
        let start = at_speed(speed);
        let mut stream = Stream::new();
        stream.run(settle_s, &start, &constant_link);
        let mut frames = stream.run(settle_s + memory_s(v), &start, &constant_link);
        let measured = frames.len();
        frames.extend(stream.run(settle_s + memory_s(v) + MEASURE_S, &start, &constant_link));
        let phase_s = speed * range(&frames, |f| f.applied_s);
        let lead_bound = phase_s + 2.0 * tick_s() / (f64::from(u16::MAX) + 1.0);
        let rate_bound = v * phase_s / tick_s() + quantization();
        let frames = &frames[measured..];
        assert!(frames.last().unwrap().at_s >= settle_s + memory_s(v) + MEASURE_S - frame_s());
        println!(
            "MEASURED regular server at {speed}: {} lead_ms=[{:.4},{:.4}] DERIVED lead_bound_ms={:.4} rate_bound={rate_bound:.5}",
            summary(frames, v),
            frames.iter().map(|f| f.lead_s).fold(f64::MAX, f64::min) * 1e3,
            frames.iter().map(|f| f.lead_s).fold(f64::MIN, f64::max) * 1e3,
            lead_bound * 1e3
        );
        for frame in frames {
            assert!(
                !frame.held && !frame.cut,
                "a regular server at {speed} plays without stops"
            );
            assert!(
                frame.lead_s.abs() <= lead_bound,
                "at {speed} the cursor leads its target by {} ms",
                frame.lead_s * 1e3
            );
            assert!(
                (frame.rate - speed).abs() <= rate_bound,
                "at {speed} the rate {} leaves the stream's rate",
                frame.rate
            );
        }
    }
}

/// After a server speed change the window mixes both speeds, and the loop carries the measured
/// rate's error until the window turns over; hull consumers rely on playback settling then.
/// DERIVED: the last sample at the old speed leaves the window W after its arrival, a link
/// time and a frame after the change, and the kernel's memory covers the loop's settling. The
/// frontier's hold diagnostics rely on `held` naming every stop at the newest completed tick,
/// including stops while the target is still behind it, which this fill produces.
#[test]
fn a_server_speed_change_settles_once_the_window_turns_over_and_every_stop_is_a_hold() {
    let timing = Timing::shipped();
    let v = timing.view_rate;
    let switch_s = timing.view_window_s * 2.0;
    let settled_s = switch_s + LINK_S + frame_s() + timing.view_window_s + memory_s(v);
    let until_s = settled_s + timing.view_window_s;
    // Fixture: a hundredth of a tick of lead separates a stop from rounding onto a tick.
    let ahead_ticks = 0.01;
    let mut witnesses = 0;
    // Fixture: slowdowns to half and nine tenths of real time, and a recovery from half.
    for (from, to) in [(1.0, 0.5), (1.0, 0.9), (0.5, 1.0)] {
        let start = speed_step(from, to, switch_s);
        let mut stream = Stream::new();
        stream.run(switch_s, &start, &constant_link);
        let frames = stream.run(until_s, &start, &constant_link);
        assert!(frames.last().unwrap().at_s >= until_s - frame_s());
        let stops: Vec<_> = frames
            .iter()
            .filter(|f| f.contact && f.behind_ticks > ahead_ticks)
            .collect();
        witnesses += stops.len();
        println!(
            "MEASURED speed {from} to {to}: held_frames={} contacts={} stops_ahead_of_target={} last_hold_after_s={:?} lead_ms=[{:.3},{:.3}] rate=[{:.4},{:.4}] worst_rate_change={:.4} DERIVED settled_after_s={:.3}",
            frames.iter().filter(|f| f.held).count(),
            contacts(&frames),
            stops.len(),
            frames
                .iter()
                .rev()
                .find(|f| f.held)
                .map(|f| f.at_s - switch_s),
            frames.iter().map(|f| f.lead_s).fold(f64::MAX, f64::min) * 1e3,
            frames.iter().map(|f| f.lead_s).fold(f64::MIN, f64::max) * 1e3,
            frames.iter().map(|f| f.rate).fold(f64::MAX, f64::min),
            frames.iter().map(|f| f.rate).fold(f64::MIN, f64::max),
            frames
                .windows(2)
                .map(|pair| (pair[1].rate - pair[0].rate).abs())
                .fold(0.0, f64::max),
            settled_s - switch_s
        );
        for frame in stops {
            assert!(frame.held, "the stop at {} s is a hold", frame.at_s);
        }
        for frame in frames.iter().filter(|f| f.at_s >= settled_s) {
            assert!(
                !frame.held && !frame.cut,
                "at {to} the turned-over window plays without stops"
            );
        }
    }
    assert!(
        witnesses > 0,
        "the fill reaches the newest tick ahead of its target"
    );
}

/// Hull and event consumers rely on a released server stall holding once, then spending its
/// backlog at no more than (1+v) and closing it within the slew-limited time. DERIVED: a
/// backlog B saturates the rate at v until 2 v / omega remains, and the critically damped exit
/// leaves 4 e^-2 v / omega, under v / omega, at B / v; one frame to leave rest after the hold
/// and one of held target follow. B is below the silence, which bounds the observation.
#[test]
fn a_released_server_stall_holds_once_then_catches_up_at_the_slew_limit() {
    let timing = Timing::shipped();
    let v = timing.view_rate;
    let omega = expected_bandwidth(v);
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
    let until_s = stall_s + pause_s + LINK_S + pause_s / v + 2.0 * frame_s() + memory_s(v);
    let frames = stream.run(until_s, &start, &constant_link);
    assert_eq!(contacts(&frames), 1, "the silence holds the cursor once");
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
    assert!(backlog_s < pause_s, "the held cursor had buffered facts");
    let deadline_s = frames[peak].at_s + backlog_s / v + 2.0 * frame_s();
    assert!(frames.last().unwrap().at_s >= deadline_s);
    let caught = frames[peak..]
        .iter()
        .find(|f| f.lead_s.abs() <= v / omega)
        .expect("the backlog closes");
    println!(
        "MEASURED released 200 ms stall: held_ms={} contacts={} backlog_ms={} caught_up_after_peak_ms={} DERIVED deadline_ms={}",
        frames.iter().filter(|f| f.held).count() as f64 * frame_s() * 1e3,
        contacts(&frames),
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
    let omega = expected_bandwidth(v);
    // Fixture: a link 100 ms slower for two windows; expiry of its lateness steps the target.
    let (switch_s, slow_s) = (timing.view_window_s * 2.0, 0.100);
    let complete = move |_: u32, s: f64| s + LINK_S + if s < switch_s { slow_s } else { 0.0 };
    let mut stream = Stream::new();
    let settled = stream.run(switch_s, &regular, &complete);
    let before = settled.last().unwrap().delay_target_s;
    // DERIVED: the slow lateness expires one window after the switch, plus its own link time.
    let until_s = switch_s + timing.view_window_s + slow_s + slow_s / v + 2.0 / omega + memory_s(v);
    let frames = stream.run(until_s, &regular, &complete);
    let drop = frames
        .iter()
        .position(|f| f.delay_target_s < before - slow_s / 2.0)
        .expect("the slow link's lateness expires from the window");
    let step_s = before - frames.last().unwrap().delay_target_s;
    let deadline_s = frames[drop].at_s + step_s / v + 2.0 / omega + 2.0 * frame_s();
    assert!(frames.last().unwrap().at_s >= deadline_s);
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
