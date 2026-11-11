//! Read-only readings of the client's view against the server stream: interpolation-timeline
//! snaps after connection setup ([`install`]) and the `client_jitter` line
//! ([`install_jitter_line`]).
//!
//! # The `client_jitter` line
//!
//! While a server stream is established (a connected link whose arrival estimator holds a newest
//! remote tick, `net::sync_margin`, and whose remote clock estimate is initialized), one `info!`
//! line per wall second, `client_jitter {json}`:
//! - `period_s`: the wall time the line covers, which can exceed the period by one frame;
//! - `ticks`: `received`, new server ticks the estimator sampled; `frontier`, how far the newest
//!   received tick advanced, so `frontier − received` ticks never arrived; `estimate`, how far
//!   lightyear's estimate of the server clock advanced. All over `period_s`;
//! - `arrival_ms`: the interval before each received tick, on the drain-frame stamp the estimator
//!   uses, so ticks drained by one frame after the first read 0;
//! - `headroom_ticks`: per frame, the newest received tick − the cursor that frame presented;
//! - `starved`: frames whose presented cursor passed the newest received tick, so every replica
//!   held its newest sample: `events` (runs begun this period), `ms` (their frame time this
//!   period) and `longest_ms` (the longest run this period, a run spanning lines at its length so
//!   far);
//! - `snaps`: each steady-state resync's cursor jump, in signed ticks;
//! - `estimate_error_ticks`: per frame, the estimate − (newest received tick + rtt/2);
//! - `min_delay_ms`: at the line, the `min_delay` applied and the delay law's target from the
//!   current digest (`net::interp_delay`, which pursues it only once armed); `rtt_ms` beside it.
//!
//! Distributions are nearest-rank `min`, `p50`, `p95`, `max`, each a measured sample, and null
//! when the period holds none; `estimate_error_ticks` is `min` and `max`.

use core::time::Duration;

use bevy::prelude::*;
use lightyear::core::tick::{Tick, TickDuration};
use lightyear::core::time::TickInstant;
use lightyear::interpolation::timeline::{InterpolationConfig, InterpolationTimeline};
use lightyear::prelude::client::RemoteTimeline;
use lightyear::prelude::{Client, Connected, NetworkTimeline, PingManager, SyncSystems};
use lightyear_sync::timeline::sync::SyncTargetTimeline;
use serde_json::{Value, json};

use super::sync_margin::ArrivalDelay;

pub(super) fn install(app: &mut App) {
    app.init_resource::<FrontierDiag>();
    app.init_resource::<PreSyncCursor>();
    app.add_observer(allow_handshake_resync);
    app.add_systems(
        PostUpdate,
        (
            note_pre_sync_cursor.before(SyncSystems::Sync),
            count_interp_resyncs.after(SyncSystems::Sync),
        ),
    );
}

/// Mount the `client_jitter` line on [`install`]'s snap detector. It reads the arrival estimator
/// (`net::sync_margin`), so it mounts only in a composition that has one.
pub(super) fn install_jitter_line(app: &mut App) {
    app.init_resource::<JitterRecord>();
    app.add_systems(PostUpdate, record_jitter.after(count_interp_resyncs));
}

#[derive(Resource, Default)]
struct FrontierDiag {
    sync_events: u64,
    // Each connection owes one handshake snap; later snaps are faults.
    handshake_spent: bool,
    /// This frame's steady-state snap: the cursor's jump in signed ticks.
    snapped: Option<f32>,
}

/// A (re)connection resets the interpolation timeline, so its first resync is the handshake.
fn allow_handshake_resync(_connected: On<Add, Connected>, mut diag: ResMut<FrontierDiag>) {
    diag.handshake_spent = false;
}

/// The interpolation cursor as it stood entering this frame's timeline sync.
#[derive(Resource, Default)]
struct PreSyncCursor(Option<TickInstant>);

fn note_pre_sync_cursor(
    timeline: Option<Res<InterpolationTimeline>>,
    mut pre: ResMut<PreSyncCursor>,
) {
    pre.0 = timeline.map(|timeline| timeline.now());
}

