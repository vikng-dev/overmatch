//! The dedicated server's stats line, armed by `OVERMATCH_SERVER_STATS_S`: once per period, one
//! line on stdout, `server_stats {json}`, for a host whose only window into the process is its
//! stdout (a container's logs). `println!` rather than the log macros, so the line reaches stdout
//! whatever `RUST_LOG` filters.
//!
//! The lever's value is a positive finite period in seconds; unset is off, the one off form (every
//! tracked runner leaves it unset for off: `scripts/edgegap/session.sh`, `scripts/perf/`). Any
//! other value refuses startup.
//!
//! JSON fields:
//! - `period_s`: the wall time the line covers, which can exceed the period by one frame;
//! - `ticks`, `tick_hz`: fixed ticks run in the period, and per wall second;
//! - `tick_us`: nearest-rank `p50`, `p95`, `p99`, `max` and summed `total` wall micros,
//!   null when no tick ran;
//! - `frame_us`: First through Last, including fixed ticks and their collectors, Update and
//!   PostUpdate; excludes runner pacing, startup and the stats-line collector. `frames` counts
//!   these samples. `post_update_us` is nested in this span; both use the tick summary shape;
//! - `linux_host`: Linux only, host steal ticks, process minor/major faults and CPU by main/compute
//!   thread since the previous line. `host_probe_us` inside it times the complete periodic probe,
//!   outside `frame_us`. Every reading is an `Ok` value or an `Err` with a named
//!   typed absence. The host's THP mode and CPU topology log once when this recorder starts;
//! - `overruns`: ticks longer than the fixed timestep, `budget_us`;
//! - `inputs`: per link, `tick_exact` and `held` counts of the input snapshots the bridge reads,
//!   `last_for_tick_offset` (`for_tick - server_tick`), `min_buffer_lookahead` and
//!   `max_buffer_lookahead` (the input buffer's newest remote tick − server tick at those reads),
//!   the ready messages Lightyear's receive gate `accepted` or dropped (`lookahead_rejected` past
//!   its future bound, `past_rejected` past its past bound), and `min_message_lookahead` and
//!   `max_message_lookahead` (`end_tick - server_tick`) observed before the gate. Offsets are
//!   signed ticks; null means no matching tick/message in this period. Counts reset per row;
//! - `wall`: the runner's `discarded_ms` (wall time past the admission cap dropped this period)
//!   and `discarding_batches` (admitted batches that dropped any), and `drift_ms`, how far
//!   simulated time trails wall time since startup (`server_runner::WallLedger`);
//! - `clients` (connected links), `tanks`, `projectiles`, `entities`, sampled at the line;
//! - `cpu_pct`: the process's user + system CPU over the period, as % of one core;
//! - `rss_mb`: resident memory at the line;
//! - `physics_us`: the same summary of avian's step (its `PhysicsSchedule`, broad phase
//!   through finalize), one sample per fixed tick, null when none ran;
//! - `intake_us`, `structure_us`, `refit_us`: one sample per tick, including zero for no
//!   work. Intake converts shell/charge/contact inputs and casts deposit geometry; structure
//!   evaluates owner laws, prepares launches and commits effects; refit prepares collider changes
//!   and partitions carriers. Deferred ECS application and AABB updates land in `phases_us`.
//! - `contacts_us`: solver-contact readout and resting-contact cache maintenance, included in
//!   `physics_us`. Intake consumes that readout later in the tick.
//! - `publish_us`: construction-ledger diff commands prepared inside the tick.
//! - `sweep_us`: the hull sweep before physics: its shape casts, cuts, holds and plastic
//!   exchanges, and the departures it routes. The refit it triggers is `refit_us`.
//! - `crumble_us`: the landing crumble after the deposits: each moving group's landing read,
//!   judged by its kernel and routed. Per-tick `detail` carries the solve and split wall unions
//!   inside this leaf, and `belt_partition_us` inside `belt_contacts_us`. Different owners can
//!   overlap those spans; never add them to the tick's phases or leaves.
//! - `phases_us`: complete schedule/phase intervals after debiting the leaf brackets above.
//!   ECS application, synchronous hooks and dispatch stay in their phase; `physics_step_us`
//!   includes sleep. Add phases and leaves once; `physics_us` and `sleep_us` are nested detail.
//! - `phases_cpu_us` and `stages_cpu_us`: process CPU micros with exactly the wall maps' keys.
//!   They include every process thread running inside the edge, including background tasks.
//!   `tick_cpu_us` sums these exclusive shares; CPU/wall is average busy threads.
//! - `quota_used_pct`: process CPU as percent of the cgroup quota, or a typed absence.
//! - `one_thread_estimate_us`: the derived two-state estimate (see `one_thread_estimate`),
//!   summarized per tick. It does not measure actual single-thread wall time.
//! - `collector_us`: stats tick sampling after `TickClosed`, outside `tick_us`; includes the
//!   armed ram census. Clock-read/bracket cost remains inside the measured stages.
//! - `sleep_us`: Avian's island-sleep bracket, included in `physics_us`. Physics covers all
//!   bodies, including tanks: a shared solver cannot attribute its cost exclusively to loose pieces.
//! - `send_us`: the same summary of the outbound path, one sample per frame that replicated:
//!   the wall time from before replicon's `ServerSystems::Send` (the change scan and serialisation)
//!   to after lightyear's `LinkSystems::Send` (the socket flush), null when none ran. It is a
//!   bracket, so it also holds any unordered `PostUpdate` system the executor ran inside it;
//! - `throttled_usec`, `nr_throttled`: the container's CPU-quota throttling over the period, from
//!   the cgroup's `cpu.stat` (microseconds throttled, enforcement periods throttled); null off
//!   Linux or where the file is unreadable;
//! - `construction`: unique recorded joint cracks and separations, and original construction
//!   chunks departed. These are correctness counts, independent of time brackets.
//! - `debris`: the `live` (awake) and `sleeping` loose pieces at the line, a construction's and the
//!   debris stress lever's (`net::debris_stress`); while the lever is armed also its cumulative
//!   `breaches` and `released`.
//!
//! RSS and throttle fields are null where the platform gives no reading. A process CPU clock
//! failure visibly refuses the armed instrument rather than emitting an idle-core reading.
//!
//! The tick wall time is the cost recorder's measurement ([`crate::cost::TickClock`]), never a
//! second timer. `physics_us` sits inside it; `send_us` runs after the fixed loop, outside it.
//!
//! `loadtest`, present only for `scripts/perf/collapse.sh --schedule`, carries the recipe,
//! sequence, clock, construction outcomes, awake dynamic bodies, loose construction chunks and
//! attempted encrypted send bytes. Its `ticks` use the ram schema plus admission-relative
//! `elapsed_s`, `events_fired` and work counts; `sends` remain outside the fixed ticks.
//! `send_bytes` and `send_clients` also appear without a loadtest. Each connected link carries its
//! period bytes, bytes/s and `max_packets`, the most datagrams one frame's send pass queued for it;
//! `send_us_per_client_s` amortizes shared send work over links and wall
//! seconds. `send_census_us` times the byte collector separately and is excluded from `send_us`.
//! Bytes exclude IP/UDP headers and do not attest delivery.
//!
//! Ram and `loadtest` tick rows also carry `contacts`, the contact census ([`super::contact_census`]),
//! and each line carries `pieces`, the piece rest records that changed since the last line.

#[cfg(any(target_os = "linux", all(test, unix)))]
mod linux;

use bevy::app::MainScheduleOrder;
use bevy::ecs::schedule::{ScheduleLabel, SingleThreadedExecutor};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use avian3d::prelude::{PhysicsSchedule, PhysicsStepSystems, RigidBody, Sleeping};
use bevy::app::FixedMain;
use bevy::prelude::*;
use bevy::utils::Parallel;
use bevy_replicon::prelude::ServerSystems;
use bevy_replicon::server::server_tick::ServerTick;
use lightyear::input::input_message::InputMessage;
use lightyear::link::LinkSystems;
use lightyear::prelude::input::native::{ActionState, NativeBuffer, NativeStateSequence};
use lightyear::prelude::input::server::InputSystems;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{
    Connected, ConnectionSystems, ControlledBy, Link, LocalTimeline, MessageReceiver, Tick,
};
use serde_json::{Value, json};

