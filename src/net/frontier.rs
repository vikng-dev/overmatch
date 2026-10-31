//! The game's damage model and netcode: read-only interpolation-buffer starvation diagnostics.
//!
//! A cursor past the newest confirmed position is a starved frame; a maximal run is one gap.
//! The instrument counts gaps and their authority-tick spans, correlates them with received fire
//! and damage facts, and counts timeline resyncs. It never writes a pose or advances gameplay.
//! Lightyear's interpolation sample, including its newest-sample hold during starvation, presents
//! unchanged. See `.agents/docs/design/net-motion-contract.md` for the motion contract.

use std::collections::VecDeque;

use avian3d::prelude::Position;
use bevy::app::AppExit;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::core::tick::Tick;
use lightyear::core::time::TickInstant;
use lightyear::interpolation::plugin::InterpolationSystems;
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::{Connected, Interpolated, NetworkTimeline, SyncSystems};

use super::protocol::NetTank;

/// ASSUMED diagnostic cadence in seconds: an operator sees the counters roughly twice a minute.
const SUMMARY_PERIOD_SECS: f32 = 30.0;

/// Mount the read-only edge and resync instruments.
pub(super) fn install(app: &mut App) {
    app.init_resource::<ImpulseTicks>();
    app.init_resource::<FrontierDiag>();
    app.init_resource::<HullEdges>();
    app.init_resource::<PreSyncCursor>();
    app.add_observer(allow_handshake_resync);
    app.add_observer(log_summary_on_disconnect);
    app.add_systems(
        PostUpdate,
        (
            note_pre_sync_cursor.before(SyncSystems::Sync),
            count_interp_resyncs.after(SyncSystems::Sync),
        ),
    );
    app.add_systems(
        Update,
        (
            drive_hull_edges.after(InterpolationSystems::Interpolate),
            log_periodic_summary,
        ),
    );
    app.add_systems(Last, log_summary_on_exit);
}

// --- Impulse ledger --------------------------------------------------------------------------

/// Which class of authority fact stamped an impulse tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImpulseClass {
    /// A fire announcement (`FireEvent::fire_tick`) — the shooter's hull surges on this tick.
    Fire,
    /// An owner-private damage confirm (`DamageConfirm::damage_tick`) — the struck hull receives
    /// the shot's impulse on this tick.
    Damage,
}

/// Ring of recent impulse-class authority ticks, consulted when a starvation gap closes.
///
/// ASSUMED memory budget: retain the most recent 256 distinct tick/class facts. Eviction can
/// remove older coincidences during a long gap, so the reported coincidence count is a lower
/// bound, not a completeness claim.
#[derive(Resource, Default, Debug)]
pub(super) struct ImpulseTicks {
    ring: VecDeque<(Tick, ImpulseClass)>,
}

impl ImpulseTicks {
    const CAP: usize = 256;

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    pub(super) fn record(&mut self, tick: Tick, class: ImpulseClass) {
        if self.ring.iter().any(|entry| *entry == (tick, class)) {
            return;
        }
        self.ring.push_back((tick, class));
        if self.ring.len() > Self::CAP {
            self.ring.pop_front();
        }
    }

    /// Whether any recorded impulse tick lies in the half-open gap span `(floor, ceil]` —
    /// lightyear's signed tick difference (monotonic ticks, saturating to the `i32` range).
    fn any_within(&self, floor: Tick, ceil: Tick) -> bool {
        self.ring
            .iter()
            .any(|(tick, _)| *tick - floor > 0 && *tick - ceil <= 0)
    }
}

// --- Diagnostics -----------------------------------------------------------------------------

/// The frontier counters. All hull-gap counts are per hull per episode (three hulls starving on
/// one shared stall count three gaps — the unit every line names).
#[derive(Resource, Default, Debug)]
pub(super) struct FrontierDiag {
    /// Closed gap episodes.
    gaps: u64,
    /// Gaps with at least one impulse-class tick inside their span.
    coincident: u64,
    /// Frames on which at least one hull was starved.
    starved_frames: u64,
    /// Sum of closed-gap durations, in ticks (each gap's maximum cursor overrun).
    gap_ticks_total: f64,
    /// The single longest closed gap, in ticks.
    gap_ticks_max: f64,
    /// Interpolation-timeline resync count, handshakes included.
    sync_events: u64,
    /// Resyncs past each connection's handshake — steady-state resyncs, which must not happen.
    steady_resyncs: u64,
    /// Whether this connection's one handshake resync has been seen. Lightyear resets the
    /// interpolation timeline on every `Connected`, so each (re)connection is owed one; cleared by
    /// [`allow_handshake_resync`].
    handshake_spent: bool,
}

impl FrontierDiag {
    fn steady_sync_events(&self) -> u64 {
        self.steady_resyncs
    }

