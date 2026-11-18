//! One presentation cursor from completed ticks: frame start maps ticks to time; departure
//! measures server work for diagnostics. No RTT or estimated server clock participates.
//! `x = previous frame Real - frame start` retains the unknown epoch offset. Stamp-map lags
//! share that offset, so their differences are comparable to local durations.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    time::Duration,
};

use bevy::prelude::*;
use bevy_replicon::client::server_mutate_ticks::MutateTickReceived;
use bevy_replicon::prelude::RepliconTick;
use bevy_replicon::shared::replication::storage::ReplicationStorage;
use lightyear::core::time::{Overstep, TickInstant};
use lightyear::interpolation::{plugin::InterpolationSystems, timeline::InterpolationTimeline};
use lightyear::prelude::{Connected, ReplicationCheckpointMap, ReplicationSystems, Tick};
use lightyear::prelude::{NetworkTimeline, SyncSystems, TimelineSync, TimelineSystems};
use serde_json::{Value, json};

use super::tick_stamp::{RECEIPT_CAP, ReceivedStamps, TickStamp};
use super::timing::{Timing, WindowMax, histogram};

pub(super) fn install(app: &mut App) {
    // The public sync set contains only the local and interpolation controllers (0.31.0).
    // Neither controls arrival-owned input or the application-owned presentation cursor.
    app.configure_sets(PostUpdate, SyncSystems::Sync.run_if(|| false));
    // Lightyear 0.31.0's samplers read the timeline without checking is_synced.
    // No completed stamp pair means no presentation epoch, so they must wait.
    app.configure_sets(
        Update,
        InterpolationSystems::All.run_if(|clock: Res<ViewClock>| clock.now().is_some()),
    );
    app.init_resource::<Timing>()
        .init_resource::<ViewClock>()
        .init_resource::<crate::WeaponClock>()
        .add_observer(reset)
        .add_observer(disconnect)
        .add_message::<super::matchmaking::ServerGone>()
        .add_systems(
            PreUpdate,
            (receive, drive, publish_weapon_clock, capture_trace_tick)
                .chain()
                .after(ReplicationSystems::Receive)
                .after(TimelineSystems::Advance),
        );
}

fn reset(
    _connect: On<Add<Connected>>,
    mut clock: ResMut<ViewClock>,
    mut storage: ResMut<ReplicationStorage>,
) {
    let epoch = clock.epoch + 1;
    *clock = ViewClock { epoch, ..default() };
    *storage.global.get_or_default::<ReceivedStamps>() = ReceivedStamps::default();
}

/// A discontinuity logged and counted by the clock driver, in authoritative ticks.
#[derive(Debug, Clone, Copy)]
struct ViewCut {
    pub from: TickInstant,
    pub to: TickInstant,
}

fn disconnect(_ended: On<Remove<Connected>>, mut clock: ResMut<ViewClock>) {
    let epoch = clock.epoch + 1;
    *clock = ViewClock { epoch, ..default() };
}

struct Sample {
    stamp_s: f64,
    work_s: f64,
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

/// The stream's rate, learned only from the stamp map at the target point.
#[derive(Clone, Copy)]
struct StreamRate {
    /// Simulation seconds per stamp second.
    rate: f64,
    /// The target point it was last read at: a stamp time and the map's tick there.
    point_s: f64,
    point: f64,
}

#[derive(Resource, Default)]
pub(crate) struct ViewClock {
    /// The presentation cursor in ticks, unquantized, so the pacing loop never feeds back the
    /// published cursor's rounding.
    cursor: Option<f64>,
    /// The cursor's rate, simulation seconds per Real second.
    rate: f64,
    delta_s: Option<f64>,
    stream: Option<StreamRate>,
    stamps: BTreeMap<Tick, f64>,
    /// The frame's rate-limited request passed the newest completed tick by more than a cursor
    /// quantum.
    pub held: bool,
    pub received: u64,
    cuts: u64,
    reported_cuts: u64,
    frame: u64,
    pub(super) epoch: u64,
    samples: BTreeMap<Tick, Sample>,
    expiry: VecDeque<(f64, Tick)>,
    maximum: WindowMax,
    pending: HashMap<RepliconTick, Arrival>,
    receipt_order: VecDeque<RepliconTick>,
    completion: CompletionLine,
    refusal: Option<StreamRefusal>,
    spread: Option<Duration>,
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
        let before = self.received;
        self.sample(arrival.tick, arrival.stamp, now_s, previous_s);
        self.completion.sampled_ticks += self.received - before;
    }