use crate::command::{ParsedTankCommand, TankCommand};

use super::contact_census::PieceRecords;
use super::debris_stress::DebrisCensus;
use super::harness::{LeverError, env_text};
use super::loadtest::Loadtest;
use super::server_host::{QuotaAbsence, Throttle, quota_cpus, read_throttle};
use super::sync_margin::MAX_INPUT_LOOKAHEAD_TICKS;
use crate::ballistics::Projectile;
use crate::construction::{
    Body,
    breakage::{Breakage, Piece, Record},
};
use crate::cost::clock::process_cpu;
use crate::cost::{
    TickClock, TickClosed, arm_tick_clock,
    phases::{self, Phases},
    stages::{COUNT, NAMES, Stages},
};
use crate::damage_cam::recorder::ShotRecorder;
use crate::scatter::MapInstance;
use crate::tank::Tank;

/// The lever: the period in seconds.
const LEVER: &str = "OVERMATCH_SERVER_STATS_S";

/// The line's grep anchor.
const PREFIX: &str = "server_stats";

/// Fact: lightyear_inputs 0.30.1 src/server.rs:68 sets its private past-input gate to 64;
/// receive_input_message applies it at :370 beside the lookahead gate, before any buffer write.
const MAX_INPUT_PAST_TICKS: i32 = 64;

#[derive(Default, serde::Serialize)]
struct InputCounts {
    tick_exact: u64,
    held: u64,
    last_for_tick_offset: Option<i32>,
    min_buffer_lookahead: Option<i32>,
    max_buffer_lookahead: Option<i32>,
    accepted: u64,
    lookahead_rejected: u64,
    past_rejected: u64,
    min_message_lookahead: Option<i32>,
    max_message_lookahead: Option<i32>,
}

/// Fold `value` into a period's signed minimum and maximum.
fn widen(min: &mut Option<i32>, max: &mut Option<i32>, value: i32) {
    *min = Some(min.map_or(value, |min| min.min(value)));
    *max = Some(max.map_or(value, |max| max.max(value)));
}

/// Census reduction only: independent input evaluation stays in the bridge. FixedPreUpdate
/// has applied Lightyear's snapshot; no ActionState writer intervenes before InputBridge.
fn count_input_use(
    timeline: Res<LocalTimeline>,
    tanks: Query<
        (
            &ActionState<TankCommand>,
            &ControlledBy,
            Option<&NativeBuffer<TankCommand>>,
        ),
        (With<TankCommand>, With<ParsedTankCommand>),
    >,
    mut stats: ResMut<ServerStats>,
) {
    for (action, owner, buffer) in &tanks {
        let counts = stats.input_by_client.entry(owner.owner).or_default();
        let offset = Tick(action.0.for_tick) - timeline.tick();
        if offset == 0 {
            counts.tick_exact += 1;
        } else {
            counts.held += 1;
        }
        counts.last_for_tick_offset = Some(offset);
        // Lightyear inserts the buffer with the client's first input message, which also sets
        // its newest remote tick; until then the tank has no buffer to read.
        if let Some(newest) = buffer.and_then(|buffer| buffer.last_remote_tick) {
            widen(
                &mut counts.min_buffer_lookahead,
                &mut counts.max_buffer_lookahead,
                newest - timeline.tick(),
            );
        }
    }
}

/// One link's ready input messages this frame, against Lightyear's receive gate.
#[derive(Default)]
struct MessageCensus {
    accepted: u64,
    lookahead_rejected: u64,
    past_rejected: u64,
    min: Option<i32>,
    max: Option<i32>,
}

/// Lightyear drains these messages in ReceiveInputs. Retaining every message observes the gate
/// without consuming, rewriting or refusing anything here (0.30.1 receive.rs:171).
fn count_input_lookahead(
    timeline: Res<LocalTimeline>,
    mut receivers: Query<
        (
            Entity,
            &mut MessageReceiver<InputMessage<NativeStateSequence<TankCommand>>>,
        ),
        (With<ClientOf>, With<Connected>),
    >,
    mut stats: ResMut<ServerStats>,
    mut worker_counts: Local<Parallel<Vec<(Entity, MessageCensus)>>>,
) {
    let tick = timeline.tick();
    let worker_counts = &mut *worker_counts;
    // Each link owns its queue; integer sums and extrema commute at the census boundary.
    receivers.par_iter_mut().for_each(|(link, mut receiver)| {
        let mut census = MessageCensus::default();
        receiver.retain_messages(|message| {
            let offset = message.end_tick - tick;
            widen(&mut census.min, &mut census.max, offset);
            if offset > MAX_INPUT_LOOKAHEAD_TICKS {
                census.lookahead_rejected += 1;
            } else if offset < -MAX_INPUT_PAST_TICKS {
                census.past_rejected += 1;
            } else {
                census.accepted += 1;
            }
            true
        });
        // A link with no ready message contributes no reading this frame.
        if census.max.is_some() {
            worker_counts.scope(|counts| counts.push((link, census)));
        }
    });
    for counts in worker_counts.iter_mut() {
        for (link, census) in counts.drain(..) {
            let counts = stats.input_by_client.entry(link).or_default();
            counts.accepted += census.accepted;
            counts.lookahead_rejected += census.lookahead_rejected;
            counts.past_rejected += census.past_rejected;
            for offset in census.min.into_iter().chain(census.max) {
                widen(
                    &mut counts.min_message_lookahead,
                    &mut counts.max_message_lookahead,
                    offset,
                );
            }
        }
    }
}

/// Arm the stats line when the lever is set.
pub(super) fn install(app: &mut App) -> Result<(), LeverError> {
    let period = period_from(env_text(LEVER)?.as_deref())?;
    super::ram::install(app, period.is_some())?;
    let Some(period) = period else {
        return Ok(());
    };
    info!(
        "server: stats line every {:.1} s on stdout ({LEVER})",
        period.as_secs_f64()
    );
    arm_tick_clock(app);
    #[cfg(target_os = "linux")]
    linux::log_startup();
    app.insert_resource(ServerStats {
        period: Period::new(period, Instant::now()),
        ticks: TickWindow::default(),
        physics: TickWindow::default(),
        sleep: TickWindow::default(),
        collector: TickWindow::default(),
        stages: std::array::from_fn(|_| TickWindow::default()),
        phases: std::array::from_fn(|_| TickWindow::default()),
        stages_cpu: std::array::from_fn(|_| TickWindow::default()),
        phases_cpu: std::array::from_fn(|_| TickWindow::default()),
        tick_cpu: TickWindow::default(),
        one_thread_estimate: TickWindow::default(),
        send: TickWindow::default(),
        frame: TickWindow::default(),
        post_update: TickWindow::default(),
        #[cfg(target_os = "linux")]
        host: linux::HostProbe::new(),
        send_census: TickWindow::default(),
        cpu_at_start: Some(process_cpu().unwrap_or_else(crate::refuse_start)),
        throttle_at_start: read_throttle(),
        husk_strikes_at_start: 0,
        discarded_at_start: (Duration::ZERO, 0),
        sequence: 0,
        send_bytes: 0,
        send_by_client: BTreeMap::new(),
        input_by_client: BTreeMap::new(),
    });
    app.add_systems(
        PreUpdate,
        count_input_lookahead.in_set(InputSystems::ValidateInputs),
    );
    app.add_systems(
        FixedUpdate,
        count_input_use.before(super::protocol::InputBridge),
    );
    app.init_resource::<PhaseStarts>();
    super::contact_census::install(app);
    phases::install(app);
    crate::cost::stages::install_lifecycle_counts(app);
    app.add_systems(FixedMain, record_tick.after(TickClosed));
    app.add_systems(
        PhysicsSchedule,
        (
            open_physics.in_set(PhysicsStepSystems::First),
            close_physics.in_set(PhysicsStepSystems::Last),
            open_sleep
                .after(super::contact_census::snapshot)
                .before(PhysicsStepSystems::Sleeping),
            close_sleep
                .after(PhysicsStepSystems::Sleeping)
                .before(PhysicsStepSystems::Finalize),
        ),
    );
    app.add_systems(
        PostUpdate,
        (
            open_send.before(ServerSystems::Send),
            close_send.after(LinkSystems::Send),
        )
            .run_if(resource_changed::<ServerTick>),
    );
    app.add_systems(
        PostUpdate,
        count_send_bytes
            .after(open_send)
            .after(ConnectionSystems::Send)
            .before(LinkSystems::Send),
    );
    install_iteration_clock(app);
    app.add_systems(IterationClock::Close, emit_line.after(close_iteration));
    Ok(())
}