    fn summary(&self, open_gaps: usize) -> String {
        format!(
            "hull-gaps={} (coincident={} open={}) starved_frames={} \
             gap_ticks(total={:.1} max={:.2}) sync_events={} (steady={})",
            self.gaps,
            self.coincident,
            open_gaps,
            self.starved_frames,
            self.gap_ticks_total,
            self.gap_ticks_max,
            self.sync_events,
            self.steady_sync_events(),
        )
    }
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

/// Count interpolation-timeline resyncs. Steady state after the handshake must be zero — a
/// nonzero steady count is every remote hull jumping, and the periodic summary flags it.
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
    diag.steady_resyncs += 1;
    warn!(
        "net: FRONTIER interpolation resync #{} — a steady-state resync snapped every remote hull",
        diag.sync_events
    );
}

/// The estimator and own-fire digests riding the FRONTIER line — each absent in worlds that never
/// mounted its module (unit fixtures, the server).
fn sibling_digests(
    arrival: &Option<Res<super::sync_margin::ArrivalDelay>>,
    own_fire: &Option<Res<super::fire_presentation::OwnFireDiag>>,
) -> String {
    let mut digest = String::new();
    if let Some(estimator) = arrival {
        digest.push(' ');
        digest.push_str(&estimator.describe());
    }
    if let Some(own_fire) = own_fire {
        digest.push(' ');
        digest.push_str(&own_fire.describe());
    }
    digest
}

fn log_periodic_summary(
    time: Res<Time<Real>>,
    diag: Res<FrontierDiag>,
    edges: Res<HullEdges>,
    arrival: Option<Res<super::sync_margin::ArrivalDelay>>,
    own_fire: Option<Res<super::fire_presentation::OwnFireDiag>>,
    mut elapsed: Local<f32>,
) {
    *elapsed += time.delta_secs();
    if *elapsed < SUMMARY_PERIOD_SECS {
        return;
    }
    *elapsed = 0.0;
    info!(
        "net: FRONTIER {}{}",
        diag.summary(edges.open_gaps()),
        sibling_digests(&arrival, &own_fire)
    );
}

fn log_summary_on_disconnect(
    _disconnected: On<Remove, Connected>,
    diag: Res<FrontierDiag>,
    edges: Res<HullEdges>,
    arrival: Option<Res<super::sync_margin::ArrivalDelay>>,
    own_fire: Option<Res<super::fire_presentation::OwnFireDiag>>,
) {
    info!(
        "net: FRONTIER (disconnect) {}{}",
        diag.summary(edges.open_gaps()),
        sibling_digests(&arrival, &own_fire)
    );
}

fn log_summary_on_exit(
    mut exits: MessageReader<AppExit>,
    diag: Res<FrontierDiag>,
    edges: Res<HullEdges>,
    arrival: Option<Res<super::sync_margin::ArrivalDelay>>,
    own_fire: Option<Res<super::fire_presentation::OwnFireDiag>>,
    mut logged: Local<bool>,
) {
    if *logged || exits.read().next().is_none() {
        return;
    }
    *logged = true;
    info!(
        "net: FRONTIER (final) {}{}",
        diag.summary(edges.open_gaps()),
        sibling_digests(&arrival, &own_fire)
    );
}

/// A starvation episode's first confirmed tick and longest cursor overrun, in ticks.
#[derive(Debug)]
struct Gap {
    floor: Tick,
    max_ticks: f64,
}

#[derive(Debug, Default)]
struct HullEdge {
    gap: Option<Gap>,
    /// Mark-and-sweep stamp; entries missing this frame belong to despawned hulls.
    stamp: u64,
}

impl HullEdge {
    fn step(&mut self, cursor: (Tick, f64), newest: Tick) -> Option<Gap> {
        let gap_ticks = f64::from(cursor.0 - newest) + cursor.1;
        if gap_ticks > 0.0 {
            let gap = self.gap.get_or_insert(Gap {
                floor: newest,
                max_ticks: gap_ticks,
            });
            gap.max_ticks = gap.max_ticks.max(gap_ticks);
            None
        } else {
            self.gap.take()
        }
    }
}

/// Per-hull trackers stay in a resource so diagnostics cannot change a replica's archetype.
#[derive(Resource, Default, Debug)]
struct HullEdges {
    map: HashMap<Entity, HullEdge>,
    frame: u64,
}

impl HullEdges {
    fn open_gaps(&self) -> usize {
        self.map.values().filter(|edge| edge.gap.is_some()).count()
    }
}