    fn sample(&mut self, tick: Tick, stamp: TickStamp, now_s: f64, previous_s: f64) {
        let start = stamp.frame_start_s();
        if self
            .stamps
            .range(..tick)
            .next_back()
            .is_some_and(|(_, before)| *before > start)
            || self
                .stamps
                .range((std::ops::Bound::Excluded(tick), std::ops::Bound::Unbounded))
                .next()
                .is_some_and(|(_, after)| *after < start)
        {
            self.refusal = Some(StreamRefusal::Order { tick });
            return;
        }

        if !self.samples.contains_key(&tick) {
            let x_s = stamp.absence_bound_s(previous_s);
            let low = self.samples.values().map(|s| s.x_s).fold(x_s, f64::min);
            let high = self.maximum.get().map_or(x_s, |x| x.max(x_s));
            let Ok(spread) = Duration::try_from_secs_f64(high - low) else {
                self.refusal = Some(StreamRefusal::Range { tick });
                return;
            };
            self.spread = Some(spread);
            self.stamps.insert(tick, stamp.frame_start_s());
            self.samples.insert(
                tick,
                Sample {
                    stamp_s: stamp.frame_start_s(),
                    work_s: stamp.departure_s() - stamp.frame_start_s(),
                    x_s,
                },
            );
            self.expiry.push_back((now_s, tick));
            self.maximum.push(now_s, x_s);
            self.received += 1;
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
        self.spread = self
            .samples
            .values()
            .map(|s| s.x_s)
            .min_by(f64::total_cmp)
            .zip(self.maximum.get())
            .map(|(low, high)| {
                Duration::try_from_secs_f64(high - low)
                    .expect("expiry only narrows the admitted representable lateness spread")
            });
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

    /// Diagnostic server work only; frame-start lateness already includes every work sample.
    fn usual_work_s(&self) -> Option<f64> {
        let mut work: Vec<_> = self.samples.values().map(|sample| sample.work_s).collect();
        if work.is_empty() {
            return None; // No ordinary work sample remains after an outage exhausts W.
        }
        work.sort_by(f64::total_cmp);
        Some(work[(work.len() - 1) / 2])
    }

    /// Waiting is a game state: no completed stamp pair means no presentation epoch yet.
    #[cfg(test)]
    pub fn set_now(&mut self, now: TickInstant) {
        self.cursor = Some(number(now));
    }
    #[cfg(test)]
    pub fn set_synced(&mut self, synced: bool) {
        self.cursor = synced.then(|| self.cursor.unwrap_or_default());
    }

    /// The published cursor, rounded to U0F16; rounding is monotone and never passes a tick.
    pub fn now(&self) -> Option<TickInstant> {
        self.cursor.map(instant)
    }

    pub fn newest(&self) -> Option<Tick> {
        self.stamps.last_key_value().map(|(&tick, _)| tick)
    }

    pub fn lateness_spread(&self) -> Option<Duration> {
        self.spread
    }

    fn stamp_at(&self, cursor: TickInstant) -> Option<f64> {
        let (&low_tick, &low) = self.stamps.range(..=cursor.tick()).next_back()?;
        if cursor == TickInstant::from(low_tick) {
            return Some(low);
        }
        let (&high_tick, &high) = self
            .stamps
            .range((
                std::ops::Bound::Excluded(low_tick),
                std::ops::Bound::Unbounded,
            ))
            .next()?;
        let fraction = f64::from((cursor - TickInstant::from(low_tick)).to_f32())
            / f64::from(high_tick - low_tick);
        Some(low + fraction * (high - low))
    }

    /// The map's tick at `time_s`, unquantized: only the public cursor rounds to U0F16. Clamp
    /// the inverse to received facts. Equal frame stamps choose the latest tick; no time exists
    /// between those samples. Missing stamps are bridged, never extrapolated.
    fn inverse(&self, time_s: f64) -> Option<f64> {
        let mut before: Option<(Tick, f64)> = None;
        for (&tick, &stamp) in &self.stamps {
            if stamp > time_s {
                return Some(match before {
                    None => f64::from(tick.0),
                    Some((low_tick, low)) => {
                        f64::from(low_tick.0)
                            + (time_s - low) / (stamp - low) * f64::from(tick - low_tick)
                    }
                });
            }
            before = Some((tick, stamp));
        }
        before.map(|(tick, _)| f64::from(tick.0))
    }

    /// Critically damped tracking of the target about the stream's rate, exact over any frame
    /// length; the target is taken to move at the stream's rate through the frame. Returns the
    /// requested cursor in ticks, advanced by 0 to (1+v) simulation seconds per Real second.
    fn pace(
        &mut self,
        previous: f64,
        target: f64,
        dt_s: f64,
        tick_s: f64,
        stream: f64,
        v: f64,
    ) -> f64 {
        let omega = bandwidth(v, tick_s);
        // How far the cursor would lead the frame-end target, advancing at the stream's rate.
        let lead_s = (previous - target) * tick_s + stream * dt_s;
        let decay = (-omega * dt_s).exp();
        let deviation = self.rate - stream;
        let pull = deviation + omega * lead_s;
        let lead_after_s = (lead_s + pull * dt_s) * decay;
        self.rate = (stream + (deviation - omega * pull * dt_s) * decay).clamp(0.0, 1.0 + v);
        let advance_s = (lead_after_s - lead_s + stream * dt_s).clamp(0.0, (1.0 + v) * dt_s);
        previous + advance_s / tick_s
    }

    /// The stream's rate at the target point, from map facts only: each advance of the point
    /// filters the map's mean slope over it at the pacing bandwidth, so a hold (no advance)
    /// freezes it and the release spans the gap with its mean slope. The first cursor frame
    /// reads the slope across the retained map; equal end stamps read as the cap.
    fn learn(&mut self, point_s: f64, point: f64, tick_s: f64, v: f64) -> f64 {
        let rate = match self.stream {
            None => {
                let ((&first, &first_s), (&newest, &newest_s)) = self
                    .stamps
                    .first_key_value()
                    .zip(self.stamps.last_key_value())
                    .expect("a cursor's first frame has completed stamps");
                f64::from(newest - first) * tick_s / (newest_s - first_s)
            }
            Some(learned) if point_s > learned.point_s => {
                let elapsed_s = point_s - learned.point_s;
                let slope = (point - learned.point) * tick_s / elapsed_s;
                let gain = 1.0 - (-bandwidth(v, tick_s) * elapsed_s).exp();
                learned.rate + gain * (slope - learned.rate)
            }
            Some(learned) => learned.rate,
        }
        .clamp(0.0, 1.0 + v);
        self.stream = Some(StreamRate {
            rate,
            point_s,
            point,
        });
        rate
    }

    fn advance(
        &mut self,
        now_s: f64,
        dt_s: f64,
        tick_s: f64,
        timing: &Timing,
        admission_s: f64,
    ) -> Option<ViewCut> {
        let released = std::mem::replace(&mut self.held, false);
        let v = timing.view_rate;
        // While an outage empties the window, the applied delay holds.
        let target = self.target(&mut self.intervals()).or(self.delta_s)?;
        let delta = self.delta_s.get_or_insert(target);
        *delta += (target - *delta).clamp(-v * dt_s, v * dt_s);
        let desired_s = now_s - *delta;
        // Retention keeps the newest completed tick, so a map exists once a delay does.
        let (&first_s, &newest_s) = self
            .stamps
            .first_key_value()
            .zip(self.stamps.last_key_value())
            .map(|((_, first), (_, newest))| (first, newest))
            .expect("an applied delay has completed stamps");
        let point_s = desired_s.clamp(first_s, newest_s);
        let point = self
            .inverse(point_s)
            .expect("the clamped point lies on the map");
        let stream = self.learn(point_s, point, tick_s, v);
        let newest = f64::from(self.newest().expect("a target requires completed stamps").0);
        let previous = self.cursor.unwrap_or(point);
        // A wall-time stall can create a long map segment, but its admitted simulation backlog
        // is bounded by the server's cap.
        let backlog_s = (point - previous) * tick_s;
        let cut = (backlog_s > timing.view_cut_s.max(admission_s)).then_some(ViewCut {
            from: instant(previous),
            to: instant(point),
        });
        let requested = if cut.is_some() || self.cursor.is_none() {
            self.rate = stream;
            point
        } else {
            if released {
                // A hold is a gap in facts, not a change in the stream's speed.
                self.rate = stream;
            }
            self.pace(previous, point, dt_s, tick_s, stream, v)
        };
        let next = requested.min(newest);
        // Resolution: the public cursor quantizes fractions to U0F16. A request within a quantum
        // of the newest tick publishes the same cursor, so it is not a hold.
        let quantum = 1.0 / (f64::from(u16::MAX) + 1.0);
        self.held = requested > newest + quantum;
        assert!(
            next >= previous,
            "the advance is nonnegative and newest never retires below the cursor"
        );
        self.cursor = Some(next);
        self.cuts += u64::from(cut.is_some());
        // Keep the cursor's predecessor and future map points through holds. Past points retire
        // only after both the statistics window and the cursor have passed them.
        let at = instant(next).tick();
        let anchor = self.stamps.range(..=at).next_back().map(|(&tick, _)| tick);
        self.stamps.retain(|tick, _| {
            Some(*tick) == anchor || *tick >= at || self.samples.contains_key(tick)
        });
        cut
    }

    pub fn take_line(&mut self, now_s: f64, tick_s: f64, cursor: TickInstant) -> Value {
        let mut intervals = self.intervals();
        let target = self.target(&mut intervals);
        let today_lag = self.stamp_at(cursor).map(|stamp| now_s - stamp);
        let latest_x = self.samples.last_key_value().map(|(_, sample)| sample.x_s);
        json!({
            "usual_work_ms": self.usual_work_s().map(|s| s * 1e3),
            "delta_star_ms": target.map(|s| s * 1e3),
            "stamp_lag_ms": today_lag.map(|s| s * 1e3),
            "applied_delta_ms": self.delta_s.map(|s| s * 1e3),
            "applied_above_latest_x_ms": self.delta_s.zip(latest_x).map(|(delta, x)| (delta - x) * 1e3),
            "cuts": self.cuts - std::mem::replace(&mut self.reported_cuts, self.cuts),
            "target_minus_applied_ms": target.zip(self.delta_s).map(|(target, applied)| (target - applied) * 1e3),
            "target_above_latest_x_ms": target.zip(latest_x).map(|(target, x)| (target - x) * 1e3),
            "stamp_interval_ms": histogram(&mut intervals, tick_s),
            "completion": std::mem::take(&mut self.completion),
            "pending_ticks": self.pending.len(),
        })
    }
}

/// The pacing bandwidth omega, per Real second, from the slew limit v and tick T: the loop's
/// natural frequency and the stream rate's filter pole. Resolution: with both at omega the rate's
/// response to target phase is the kernel omega^2 (3 - 5 x + x^2) e^-x, x = omega t, of zero area
/// and L1 norm 2 kappa omega, kappa the form constant below. Production phase within p peak to
/// peak therefore moves the rate by at most kappa omega p, and omega = v / (kappa T) is the widest
/// band for which a tick of it stays within the slew limit v.
fn bandwidth(v: f64, tick_s: f64) -> f64 {
    // The kernel's sign changes at x = (5 -+ sqrt 13) / 2, where its integral is (3 - 2 x) e^-x.
    let root = 13.0_f64.sqrt();
    let kappa =
        (root - 2.0) * (-(5.0 - root) / 2.0).exp() + (root + 2.0) * (-(5.0 + root) / 2.0).exp();
    v / (kappa * tick_s)
}

#[cfg(test)]
fn number(cursor: TickInstant) -> f64 {
    f64::from(cursor.tick().0) + f64::from(cursor.overstep().to_f32())
}
fn instant(tick: f64) -> TickInstant {
    TickInstant::from_tick_and_overstep(
        Tick(tick.floor() as u32),
        Overstep::from_f32(tick.fract() as f32),
    )
}

fn drive(
    real: Res<Time<Real>>,
    tick: Res<lightyear::core::tick::TickDuration>,
    timing: Res<Timing>,
    mut clock: ResMut<ViewClock>,
    mut timeline: ResMut<InterpolationTimeline>,
) {
    if let Some(cut) = clock.advance(
        real.elapsed_secs_f64(),
        real.delta_secs_f64(),
        tick.0.as_secs_f64(),
        &timing,
        timing.admission_s,
    ) {
        warn!(
            "net: view cut {:?} -> {:?}: presentation backlog exceeded continuity budget",
            cut.from, cut.to
        );
    }
    match clock.now() {
        Some(now) => {
            timeline.set_now(now);
            timeline.set_synced(true);
        }
        None => timeline.set_synced(false), // Waiting for a completed stamp pair; samplers are gated.
    }
}

/// Client trace labels observe the same server-domain view instant across every schedule.
fn capture_trace_tick(
    clock: Res<ViewClock>,
    mut trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    if let Some(trace) = trace.as_mut() {
        trace.observe_view_tick(clock.now().map(|now| now.tick().0));
    }
}

/// Outside server input is refused through the visible match-close path.
#[derive(Debug, Clone)]
pub(super) enum StreamRefusal {
    Order { tick: Tick },
    Range { tick: Tick },
}
impl std::fmt::Display for StreamRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Order { tick } => write!(
                f,
                "server tick {tick:?} has a frame-start stamp outside its authoritative neighbours; refusing the stream"
            ),
            Self::Range { tick } => write!(
                f,
                "server tick {tick:?} has an unrepresentable lateness spread; refusing the stream"
            ),
        }
    }
}