/// Schedules outside the measured iteration contain only the recorder. The frame bracket
/// covers First through Last, including fixed ticks, their collectors, Update and PostUpdate.
/// Runner pacing/sleep, startup and the stats-line collector are outside it. PostUpdate is nested.
#[derive(ScheduleLabel, Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum IterationClock {
    Open,
    PostOpen,
    PostClose,
    Close,
}

fn install_iteration_clock(app: &mut App) {
    app.add_systems(IterationClock::Open, |mut starts: ResMut<PhaseStarts>| {
        starts.frame = Some(Instant::now());
    })
    .add_systems(
        IterationClock::PostOpen,
        |mut starts: ResMut<PhaseStarts>| {
            starts.post_update = Some(Instant::now());
        },
    )
    .add_systems(
        IterationClock::PostClose,
        |mut starts: ResMut<PhaseStarts>, mut stats: ResMut<ServerStats>| {
            stats.post_update.push(
                starts
                    .post_update
                    .take()
                    .expect("PostUpdate opens before it closes")
                    .elapsed()
                    .as_secs_f64()
                    * 1e6,
            );
        },
    )
    .add_systems(IterationClock::Close, close_iteration);
    for label in [
        IterationClock::Open,
        IterationClock::PostOpen,
        IterationClock::PostClose,
        IterationClock::Close,
    ] {
        app.edit_schedule(label, |schedule| {
            schedule.set_executor(SingleThreadedExecutor::new());
        });
    }
    let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
    order.insert_before(First, IterationClock::Open);
    order.insert_before(PostUpdate, IterationClock::PostOpen);
    order.insert_after(PostUpdate, IterationClock::PostClose);
    order.insert_after(Last, IterationClock::Close);
}

fn close_iteration(mut starts: ResMut<PhaseStarts>, mut stats: ResMut<ServerStats>) {
    stats.frame.push(
        starts
            .frame
            .take()
            .expect("First opens the iteration before Last closes it")
            .elapsed()
            .as_secs_f64()
            * 1e6,
    );
}

/// The lever's value as a period; `None` is off.
pub(super) fn period_from(value: Option<&str>) -> Result<Option<Duration>, LeverError> {
    let Some(value) = value.map(str::trim) else {
        return Ok(None);
    };
    let malformed = || {
        LeverError::invalid(
            LEVER,
            value,
            "positive finite seconds representable as a nonzero Duration (unset is off)",
        )
    };
    let secs = value.parse::<f64>().map_err(|_| malformed())?;
    let period = Duration::try_from_secs_f64(secs).map_err(|_| malformed())?;
    if period.is_zero() {
        return Err(malformed());
    }
    Ok(Some(period))
}

#[derive(Resource)]
struct ServerStats {
    period: Period,
    ticks: TickWindow,
    /// The period's avian steps, wall micros.
    physics: TickWindow,
    sleep: TickWindow,
    collector: TickWindow,
    stages: [TickWindow; COUNT],
    phases: [TickWindow; phases::COUNT],
    stages_cpu: [TickWindow; COUNT],
    phases_cpu: [TickWindow; phases::COUNT],
    tick_cpu: TickWindow,
    one_thread_estimate: TickWindow,
    /// The period's outbound brackets, wall micros.
    send: TickWindow,
    frame: TickWindow,
    post_update: TickWindow,
    #[cfg(target_os = "linux")]
    host: linux::HostProbe,
    send_census: TickWindow,
    /// The process's CPU time when the current period opened.
    cpu_at_start: Option<Duration>,
    /// The cgroup's throttle counters when the current period opened.
    throttle_at_start: Option<Throttle>,
    /// The recorder's husk-strike count when the current period opened.
    husk_strikes_at_start: u64,
    /// The runner's dropped wall time and batches when the current period opened.
    discarded_at_start: (Duration, u64),
    sequence: u64,
    send_bytes: u64,
    send_by_client: BTreeMap<Entity, SendCounts>,
    input_by_client: BTreeMap<Entity, InputCounts>,
}

/// One link's queued sends over a period.
#[derive(Default)]
struct SendCounts {
    bytes: u64,
    /// The most datagrams one frame's send pass queued for the link.
    max_packets: u64,
}

/// The open phase brackets' starts.
#[derive(Resource, Default)]
struct PhaseStarts {
    physics: Option<Instant>,
    sleep: Option<Instant>,
    physics_us: f64,
    sleep_us: f64,
    send: Option<Instant>,
    frame: Option<Instant>,
    post_update: Option<Instant>,
    send_census_us: f64,
}

/// The wall-clock period gate.
struct Period {
    length: Duration,
    started: Instant,
}

impl Period {
    fn new(length: Duration, now: Instant) -> Self {
        Self {
            length,
            started: now,
        }
    }

    /// The closed period's wall length once at least `length` has passed by `now`, opening the next
    /// period at `now`. The next period starts at the close, not at `started + length`, so a stalled
    /// frame yields one long period rather than a burst of back-to-back lines.
    fn close(&mut self, now: Instant) -> Option<Duration> {
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed < self.length {
            return None;
        }
        self.started = now;
        Some(elapsed)
    }
}

/// One period's wall-microsecond samples.
#[derive(Default)]
struct TickWindow(Vec<f64>);

/// A period's ticks, summarised.
#[derive(Debug, PartialEq)]
struct TickSummary {
    ticks: usize,
    p50: f64,
    p95: f64,
    p99: f64,
    total: f64,
    max: f64,
    /// Ticks strictly longer than the budget.
    overruns: usize,
}

impl TickWindow {
    fn push(&mut self, us: f64) {
        self.0.push(us);
    }

    /// Summarise the period and empty the window for the next one; `None` when no tick ran.
    /// Percentiles are nearest-rank, so each is a tick that was measured.
    fn drain(&mut self, budget_us: f64) -> Option<TickSummary> {
        if self.0.is_empty() {
            return None;
        }
        self.0.sort_by(f64::total_cmp);
        let sorted = &self.0;
        let rank = |p: f64| {
            let rank = (p * sorted.len() as f64).ceil() as usize;
            sorted[rank.clamp(1, sorted.len()) - 1]
        };
        let summary = TickSummary {
            ticks: sorted.len(),
            p50: rank(0.50),
            p95: rank(0.95),
            p99: rank(0.99),
            total: sorted.iter().sum(),
            max: sorted[sorted.len() - 1],
            overruns: sorted.iter().filter(|us| **us > budget_us).count(),
        };
        self.0.clear();
        Some(summary)
    }
}

/// The runner's wall-time accounts at the line ([`super::server_runner::WallLedger`]).
struct WallReading {
    /// Dropped in this period.
    discarded: Duration,
    discarding_batches: u64,
    /// Since startup.
    drift: Duration,
}

/// Everything one line reports.
struct Snapshot {
    elapsed: Duration,
    budget_us: f64,
    ticks: Option<TickSummary>,
    physics: Option<TickSummary>,
    sleep: Option<TickSummary>,
    collector: Option<TickSummary>,
    stages: [Option<TickSummary>; COUNT],
    phases: [Option<TickSummary>; phases::COUNT],
    stages_cpu: [Option<TickSummary>; COUNT],
    phases_cpu: [Option<TickSummary>; phases::COUNT],
    tick_cpu: Option<TickSummary>,
    one_thread_estimate: Option<TickSummary>,
    send: Option<TickSummary>,
    frame: Option<TickSummary>,
    post_update: Option<TickSummary>,
    #[cfg(target_os = "linux")]
    host: Value,
    send_census: Option<TickSummary>,
    send_bytes: u64,
    send_clients: Value,
    inputs: Value,
    debris: Value,
    clients: usize,
    tanks: usize,
    projectiles: usize,
    entities: usize,
    cpu_pct: Option<f64>,
    quota_used_pct: Result<f64, QuotaAbsence>,
    rss_bytes: Option<u64>,
    /// The period's throttling, when the cgroup reports it.
    throttled: Option<Throttle>,
    /// The period's rounds that struck a knocked-out tank (`ShotRecorder::husk_strikes`): a
    /// counter, never a line per strike, so automatic fire into husks cannot flood the log.
    husk_strikes: u64,
    wall: WallReading,
    workload: Option<Value>,
    construction: Value,
    ram: Option<Value>,
}