fn drive_hull_edges(
    cursor: Option<Res<InterpolationTimeline>>,
    hulls: Query<(Entity, &ConfirmedHistory<Position>), (With<NetTank>, With<Interpolated>)>,
    mut edges: ResMut<HullEdges>,
    impulses: Res<ImpulseTicks>,
    mut diag: ResMut<FrontierDiag>,
) {
    let Some(timeline) = cursor.filter(|timeline| timeline.is_synced()) else {
        // Before synchronization, or between links, no starvation span is defined.
        edges.map.clear();
        return;
    };
    let cursor = (timeline.tick(), f64::from(timeline.overstep().to_f32()));
    edges.frame += 1;
    let frame = edges.frame;
    let mut starved_this_frame = false;
    for (entity, history) in &hulls {
        let Some((newest, _)) = history.newest_present() else {
            // A newly replicated hull may not have received its first history sample yet.
            continue;
        };
        let edge = edges.map.entry(entity).or_default();
        edge.stamp = frame;
        starved_this_frame |= f64::from(cursor.0 - newest) + cursor.1 > 0.0;
        if let Some(gap) = edge.step(cursor, newest) {
            let coincident = impulses.any_within(gap.floor, cursor.0);
            diag.gaps += 1;
            diag.gap_ticks_total += gap.max_ticks;
            diag.gap_ticks_max = diag.gap_ticks_max.max(gap.max_ticks);
            if coincident {
                diag.coincident += 1;
            }
            info!(
                "net: FRONTIER gap closed {entity} span=({},{}] gap={:.2}t impulse={}",
                gap.floor.0, cursor.0.0, gap.max_ticks, coincident
            );
        }
    }
    if starved_this_frame {
        diag.starved_frames += 1;
    }
    edges.map.retain(|_, edge| edge.stamp == frame);
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::prelude::{AngularVelocity, LinearVelocity, Rotation};
    use bevy::ecs::system::RunSystemOnce;
    use lightyear::prelude::TimelineSync;

    fn edge_world() -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<ImpulseTicks>();
        world.init_resource::<FrontierDiag>();
        world.init_resource::<HullEdges>();
        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(Tick(103)));
        timeline.set_synced(true);
        world.insert_resource(timeline);
        let mut history = ConfirmedHistory::<Position>::default();
        history.insert_present(Tick(100), Position(Vec3::new(10.0, 0.0, -4.0)));
        let hull = world
            .spawn((
                NetTank,
                Interpolated,
                history,
                Position(Vec3::new(10.0, 0.0, -4.0)),
                Rotation(Quat::IDENTITY),
                LinearVelocity(Vec3::new(500.0, 20.0, 10.0)),
                AngularVelocity(Vec3::splat(50.0)),
            ))
            .id();
        (world, hull)
    }

    /// The server-stream consumer relies on starvation diagnostics leaving its pose untouched,
    /// independently of velocity or unknown future impulses. Fixture velocities deliberately
    /// exceed ordinary driving; a restored projection or blend would fail both pose checks.
    #[test]
    fn starvation_and_fresh_impulse_samples_never_rewrite_the_presented_pose() {
        let (mut world, hull) = edge_world();
        world.run_system_once(drive_hull_edges).unwrap();
        assert_eq!(
            world.get::<Position>(hull).unwrap().0,
            Vec3::new(10.0, 0.0, -4.0)
        );
        assert_eq!(world.get::<Rotation>(hull).unwrap().0, Quat::IDENTITY);
        assert_eq!(world.resource::<FrontierDiag>().starved_frames, 1);
        assert_eq!(world.resource::<HullEdges>().open_gaps(), 1);
        // Fixture incoming authority sample includes an impulse. Model the pose written by the
        // interpolation system before the instrument, then assert there is no corrective blend.
        let fresh = Vec3::new(-50.0, 80.0, 30.0);
        let rotation = Quat::from_rotation_y(1.0);
        world
            .get_mut::<ConfirmedHistory<Position>>(hull)
            .unwrap()
            .insert_present(Tick(105), Position(fresh));
        world.get_mut::<Position>(hull).unwrap().0 = fresh;
        world.get_mut::<Rotation>(hull).unwrap().0 = rotation;
        world
            .resource_mut::<ImpulseTicks>()
            .record(Tick(102), ImpulseClass::Fire);
        world.run_system_once(drive_hull_edges).unwrap();
        assert_eq!(world.get::<Position>(hull).unwrap().0, fresh);
        assert_eq!(world.get::<Rotation>(hull).unwrap().0, rotation);
        let diag = world.resource::<FrontierDiag>();
        assert_eq!((diag.gaps, diag.coincident, diag.starved_frames), (1, 1, 1));
        assert_eq!(diag.gap_ticks_max, 3.0);
        assert_eq!(world.resource::<HullEdges>().open_gaps(), 0);
    }

    /// Diagnostics must not carry gaps across a missing cursor or a despawned hull. A new link
    /// starts a new observation span; neither absence is evidence of a completed starvation gap.
    #[test]
    fn leaving_or_despawning_discards_open_observations() {
        let (mut world, hull) = edge_world();
        world.run_system_once(drive_hull_edges).unwrap();
        world.despawn(hull);
        world.run_system_once(drive_hull_edges).unwrap();
        assert_eq!(world.resource::<HullEdges>().open_gaps(), 0);
        assert_eq!(world.resource::<FrontierDiag>().gaps, 0);
        world.remove_resource::<InterpolationTimeline>();
        world.run_system_once(drive_hull_edges).unwrap();
        assert_eq!(world.resource::<FrontierDiag>().gaps, 0);
    }
}