/// Report interpolation-timeline resyncs after the handshake: every remote hull jumps.
///
/// Lightyear emits no event for this timeline's resync, so it is detected: the cursor advances
/// only in `PreUpdate` (`TimelineSystems::Advance`), and inside `SyncSystems::Sync` the speed
/// controller only rescales it — the one write to `now` there is `TimelineSync::resync`'s snap. So
/// `now` differing across the sync set IS a resync. (A snap onto the exact current instant is
/// invisible, and moves nothing to observe.)
fn count_interp_resyncs(
    timeline: Option<Res<InterpolationTimeline>>,
    pre: Res<PreSyncCursor>,
    mut diag: ResMut<FrontierDiag>,
) {
    diag.snapped = None;
    let (Some(timeline), Some(before)) = (timeline, pre.0) else {
        return;
    };
    if timeline.now() == before {
        return;
    }
    diag.sync_events += 1;
    if !diag.handshake_spent {
        diag.handshake_spent = true;
        return;
    }
    diag.snapped = Some((timeline.now() - before).to_f32());
    warn!(
        "net: FRONTIER interpolation resync #{} — a steady-state resync snapped every remote hull",
        diag.sync_events
    );
}

/// Resolution: one line per wall second resolves per-second counts without flooding the log, and
/// matches the hosted server's stats period (`scripts/edgegap/session.sh`, `--stats-s` default).
const LINE_PERIOD_S: f64 = 1.0;

/// The `client_jitter` line's state; `stream` is `None` while no server stream is established.
#[derive(Resource, Default)]
struct JitterRecord {
    stream: Option<Stream>,
    /// The open period's samples, emptied by each line and when a stream opens, and reused.
    arrivals_ms: Vec<f64>,
    headroom_ticks: Vec<f64>,
    snaps: Vec<f64>,
}

/// One established stream: what carries from line to line, and the open period.
struct Stream {
    /// The estimator's link ([`ArrivalDelay::link`]); a new link opens a new stream.
    link: u64,
    /// Estimator samples as of the previous frame.
    samples: u64,
    /// The drain-frame stamp of the newest received tick, `Time<Real>` seconds.
    last_arrival_s: Option<f64>,
    starvation: Starvation,
    period: Period,
}

/// The stream's readings when the open period began, and its per-frame extrema.
struct Period {
    opened_s: f64,
    samples: u64,
    frontier: Tick,
    estimate: TickInstant,
    estimate_error_ticks: Option<(f64, f64)>,
}

impl Period {
    fn open(opened_s: f64, samples: u64, frontier: Tick, estimate: TickInstant) -> Self {
        Self {
            opened_s,
            samples,
            frontier,
            estimate,
            estimate_error_ticks: None,
        }
    }
}

/// Runs of frames whose presented cursor passed the newest received tick.
#[derive(Default)]
struct Starvation {
    /// The ongoing run's frame time; it carries across lines.
    run_ms: Option<f64>,
    events: u64,
    ms: f64,
    longest_ms: f64,
}

