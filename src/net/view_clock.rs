//! Received server-time record for the future presentation clock. Nothing here drives a timeline.
//! `x = previous frame Real - stamp` retains the unknown epoch offset; only differences of lags
//! are comparable to local durations. Tick labels resolve through Lightyear's checkpoint map.

use std::collections::{BTreeMap, HashMap};

use bevy::prelude::*;
use bevy_replicon::client::confirm_history::EntityReplicated;
use bevy_replicon::shared::replication::storage::ReplicationStorage;
use lightyear::core::time::TickInstant;
use lightyear::prelude::ReplicationCheckpointMap;
use lightyear::prelude::{Connected, ReplicationSystems, Tick};
use serde_json::{Value, json};

use super::tick_stamp::ReceivedStamps;
use super::timing::{Timing, histogram};

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
    storage.global.get_or_default::<ReceivedStamps>().0.clear();
}

struct Sample {
    stamp_s: f64,
    x_s: f64,
    seen_s: f64,
}

#[derive(Default)]
struct Application {
    opened_s: f64,
    stamp: Option<(u64, f64)>,
    entities: HashMap<Entity, (u64, f64)>,
    late: bool,
}

#[derive(Default, serde::Serialize)]
struct AtomicLine {
    stamped_ticks: u64,
    entity_keyframes: u64,
    late_ticks: u64,
    worst_lag_frames: u64,
    worst_lag_ms: f64,
    expired_without_stamp: u64,
    unmapped_stamps: u64,
}

#[derive(Resource, Default)]
pub(super) struct ViewClock {
    frame: u64,
    samples: BTreeMap<Tick, Sample>,
    applications: BTreeMap<u32, Application>,
    atomic: AtomicLine,
}

impl ViewClock {
    fn sample(&mut self, tick: Tick, stamp_s: f64, now_s: f64, previous_s: f64) {
        self.samples.entry(tick).or_insert(Sample {
            stamp_s,
            x_s: previous_s - stamp_s,
            seen_s: now_s,
        });
    }

    /// Duplicate entity receipts never count as a first keyframe. Compare after the complete
    /// receive pass, so the order of the stamp entity and other entities inside it cannot matter.
    fn apply(&mut self, checkpoint: u32, entity: Option<Entity>, now_s: f64) {
        let application = self
            .applications
            .entry(checkpoint)
            .or_insert_with(|| Application {
                opened_s: now_s,
                ..default()
            });
        let first = if let Some(entity) = entity {
            match application.entities.entry(entity) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert((self.frame, now_s));
                    self.atomic.entity_keyframes += 1;
                    Some((self.frame, now_s))
                }
                std::collections::hash_map::Entry::Occupied(_) => return,
            }
        } else if application.stamp.is_none() {
            application.stamp = Some((self.frame, now_s));
            self.atomic.stamped_ticks += 1;
            None
        } else {
            return; // A retransmitted stamp is not a new first arrival.
        };
        if let Some((stamp_frame, stamp_seen_s)) = application.stamp {
            // Entity arrivals before the stamp cannot be late. When the stamp arrives, the
            // maximum first-entity frame suffices; later entities are compared independently.
            let latest = first.or_else(|| {
                application
                    .entities
                    .values()
                    .copied()
                    .max_by_key(|&(frame, _)| frame)
            });
            if let Some((entity_frame, entity_seen_s)) = latest
                && entity_frame > stamp_frame
            {
                if !application.late {
                    self.atomic.late_ticks += 1;
                    application.late = true;
                }
                self.atomic.worst_lag_frames =
                    self.atomic.worst_lag_frames.max(entity_frame - stamp_frame);
                self.atomic.worst_lag_ms = self
                    .atomic
                    .worst_lag_ms
                    .max((entity_seen_s - stamp_seen_s) * 1e3);
            }
        }
    }

    fn expire(&mut self, now_s: f64, window_s: f64) {
        self.samples
            .retain(|_, sample| now_s - sample.seen_s <= window_s);
        self.applications.retain(|_, application| {
            let keep = now_s - application.opened_s <= window_s;
            if !keep && application.stamp.is_none() {
                self.atomic.expired_without_stamp += 1;
            }
            keep
        });
    }

    /// Neighbouring received stamps in authoritative tick order. A lost tick leaves its whole
    /// production gap in the distribution; neither nominal intervals nor missing stamps are invented.
    fn intervals(&self) -> Vec<f64> {
        let mut intervals = Vec::new();
        let mut previous: Option<&Sample> = None;
        for sample in self.samples.values() {
            if let Some(before) = previous {
                intervals.push(sample.stamp_s - before.stamp_s);
            }
            previous = Some(sample);
        }
        intervals
    }

    fn target(&self, intervals: &mut [f64]) -> Option<f64> {
        if intervals.is_empty() {
            return None; // Fewer than two received ticks: the production interval is unknown.
        }
        intervals.sort_by(f64::total_cmp);
        // Nearest-rank median, matching the measured distribution in the log.
        let median = intervals[(intervals.len() - 1) / 2];
        self.samples
            .values()
            .map(|sample| sample.x_s)
            .max_by(f64::total_cmp)
            .map(|x| median + x)
    }

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
        let atomic = std::mem::take(&mut self.atomic);
        json!({
            "delta_star_ms": target.map(|s| s * 1e3),
            "today_stamp_lag_ms": today_lag.map(|s| s * 1e3),
            "target_minus_today_ms": target.zip(today_lag).map(|(target, today)| (target - today) * 1e3),
            "target_above_latest_x_ms": target.zip(latest_x).map(|(target, x)| (target - x) * 1e3),
            "stamp_interval_ms": histogram(&mut intervals, tick_s),
            "atomic": atomic,
            "unmatched_ticks": self.applications.values().filter(|application| application.stamp.is_none()).count(),
        })
    }
}