/// One decimal: the line is read by eye as well as parsed.
fn tenth(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// The line, `server_stats ` then one JSON object.
fn stats_line(snapshot: &Snapshot) -> String {
    let period_s = snapshot.elapsed.as_secs_f64();
    let ticks = snapshot.ticks.as_ref();
    let count = ticks.map_or(0, |summary| summary.ticks);
    let percentiles = |summary: Option<&TickSummary>| {
        summary.map_or(Value::Null, |summary| {
            json!({
                "p50": tenth(summary.p50),
                "p95": tenth(summary.p95),
                "p99": tenth(summary.p99),
                "total": tenth(summary.total),
                "max": tenth(summary.max),
            })
        })
    };
    let tick_us = percentiles(ticks);
    let mut body = json!({
        "period_s": period_s,
        "ticks": count,
        "tick_hz": tenth(count as f64 / period_s),
        "tick_us": tick_us,
        "frames": snapshot.frame.as_ref().map_or(0, |summary| summary.ticks),
        "overruns": ticks.map_or(0, |summary| summary.overruns),
        "budget_us": tenth(snapshot.budget_us),
        "clients": snapshot.clients,
        "tanks": snapshot.tanks,
        "projectiles": snapshot.projectiles,
        "entities": snapshot.entities,
        "cpu_pct": snapshot.cpu_pct.map(tenth),
        "quota_used_pct": snapshot.quota_used_pct,
        "tick_cpu_us": percentiles(snapshot.tick_cpu.as_ref()),
        "one_thread_estimate_us": percentiles(snapshot.one_thread_estimate.as_ref()),
        "rss_mb": snapshot.rss_bytes.map(|bytes| tenth(bytes as f64 / (1024.0 * 1024.0))),
        "physics_us": percentiles(snapshot.physics.as_ref()),
        "sleep_us": percentiles(snapshot.sleep.as_ref()),
        "collector_us": percentiles(snapshot.collector.as_ref()),
        "send_us": percentiles(snapshot.send.as_ref()),
        "frame_us": percentiles(snapshot.frame.as_ref()),
        "post_update_us": percentiles(snapshot.post_update.as_ref()),
        "send_census_us": percentiles(snapshot.send_census.as_ref()),
        "send_bytes": snapshot.send_bytes,
        "send_clients": snapshot.send_clients,
        "inputs": snapshot.inputs,
        // Amortized shared send work, not a separately timed per-link path.
        "send_us_per_client_s": snapshot.send.as_ref().filter(|_| snapshot.clients > 0).map(|s| tenth(s.total / period_s / snapshot.clients as f64)),
        "throttled_usec": snapshot.throttled.map(|t| t.throttled_usec),
        "nr_throttled": snapshot.throttled.map(|t| t.nr_throttled),
        "husk_strikes": snapshot.husk_strikes,
        "wall": {
            "discarded_ms": tenth(snapshot.wall.discarded.as_secs_f64() * 1e3),
            "discarding_batches": snapshot.wall.discarding_batches,
            "drift_ms": tenth(snapshot.wall.drift.as_secs_f64() * 1e3),
        },
    });
    for (name, summary) in NAMES.into_iter().zip(&snapshot.stages) {
        body[name] = percentiles(summary.as_ref());
    }
    body["phases_us"] = Value::Object(
        phases::NAMES
            .into_iter()
            .zip(&snapshot.phases)
            .map(|(name, summary)| (name.to_owned(), percentiles(summary.as_ref())))
            .collect(),
    );
    body["phases_cpu_us"] = Value::Object(
        phases::NAMES
            .into_iter()
            .zip(&snapshot.phases_cpu)
            .map(|(name, summary)| (name.to_owned(), percentiles(summary.as_ref())))
            .collect(),
    );
    body["stages_cpu_us"] = Value::Object(
        NAMES
            .into_iter()
            .zip(&snapshot.stages_cpu)
            .map(|(name, summary)| (name.to_owned(), percentiles(summary.as_ref())))
            .collect(),
    );
    #[cfg(target_os = "linux")]
    {
        body["linux_host"] = snapshot.host.clone();
    }
    body["debris"] = snapshot.debris.clone();
    body["construction"] = snapshot.construction.clone();
    if let Some(workload) = &snapshot.workload {
        body["loadtest"] = workload.clone();
    }
    if let Some(ram) = &snapshot.ram {
        body["ram"] = ram.clone();
    }
    format!("{PREFIX} {body}")
}

/// `FixedMain`, after all fixed schedules and their final flushes close.
fn record_tick(
    clock: Res<TickClock>,
    stages: Res<Stages>,
    phases: Res<Phases>,
    mut stats: ResMut<ServerStats>,
    starts: Res<PhaseStarts>,
    mut traces: (super::ram::Census, Option<ResMut<Loadtest>>),
    fixed: Res<Time<Fixed>>,
) {
    let (census, load) = &mut traces;
    if let Some(closed) = clock.closed() {
        let collect_start = Instant::now();
        stats.ticks.push(closed.us);
        let measured = stages.take_us();
        let cpu = stages.take_cpu_us();
        let phases_cpu = phases.cpu_times();
        let tick_cpu_us = cpu.values.iter().chain(&phases_cpu.values).sum();
        stats.tick_cpu.push(tick_cpu_us);
        stats
            .one_thread_estimate
            .push(one_thread_estimate(closed.us, tick_cpu_us));
        for (window, us) in stats.stages_cpu.iter_mut().zip(cpu.values) {
            window.push(us);
        }
        for (window, us) in stats.phases_cpu.iter_mut().zip(phases_cpu.values) {
            window.push(us);
        }
        // DERIVED: COUNT phases share COUNT + 1 CPU edges; leaves count their actual edges.
        stages.add(
            crate::cost::stages::Work::ProcessCpuReads,
            (phases::COUNT + 1) as u64,
        );
        stages.add(
            crate::cost::stages::Work::ContactPoints,
            census.contact_points() as u64,
        );
        let work = stages.take_counts();
        let mut sample = census.sample(
            closed.us,
            starts.physics_us,
            starts.sleep_us,
            (measured, cpu),
            &phases,
            fixed.timestep().as_secs_f64(),
        );
        sample.detail = stages.take_details();
        let recording = census.recording() || load.as_deref().is_some_and(Loadtest::recording);
        let rows = match recording.then(|| census.contact_tick()) {
            Some(contacts) => [
                census.record(sample, work, contacts.clone()),
                load.as_deref_mut()
                    .and_then(|load| load.record(sample, work, contacts)),
            ],
            None => [None, None],
        };
        for (window, us) in stats.stages.iter_mut().zip(measured) {
            window.push(us);
        }
        for (window, us) in stats.phases.iter_mut().zip(phases.times().values) {
            window.push(us);
        }
        let collector_us = collect_start.elapsed().as_secs_f64() * 1.0e6;
        stats.collector.push(collector_us);
        for row in rows.into_iter().flatten() {
            *row = collector_us;
        }
    }
}

fn open_physics(mut starts: ResMut<PhaseStarts>) {
    starts.physics = Some(Instant::now());
}

fn close_physics(mut starts: ResMut<PhaseStarts>, mut stats: ResMut<ServerStats>) {
    let start = starts
        .physics
        .take()
        .expect("the physics bracket opens in PhysicsStepSystems::First before it closes in Last");
    starts.physics_us = start.elapsed().as_secs_f64() * 1.0e6;
    stats.physics.push(starts.physics_us);
}

fn open_sleep(mut starts: ResMut<PhaseStarts>) {
    starts.sleep = Some(Instant::now());
}

fn close_sleep(mut starts: ResMut<PhaseStarts>, mut stats: ResMut<ServerStats>) {
    let start = starts
        .sleep
        .take()
        .expect("the physics sleep bracket opens before Sleeping");
    starts.sleep_us = start.elapsed().as_secs_f64() * 1.0e6;
    stats.sleep.push(starts.sleep_us);
}

fn open_send(mut starts: ResMut<PhaseStarts>) {
    starts.send = Some(Instant::now());
    starts.send_census_us = 0.0;
}

fn close_send(
    mut starts: ResMut<PhaseStarts>,
    mut stats: ResMut<ServerStats>,
    mut ram: Option<ResMut<super::ram::Ram>>,
    mut load: Option<ResMut<Loadtest>>,
) {
    let start = starts
        .send
        .take()
        .expect("the send bracket opens before ServerSystems::Send under the same ServerTick gate");
    let us = start.elapsed().as_secs_f64() * 1.0e6 - starts.send_census_us;
    assert!(us >= 0.0, "send census is nested within the send bracket");
    stats.send.push(us);
    if let Some(load) = load.as_deref_mut() {
        load.trace.send(us);
    }
    if let Some(ram) = ram.as_deref_mut() {
        ram.trace.send(us);
    }
}

/// Encrypted UDP payload bytes queued for transport, including netcode overhead. Rotating the
/// FIFO once preserves packet order (lightyear_link 0.30.1 exposes no production iterator).
/// This counts attempted sends, not IP/UDP headers or delivery; only an armed stats reader pays it.
fn count_send_bytes(
    mut stats: ResMut<ServerStats>,
    mut starts: ResMut<PhaseStarts>,
    mut links: Query<(Entity, &mut Link)>,
    mut worker_bytes: Local<Parallel<Vec<(Entity, u64, u64)>>>,
) {
    let started = Instant::now();
    // Each link owns its FIFO. Subtotals and maxima commit once, before the transport flush.
    let worker_bytes = &mut *worker_bytes;
    links.par_iter_mut().for_each(|(entity, mut link)| {
        let packets = link.send.len() as u64;
        worker_bytes.scope(|bytes| bytes.push((entity, queued_bytes(&mut link), packets)));
    });
    for bytes in worker_bytes.iter_mut() {
        for (entity, bytes, packets) in bytes.drain(..) {
            stats.send_bytes += bytes;
            let link = stats.send_by_client.entry(entity).or_default();
            link.bytes += bytes;
            link.max_packets = link.max_packets.max(packets);
        }
    }
    starts.send_census_us = started.elapsed().as_secs_f64() * 1.0e6;
    stats.send_census.push(starts.send_census_us);
}

fn queued_bytes(link: &mut Link) -> u64 {
    let mut bytes = 0;
    for _ in 0..link.send.len() {
        let packet = link
            .send
            .pop()
            .expect("the FIFO length was sampled before rotation");
        bytes += packet.len() as u64;
        link.send.push(packet);
    }
    bytes
}

/// The counts the stats line reads off the world when it prints.
#[derive(bevy::ecs::system::SystemParam)]
struct Census<'w, 's> {
    ram: Option<ResMut<'w, super::ram::Ram>>,
    clients: Query<'w, 's, Entity, (With<ClientOf>, With<Connected>)>,
    tanks: Query<'w, 's, (), With<Tank>>,
    projectiles: Query<'w, 's, (), With<Projectile>>,
    all: Query<'w, 's, Entity>,
    bodies: Query<'w, 's, &'static RigidBody, Without<Sleeping>>,
    pieces: Query<'w, 's, &'static Piece, With<Breakage>>,
    loose: super::loadtest::LoosePieces<'w, 's>,
    constructions: Query<'w, 's, (&'static MapInstance, &'static Breakage), With<Body>>,
    battle: Query<'w, 's, &'static super::battle::BattleState>,
    piece_records: ResMut<'w, PieceRecords>,
}

/// After Last: once the period has passed, print its line and open the next one.
fn emit_line(
    mut stats: ResMut<ServerStats>,
    fixed: Res<Time<Fixed>>,
    mut census: Census,
    debris: DebrisCensus,
    (recorder, wall): (Res<ShotRecorder>, super::server_runner::WallLedger),
    load: Option<ResMut<Loadtest>>,
    record: Res<Record>,
) {
    let now = Instant::now();
    let Some(elapsed) = stats.period.close(now) else {
        return;
    };
    let budget_us = fixed.timestep().as_secs_f64() * 1.0e6;
    let cpu = Some(process_cpu().unwrap_or_else(crate::refuse_start));
    let cpu_pct = cpu
        .zip(stats.cpu_at_start)
        .map(|(now, then)| now.saturating_sub(then).as_secs_f64() / elapsed.as_secs_f64() * 100.0);
    stats.cpu_at_start = cpu;
    let throttle = read_throttle();
    let throttled = throttle
        .zip(stats.throttle_at_start)
        .map(|(now, then)| now.since(then));
    stats.throttle_at_start = throttle;
    let husk_strikes = recorder.husk_strikes() - stats.husk_strikes_at_start;
    stats.husk_strikes_at_start = recorder.husk_strikes();
    let discarded = wall.discarded();
    let (discarded_then, batches_then) =
        std::mem::replace(&mut stats.discarded_at_start, discarded);
    let wall = WallReading {
        discarded: discarded.0 - discarded_then,
        discarding_batches: discarded.1 - batches_then,
        drift: wall.drift(),
    };
    stats.sequence += 1;
    let pieces = census.piece_records.drain();
    let send_bytes = std::mem::take(&mut stats.send_bytes);
    let send_by_client = std::mem::take(&mut stats.send_by_client);
    let send_clients: Vec<_> = census.clients.iter().map(|link| {
        // A connected link may queue no packet in this period.
        let (bytes, max_packets) = send_by_client.get(&link).map_or((0, 0), |sent| (sent.bytes, sent.max_packets));
        json!({"link": link.to_string(), "bytes": bytes, "bytes_per_s": bytes as f64 / elapsed.as_secs_f64(), "max_packets": max_packets})
    }).collect();
    let inputs: Vec<_> = std::mem::take(&mut stats.input_by_client)
        .into_iter()
        .map(|(link, counts)| json!({"link": link.to_string(), "counts": counts}))
        .collect();
    let workload = load.map(|mut load| {
        let mut value = load.json(&record, &census.constructions, &census.loose, now, elapsed);
        value["pieces"] = pieces.clone();
        value["sequence"] = json!(stats.sequence);
        value["send_bytes"] = json!(send_bytes);
        value["awake_bodies"] = json!(
            census
                .bodies
                .iter()
                .filter(|body| body.is_dynamic())
                .count()
        );
        value["construction_pieces"] = json!(
            census
                .pieces
                .iter()
                .map(|piece| piece.chunks.len())
                .sum::<usize>()
        );
        value["constructions"] = json!(census.constructions.iter().count());
        value["combat"] = json!(
            census
                .battle
                .iter()
                .any(|b| matches!(b.phase, super::battle::BattlePhase::Combat { .. }))
        );
        value
    });
    let mut cracks = std::collections::BTreeSet::new();
    let mut separated = std::collections::BTreeSet::new();
    for entry in &record.entries {
        match entry.event {
            crate::construction::breakage::Event::Crack(j) => {
                cracks.insert((entry.construction, j));
            }
            crate::construction::breakage::Event::Separate(j) => {
                cracks.insert((entry.construction, j));
                separated.insert((entry.construction, j));
            }
            crate::construction::breakage::Event::Wear(_)
            | crate::construction::breakage::Event::Dust(_)
            | crate::construction::breakage::Event::Hole(..) => {}
        }
    }
    let departed: usize = census
        .constructions
        .iter()
        .map(|(_, body)| {
            body.live
                .state
                .fate
                .iter()
                .filter(|&&f| f == crate::construction::fracture::Fate::Fallen)
                .count()
        })
        .sum();
    let construction = json!({"cracked_joints": cracks.len(), "separated_joints": separated.len(), "departed_chunks": departed});
    let snapshot = Snapshot {
        elapsed,
        budget_us,
        ticks: stats.ticks.drain(budget_us),
        physics: stats.physics.drain(budget_us),
        sleep: stats.sleep.drain(budget_us),
        collector: stats.collector.drain(budget_us),
        stages: std::array::from_fn(|i| stats.stages[i].drain(budget_us)),
        phases: std::array::from_fn(|i| stats.phases[i].drain(budget_us)),
        stages_cpu: std::array::from_fn(|i| stats.stages_cpu[i].drain(budget_us)),
        phases_cpu: std::array::from_fn(|i| stats.phases_cpu[i].drain(budget_us)),
        tick_cpu: stats.tick_cpu.drain(budget_us),
        one_thread_estimate: stats.one_thread_estimate.drain(budget_us),
        send: stats.send.drain(budget_us),
        frame: stats.frame.drain(budget_us),
        post_update: stats.post_update.drain(budget_us),
        #[cfg(target_os = "linux")]
        host: stats.host.sample(elapsed),
        send_census: stats.send_census.drain(budget_us),
        send_bytes,
        send_clients: json!(send_clients),
        inputs: json!(inputs),
        debris: debris.json(),
        clients: census.clients.iter().count(),
        tanks: census.tanks.iter().count(),
        projectiles: census.projectiles.iter().count(),
        entities: census.all.iter().count(),
        quota_used_pct: quota_cpus()
            .map(|quota| cpu_pct.expect("armed process CPU clock is available") / quota),
        cpu_pct,
        rss_bytes: resident_bytes(),
        throttled,
        husk_strikes,
        wall,
        workload,
        construction,
        ram: census.ram.as_deref_mut().map(|ram| {
            let mut value = ram.json();
            value["pieces"] = pieces.clone();
            value
        }),
    };
    println!("{}", stats_line(&snapshot));
}

/// DERIVED two-state estimate: wall * clamp(2 - cpu/wall, 0, 1).
/// Equals one-thread wall only if exactly one or two threads run throughout the interval.
/// Waiting, background work and more workers break that model; this is not a measured duration.
fn one_thread_estimate(wall: f64, cpu: f64) -> f64 {
    (2.0 * wall - cpu).clamp(0.0, wall)
}

/// The process's resident memory, from the kernel's own `VmRSS` line.
#[cfg(target_os = "linux")]
fn resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kib: u64 = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .trim()
        .strip_suffix("kB")?
        .trim()
        .parse()
        .ok()?;
    Some(kib * 1024)
}