impl Starvation {
    fn frame(&mut self, starved: bool, frame_ms: f64) {
        if !starved {
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

    /// The period's reading; the next period opens with only the ongoing run.
    fn take_line(&mut self) -> Value {
        let line = json!({
            "events": self.events,
            "ms": rounded(self.ms, MS_PLACES),
            "longest_ms": rounded(self.longest_ms, MS_PLACES),
        });
        *self = Self {
            run_ms: self.run_ms,
            longest_ms: self.run_ms.unwrap_or(0.0),
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

/// After the snap detector: fold this frame into the open period, and print the period's line once
/// it has run [`LINE_PERIOD_S`]. The estimator's digest is current: `refresh_arrival_stats` runs
/// before `SyncSystems::Sync`.
fn record_jitter(
    diag: Res<FrontierDiag>,
    pre: Res<PreSyncCursor>,
    estimator: Res<ArrivalDelay>,
    real: Res<Time<Real>>,
    (tick, config, links): (
        Res<TickDuration>,
        Res<InterpolationConfig>,
        Query<(&RemoteTimeline, &PingManager), (With<Client>, With<Connected>)>,
    ),
    mut record: ResMut<JitterRecord>,
) {
    let link = (!links.is_empty())
        .then(|| {
            links
                .single()
                .expect("the client has exactly one server link")
        })
        .filter(|(remote, _)| remote.is_initialized());
    // No stream: before connection and its handshake, or after the link ends.
    let (Some((remote, pings)), Some(frontier), Some(cursor)) =
        (link, estimator.newest_remote(), pre.0)
    else {
        record.stream = None;
        return;
    };
    let now = real.elapsed_secs_f64();
    let samples = estimator.stats.samples;
    let estimate = remote.current_estimate();
    let record = &mut *record;
    if record
        .stream
        .as_ref()
        .is_none_or(|stream| stream.link != estimator.link())
    {
        record.stream = None;
        record.arrivals_ms.clear();
        record.headroom_ticks.clear();
        record.snaps.clear();
    }
    let stream = record.stream.get_or_insert_with(|| Stream {
        link: estimator.link(),
        samples,
        last_arrival_s: None,
        starvation: Starvation::default(),
        period: Period::open(now, samples, frontier, estimate),
    });

    let received = samples - stream.samples;
    stream.samples = samples;
    if received > 0 {
        if let Some(last) = stream.last_arrival_s {
            record.arrivals_ms.push((now - last) * 1e3);
        }
        // The rest of this frame's ticks drained with the first.
        record
            .arrivals_ms
            .extend(std::iter::repeat_n(0.0, received as usize - 1));
        stream.last_arrival_s = Some(now);
    }
    let headroom = f64::from((TickInstant::from(frontier) - cursor).to_f32());
    record.headroom_ticks.push(headroom);
    stream
        .starvation
        .frame(headroom < 0.0, real.delta_secs_f64() * 1e3);
    let half_rtt_ticks = pings.rtt().as_secs_f64() / 2.0 / tick.0.as_secs_f64();
    let error = f64::from((estimate - TickInstant::from(frontier)).to_f32()) - half_rtt_ticks;
    let (low, high) = stream
        .period
        .estimate_error_ticks
        .get_or_insert((error, error));
    *low = low.min(error);
    *high = high.max(error);
    record.snaps.extend(diag.snapped.map(f64::from));

    let period_s = now - stream.period.opened_s;
    if period_s < LINE_PERIOD_S {
        return;
    }
    let period = &stream.period;
    let ms = |duration: Duration| rounded(duration.as_secs_f64() * 1e3, MS_PLACES);
    let target = super::interp_delay::derived_min_delay(pings.rtt(), &estimator.stats, tick.0);
    let line = json!({
        "period_s": period_s,
        "ticks": {
            "received": samples - period.samples,
            "frontier": frontier - period.frontier,
            "estimate": rounded(f64::from((estimate - period.estimate).to_f32()), TICK_PLACES),
        },
        "arrival_ms": summary(&mut record.arrivals_ms, MS_PLACES),
        "headroom_ticks": summary(&mut record.headroom_ticks, TICK_PLACES),
        "starved": stream.starvation.take_line(),
        "snaps": record.snaps.iter().map(|&jump| rounded(jump, TICK_PLACES)).collect::<Vec<_>>(),
        "estimate_error_ticks": period.estimate_error_ticks.map(|(low, high)| json!({
            "min": rounded(low, TICK_PLACES),
            "max": rounded(high, TICK_PLACES),
        })),
        "min_delay_ms": {"applied": ms(config.min_delay), "target": ms(target)},
        "rtt_ms": ms(pings.rtt()),
    });
    info!("client_jitter {line}");
    record.arrivals_ms.clear();
    record.headroom_ticks.clear();
    record.snaps.clear();
    stream.period = Period::open(now, samples, frontier, estimate);
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

    /// A starvation event is a run of starved frames, counted once in the line where it begins;
    /// a run that spans lines keeps growing its length. Counting frames as events, or restarting
    /// the run at each line, would misreport the jitter a fix is judged on.
    #[test]
    fn a_starved_run_is_one_event_across_lines() {
        let mut starvation = Starvation::default();
        // Fixture: a 10 ms frame cadence; a two-frame run, a break, then a run across the line.
        for starved in [true, true, false, true] {
            starvation.frame(starved, 10.0);
        }
        assert_eq!(
            starvation.take_line(),
            json!({"events": 2, "ms": 30.0, "longest_ms": 20.0})
        );
        for starved in [true, true, false] {
            starvation.frame(starved, 10.0);
        }
        assert_eq!(
            starvation.take_line(),
            json!({"events": 0, "ms": 20.0, "longest_ms": 30.0})
        );
    }
}
