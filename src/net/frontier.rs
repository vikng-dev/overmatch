//! Per-second presentation diagnostics from completed stamps and the application-owned cursor.
//! `holds` counts runs of frames whose rate-limited request passed the newest completed tick
//! (`ViewClock::held`), whether or not the target had reached it; nothing extrapolates. The
//! offset-bearing raw deltas stay inside `view_clock`; only their differences are durations.

use super::view_clock::ViewClock;
use bevy::prelude::*;
use lightyear::core::tick::{Tick, TickDuration};
use lightyear::prelude::Connected;
use serde_json::{Value, json};

pub(super) fn install(app: &mut App) {
    app.init_resource::<JitterRecord>()
        .add_observer(|_: On<Add<Connected>>, mut record: ResMut<JitterRecord>| *record = default())
        .add_observer(
            |_: On<Remove<Connected>>, mut record: ResMut<JitterRecord>| *record = default(),
        )
        .add_systems(PostUpdate, record_jitter);
}

#[derive(Resource, Default)]
struct JitterRecord {
    stream: Option<Stream>,
    arrivals_ms: Vec<f64>,
    headroom_ticks: Vec<f64>,
}
struct Stream {
    arrivals: Arrivals,
    holds: Holds,
    opened_s: f64,
    samples: u64,
    frontier: Tick,
}

/// Resolution: one line per wall second resolves per-second counts without flooding the log, and
/// matches the hosted server's stats period (`scripts/edgegap/session.sh`, `--stats-s` default).
const LINE_PERIOD_S: f64 = 1.0;

/// Received ticks as intervals on the completion frame's Real sample.
struct Arrivals {
    /// Completed stamped ticks as of the previous frame.
    samples: u64,
    /// The drain-frame stamp of the newest received tick, or of the stream's opening frame
    /// before any, `Time<Real>` seconds.
    last_s: f64,
}

impl Arrivals {
    /// Push this frame's intervals: the wait since the last receipt, then 0 for each further tick
    /// the same frame drained.
    fn frame(&mut self, now_s: f64, samples: u64, intervals_ms: &mut Vec<f64>) {
        let received = samples - self.samples;
        self.samples = samples;
        if received == 0 {
            return;
        }
        intervals_ms.push((now_s - self.last_s) * 1e3);
        intervals_ms.extend(std::iter::repeat_n(0.0, received as usize - 1));
        self.last_s = now_s;
    }
}

/// Runs of frames whose target exhausted the completed tick buffer.
#[derive(Default)]
struct Holds {
    /// The ongoing run's frame time; it carries across lines.
    run_ms: Option<f64>,
    events: u64,
    ms: f64,
    longest_ms: f64,
}

impl Holds {
    fn frame(&mut self, held: bool, frame_ms: f64) {
        if !held {
            self.run_ms = None;
            return;
        }
        let run = self.run_ms.get_or_insert_with(|| {
            self.events += 1;
            0.0
        });
        *run += frame_ms;
        self.ms += frame_ms;
        self.longest_ms = self.longest_ms.max(*run);
    }

    /// The period's reading; the next period opens with only the ongoing run, whose whole length
    /// counts toward that period's longest once it holds a frame there.
    fn take_line(&mut self) -> Value {
        let line = json!({
            "events": self.events,
            "ms": rounded(self.ms, MS_PLACES),
            "longest_ms": rounded(self.longest_ms, MS_PLACES),
        });
        *self = Self {
            run_ms: self.run_ms,
            ..Self::default()
        };
        line
    }
}

/// Resolution: a tenth of a millisecond and a hundredth of a tick (DERIVED 0.16 ms at 64 Hz),
/// below anything the line's readers compare; the line is read by eye as well as parsed.
const MS_PLACES: f64 = 10.0;
const TICK_PLACES: f64 = 100.0;

fn rounded(value: f64, places: f64) -> f64 {
    (value * places).round() / places
}

/// Nearest-rank `min`, `p50`, `p95`, `max`, each a measured sample; null when there is none.
/// Sorts in place.
fn summary(values: &mut [f64], places: f64) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    values.sort_by(f64::total_cmp);
    let rank = |p: f64| {
        let rank = (p * values.len() as f64).ceil() as usize;
        rounded(values[rank.clamp(1, values.len()) - 1], places)
    };
    json!({
        "min": rank(0.0),
        "p50": rank(0.5),
        "p95": rank(0.95),
        "max": rank(1.0),
    })
}

