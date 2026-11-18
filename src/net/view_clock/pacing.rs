//! Installer-mounted pacing proofs. Fixture production stamps reach the shipped view clock
//! through the replication completion path; production and link times are fixture data. Every
//! bound is derived here from v, T, W and the kernel constant kappa, never read from the
//! implementation. The law and its derivations: `.agents/docs/design/net-motion-contract.md`.
use super::*;
use bevy::time::TimeUpdateStrategy;

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
fn stamp(start_s: f64) -> TickStamp {
    TickStamp::new(start_s, 0.0).unwrap()
}
fn frame_s() -> f64 {
    FRAME.as_secs_f64()
}
/// One U0F16 cursor quantum, in simulation seconds.
fn quantum_s() -> f64 {
    tick_s() / (f64::from(u16::MAX) + 1.0)
}
/// DERIVED: both endpoint oversteps of a frame's advance round to U0F16 quanta.
fn quantization(dt_s: f64) -> f64 {
    2.0 * quantum_s() / dt_s
}
/// DERIVED: half the L1 norm, in x = omega t, of (3 - 5 x + x^2) e^-x, the rate's response to
/// target phase through a stream-rate filter and a critically damped loop sharing the pole
/// omega. Its integral is (3 x - x^2) e^-x, so the norm is twice the sum of |(3 - 2 x) e^-x| at
/// the sign changes x = (5 -+ sqrt 13) / 2.
fn kappa() -> f64 {
    let root = 13.0_f64.sqrt();
    (root - 2.0) * (-(5.0 - root) / 2.0).exp() + (root + 2.0) * (-(5.0 + root) / 2.0).exp()
}
/// DERIVED: phase within p peak to peak moves the rate by at most kappa omega p, so the widest
/// band keeping a tick of phase within the slew limit v is v / (kappa T).
fn expected_bandwidth(v: f64) -> f64 {
    v / (kappa() * tick_s())
}
/// DERIVED: beyond x = 20 every kernel of the form (quadratic in x) e^-x that these laws produce
/// keeps below 1e-6 of its L1 norm, so this much history bounds what reaches a measured frame.
fn memory_s(v: f64) -> f64 {
    20.0 / expected_bandwidth(v)
}
/// The least x beyond which `decay(x)` stays below `level`, for a decay that falls monotonically
/// past its peak at x >= 0.
fn decayed_by(level: f64, decay: impl Fn(f64) -> f64) -> f64 {
    let (mut low, mut high) = (0.0, 100.0);
    for _ in 0..200 {
        let mid = (low + high) / 2.0;
        if decay(mid) < level {
            high = mid
        } else {
            low = mid
        }
    }
    high
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
/// Fixture: regular production silent for `pause_s` from `at_s`, then the admitted debt of
/// `admission_s` spent one tick per millisecond, then regular again; the rest is dropped.
fn stall(at_s: f64, pause_s: f64, admission_s: f64) -> impl Fn(u32) -> f64 {
    let first = (at_s / tick_s()).ceil() as u32;
    let debt = (pause_s.min(admission_s) / tick_s()).floor() as u32;
    let compute_s = 0.001;
    move |k| {
        if k < first {
            regular(k)
        } else if k < first + debt {
            at_s + pause_s + f64::from(k - first) * compute_s
        } else {
            at_s + pause_s
                + f64::from(debt - 1) * compute_s
                + f64::from(k + 1 - first - debt) * tick_s()
        }
    }
}

struct Frame {
    at_s: f64,
    dt_s: f64,
    /// Cursor advance over the frame, in simulation seconds.
    advance_s: f64,
    /// Cursor minus its inverse-map target, in simulation seconds.
    lead_s: f64,
    /// The cut gate's backlog: the target minus the previous cursor, in simulation seconds.
    backlog_s: f64,
    applied_s: f64,
    delay_target_s: f64,
    held: bool,
    cut: bool,
}

impl Frame {
    /// Simulation seconds per Real second over the frame.
    fn rate(&self) -> f64 {
        self.advance_s / self.dt_s
    }
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

    fn run(
        &mut self,
        until_s: f64,
        start: &impl Fn(u32) -> f64,
        complete: &impl Fn(u32, f64) -> f64,
    ) -> Vec<Frame> {
        self.run_frames(until_s, start, complete, &|_| FRAME)
    }

    /// Client frames of length `frame(i)` until `until_s`. Each frame sends every fixture tick
    /// started by its end and delivers those completed by its end; `start` gives a tick's
    /// frame-start stamp and `complete` its client completion time. Frames before a view epoch
    /// are not recorded.
    fn run_frames(
        &mut self,
        until_s: f64,
        start: &impl Fn(u32) -> f64,
        complete: &impl Fn(u32, f64) -> f64,
        frame: &impl Fn(usize) -> Duration,
    ) -> Vec<Frame> {
        let mut frames = Vec::new();
        let mut i = 0;
        while self.now_s() < until_s {
            let length = frame(i);
            i += 1;
            self.app
                .insert_resource(TimeUpdateStrategy::ManualDuration(length));
            let due = self.now_s() + length.as_secs_f64();
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
            assert!(now >= before, "presentation never rewinds");
            assert!(
                now <= TickInstant::from(clock.newest().unwrap()),
                "presentation never passes completed facts"
            );
            let applied_s = clock.delta_s.unwrap();
            let target = clock.inverse(at_s - applied_s).unwrap();
            frames.push(Frame {
                at_s,
                dt_s: length.as_secs_f64(),
                advance_s: (number(now) - number(before)) * tick_s(),
                lead_s: (number(now) - target) * tick_s(),
                backlog_s: (target - number(before)) * tick_s(),
                applied_s,
                delay_target_s: clock.target(&mut clock.intervals()).unwrap(),
                held: clock.held,
                cut: clock.cuts > cuts,
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

fn hold_episodes(frames: &[Frame]) -> usize {
    frames
        .windows(2)
        .filter(|pair| pair[1].held && !pair[0].held)
        .count()
        + usize::from(frames.first().is_some_and(|f| f.held))
}

fn summary(frames: &[Frame], v: f64) -> String {
    let mut rates: Vec<f64> = frames.iter().map(Frame::rate).collect();
    rates.sort_by(f64::total_cmp);
    let n = rates.len() as f64;
    let rank = |p: f64| rates[((p * n).ceil() as usize).clamp(1, rates.len()) - 1];
    let share = |f: &dyn Fn(f64) -> bool| rates.iter().filter(|&&r| f(r)).count() as f64 / n;
    format!(
        "n={} min={:.4} p1={:.4} p5={:.4} p50={:.4} p95={:.4} p99={:.4} max={:.4} at_1+v={:.4} outside_1pct={:.4} held_frames={} episodes={}",
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
        hold_episodes(frames),
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

/// R1. Remote hull motion relies on the cursor playing irregular production at a regular rate.
/// DERIVED: the rate's response to target phase has L1 gain kappa omega = v / T, so its deviation
/// is at most v / T times the target's peak-to-peak phase, the production spread plus the
/// applied delay's range. Following the stamp map segment by segment leaves this band.
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
    let band = v * (SPREAD_TICKS + applied / tick_s()) + quantization(frame_s());
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
            (frame.rate() - 1.0).abs() <= band,
            "rate {} outside the pacing bound 1 +/- {band}",
            frame.rate()
        );
    }
}

/// Observations only, for the estimator-coverage section of the pacing report; `Stream::run`
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

/// R2. A slow server shows as smooth slow motion (#328); hull and event consumers rely on a
/// regular stream at any speed the cursor can follow playing without stops. DERIVED: the map's
/// slope at the target point is the speed, so the stream rate learns it exactly and the loop
/// about it has no steady lead. What remains is the applied delay's motion, a target phase of
/// at most the speed times its range: the cursor stays within that phase, and the rate within
/// v / T times it.
#[test]
fn a_regular_server_at_any_followable_speed_plays_at_its_speed_without_holds_or_lead() {
    let timing = Timing::shipped();
    let v = timing.view_rate;
    let settle_s = timing.view_window_s * 2.0;
    // Fixture: server speeds from half real time to halfway into the slew limit's headroom.
    for speed in [0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.0 + v / 2.0] {
        let start = at_speed(speed);
        let mut stream = Stream::new();
        stream.run(settle_s, &start, &constant_link);
        let mut frames = stream.run(settle_s + memory_s(v), &start, &constant_link);
        let measured = frames.len();
        frames.extend(stream.run(settle_s + memory_s(v) + MEASURE_S, &start, &constant_link));
        let phase_s = speed * range(&frames, |f| f.applied_s);
        let lead_bound = phase_s + quantum_s();
        let rate_bound = v * phase_s / tick_s() + quantization(frame_s());
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
                (frame.rate() - speed).abs() <= rate_bound,
                "at {speed} the rate {} leaves the stream's rate",
                frame.rate()
            );
        }
    }
}

/// R3. Hull consumers rely on playback locking onto a server's new speed without stops once the
/// delay covers the new cadence. DERIVED, for a step from 1 to s:
/// - The step in the map's slope reaches the cursor through the type-2 loop as a lead of
///   |delta| t (1 + omega t) e^(-omega t), at most 0.840 |delta| / omega.
/// - The nearest-rank median interval turns to T / s once new intervals outnumber old ones by
///   two: (W + 3 T) / (1 + s) after the last old tick's receipt, a link time, a tick and a frame
///   after the switch. The delay target then rises by T / s - T, plus at most a frame from the
///   lateness maximum, and slews at v.
/// - A rising delay slows the target by s v, which the loop about the unchanged stream rate
///   follows leading by at most 2 s v / omega; after the slew that lead decays within
///   (2 s v / omega) (1 + x) e^-x. Under a cursor quantum the cursor cannot reach a tick its
///   target has not.
///
/// Before that, the old median under-covers the slower cadence and the cursor holds; a speed-up
/// over-covers, so it never holds.
#[test]
fn a_server_speed_change_locks_in_once_the_delay_covers_the_new_cadence() {
    let timing = Timing::shipped();
    let v = timing.view_rate;
    let omega = expected_bandwidth(v);
    let w = timing.view_window_s;
    let switch_s = w * 2.0;
    let golden = (1.0 + 5.0_f64.sqrt()) / 2.0;
    let peak = golden * (1.0 + golden) * (-golden).exp();
    // Fixture: slowdowns to half, three quarters and nine tenths of real time, and a recovery.
    for (from, to) in [(1.0, 0.5), (1.0, 0.75), (1.0, 0.9), (0.5, 1.0)] {
        let delta = to - from;
        let start = speed_step(from, to, switch_s);
        let lead_after_slew = |x: f64| 2.0 * to * v / omega * (1.0 + x) * (-x).exp();
        let covered_s = switch_s
            + LINK_S
            + tick_s() / from
            + frame_s()
            + (w + 3.0 * tick_s()) / (1.0 + to)
            + (tick_s() / to - tick_s() + frame_s()).max(0.0) / v
            + decayed_by(quantum_s(), lead_after_slew) / omega;
        let until_s = covered_s.max(switch_s + w) + memory_s(v);
        let mut stream = Stream::new();
        stream.run(switch_s, &start, &constant_link);
        let frames = stream.run(until_s, &start, &constant_link);
        assert!(frames.last().unwrap().at_s >= until_s - frame_s());
        let lead_bound = peak * delta.abs() / omega + quantum_s();
        let (lead_at, lead) = frames
            .iter()
            .map(|f| (f.at_s, f.lead_s))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        let (lag_at, lag) = frames
            .iter()
            .map(|f| (f.at_s, -f.lead_s))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        let first_move = frames
            .iter()
            .find(|f| (f.applied_s - frames[0].applied_s).abs() > 1e-12)
            .map(|f| f.at_s - switch_s);
        println!(
            "MEASURED speed {from} to {to}: held_frames={} episodes={} last_hold_after_s={:?} lead_max_ms={:.3} at_s={:.3} lag_max_ms={:.3} at_s={:.3} rate=[{:.4},{:.4}] delay_first_moves_after_s={first_move:?} DERIVED covered_after_s={:.3} lead_bound_ms={:.3}",
            frames.iter().filter(|f| f.held).count(),
            hold_episodes(&frames),
            frames
                .iter()
                .rev()
                .find(|f| f.held)
                .map(|f| f.at_s - switch_s),
            lead * 1e3,
            lead_at - switch_s,
            lag * 1e3,
            lag_at - switch_s,
            frames.iter().map(Frame::rate).fold(f64::MAX, f64::min),
            frames.iter().map(Frame::rate).fold(f64::MIN, f64::max),
            covered_s - switch_s,
            lead_bound * 1e3
        );
        assert!(
            lead <= lead_bound,
            "the step's lead stays within its derived peak"
        );
        for frame in &frames {
            assert!(!frame.cut, "a speed change is paced, not cut");
            if delta > 0.0 || frame.at_s >= covered_s {
                assert!(
                    !frame.held,
                    "at {to} the cursor holds at {} s, after the delay covers the cadence",
                    frame.at_s - switch_s
                );
            }
        }
    }
}

/// R3. A server stall changes no rate the cursor plays once it has caught up, neither while the
/// stall's samples stay in the statistics window nor when they leave it W later: no window takes
/// part in the rate. DERIVED: the admitted debt is a backlog of at most `admission_s`, which the
/// cursor spends at the slew limit v, and the loop and the stream rate settle within their
/// memory. Afterwards only the applied delay's motion reaches the rate, at most v / T times the
/// phase it moves.
#[test]
fn a_server_stall_leaves_the_rate_unchanged_across_the_statistics_window() {
    let timing = Timing::shipped();
    let v = timing.view_rate;
    let w = timing.view_window_s;
    // Fixture: a 1 s server silence at two windows; the server admits its capped debt.
    let (stall_s, pause_s) = (w * 2.0, 1.0);
    let start = stall(stall_s, pause_s, timing.admission_s);
    let caught_s = stall_s + pause_s + LINK_S + timing.admission_s / v + memory_s(v);
    let until_s = stall_s + pause_s + LINK_S + w + memory_s(v);
    let mut stream = Stream::new();
    stream.run(stall_s, &start, &constant_link);
    let frames = stream.run(until_s, &start, &constant_link);
    assert!(frames.last().unwrap().at_s >= until_s - frame_s());
    let remembered = frames
        .iter()
        .position(|f| f.at_s >= caught_s - memory_s(v))
        .unwrap();
    let phase_s = range(&frames[remembered..], |f| f.applied_s);
    let limit = v * phase_s / tick_s() + quantization(frame_s());
    let worst = frames
        .iter()
        .filter(|f| f.at_s >= caught_s)
        .map(|f| (f.rate() - 1.0).abs())
        .fold(0.0, f64::max);
    println!(
        "MEASURED 1 s capped stall: held_frames={} episodes={} cuts={} peak_backlog_ms={:.3} settled_rate_deviation={worst:.6} applied_range_ms={:.4} DERIVED caught_after_s={:.3} limit={limit:.6}",
        frames.iter().filter(|f| f.held).count(),
        hold_episodes(&frames),
        frames.iter().filter(|f| f.cut).count(),
        frames.iter().map(|f| f.backlog_s).fold(f64::MIN, f64::max) * 1e3,
        phase_s * 1e3,
        caught_s - stall_s,
    );
    for frame in &frames {
        assert!(frame.rate() <= 1.0 + v + quantization(frame_s()) || frame.cut);
    }
    assert!(
        worst <= limit,
        "the stall's samples moved the settled rate by {worst}"
    );
}

/// R3. Playback acceleration stays within what the derived law allows, so a server speed change
/// or a delay step never becomes a rate jump. DERIVED: the rate's response to a step delta in the
/// map's slope accelerates by delta omega (3 x - x^2) e^-x, at most (sqrt 13 - 2)
/// e^(-(5 - sqrt 13) / 2) |delta| omega = 0.80 |delta| omega; a delay slewing at v slows the target
/// by s v, which the loop alone answers with s v omega^2 t e^(-omega t), at most v omega / e for
/// s <= 1. Each frame's rate is the mean over the frame, so consecutive frames differ by at most
/// that times F, plus both frames' quantization. A rate state kept relative to the learned rate
/// steps playback by each of its updates.
#[test]
fn playback_acceleration_stays_within_the_derived_law() {
    let timing = Timing::shipped();
    let v = timing.view_rate;
    let omega = expected_bandwidth(v);
    let root = 13.0_f64.sqrt();
    let step_peak = (root - 2.0) * (-(5.0 - root) / 2.0).exp();
    let worst = |frames: &[Frame]| {
        frames
            .windows(2)
            .map(|pair| (pair[1].rate() - pair[0].rate()).abs())
            .fold(0.0, f64::max)
    };
    // Fixture: a recovery from half speed. Late ticks before the switch hold the lateness
    // maximum, so the delay stays still until the median turns over.
    let switch_s = timing.view_window_s * 2.0;
    let (from, to) = (0.5, 1.0);
    let first = (switch_s * from / tick_s()).ceil() as u32;
    let start = speed_step(from, to, switch_s);
    let complete = move |k: u32, s: f64| {
        s + LINK_S
            + if k < first && late(k) {
                2.0 * tick_s()
            } else {
                0.0
            }
    };
    let mut stream = Stream::new();
    stream.run(switch_s, &start, &complete);
    let frames = stream.run(switch_s + memory_s(v) * 4.0, &start, &complete);
    let still = frames
        .iter()
        .position(|f| (f.applied_s - frames[0].applied_s).abs() > 1e-12)
        .unwrap_or(frames.len());
    assert!(
        still > 0 && frames[still - 1].at_s >= switch_s + memory_s(v),
        "the delay stays still through the response"
    );
    let speed_limit = step_peak * (to - from) * omega * frame_s() + 2.0 * quantization(frame_s());
    let speed_worst = worst(&frames[..still]);
    assert!(frames[..still].iter().all(|f| !f.held && !f.cut));
    // Fixture: a link 100 ms slower for two windows; expiry of its lateness steps the target.
    let (slow_from_s, slow_s) = (timing.view_window_s * 2.0, 0.100);
    let complete = move |_: u32, s: f64| s + LINK_S + if s < slow_from_s { slow_s } else { 0.0 };
    let mut stream = Stream::new();
    stream.run(slow_from_s, &regular, &complete);
    let until_s = slow_from_s + timing.view_window_s + slow_s + slow_s / v + memory_s(v);
    let slewed = stream.run(until_s, &regular, &complete);
    assert!(
        slewed
            .iter()
            .any(|f| f.delay_target_s < slewed[0].delay_target_s - slow_s / 2.0)
    );
    let delay_limit = v * omega / std::f64::consts::E * frame_s() + 2.0 * quantization(frame_s());
    let delay_worst = worst(&slewed);
    assert!(slewed.iter().all(|f| !f.held && !f.cut));
    println!(
        "MEASURED speed {from} to {to}: worst_rate_change_per_frame={speed_worst:.5} DERIVED limit={speed_limit:.5}; delay step: worst={delay_worst:.5} rate=[{:.4},{:.4}] lag_max_ms={:.3} DERIVED limit={delay_limit:.5} slew_lag_ms={:.3}",
        slewed.iter().map(Frame::rate).fold(f64::MAX, f64::min),
        slewed.iter().map(Frame::rate).fold(f64::MIN, f64::max),
        -slewed.iter().map(|f| f.lead_s).fold(f64::MAX, f64::min) * 1e3,
        2.0 * v / omega * 1e3,
    );
    assert!(
        speed_worst <= speed_limit,
        "a speed change never becomes a rate jump"
    );
    assert!(
        delay_worst <= delay_limit,
        "a delay step never becomes a rate jump"
    );
}

/// R4. The frontier's hold diagnostics rely on `held` naming exactly the frames whose
/// rate-limited request passes the newest completed tick. Forced reachable states:
/// - a cut lands half a tick below the newest tick and a falling delay carries the target onto
///   it within one frame whose whole request completes: not held, though the target clamps;
/// - silence: every frame requests past the newest tick;
/// - a rising delay slows the target, and the cursor reaches the newest tick ahead of it: held,
///   though the target has not reached the newest stamp.
#[test]
fn hold_flags_exactly_requests_past_the_newest_tick() {
    let timing = Timing::shipped();
    let (t, w) = (tick_s(), timing.view_window_s);
    let advance = |clock: &mut ViewClock, now_s: f64, dt_s: f64| {
        clock.advance(now_s, dt_s, t, &timing, timing.admission_s)
    };
    // Fixture: binary multiples of T place the target exactly. Ticks complete with lateness
    // 2.5 T; tick 1, when `first_x` is given, sets a higher maximum and expires after frame 1.
    let cut_below_newest = |first_x: f64| {
        let mut clock = ViewClock::default();
        let (now0, now1) = (2.0 * t + t + first_x, 39.5 * t + t + first_x);
        clock.sample(Tick(1), stamp(t), now1 - w, t + first_x);
        clock.sample(Tick(2), stamp(2.0 * t), now0, 2.0 * t + 2.5 * t);
        advance(&mut clock, now0, t);
        for tick in (3..=40).filter(|&tick| tick != 35) {
            let s = f64::from(tick) * t;
            clock.sample(Tick(tick), stamp(s), now1, s + 2.5 * t);
        }
        let cut = advance(&mut clock, now1, now1 - now0).expect("the backlog exceeds the budget");
        assert_eq!(number(cut.to), 39.5);
        assert!(!clock.held);
        (clock, now1)
    };
    // Falling delay: tick 1's lateness 3 T expires, so the delay falls by v dt to 3.9 T and the
    // target point reaches 40.1 T; the whole half-tick request completes onto tick 40.
    let (mut clock, now1) = cut_below_newest(3.0 * t);
    let now2 = now1 + 0.5 * t;
    clock.expire(now2, w);
    advance(&mut clock, now2, 0.5 * t);
    assert!(
        now2 - clock.delta_s.unwrap() > 40.0 * t,
        "the target clamps to the newest tick"
    );
    assert_eq!(clock.now(), Some(Tick(40).into()));
    assert!(
        !clock.held,
        "a completed request onto a clamped target is not held"
    );
    // Silence: no completion through more than a window of display frames.
    let mut now = now2;
    while now < now2 + w * 1.5 {
        now += frame_s();
        clock.expire(now, w);
        advance(&mut clock, now, frame_s());
        assert!(clock.held, "a silent stream holds every frame");
        assert_eq!(clock.now(), Some(Tick(40).into()));
    }
    // Rising delay: tick 35 completes late, raising the target by T, while 0.6 T passes.
    let (mut clock, now1) = cut_below_newest(2.5 * t);
    clock.sample(Tick(35), stamp(35.0 * t), now1, 35.0 * t + 3.5 * t);
    let now2 = now1 + 0.6 * t;
    advance(&mut clock, now2, 0.6 * t);
    assert!(
        clock.inverse(now2 - clock.delta_s.unwrap()).unwrap() < 40.0,
        "the target is behind the newest tick"
    );
    assert_eq!(clock.now(), Some(Tick(40).into()));
    assert!(clock.held, "a request past the newest tick is held");
}

/// R4. A hold is a gap in facts, not a change in the stream's speed, so the cursor resumes at
/// the stream's rate and approaches its target without overshoot; hull consumers rely on one
/// hold per stall and a catch-up at the slew limit. DERIVED:
/// - From rate deviation 0 and lead z0 <= 0 the loop's deviation is -omega^2 z0 t e^(-omega t),
///   so the first frame's rate lies in [q, q + omega^2 |z0| F / 2], and the lead
///   z0 (1 + omega t) e^(-omega t) never turns positive. Release from rest dips below q; release
///   at the fastest admissible approach, q - omega z0, exceeds the upper end.
/// - A backlog B saturates the rate at 1 + v until the loop's demand falls, at a lead of 2 v /
///   omega; from there the lead -(v / omega)(2 + x) e^-x closes. With a frame each for the ramp
///   onto the cap and the measurement, the cursor is within 1 ms of its target by B / v plus the
///   exit's decay time.
#[test]
fn a_released_hold_resumes_at_the_stream_rate_and_closes_its_backlog_at_the_slew_limit() {
    let timing = Timing::shipped();
    let (t, v, w) = (tick_s(), timing.view_rate, timing.view_window_s);
    let omega = expected_bandwidth(v);
    // Fixture: a regular stream completing with lateness 2.5 T stops completing after tick 8;
    // ticks 9 to 12 then complete at once, on the stream's own schedule.
    let mut clock = ViewClock::default();
    for tick in 1..=8 {
        let s = f64::from(tick) * t;
        clock.sample(Tick(tick), stamp(s), s + 2.5 * t, s + 2.5 * t);
        clock.advance(s + 2.5 * t, t, t, &timing, timing.admission_s);
    }
    let mut now = 10.5 * t;
    while !clock.held {
        now += frame_s();
        clock.advance(now, frame_s(), t, &timing, timing.admission_s);
    }
    for _ in 0..3 {
        now += frame_s();
        clock.advance(now, frame_s(), t, &timing, timing.admission_s);
    }
    for tick in 9..=12 {
        let s = f64::from(tick) * t;
        clock.sample(Tick(tick), stamp(s), now, s + 2.5 * t);
    }
    now += frame_s();
    let before = number(clock.now().unwrap());
    clock.advance(now, frame_s(), t, &timing, timing.admission_s);
    let target = clock.inverse(now - clock.delta_s.unwrap()).unwrap();
    let stream_rate = 1.0; // Fixture: the stream's schedule is regular at real time.
    let z0 = (before - target) * t + stream_rate * frame_s();
    let rate = (number(clock.now().unwrap()) - before) * t / frame_s();
    let ceiling = stream_rate + omega * omega * -z0 * frame_s() / 2.0 + quantization(frame_s());
    println!(
        "MEASURED release: z0_ms={:.3} first_rate={rate:.5} DERIVED [{stream_rate}, {ceiling:.5}]",
        z0 * 1e3
    );
    assert!(z0 < 0.0 && !clock.held);
    assert!(
        rate >= stream_rate - quantization(frame_s()) && rate <= ceiling,
        "the released cursor resumes at the stream's rate: {rate}"
    );

    // Fixture: regular production, a server silence of 0.8 of the continuity floor (200 ms as
    // shipped; the rest is margin for the release's ramp), then the admitted debt spent one tick
    // per 1 ms server frame.
    let floor_s = timing.view_cut_s.max(timing.admission_s);
    let (stall_s, pause_s) = (w * 2.0, 0.8 * floor_s);
    let start = stall(stall_s, pause_s, timing.admission_s);
    let mut stream = Stream::new();
    stream.run(stall_s, &start, &constant_link);
    let exit = decayed_by(1e-3, |x| v / omega * (2.0 + x) * (-x).exp()) / omega;
    let until_s = stall_s + pause_s + LINK_S + timing.admission_s / v + exit + memory_s(v);
    let frames = stream.run(until_s, &start, &constant_link);
    assert_eq!(
        hold_episodes(&frames),
        1,
        "the silence holds the cursor once"
    );
    assert!(
        frames.iter().all(|f| !f.cut),
        "a backlog inside the continuity floor is paced, not cut"
    );
    let (peak, backlog_s) = frames
        .iter()
        .enumerate()
        .map(|(i, f)| (i, -f.lead_s))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    let deadline_s = frames[peak].at_s + backlog_s / v + exit + 2.0 * frame_s();
    assert!(frames.last().unwrap().at_s >= deadline_s);
    let caught = frames[peak..]
        .iter()
        .find(|f| f.lead_s.abs() <= 1e-3)
        .expect("the backlog closes");
    println!(
        "MEASURED released {:.0} ms stall: held_ms={:.3} episodes={} backlog_ms={:.3} gate_backlog_ms={:.3} lead_max_ms={:.4} within_1ms_after_peak_ms={:.1} DERIVED deadline_ms={:.1}",
        pause_s * 1e3,
        frames.iter().filter(|f| f.held).count() as f64 * frame_s() * 1e3,
        hold_episodes(&frames),
        backlog_s * 1e3,
        frames.iter().map(|f| f.backlog_s).fold(f64::MIN, f64::max) * 1e3,
        frames.iter().map(|f| f.lead_s).fold(f64::MIN, f64::max) * 1e3,
        (caught.at_s - frames[peak].at_s) * 1e3,
        (deadline_s - frames[peak].at_s) * 1e3
    );
    for frame in &frames {
        assert!(
            frame.lead_s <= quantum_s(),
            "the release never overshoots: {} ms",
            frame.lead_s * 1e3
        );
        assert!(
            frame.rate() <= 1.0 + v + quantization(frame_s()),
            "catch-up respects the slew limit"
        );
        if frame.at_s >= deadline_s {
            assert!(
                frame.lead_s.abs() <= 1e-3,
                "the cursor is {} ms from its target after the derived catch-up time",
                frame.lead_s * 1e3
            );
        }
    }
}

/// Every presentation consumer relies on these at any client frame length, including none, a
/// hitch and a quarter-second stall: the cursor never rewinds (`Stream::run`), never passes the
/// newest completed tick (`Stream::run`), and advances at most (1 + v) of each frame's length
/// except at a cut.
#[test]
fn any_frame_length_keeps_the_cursor_forward_behind_facts_and_within_the_slew_limit() {
    let timing = Timing::shipped();
    let v = timing.view_rate;
    let start = |k: u32| if late(k) { regular(k) } else { jittered(k) };
    let settle_s = timing.view_window_s * 2.0;
    // Fixture: frame lengths none, 2 to 60 ms and 250 ms, in a fixed irregular order.
    let lengths_ms = [
        0, 2, 37, 250, 0, 9, 60, 3, 16, 250, 44, 0, 23, 5, 31, 60, 2, 11,
    ];
    let frame = |i: usize| Duration::from_millis(lengths_ms[i % lengths_ms.len()]);
    let mut stream = Stream::new();
    stream.run(settle_s, &start, &slack_link);
    let frames = stream.run_frames(settle_s + MEASURE_S, &start, &slack_link, &frame);
    assert!(frames.iter().any(|f| f.dt_s == 0.0) && frames.iter().any(|f| f.dt_s >= 0.25));
    println!(
        "MEASURED irregular frame lengths: frames={} held={} cuts={}",
        frames.len(),
        frames.iter().filter(|f| f.held).count(),
        frames.iter().filter(|f| f.cut).count()
    );
    for frame in frames.iter().filter(|f| !f.cut) {
        assert!(
            frame.advance_s <= (1.0 + v) * frame.dt_s + 2.0 * quantum_s(),
            "a {} ms frame advanced {} ms",
            frame.dt_s * 1e3,
            frame.advance_s * 1e3
        );
    }
}
