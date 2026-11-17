//! Received tick/time map and completion-frame absence bound; nothing drives a timeline yet.
//! `x = previous frame Real - departure` retains the unknown epoch offset. Stamp-map lags
//! share that offset, so their differences are comparable to local durations.

use std::collections::{BTreeMap, HashMap, VecDeque};

use bevy::prelude::*;
use bevy_replicon::client::server_mutate_ticks::MutateTickReceived;
use bevy_replicon::prelude::RepliconTick;
use bevy_replicon::shared::replication::storage::ReplicationStorage;
use lightyear::core::time::TickInstant;
use lightyear::prelude::{Connected, ReplicationCheckpointMap, ReplicationSystems, Tick};
use serde_json::{Value, json};

use super::tick_stamp::{RECEIPT_CAP, ReceivedStamps, TickStamp};
use super::timing::{Timing, WindowMax, histogram};

pub(super) fn install(app: &mut App) {
    app.init_resource::<ViewClock>()
        .add_observer(reset)
        .add_systems(PreUpdate, receive.after(ReplicationSystems::Receive));
}

fn reset(
    _connect: On<Add<Connected>>,
    mut clock: ResMut<ViewClock>,
    mut storage: ResMut<ReplicationStorage>,
) {
    *clock = ViewClock::default();
    *storage.global.get_or_default::<ReceivedStamps>() = ReceivedStamps::default();
}

struct Sample {
    stamp_s: f64,
    x_s: f64,
}
struct Arrival {
    tick: Tick,
    stamp: TickStamp,
    frame: u64,
    seen_s: f64,
}

#[derive(Default, serde::Serialize)]
struct CompletionLine {
    completed_ticks: u64,
    sampled_ticks: u64,
    late_ticks: u64,
    worst_lag_frames: u64,
    worst_lag_ms: f64,
    evicted_incomplete: u64,
    completed_without_stamp: u64,
    unmapped_stamps: u64,
    receipt_overflow: u64,
}

#[derive(Resource, Default)]
pub(super) struct ViewClock {
    frame: u64,
    samples: BTreeMap<Tick, Sample>,
    expiry: VecDeque<(f64, Tick)>,
    maximum: WindowMax,
    pending: HashMap<RepliconTick, Arrival>,
    receipt_order: VecDeque<RepliconTick>,
    completion: CompletionLine,
    stamped_without_completion: usize,
}

impl ViewClock {
    fn arrive(&mut self, checkpoint: RepliconTick, tick: Tick, stamp: TickStamp, now_s: f64) {
        if self.samples.contains_key(&tick) || self.pending.contains_key(&checkpoint) {
            return; // A retransmitted stamp is not a new first arrival.
        }
        // Resolution: bound unresolved receipts by the upstream completion tracker's capacity,
        // independently of W. A lost mutate message never produces a completion event.
        if self.receipt_order.len() == RECEIPT_CAP {
            let oldest = self
                .receipt_order
                .pop_front()
                .expect("receipt queue is at capacity");
            if self.pending.remove(&oldest).is_some() {
                self.completion.evicted_incomplete += 1;
            }
        }
        self.stamped_without_completion += 1;
        self.receipt_order.push_back(checkpoint);
        self.pending.insert(
            checkpoint,
            Arrival {
                tick,
                stamp,
                frame: self.frame,
                seen_s: now_s,
            },
        );
    }

    fn complete(&mut self, checkpoint: RepliconTick, now_s: f64, previous_s: f64) {
        self.stamped_without_completion = 0;
        self.completion.completed_ticks += 1;
        let Some(arrival) = self.pending.remove(&checkpoint) else {
            // Initial reliable state, receipt overflow or a skipped stale stamp can leave no
            // retained stamp for a completed mutate tick. Do not invent a time sample.
            self.completion.completed_without_stamp += 1;
            return;
        };
        let lag = self.frame - arrival.frame;
        if lag > 0 {
            self.completion.late_ticks += 1;
            self.completion.worst_lag_frames = self.completion.worst_lag_frames.max(lag);
            self.completion.worst_lag_ms = self
                .completion
                .worst_lag_ms
                .max((now_s - arrival.seen_s) * 1e3);
        }
        self.sample(arrival.tick, arrival.stamp, now_s, previous_s);
        self.completion.sampled_ticks += 1;
    }