/// Client HUD time is published every render frame, before its fixed gameplay readers.
pub(super) fn publish_weapon_clock(clock: Res<ViewClock>, mut weapon: ResMut<crate::WeaponClock>) {
    if let Some(now) = clock.now() {
        weapon.0 = crate::sim_tick::SimTick::new(now.tick().0);
    } // Waiting for a stamp pair cannot mature a countdown.
}

fn receive(
    real: Res<Time<Real>>,
    timing: Res<Timing>,
    checkpoints: Res<ReplicationCheckpointMap>,
    mut storage: ResMut<ReplicationStorage>,
    mut completed: MessageReader<MutateTickReceived>,
    mut gone: MessageWriter<super::matchmaking::ServerGone>,
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
    // without it no whole-tick completion events exist. The clean-link test
    // shipped_replication_completes_stamped_ticks_for_view_clock_samples guards that build dependency.
    // Use every completion message, not just last_confirmed_checkpoint, which mirrors only the
    // newest completed tick. Replicon's contains() assumes old ticks complete after 64 ticks;
    // these events instead require every message of that particular tick to have been applied.
    for event in completed.read() {
        clock.complete(event.tick, now, now - real.delta_secs_f64());
    }
    if let Some(refusal) = clock.refusal.take() {
        warn!("client: {refusal}");
        gone.write(super::matchmaking::ServerGone::Stream(refusal));
    }
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
                .insert_resource(lightyear::core::tick::TickDuration(
                    super::super::server::TICK,
                ))
                .init_resource::<ReplicationStorage>()
                .init_resource::<ReplicationCheckpointMap>()
                .add_message::<MutateTickReceived>()
                .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                    0.0625,
                )));
            app.init_resource::<InterpolationTimeline>();
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

    /// Every client lifecycle producer relies on the installed clock's server-domain label,
    /// including rows emitted on fixed frames. A local transport label cannot leak into it.
    #[test]
    fn installed_clock_labels_all_client_trace_rows_and_names_waiting() {
        use bevy::time::TimeUpdateStrategy;
        let path = std::env::temp_dir().join(format!(
            "view-clock-trace-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ReplicationStorage>()
            .init_resource::<ReplicationCheckpointMap>()
            .init_resource::<InterpolationTimeline>()
            .insert_resource(lightyear::core::tick::TickDuration(
                super::super::server::TICK,
            ))
            .insert_resource(TimeUpdateStrategy::ManualDuration(
                super::super::server::TICK,
            ))
            .insert_resource(crate::shot_trace::ShotTrace::for_test(&path, None))
            .add_message::<MutateTickReceived>();
        install(&mut app);
        app.insert_resource(Time::<Fixed>::from_duration(super::super::server::TICK))
            .add_systems(
                FixedPreUpdate,
                |clock: Res<ViewClock>, mut trace: ResMut<crate::shot_trace::ShotTrace>| {
                    // Fixture: unrelated transport count; compare against this fixed reader's cursor.
                    trace.global_row(
                        "probe",
                        999,
                        json!({"view": clock.now().map(|now| now.tick().0)}),
                    );
                },
            );
        app.update();
        app.update(); // Waiting must be observed inside a fixed frame, too.
        for tick in [1, 4] {
            let checkpoint = RepliconTick::new(tick);
            app.world_mut()
                .resource_mut::<ReplicationCheckpointMap>()
                .record(checkpoint, Tick(tick));
            app.world_mut()
                .resource_mut::<ReplicationStorage>()
                .global
                .get_or_default::<ReceivedStamps>()
                .queue
                .push_back((
                    checkpoint,
                    stamp(f64::from(tick) * super::super::server::TICK.as_secs_f64()),
                ));
            app.world_mut()
                .write_message(MutateTickReceived { tick: checkpoint });
        }
        // Fixture: observe the first ready frame and subsequent cursor progress in fixed readers.
        for _ in 0..8 {
            app.update();
        }
        app.insert_resource(ViewClock::default());
        app.update(); // Returning to waiting must clear the cached label before fixed readers.
        drop(app);
        let rows: Vec<Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(!rows.is_empty(), "the fixed reader must run");
        assert!(
            rows.iter().any(|row| row["view"].is_null()),
            "waiting has no invented tick zero"
        );
        assert!(
            rows.last().unwrap()["view"].is_null(),
            "the final fixed reader must observe waiting again"
        );
        let ready: Vec<_> = rows.iter().filter(|row| !row["view"].is_null()).collect();
        assert!(
            !ready.is_empty(),
            "completed stamps must establish a view epoch"
        );
        assert_ne!(
            ready.first().unwrap()["view"],
            ready.last().unwrap()["view"],
            "the reader must witness cursor progress, not only a constant label"
        );
        for row in rows {
            assert_eq!(
                row["t"], row["view"],
                "fixed input trace readers need this frame's view label"
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    /// View-clock samples rely on the shipped server tracking whole-tick mutation completion.
    /// A clean UDP link must deliver completion events for ticks carrying the shipped stamp.
    #[test]
    fn shipped_replication_completes_stamped_ticks_for_view_clock_samples() {
        use bevy::ecs::message::MessageCursor;
        use lightyear::prelude::{NetworkTarget, Replicate};
        use std::time::Duration;

        let _udp = super::super::test_harness::lock_real_udp_test();
        let (mut server, mut client) = super::super::client::match_exit::in_match();
        client.init_resource::<Timing>();
        install(&mut client);
        server
            .world_mut()
            .spawn((stamp(0.0), Replicate::to_clients(NetworkTarget::All)));
        let mut completed = MessageCursor::<MutateTickReceived>::default();
        // Fixture: allow initial reliable replication and subsequent stamped mutations to arrive.
        for _ in 0..500 {
            server.update();
            std::thread::sleep(Duration::from_micros(300));
            client.update();
            let clock = client.world().resource::<ViewClock>();
            let checkpoints = client.world().resource::<ReplicationCheckpointMap>();
            if completed
                .read(client.world().resource::<Messages<MutateTickReceived>>())
                .any(|event| {
                    checkpoints
                        .get(event.tick)
                        .is_some_and(|tick| clock.samples.contains_key(&tick))
                })
            {
                return;
            }
        }
        panic!("view-clock samples require whole-tick completion events for stamped ticks");
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
    /// Event and hull consumers rely on monotonic, bounded pacing even when the target rises or
    /// expires. The game's cosmetic path cache must survive a delay rise past its catch-up horizon.
    #[test]
    fn paced_delay_changes_keep_the_cursor_and_an_old_public_path_forward() {
        use crate::ballistics::path::Replica;
        let timing = Timing::shipped();
        let tick_s = super::super::server::TICK.as_secs_f64();
        let cap_s = timing.admission_s;
        let mut clock = ViewClock::default();
        let mut path = Replica::born(0, Vec3::ZERO, Vec3::Z * 800.0); // Fixture velocity.
        let mut previous = 0.0;
        let mut position = 0.0;
        let mut previous_delta: Option<f64> = None;
        let change_at = crate::ballistics::MAX_COSMETIC_CATCH_UP_TICKS * 2;
        for tick in 1..change_at + 800 {
            let s = f64::from(tick) * tick_s;
            // Fixture: independently arriving completed samples, then an increased link bound.
            let x = if tick > change_at { 0.100 } else { 0.040 };
            clock.sample(Tick(tick), stamp(s), s + x + tick_s, s + x);
            clock.expire(s + x + tick_s, timing.view_window_s);
            let cut = clock.advance(s + x + tick_s, tick_s, tick_s, &timing, cap_s);
            assert!(cut.is_none(), "a bounded delay change is paced, not cut");
            if let Some(now) = clock.now() {
                let at = number(now);
                assert!(at >= previous, "presentation never rewinds");
                let quantization = 2.0 / f64::from(u16::MAX); // DERIVED: both endpoint oversteps round.
                assert!(
                    at - previous <= (1.0 + timing.view_rate) + quantization || previous == 0.0
                );
                let (p, _, _) = path
                    .sample(at, tick_s as f32, 0.0)
                    .expect("a forward view cursor retains an old public path");
                assert!(p.z >= position);
                position = p.z;
                previous = at;
            }
            if let (Some(before), Some(after)) = (previous_delta, clock.delta_s) {
                assert!(
                    (after - before).abs() <= timing.view_rate * tick_s + 1e-9,
                    "delta slew cannot exceed its authored real-time budget"
                );
            }
            previous_delta = clock.delta_s;
        }
    }

    /// A link outage beyond continuity is a typed cut; a fresh stamp cannot reinterpret the
    /// unknown epoch as a duration. Both clock-origin choices must produce the same cursor.
    #[test]
    fn a_link_outage_cuts_once_and_clock_origins_cancel() {
        let timing = Timing::shipped();
        let tick_s = super::super::server::TICK.as_secs_f64();
        let admission_s = timing.admission_s;
        let mut cuts = Vec::new();
        for offset in [-10_000.0, 20_000.0] {
            // Fixture: either process may have the older epoch.
            let mut clock = ViewClock::default();
            for tick in 1..=4 {
                let s = f64::from(tick) * tick_s;
                clock.sample(Tick(tick), stamp(s), s + offset + 0.040, s + offset + 0.030);
                clock.advance(s + offset + 0.040, tick_s, tick_s, &timing, admission_s);
            }
            let before = clock.now().unwrap();
            // Fixture: no completed updates during an outage longer than both W and the cut gate.
            let gap_s = timing.view_window_s + timing.view_cut_s.max(admission_s) * 4.0;
            clock.expire(gap_s + offset - tick_s, timing.view_window_s);
            clock.advance(
                gap_s + offset - tick_s,
                gap_s - tick_s,
                tick_s,
                &timing,
                admission_s,
            );
            // A frame far longer than the pacing time constant converges onto the clamped target
            // inside it; every frame after it requests past the newest tick.
            clock.expire(gap_s + offset, timing.view_window_s);
            clock.advance(gap_s + offset, tick_s, tick_s, &timing, admission_s);
            assert!(clock.held);
            assert!(clock.now().unwrap() >= before);
            assert_eq!(clock.now().unwrap(), TickInstant::from(Tick(4)));
            for tick in 1000..1004 {
                let s = f64::from(tick) * tick_s;
                clock.sample(Tick(tick), stamp(s), s + offset + 0.040, s + offset + 0.030);
            }
            let s = 1003.0 * tick_s;
            let cut = clock
                .advance(s + offset + 0.040, tick_s, tick_s, &timing, admission_s)
                .expect("a link backlog past the continuity budget emits a cut");
            assert_eq!(cut.from, TickInstant::from(Tick(4)));
            assert!(cut.to > cut.from);
            cuts.push(cut.to);
            assert!(
                clock
                    .advance(s + offset + 0.040, 0.0, tick_s, &timing, admission_s)
                    .is_none(),
                "an applied cut is not emitted twice"
            );
        }
        assert_eq!(
            cuts[0], cuts[1],
            "the unknown clock offset cancels from the cursor"
        );
    }

    /// A peer's individually finite stamps can still contradict the tick order. Refuse that
    /// stream before division/interpolation; loss and reordering of a consistent stream are legal.
    #[test]
    fn out_of_order_receipts_are_valid_but_reversed_time_is_refused() {
        let mut clock = ViewClock::default();
        for tick in [1, 3, 2] {
            clock.sample(Tick(tick), stamp(f64::from(tick)), 4.0, 3.0);
        }
        assert!(clock.refusal.is_none());
        assert_eq!(clock.inverse(2.5), Some(2.5));
        clock.sample(Tick(4), stamp(0.0), 5.0, 4.0);
        assert!(matches!(
            clock.refusal,
            Some(StreamRefusal::Order { tick: Tick(4) })
        ));
        assert_eq!(clock.newest(), Some(Tick(3)));
    }
    /// A replacement authority may start at an earlier tick. The shipped installer must clear
    /// both the view epoch on disconnect and queued old receipts on connect, without an arming latch.
    #[test]
    fn connection_boundaries_clear_the_cursor_and_old_tick_receipts() {
        use lightyear::prelude::{PeerId, RemoteId};
        let mut app = App::new();
        app.init_resource::<ReplicationStorage>();
        install(&mut app);
        app.world_mut()
            .resource_mut::<ViewClock>()
            .set_now(Tick(5000).into());
        app.world_mut()
            .resource_mut::<ReplicationStorage>()
            .global
            .get_or_default::<ReceivedStamps>()
            .queue
            .push_back((RepliconTick::new(1), stamp(1.0)));
        let link = app
            .world_mut()
            .spawn((RemoteId(PeerId::Netcode(99)), Connected))
            .id();
        assert!(app.world().resource::<ViewClock>().now().is_none());
        assert!(
            app.world()
                .resource::<ReplicationStorage>()
                .global
                .get::<ReceivedStamps>()
                .unwrap()
                .queue
                .is_empty()
        );
        app.world_mut()
            .resource_mut::<ViewClock>()
            .set_now(Tick(5000).into());
        app.world_mut().entity_mut(link).remove::<Connected>();
        let clock = app.world().resource::<ViewClock>();
        assert!(clock.now().is_none());
        assert!(clock.stamps.is_empty() && clock.pending.is_empty() && clock.samples.is_empty());
        assert_eq!(clock.received, 0);
    }
    /// Independently drifting process clocks must not accumulate an unbounded playback error.
    /// Hull presentation relies on a window-bounded epoch error, not a synchronized local tick.
    #[test]
    fn oscillator_drift_is_bounded_by_the_lateness_window() {
        let timing = Timing::shipped();
        let tick_s = super::super::server::TICK.as_secs_f64();
        // Fixture: +/-50 ppm oscillator error, arbitrary offset and a constant clean-link delay.
        for epsilon in [-50e-6_f64, 50e-6] {
            let mut clock = ViewClock::default();
            let mut before = None;
            let ticks = (timing.view_window_s * 4.0 / tick_s).ceil() as u32;
            for tick in 1..=ticks {
                let server = f64::from(tick) * tick_s;
                let x = 20.0 + 0.040 + server * epsilon;
                let now = server + x + tick_s;
                clock.expire(now, timing.view_window_s);
                clock.sample(Tick(tick), stamp(server), now, server + x);
                clock.advance(
                    now,
                    tick_s * (1.0 + epsilon),
                    tick_s,
                    &timing,
                    timing.admission_s,
                );
                if let Some(cursor) = clock.now() {
                    assert!(before.is_none_or(|before| cursor >= before));
                    before = Some(cursor);
                }
                if server > timing.view_window_s * 2.0 {
                    let bias = clock.target(&mut clock.intervals()).unwrap() - x - tick_s;
                    // DERIVED: max_W(x) - current x <= |epsilon| W; float subtraction tolerance.
                    assert!(bias >= -1e-9 && bias <= epsilon.abs() * timing.view_window_s + 1e-9);
                }
            }
        }
    }

    /// The game's presentation needs the next frame-start keyframe before crossing its tick.
    /// Frame-start lateness includes compute, including isolated stalls, without a second term.
    #[test]
    fn frame_start_lateness_covers_both_compute_and_link_work() {
        let timing = Timing::shipped();
        let tick = super::super::server::TICK.as_secs_f64();
        // Fixture: binary fractions make the missing compute term an exact counterexample.
        let (work, down, frame) = (tick / 4.0, tick / 4.0, tick / 8.0);
        let mut clock = ViewClock::default();
        for k in 1..=8 {
            let start = f64::from(k) * tick;
            clock.sample(
                Tick(k),
                TickStamp::new(start, work as f32).unwrap(),
                start + work + down,
                start + work + down - frame,
            );
        }
        let target = clock.target(&mut clock.intervals()).unwrap();
        assert!((target - (tick + work + down - frame)).abs() < f64::EPSILON);
        let last_missing_read = 8.0 * tick + tick + work + down - frame;
        clock.advance(last_missing_read, frame, tick, &timing, timing.admission_s);
        assert!(
            !clock.held,
            "a steady link must cover the last read before the next keyframe completes"
        );
        // Fixture: one server computation lasts a second; departure still bounds the same link.
        clock.sample(
            Tick(9),
            TickStamp::new(9.0 * tick, 1.0).unwrap(),
            9.0 * tick + 1.0 + down,
            9.0 * tick + 1.0 + down - frame,
        );
        assert!(
            (clock.target(&mut clock.intervals()).unwrap() - (tick + 1.0 + down - frame)).abs()
                < f64::EPSILON,
            "server work and network lateness have the same window treatment"
        );
    }
}

#[cfg(test)]
mod pacing;
#[cfg(test)]
mod udp;