/// The process's resident memory, from the task info `proc_pidinfo` reports.
#[cfg(target_os = "macos")]
fn resident_bytes() -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = size_of::<libc::proc_taskinfo>() as libc::c_int;
    let pid = std::process::id() as libc::c_int;
    // SAFETY: the buffer is valid for `size` bytes, the most `proc_pidinfo` writes.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // SAFETY: a return of the full size means the whole struct was written.
    Some(unsafe { info.assume_init() }.pti_resident_size)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn resident_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reporting must clamp the stated two-state estimate without mistaking it for a measured
    /// one-thread duration; empty/waiting and wider-worker intervals are valid instrument inputs.
    #[test]
    fn two_state_estimate_stays_inside_wall() {
        // Fixture: CPU spans below one worker, between one and two, and above two.
        for (wall, cpu, expected) in [
            (0.0, 0.0, 0.0),
            (10.0, 5.0, 10.0),
            (10.0, 15.0, 5.0),
            (10.0, 30.0, 0.0),
        ] {
            assert_eq!(one_thread_estimate(wall, cpu), expected);
        }
    }

    fn fixture_stats() -> ServerStats {
        // Fixture: an arbitrary reporting period, with empty sampling windows.
        ServerStats {
            period: Period::new(Duration::from_secs(1), Instant::now()),
            ticks: TickWindow::default(),
            physics: TickWindow::default(),
            sleep: TickWindow::default(),
            collector: TickWindow::default(),
            stages: std::array::from_fn(|_| TickWindow::default()),
            phases: std::array::from_fn(|_| TickWindow::default()),
            stages_cpu: std::array::from_fn(|_| TickWindow::default()),
            phases_cpu: std::array::from_fn(|_| TickWindow::default()),
            tick_cpu: TickWindow::default(),
            one_thread_estimate: TickWindow::default(),
            send: TickWindow::default(),
            frame: TickWindow::default(),
            post_update: TickWindow::default(),
            #[cfg(target_os = "linux")]
            host: linux::HostProbe::new(),
            send_census: TickWindow::default(),
            cpu_at_start: None,
            throttle_at_start: None,
            husk_strikes_at_start: 0,
            discarded_at_start: (Duration::ZERO, 0),
            sequence: 0,
            send_bytes: 0,
            send_by_client: BTreeMap::new(),
            input_by_client: BTreeMap::new(),
        }
    }

    /// Operators must distinguish a missing tick's held command from a tick-exact input, and
    /// attribute it to its client. Counting buffers or post-attestation commands loses that fact.
    /// The buffer's reach is the newest input past the simulated tick, signed so a clock running
    /// ahead reads positive; a client that has sent nothing yet has no reach, not a zero one.
    #[test]
    fn input_counts_follow_the_bridge_snapshot_and_keep_clients_separate() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let mut timeline = LocalTimeline::default();
        // Fixture: consecutive server ticks and two clients with different input provenance.
        timeline.apply_delta(100);
        world.insert_resource(timeline);
        world.insert_resource(fixture_stats());
        let first = world.spawn_empty().id();
        let second = world.spawn_empty().id();
        // Only the first client has sent an input yet, for tick 103.
        for (owner, for_tick, newest) in [(first, 100, Some(103)), (second, 99, None)] {
            let mut tank = world.spawn((
                ActionState(TankCommand {
                    for_tick,
                    ..default()
                }),
                TankCommand::default(),
                ControlledBy {
                    owner,
                    lifetime: default(),
                },
            ));
            if let Some(newest) = newest {
                let mut buffer = NativeBuffer::<TankCommand>::default();
                buffer.last_remote_tick = Some(Tick(newest));
                tank.insert(buffer);
            }
        }
        world.run_system_once(count_input_use).unwrap();
        world.resource_mut::<LocalTimeline>().apply_delta(1);
        world.run_system_once(count_input_use).unwrap();
        let counts = &world.resource::<ServerStats>().input_by_client;
        assert_eq!(counts[&first].tick_exact, 1);
        assert_eq!(counts[&first].held, 1);
        assert_eq!(counts[&first].last_for_tick_offset, Some(-1));
        assert_eq!(counts[&second].tick_exact, 0);
        assert_eq!(counts[&second].held, 2);
        assert_eq!(counts[&second].last_for_tick_offset, Some(-2));
        assert_eq!(
            (
                counts[&first].min_buffer_lookahead,
                counts[&first].max_buffer_lookahead
            ),
            (Some(2), Some(3))
        );
        assert_eq!(counts[&second].max_buffer_lookahead, None);
    }

    /// The clamp and census mirror both private library gates. Drive its actual receive path:
    /// each gate is accepted, one past it is dropped, every message lands in exactly one count,
    /// and the census leaves all of them available. Changing either upstream bound in either
    /// direction must fail this tripwire.
    #[test]
    fn the_lookahead_census_matches_lightyears_receive_gate_without_consuming_messages() {
        use bevy::ecs::system::RunSystemOnce;
        use lightyear::prelude::*;
        use lightyear_inputs::input_message::{ActionStateSequence, InputTarget, PerTargetData};
        type Inputs = InputMessage<NativeStateSequence<TankCommand>>;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .add_plugins(lightyear::prelude::client::ClientPlugins {
                tick_duration: super::super::server::TICK,
            });
        app.add_plugins(lightyear_inputs::server::ServerInputPlugin::<
            NativeStateSequence<TankCommand>,
        >::default());
        app.add_channel::<super::super::protocol::SpawnChannel>(ChannelSettings::default());
        app.insert_resource(fixture_stats());
        app.finish();
        app.cleanup();
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        // Fixture: messages adjacent to each gate, delivered without a socket or a clock step, at
        // a server tick far enough from zero that the past gate is not hidden by tick saturation.
        let link = app
            .world_mut()
            .spawn((
                ClientOf,
                Connected,
                RemoteId(PeerId::Netcode(1)),
                lightyear::connection::host::HostClient { buffer: Vec::new() },
                MessageSender::<Inputs>::default(),
                MessageReceiver::<Inputs>::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(1_000);
        let tick = app.world().resource::<LocalTimeline>().tick();
        let offsets = [
            -MAX_INPUT_PAST_TICKS - 1,
            -MAX_INPUT_PAST_TICKS,
            MAX_INPUT_LOOKAHEAD_TICKS,
            MAX_INPUT_LOOKAHEAD_TICKS + 1,
        ];
        let accepted =
            |offset: i32| (-MAX_INPUT_PAST_TICKS..=MAX_INPUT_LOOKAHEAD_TICKS).contains(&offset);
        let mut targets = Vec::new();
        for offset in offsets {
            let target = app
                .world_mut()
                .spawn(NativeBuffer::<TankCommand>::default())
                .id();
            let end = tick + offset;
            let mut source = NativeBuffer::<TankCommand>::default();
            source.set(end, ActionState(TankCommand::default()));
            let mut message = Inputs::new(end);
            message.inputs.push(PerTargetData {
                target: InputTarget::Entity(target),
                states: NativeStateSequence::<TankCommand>::build_from_input_buffer(
                    &source, 1, end,
                )
                .unwrap(),
            });
            targets.push((offset, target));
            app.world_mut()
                .get_mut::<MessageSender<Inputs>>(link)
                .unwrap()
                .send::<super::super::protocol::SpawnChannel>(message);
        }
        app.update(); // Local delivery fills the receiver in Last.
        app.world_mut()
            .run_system_once(count_input_lookahead)
            .unwrap();
        let counts = &app.world().resource::<ServerStats>().input_by_client[&link];
        assert_eq!(
            (
                counts.accepted,
                counts.lookahead_rejected,
                counts.past_rejected
            ),
            (2, 1, 1)
        );
        assert_eq!(
            (counts.min_message_lookahead, counts.max_message_lookahead),
            (Some(offsets[0]), Some(offsets[3]))
        );
        let mut received = Vec::new();
        app.world_mut()
            .get_mut::<MessageReceiver<Inputs>>(link)
            .unwrap()
            .retain_messages(|message| {
                received.push(message.end_tick - tick);
                true
            });
        assert_eq!(received, offsets);
        // The missing server relationship kept delivery pending for the census. Attach it now
        // and let the plugin's private receive_input_message system drain the real receiver.
        let server = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(link)
            .insert(lightyear::link::prelude::LinkOf { server });
        app.update();
        for (offset, target) in targets {
            let buffer = app
                .world()
                .get::<NativeBuffer<TankCommand>>(target)
                .unwrap();
            assert_eq!(
                buffer.last_remote_tick,
                accepted(offset).then_some(tick + offset),
                "the real receive path must accept at each mirrored gate and drop one past it",
            );
        }
    }

    fn bracket_world() -> World {
        let mut world = World::new();
        world.init_resource::<PhaseStarts>();
        world.insert_resource(fixture_stats());
        world
    }

    /// Operators rely on every physics close consuming its matching opening, never stale data.
    #[test]
    #[should_panic(expected = "the physics bracket opens in PhysicsStepSystems::First")]
    fn physics_close_requires_its_opening() {
        use bevy::ecs::system::RunSystemOnce;
        bracket_world().run_system_once(close_physics).unwrap();
    }

    /// A missing send opening must refuse the measurement rather than hide a broken bracket.
    #[test]
    #[should_panic(expected = "the send bracket opens before ServerSystems::Send")]
    fn send_close_requires_its_opening() {
        use bevy::ecs::system::RunSystemOnce;
        bracket_world().run_system_once(close_send).unwrap();
    }

    /// Each installed pair records once and consumes its opening, for the stats reader.
    #[test]
    fn paired_brackets_record_and_consume_their_openings() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = bracket_world();
        world.run_system_once(open_physics).unwrap();
        world.run_system_once(close_physics).unwrap();
        world.run_system_once(open_send).unwrap();
        world.run_system_once(close_send).unwrap();
        assert!(world.resource::<PhaseStarts>().physics.is_none());
        assert!(world.resource::<PhaseStarts>().send.is_none());
        assert_eq!(world.resource::<ServerStats>().physics.0.len(), 1);
        assert_eq!(world.resource::<ServerStats>().send.0.len(), 1);
    }

    /// Operators need complete First/Last and PostUpdate intervals, including late systems.
    /// Running the installed timer schedules witnesses their position in MainScheduleOrder.
    #[test]
    fn iteration_brackets_surround_first_last_and_post_update() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<PhaseStarts>()
            .insert_resource(fixture_stats());
        install_iteration_clock(&mut app);
        app.add_systems(First, |starts: Res<PhaseStarts>| {
            assert!(starts.frame.is_some());
            assert!(starts.post_update.is_none());
        })
        .add_systems(PostUpdate, |starts: Res<PhaseStarts>| {
            assert!(starts.frame.is_some());
            assert!(starts.post_update.is_some());
        })
        .add_systems(Last, |starts: Res<PhaseStarts>, stats: Res<ServerStats>| {
            assert!(starts.frame.is_some());
            assert!(starts.post_update.is_none());
            assert_eq!(stats.post_update.0.len(), stats.frame.0.len() + 1);
        });
        for count in 1..=2 {
            app.update();
            assert!(app.world().resource::<PhaseStarts>().frame.is_none());
            assert_eq!(app.world().resource::<ServerStats>().frame.0.len(), count);
            assert_eq!(
                app.world().resource::<ServerStats>().post_update.0.len(),
                count
            );
        }
    }

    /// The game's destruction census counts authority-owned chunks. Pose carriers repeat Piece
    /// metadata, and retired carriers retain it; neither is additional physical matter.
    #[test]
    fn construction_census_excludes_live_and_retired_pose_carriers() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<PieceRecords>();
        let kind = crate::construction::tests::column_construction();
        // Fixture: one authority group and its live/retired network metadata.
        let piece = Piece {
            construction: 0,
            chunks: (0..kind.layout.chunks.len()).map(|c| c as u16).collect(),
            born: 0,
            retired: None,
        };
        let expected = piece.chunks.len();
        world.spawn((piece.clone(), Breakage::new(kind, 0)));
        world.spawn(piece.clone());
        world.spawn(Piece {
            retired: Some(1),
            ..piece
        });
        let count = world
            .run_system_once(|census: Census| {
                census
                    .pieces
                    .iter()
                    .map(|piece| piece.chunks.len())
                    .sum::<usize>()
            })
            .unwrap();
        assert_eq!(
            count, expected,
            "metadata must never double-count construction matter"
        );
    }

    /// Players rely on the instrument preserving both packet contents and send order. Counting
    /// by draining without restoring, or rotating an incomplete turn, changes the transport.
    #[test]
    fn byte_census_preserves_the_send_fifo() {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default())
            .insert_resource(fixture_stats())
            .init_resource::<PhaseStarts>()
            .add_systems(Update, count_send_bytes);
        // Fixture: independent links with unequal packets expose missing workers and reordering.
        let links = 32;
        let packets = [vec![1], vec![2, 3], vec![4, 5, 6]];
        for _ in 0..links {
            let mut link = Link::default();
            for packet in &packets {
                link.send.push(packet.clone().into());
            }
            app.world_mut().spawn(link);
        }
        app.update();
        let expected = links * packets.iter().map(Vec::len).sum::<usize>() as u64;
        assert_eq!(app.world().resource::<ServerStats>().send_bytes, expected);
        let per_link = &app.world().resource::<ServerStats>().send_by_client;
        assert_eq!(per_link.len(), links as usize);
        assert!(
            per_link
                .values()
                .all(|sent| sent.bytes == expected / links
                    && sent.max_packets == packets.len() as u64)
        );
        for mut link in app
            .world_mut()
            .query::<&mut Link>()
            .iter_mut(app.world_mut())
        {
            assert_eq!(
                link.send.drain().map(|p| p.to_vec()).collect::<Vec<_>>(),
                packets
            );
        }
        app.update();
        assert_eq!(
            app.world().resource::<ServerStats>().send_bytes,
            expected,
            "worker subtotals must reset before the next frame"
        );
    }

    /// The lever's two readings: unset is off, a positive period is that period. A mutant that
    /// turns an unset lever on prints a line from every server; one that reads a word or a zero as
    /// off or as a default runs a host its operator configured otherwise, unseen.
    #[test]
    fn the_lever_is_a_period_in_seconds_and_unset_is_off() {
        assert_eq!(period_from(None).unwrap(), None);
        assert_eq!(
            period_from(Some("2.5")).unwrap(),
            Some(Duration::from_millis(2500))
        );
        for bad in [
            "",
            "on",
            "off",
            "0",
            "false",
            "5s",
            "-1",
            "NaN",
            "inf",
            "1e100",
            "0.000000000001",
        ] {
            assert!(period_from(Some(bad)).is_err(), "{bad:?}");
        }
    }

    /// One line per period. A period that never restarts prints a line every frame from the first
    /// close on; one that restarts at `started + length` prints a burst after a stall.
    #[test]
    fn a_period_closes_once_per_length_and_restarts_at_the_close() {
        let length = Duration::from_secs(10);
        let t0 = Instant::now();
        let mut period = Period::new(length, t0);
        assert_eq!(period.close(t0 + Duration::from_secs(9)), None);
        assert_eq!(period.close(t0 + length), Some(length));
        assert_eq!(period.close(t0 + length + Duration::from_secs(1)), None);
        // A 25 s stall closes one 25 s period, then waits a full length again.
        let stalled = t0 + length + Duration::from_secs(25);
        assert_eq!(period.close(stalled), Some(Duration::from_secs(25)));
        assert_eq!(period.close(stalled + Duration::from_secs(9)), None);
        assert_eq!(period.close(stalled + length), Some(length));
    }

    /// The tick summary is nearest-rank over the period's ticks, counts only ticks strictly past
    /// the budget, and starts the next period empty. A window that is not emptied blends every
    /// period into the first.
    #[test]
    fn a_tick_window_summarises_its_period_and_empties() {
        let mut window = TickWindow::default();
        assert_eq!(window.drain(15_625.0), None);
        // 1..=100 µs, pushed out of order. Against a 99 µs budget the 100 µs tick overruns and the
        // 99 µs tick, exactly on it, does not.
        for us in (1..=100).rev() {
            window.push(f64::from(us));
        }
        let summary = window.drain(99.0).expect("100 ticks ran");
        assert_eq!(
            summary,
            TickSummary {
                ticks: 100,
                p50: 50.0,
                p95: 95.0,
                p99: 99.0,
                total: 5050.0,
                max: 100.0,
                overruns: 1,
            }
        );
        window.push(7.0);
        let next = window.drain(99.0).expect("one tick ran");
        assert_eq!(
            (next.ticks, next.p50, next.p95, next.max),
            (1, 7.0, 7.0, 7.0)
        );
    }

    /// The line is `server_stats ` and one JSON object carrying every field the night's analysis
    /// reads (`scripts/perf/online_cost.py` flattens them to dotted keys: `physics_us.p95`,
    /// `debris.live`); a missing reading is null, not zero, so an absent CPU reading cannot pass for
    /// an idle core.
    #[test]
    fn the_line_is_a_prefix_then_one_json_object() {
        let mut full = Snapshot {
            elapsed: Duration::from_secs(10),
            budget_us: 15_625.0,
            ticks: Some(TickSummary {
                ticks: 640,
                p50: 812.34,
                p95: 1_500.0,
                p99: 2_000.0,
                total: 600_000.0,
                max: 16_000.0,
                overruns: 1,
            }),
            physics: Some(TickSummary {
                ticks: 640,
                p50: 301.04,
                p95: 455.0,
                p99: 600.0,
                total: 200_000.0,
                max: 900.0,
                overruns: 0,
            }),
            send: Some(TickSummary {
                ticks: 640,
                p50: 120.0,
                p95: 180.06,
                p99: 200.0,
                total: 80_000.0,
                max: 250.0,
                overruns: 0,
            }),
            frame: None,
            post_update: None,
            #[cfg(target_os = "linux")]
            host: json!({}),
            send_census: None,
            send_bytes: 6000,
            send_clients: json!([{"link": "fixture", "bytes": 6000, "bytes_per_s": 600.0}]),
            inputs: json!([]),
            sleep: None,
            collector: None,
            stages: std::array::from_fn(|_| None),
            phases: std::array::from_fn(|_| None),
            stages_cpu: std::array::from_fn(|_| None),
            phases_cpu: std::array::from_fn(|_| None),
            tick_cpu: None,
            one_thread_estimate: None,
            debris: json!({"live": 35, "sleeping": 140}),
            clients: 2,
            tanks: 6,
            projectiles: 3,
            entities: 1_234,
            cpu_pct: Some(37.56),
            quota_used_pct: Err(QuotaAbsence::NotLinux),
            rss_bytes: Some(200 * 1024 * 1024),
            throttled: Some(Throttle {
                nr_throttled: 3,
                throttled_usec: 45_000,
            }),
            husk_strikes: 4,
            wall: WallReading {
                discarded: Duration::from_micros(250_049),
                discarding_batches: 1,
                drift: Duration::from_millis(1_500),
            },
            workload: None,
            construction: json!({"cracked_joints": 0, "separated_joints": 0, "departed_chunks": 0}),
            ram: None,
        };
        let line = stats_line(&full);
        let json = line
            .strip_prefix("server_stats ")
            .expect("the grep anchor leads the line");
        assert!(!json.contains('\n'), "one line");
        let value: Value = serde_json::from_str(json).expect("the rest is one JSON object");
        let mut expected = json!({
            "period_s": 10.0,
            "ticks": 640,
            "tick_hz": 64.0,
            "frames": 0,
            "tick_us": {"p50": 812.3, "p95": 1500.0, "p99": 2000.0, "total": 600000.0, "max": 16000.0},
            "overruns": 1,
            "budget_us": 15625.0,
            "clients": 2,
            "tanks": 6,
            "projectiles": 3,
            "entities": 1234,
            "cpu_pct": 37.6,
            "quota_used_pct": {"Err": {"absence": "not_linux"}},
            "tick_cpu_us": null,
            "one_thread_estimate_us": null,
            "stages_cpu_us": NAMES.into_iter().map(|name| (name.to_owned(), Value::Null)).collect::<serde_json::Map<_, _>>(),
            "phases_cpu_us": phases::NAMES.into_iter().map(|name| (name.to_owned(), Value::Null)).collect::<serde_json::Map<_, _>>(),
            "rss_mb": 200.0,
            "sleep_us": null,
            "collector_us": null,
            "frame_us": null,
            "post_update_us": null,

            "phases_us": phases::NAMES.into_iter().map(|name| (name.to_owned(), Value::Null)).collect::<serde_json::Map<_, _>>(),
            "physics_us": {"p50": 301.0, "p95": 455.0, "p99": 600.0, "total": 200000.0, "max": 900.0},
            "send_census_us": null,
            "send_bytes": 6000,
            "send_clients": [{"link": "fixture", "bytes": 6000, "bytes_per_s": 600.0}],
            "inputs": [],
            "send_us_per_client_s": 4000.0,
            "send_us": {"p50": 120.0, "p95": 180.1, "p99": 200.0, "total": 80000.0, "max": 250.0},
            "throttled_usec": 45000,
            "nr_throttled": 3,
            "husk_strikes": 4,
            "wall": {"discarded_ms": 250.0, "discarding_batches": 1, "drift_ms": 1500.0},
            "debris": {"live": 35, "sleeping": 140},
            "construction": {"cracked_joints": 0, "separated_joints": 0, "departed_chunks": 0},
        });
        for name in NAMES {
            expected[name] = Value::Null;
        }
        #[cfg(target_os = "linux")]
        {
            expected["linux_host"] = json!({});
        }
        assert_eq!(value, expected);

        full.elapsed = Duration::from_micros(10_012_345);
        let precise: Value =
            serde_json::from_str(stats_line(&full).strip_prefix("server_stats ").unwrap()).unwrap();
        assert_eq!(
            precise["period_s"], 10.012345,
            "capacity checks need the exact wall span"
        );

        let empty = Snapshot {
            ticks: None,
            physics: None,
            send: None,
            cpu_pct: None,
            rss_bytes: None,
            throttled: None,
            ..full
        };
        let value: Value =
            serde_json::from_str(stats_line(&empty).strip_prefix("server_stats ").unwrap())
                .unwrap();
        assert_eq!(value["ticks"], json!(0));
        assert_eq!(value["tick_us"], Value::Null);
        assert_eq!(value["cpu_pct"], Value::Null);
        assert_eq!(value["rss_mb"], Value::Null);
        assert_eq!(value["physics_us"], Value::Null);
        assert_eq!(value["send_us"], Value::Null);
        assert_eq!(value["throttled_usec"], Value::Null);
        assert_eq!(value["nr_throttled"], Value::Null);
        assert_eq!(
            value["debris"],
            json!({"live": 35, "sleeping": 140}),
            "the piece count prints with no lever armed"
        );
    }
}