    fn sample(&mut self, tick: Tick, stamp: TickStamp, now_s: f64, previous_s: f64) {
        if let std::collections::btree_map::Entry::Vacant(entry) = self.samples.entry(tick) {
            let x_s = stamp.absence_bound_s(previous_s);
            entry.insert(Sample {
                stamp_s: stamp.frame_start_s(),
                x_s,
            });
            self.expiry.push_back((now_s, tick));
            self.maximum.push(now_s, x_s);
        }
    }

    fn expire(&mut self, now_s: f64, window_s: f64) {
        let oldest = now_s - window_s;
        while self
            .expiry
            .front()
            .is_some_and(|&(seen_s, _)| seen_s < oldest)
        {
            let (_, tick) = self.expiry.pop_front().expect("front expiry exists");
            self.samples.remove(&tick);
        }
        self.maximum.expire(oldest);
    }

    /// Adjacent received stamps in authoritative tick order include the whole production gap
    /// over a lost tick; neither nominal intervals nor missing stamps are invented.
    fn intervals(&self) -> Vec<f64> {
        self.samples
            .values()
            .zip(self.samples.values().skip(1))
            .map(|(before, after)| after.stamp_s - before.stamp_s)
            .collect()
    }

    fn target(&self, intervals: &mut [f64]) -> Option<f64> {
        if intervals.is_empty() {
            return None; // Fewer than two completed stamped ticks: production interval unknown.
        }
        intervals.sort_by(f64::total_cmp);
        // Nearest-rank median, matching the measured distribution in the log.
        let median = intervals[(intervals.len() - 1) / 2];
        self.maximum.get().map(|x| median + x)
    }

    /// Startup or starvation may leave the cursor without retained bracketing stamps.
    fn stamp_at(&self, cursor: TickInstant) -> Option<f64> {
        let (&low_tick, low) = self.samples.range(..=cursor.tick()).next_back()?;
        let (&high_tick, high) = self
            .samples
            .range((
                std::ops::Bound::Excluded(cursor.tick()),
                std::ops::Bound::Unbounded,
            ))
            .next()?;
        let fraction = f64::from((cursor - TickInstant::from(low_tick)).to_f32())
            / f64::from(high_tick - low_tick);
        Some(low.stamp_s + fraction * (high.stamp_s - low.stamp_s))
    }

    pub fn take_line(&mut self, now_s: f64, tick_s: f64, cursor: TickInstant) -> Value {
        let mut intervals = self.intervals();
        let target = self.target(&mut intervals);
        let today_lag = self.stamp_at(cursor).map(|stamp| now_s - stamp);
        let latest_x = self.samples.last_key_value().map(|(_, sample)| sample.x_s);
        json!({
            "delta_star_ms": target.map(|s| s * 1e3),
            "today_stamp_lag_ms": today_lag.map(|s| s * 1e3),
            "target_minus_today_ms": target.zip(today_lag).map(|(target, today)| (target - today) * 1e3),
            "target_above_latest_x_ms": target.zip(latest_x).map(|(target, x)| (target - x) * 1e3),
            "stamp_interval_ms": histogram(&mut intervals, tick_s),
            "completion": std::mem::take(&mut self.completion),
            "pending_ticks": self.pending.len(),
        })
    }
}