fn record_jitter(
    real: Res<Time<Real>>,
    tick: Res<TickDuration>,
    mut record: ResMut<JitterRecord>,
    mut clock: ResMut<ViewClock>,
) {
    let (Some(cursor), Some(frontier)) = (clock.now(), clock.newest()) else {
        return;
    }; // Waiting for a complete stamp pair.
    let now = real.elapsed_secs_f64();
    let samples = clock.received;
    let record = &mut *record;
    let stream = record.stream.get_or_insert_with(|| Stream {
        arrivals: Arrivals {
            samples,
            last_s: now,
        },
        holds: Holds::default(),
        opened_s: now,
        samples,
        frontier,
    });
    stream.arrivals.frame(now, samples, &mut record.arrivals_ms);
    record
        .headroom_ticks
        .push(f64::from(frontier - cursor.tick()) - f64::from(cursor.overstep().to_f32()));
    stream.holds.frame(clock.held, real.delta_secs_f64() * 1e3);
    let period_s = now - stream.opened_s;
    if period_s < LINE_PERIOD_S {
        return;
    }
    let line = json!({
        "period_s": period_s,
        "ticks": { "received": samples - stream.samples, "frontier": frontier - stream.frontier },
        "arrival_ms": summary(&mut record.arrivals_ms, MS_PLACES),
        "headroom_ticks": summary(&mut record.headroom_ticks, TICK_PLACES),
        "holds": stream.holds.take_line(),
        "view_clock": clock.take_line(now, tick.0.as_secs_f64(), cursor),
    });
    info!("client_jitter {line}");
    record.arrivals_ms.clear();
    record.headroom_ticks.clear();
    stream.opened_s = now;
    stream.samples = samples;
    stream.frontier = frontier;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Readers compare the line's distributions against measured samples, so every percentile
    /// must be one (nearest rank) with the extremes at the ends. An interpolating or off-by-one
    /// rank reports a value no frame had.
    #[test]
    fn a_summary_reports_measured_samples_at_nearest_rank() {
        // Fixture: 1..=100 out of order; nearest rank of p over n = 100 is the value 100·p.
        let mut values: Vec<f64> = (1..=100).rev().map(f64::from).collect();
        assert_eq!(
            summary(&mut values, MS_PLACES),
            json!({"min": 1.0, "p50": 50.0, "p95": 95.0, "max": 100.0})
        );
        assert_eq!(
            summary(&mut [7.0], MS_PLACES),
            json!({"min": 7.0, "p50": 7.0, "p95": 7.0, "max": 7.0})
        );
        assert_eq!(summary(&mut [], MS_PLACES), Value::Null);
    }

    /// A holds event is a run of held frames, counted once in the line where it begins;
    /// a run that spans lines keeps growing its length, and a line with no held frame reports
    /// none. Counting frames as events, restarting the run at each line, or carrying a run that
    /// ended at the boundary into a healthy line would misreport the jitter a fix is judged on.
    #[test]
    fn a_held_run_is_one_event_across_lines() {
        let mut holds = Holds::default();
        // Fixture: a 10 ms frame cadence; a two-frame run, a break, then a run across the line.
        for held in [true, true, false, true] {
            holds.frame(held, 10.0);
        }
        assert_eq!(
            holds.take_line(),
            json!({"events": 2, "ms": 30.0, "longest_ms": 20.0})
        );
        for held in [true, true, false] {
            holds.frame(held, 10.0);
        }
        assert_eq!(
            holds.take_line(),
            json!({"events": 0, "ms": 20.0, "longest_ms": 30.0})
        );
        // A run that reaches the boundary, then a healthy line.
        holds.frame(true, 10.0);
        assert_eq!(
            holds.take_line(),
            json!({"events": 1, "ms": 10.0, "longest_ms": 10.0})
        );
        holds.frame(false, 10.0);
        assert_eq!(
            holds.take_line(),
            json!({"events": 0, "ms": 0.0, "longest_ms": 0.0})
        );
    }

    /// Every received tick has an interval, the first after a stream opens included: a stall
    /// right after the opening is jitter like any other. Starting from no stamp drops it; ticks
    /// one frame drains after the first waited nothing more.
    #[test]
    fn a_streams_first_receipt_measures_from_its_opening() {
        // Fixture: a stream opened at 1.0 s with 10 samples, then 200 ms without a receipt.
        let mut arrivals = Arrivals {
            samples: 10,
            last_s: 1.0,
        };
        let mut intervals = Vec::new();
        arrivals.frame(1.1, 10, &mut intervals);
        assert!(intervals.is_empty(), "a frame without a receipt adds none");
        arrivals.frame(1.2, 12, &mut intervals);
        arrivals.frame(1.25, 13, &mut intervals);
        let ms: Vec<f64> = intervals.iter().map(|ms| rounded(*ms, MS_PLACES)).collect();
        assert_eq!(ms, [200.0, 0.0, 50.0]);
    }
}