fn receive(
    real: Res<Time<Real>>,
    timing: Res<Timing>,
    checkpoints: Res<ReplicationCheckpointMap>,
    mut storage: ResMut<ReplicationStorage>,
    mut entities: MessageReader<EntityReplicated>,
    mut clock: ResMut<ViewClock>,
) {
    let now = real.elapsed_secs_f64();
    clock.frame += 1;
    clock.expire(now, timing.view_window_s);
    let received = &mut storage.global.get_or_default::<ReceivedStamps>().0;
    for (checkpoint, stamp) in received.drain(..) {
        clock.apply(checkpoint.get(), None, now);
        if let Some(tick) = checkpoints.get(checkpoint) {
            clock.sample(tick, stamp.0, now, now - real.delta_secs_f64());
        } else {
            // Lightyear retains a bounded checkpoint map; a large drain can evict a mapping.
            clock.atomic.unmapped_stamps += 1;
        }
    }
    for event in entities.read() {
        clock.apply(event.tick.get(), Some(event.entity), now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The view target must not charge a client's stalled drain frame to the network.
    #[test]
    fn x_uses_the_previous_frame_so_a_client_stall_adds_nothing() {
        use super::super::tick_stamp::TickStamp;
        use bevy::time::TimeUpdateStrategy;
        // Fixture: same last healthy frame, different drain-frame durations.
        let mut readings = Vec::new();
        for frame_s in [0.0625, 1.0] {
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .init_resource::<Timing>()
                .init_resource::<ReplicationStorage>()
                .init_resource::<ReplicationCheckpointMap>()
                .add_message::<EntityReplicated>()
                .insert_resource(TimeUpdateStrategy::ManualDuration(
                    std::time::Duration::from_secs_f64(0.0625),
                ));
            install(&mut app);
            app.update();
            app.update();
            let previous = app.world().resource::<Time<Real>>().elapsed_secs_f64();
            let checkpoint = bevy_replicon::prelude::RepliconTick::new(1);
            app.world_mut()
                .resource_mut::<ReplicationCheckpointMap>()
                .record(checkpoint, Tick(1));
            app.world_mut()
                .resource_mut::<ReplicationStorage>()
                .global
                .get_or_default::<ReceivedStamps>()
                .0
                .push((checkpoint, TickStamp(0.0)));
            app.insert_resource(TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_secs_f64(frame_s),
            ));
            app.update();
            let clock = app.world().resource::<ViewClock>();
            assert_eq!(clock.samples[&Tick(1)].x_s, previous);
            readings.push(clock.samples[&Tick(1)].x_s);
        }
        assert_eq!(readings[0], readings[1]);
    }

    /// A stall before the next server frame moves the stamp map rather than the network bound.
    #[test]
    fn a_server_stall_moves_stamps_without_raising_x() {
        // Fixture: equal downlink wait, regular intervals surrounding one server pause.
        let mut clock = ViewClock::default();
        for (tick, stamp) in [(1, 1.0), (2, 1.125), (3, 2.125), (4, 2.25), (5, 2.375)] {
            clock.sample(Tick(tick), stamp, stamp + 0.125, stamp + 0.0625);
        }
        assert!(clock.samples.values().all(|sample| sample.x_s == 0.0625));
        assert_eq!(clock.target(&mut clock.intervals()), Some(0.125 + 0.0625));
    }

    /// D3 measures the received stream's intervals; removing missing-tick pairs silently changes
    /// its median under loss and leaves the future view clock without that observation.
    #[test]
    fn stamp_intervals_include_the_gap_over_a_lost_tick() {
        let tick_s = super::super::server::TICK.as_secs_f64();
        let mut clock = ViewClock::default();
        // Fixture: the middle stamp is lost, with an unchanged absence bound on the other two.
        clock.sample(Tick(1), 0.0, 0.5, 0.25);
        clock.sample(
            Tick(3),
            2.0 * tick_s,
            0.5 + 2.0 * tick_s,
            0.25 + 2.0 * tick_s,
        );
        assert_eq!(clock.intervals(), [2.0 * tick_s]);
        assert_eq!(
            clock.target(&mut clock.intervals()),
            Some(0.25 + 2.0 * tick_s)
        );
    }

    /// D3 uses the window's worst absence bound and median production interval, with no RTT.
    #[test]
    fn target_forgets_expired_lateness_and_preserves_the_clock_offset() {
        let timing = Timing::shipped();
        let mut clock = ViewClock::default();
        // Fixture: exact binary fractions, a late oldest tick, and a constant epoch offset.
        for (tick, stamp, x) in [(1, 1.0, 8.0), (2, 1.125, 4.0), (3, 1.25, 4.0)] {
            clock.sample(Tick(tick), stamp, f64::from(tick), stamp + x);
        }
        assert_eq!(clock.target(&mut clock.intervals()), Some(8.125));
        clock.expire(1.0 + timing.view_window_s + 0.5, timing.view_window_s);
        assert_eq!(clock.target(&mut clock.intervals()), Some(4.125));
    }

    /// A split replication update under-reads D2. Count each late tick once, ignore duplicates,
    /// retain open ticks across summary boundaries, and do not mistake a lost stamp for atomicity.
    #[test]
    fn atomic_check_observes_every_entity_first_arrival_across_lines() {
        let mut clock = ViewClock::default();
        // Fixture: three entity identities and frames, one split tick and one stamp arriving last.
        let (a, b, c) = (
            Entity::from_bits(1),
            Entity::from_bits(2),
            Entity::from_bits(3),
        );
        clock.apply(1, Some(a), 1.0);
        clock.apply(1, None, 1.0);
        clock.frame += 1;
        clock.apply(1, Some(a), 1.1);
        clock.apply(1, Some(b), 1.1);
        assert_eq!(clock.atomic.late_ticks, 1);
        assert_eq!(clock.atomic.worst_lag_frames, 1);
        clock.take_line(
            1.1,
            super::super::server::TICK.as_secs_f64(),
            TickInstant::from(Tick(1)),
        );
        clock.frame += 1;
        clock.apply(1, Some(c), 1.2);
        assert_eq!(clock.atomic.late_ticks, 0);
        assert_eq!(clock.atomic.worst_lag_frames, 2);
        clock.apply(2, Some(a), 1.2);
        clock.frame += 1;
        clock.apply(2, None, 1.3);
        assert_eq!(clock.atomic.late_ticks, 0);
        clock.apply(3, Some(a), 1.3);
        let timing = Timing::shipped();
        clock.expire(2.0 + timing.view_window_s, timing.view_window_s);
        assert_eq!(clock.atomic.expired_without_stamp, 1);
    }
}