fn receive(
    real: Res<Time<Real>>,
    timing: Res<Timing>,
    checkpoints: Res<ReplicationCheckpointMap>,
    mut storage: ResMut<ReplicationStorage>,
    mut completed: MessageReader<MutateTickReceived>,
    mut clock: ResMut<ViewClock>,
) {
    let now = real.elapsed_secs_f64();
    clock.frame += 1;
    clock.expire(now, timing.view_window_s);
    let received = storage.global.get_or_default::<ReceivedStamps>();
    clock.completion.receipt_overflow += std::mem::take(&mut received.overflow);
    for (checkpoint, stamp) in received.queue.drain(..) {
        if let Some(tick) = checkpoints.get(checkpoint) {
            clock.arrive(checkpoint, tick, stamp, now);
        } else {
            // Lightyear's bounded map can evict a mapping during an unusually large receive drain.
            clock.completion.unmapped_stamps += 1;
        }
    }
    // Lightyear's interpolation feature enables Replicon's server track_mutate_messages;
    // without it no whole-tick completion events exist. That remote setting is not readable
    // here, so continued stamped arrivals without completion must fail within the pending cap.
    // Use every completion message, not just last_confirmed_checkpoint, which mirrors only the
    // newest completed tick. Replicon's contains() assumes old ticks complete after 64 ticks;
    // these events instead require every message of that particular tick to have been applied.
    for event in completed.read() {
        clock.complete(event.tick, now, now - real.delta_secs_f64());
    }
    assert!(
        clock.stamped_without_completion <= RECEIPT_CAP,
        "stamped ticks exceeded the pending window without completion: Lightyear interpolation must enable Replicon track_mutate_messages"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(start: f64) -> TickStamp {
        TickStamp::new(start, 0.0).unwrap()
    }

    /// The scheduled consumer must wait for ALL mutate messages, even across summary boundaries.
    /// Client stalls cannot enter x: its time bound is the frame preceding completion.
    #[test]
    fn completion_frame_samples_x_and_reports_split_ticks() {
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        // Fixture: identical first two frames, different completion-frame duration.
        let mut readings = Vec::new();
        for frame_s in [0.0625, 1.0] {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .init_resource::<Timing>()
                .init_resource::<ReplicationStorage>()
                .init_resource::<ReplicationCheckpointMap>()
                .add_message::<MutateTickReceived>()
                .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                    0.0625,
                )));
            install(&mut app);
            app.update();
            app.update();
            let checkpoint = RepliconTick::new(1);
            app.world_mut()
                .resource_mut::<ReplicationCheckpointMap>()
                .record(checkpoint, Tick(1));
            app.world_mut()
                .resource_mut::<ReplicationStorage>()
                .global
                .get_or_default::<ReceivedStamps>()
                .queue
                .push_back((checkpoint, stamp(0.0)));
            app.update();
            let previous = app.world().resource::<Time<Real>>().elapsed_secs_f64();
            let mut clock = app.world_mut().resource_mut::<ViewClock>();
            assert!(clock.samples.is_empty());
            clock.take_line(
                previous,
                super::super::server::TICK.as_secs_f64(),
                Tick(1).into(),
            );
            app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                frame_s,
            )));
            app.world_mut()
                .write_message(MutateTickReceived { tick: checkpoint });
            app.update();
            let clock = app.world().resource::<ViewClock>();
            assert_eq!(clock.samples[&Tick(1)].x_s, previous);
            assert_eq!(clock.completion.late_ticks, 1);
            assert_eq!(clock.completion.worst_lag_frames, 1);
            assert!((clock.completion.worst_lag_ms - frame_s * 1e3).abs() < 1e-9);
            readings.push(clock.samples[&Tick(1)].x_s);
        }
        assert_eq!(readings[0], readings[1]);
    }

    /// A client cannot silently collect stamps forever if the server's completion tracking is off.
    #[test]
    #[should_panic(expected = "stamped ticks exceeded the pending window without completion")]
    fn stamped_ticks_without_completions_break_the_pending_window_invariant() {
        use bevy::time::TimeUpdateStrategy;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Timing>()
            .init_resource::<ReplicationStorage>()
            .init_resource::<ReplicationCheckpointMap>()
            .add_message::<MutateTickReceived>()
            .insert_resource(TimeUpdateStrategy::ManualDuration(
                super::super::server::TICK,
            ));
        install(&mut app);
        // Fixture: one more stamped tick than the pending window can retain, no completions.
        for tick in 0..=RECEIPT_CAP as u32 {
            let checkpoint = RepliconTick::new(tick);
            app.world_mut()
                .resource_mut::<ReplicationCheckpointMap>()
                .record(checkpoint, Tick(tick));
            app.world_mut()
                .resource_mut::<ReplicationStorage>()
                .global
                .get_or_default::<ReceivedStamps>()
                .queue
                .push_back((checkpoint, stamp(f64::from(tick))));
            app.update();
        }
    }

    /// D3 measures received production intervals, including missing ticks rather than a nominal T.
    #[test]
    fn stamp_intervals_include_the_gap_over_a_lost_tick() {
        let tick_s = super::super::server::TICK.as_secs_f64();
        let mut clock = ViewClock::default();
        // Fixture: the middle stamp is lost; epoch offset is unchanged.
        clock.sample(Tick(1), stamp(0.0), 0.5, 0.25);
        clock.sample(
            Tick(3),
            stamp(2.0 * tick_s),
            0.5 + 2.0 * tick_s,
            0.25 + 2.0 * tick_s,
        );
        assert_eq!(clock.intervals(), [2.0 * tick_s]);
        assert_eq!(
            clock.target(&mut clock.intervals()),
            Some(0.25 + 2.0 * tick_s)
        );
    }

    /// D3 forgets expired lateness without discarding the unknown clock offset.
    #[test]
    fn target_forgets_expired_lateness_and_preserves_the_clock_offset() {
        let timing = Timing::shipped();
        let mut clock = ViewClock::default();
        // Fixture: observations at fractions of the authored window, with a late oldest tick.
        for (tick, start, x, seen) in [
            (1, 1.0, 8.0, 0.0),
            (2, 1.125, 4.0, 0.5),
            (3, 1.25, 4.0, 0.75),
        ] {
            clock.sample(
                Tick(tick),
                stamp(start),
                seen * timing.view_window_s,
                start + x,
            );
        }
        assert_eq!(clock.target(&mut clock.intervals()), Some(8.125));
        clock.expire(1.25 * timing.view_window_s, timing.view_window_s);
        assert_eq!(clock.target(&mut clock.intervals()), Some(4.125));
    }

    /// A lost mutation never completes merely because newer ticks complete or the mask ages out.
    #[test]
    fn lost_tick_stays_unsampled_and_pending_memory_is_bounded() {
        use bevy_replicon::client::server_mutate_ticks::ServerMutateTicks;
        let mut clock = ViewClock::default();
        let mut tracker = ServerMutateTicks::default();
        // Fixture: the first of two messages arrives; the second is permanently lost.
        let lost = RepliconTick::new(1);
        clock.arrive(lost, Tick(1), stamp(1.0), 1.0);
        assert!(!tracker.confirm(lost, 2));
        for tick in 2..=(RECEIPT_CAP as u32 + 2) {
            let checkpoint = RepliconTick::new(tick);
            clock.arrive(
                checkpoint,
                Tick(tick),
                stamp(f64::from(tick)),
                f64::from(tick),
            );
            if tracker.confirm(checkpoint, 1) {
                clock.complete(checkpoint, f64::from(tick), f64::from(tick));
            }
        }
        assert!(
            tracker.contains(lost),
            "upstream mask treats aged-out ticks as applied"
        );
        assert!(
            !clock.samples.contains_key(&Tick(1)),
            "only an actual completion may sample x"
        );
        assert_eq!(clock.completion.evicted_incomplete, 1);
        assert!(clock.pending.len() <= RECEIPT_CAP);
        assert!(clock.receipt_order.len() <= RECEIPT_CAP);
    }
}
