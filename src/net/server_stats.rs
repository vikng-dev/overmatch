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
//! - `overruns`: ticks longer than the fixed timestep, `budget_us`;
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
//!   judged by its kernel and routed.
//! - `phases_us`: complete schedule/phase intervals after debiting the leaf brackets above.
//!   ECS application, synchronous hooks and dispatch stay in their phase; `physics_step_us`
//!   includes sleep. Add phases and leaves once; `physics_us` and `sleep_us` are nested detail.
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
//! `cpu_pct`, `rss_mb` and the throttle fields are null where the platform gives no reading.
//!
//! The tick wall time is the cost recorder's measurement ([`crate::cost::TickClock`]), never a
//! second timer. `physics_us` sits inside it; `send_us` runs after the fixed loop, outside it.
//!
//! `loadtest`, present only for `scripts/perf/collapse.sh --schedule`, carries the recipe,
//! sequence, clock, construction outcomes, awake dynamic bodies, loose construction chunks and
//! attempted encrypted send bytes. Its `ticks` use the ram schema plus admission-relative
//! `elapsed_s`, `events_fired` and work counts; `sends` remain outside the fixed ticks.
//! Bytes exclude IP/UDP headers and do not attest delivery.

use std::time::{Duration, Instant};

use avian3d::prelude::{PhysicsSchedule, PhysicsStepSystems, RigidBody, Sleeping};
use bevy::app::FixedMain;
use bevy::prelude::*;
use bevy::utils::Parallel;
use bevy_replicon::prelude::ServerSystems;
use bevy_replicon::server::server_tick::ServerTick;
use lightyear::link::LinkSystems;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{Connected, ConnectionSystems, Link};
use serde_json::{Value, json};

use super::debris_stress::DebrisCensus;
use super::harness::{LeverError, env_text};
use super::loadtest::Loadtest;
use super::server_host::{Throttle, read_throttle};
use crate::ballistics::Projectile;
use crate::construction::{
    Body,
    breakage::{Breakage, Piece, Record},
};
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
    app.insert_resource(ServerStats {
        period: Period::new(period, Instant::now()),
        ticks: TickWindow::default(),
        physics: TickWindow::default(),
        sleep: TickWindow::default(),
        collector: TickWindow::default(),
        stages: std::array::from_fn(|_| TickWindow::default()),
        phases: std::array::from_fn(|_| TickWindow::default()),
        send: TickWindow::default(),
        cpu_at_start: process_cpu(),
        throttle_at_start: read_throttle(),
        husk_strikes_at_start: 0,
        sequence: 0,
        send_bytes: 0,
    });
    app.init_resource::<PhaseStarts>();
    phases::install(app);
    crate::cost::stages::install_lifecycle_counts(app);
    app.add_systems(FixedMain, record_tick.after(TickClosed));
    app.add_systems(
        PhysicsSchedule,
        (
            open_physics.in_set(PhysicsStepSystems::First),
            close_physics.in_set(PhysicsStepSystems::Last),
            open_sleep
                .after(PhysicsStepSystems::Solver)
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
    if app.world().contains_resource::<Loadtest>() {
        app.add_systems(
            PostUpdate,
            count_send_bytes
                .after(ConnectionSystems::Send)
                .before(LinkSystems::Send),
        );
    }
    app.add_systems(Last, emit_line);
    Ok(())
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
    /// The period's outbound brackets, wall micros.
    send: TickWindow,
    /// The process's CPU time when the current period opened.
    cpu_at_start: Option<Duration>,
    /// The cgroup's throttle counters when the current period opened.
    throttle_at_start: Option<Throttle>,
    /// The recorder's husk-strike count when the current period opened.
    husk_strikes_at_start: u64,
    sequence: u64,
    send_bytes: u64,
}

/// The open phase brackets' starts.
#[derive(Resource, Default)]
struct PhaseStarts {
    physics: Option<Instant>,
    sleep: Option<Instant>,
    physics_us: f64,
    sleep_us: f64,
    send: Option<Instant>,
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

/// One period's fixed-tick wall micros.
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
    send: Option<TickSummary>,
    debris: Value,
    clients: usize,
    tanks: usize,
    projectiles: usize,
    entities: usize,
    cpu_pct: Option<f64>,
    rss_bytes: Option<u64>,
    /// The period's throttling, when the cgroup reports it.
    throttled: Option<Throttle>,
    /// The period's rounds that struck a knocked-out tank (`ShotRecorder::husk_strikes`): a
    /// counter, never a line per strike, so automatic fire into husks cannot flood the log.
    husk_strikes: u64,
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
        "period_s": tenth(period_s),
        "ticks": count,
        "tick_hz": tenth(count as f64 / period_s),
        "tick_us": tick_us,
        "overruns": ticks.map_or(0, |summary| summary.overruns),
        "budget_us": tenth(snapshot.budget_us),
        "clients": snapshot.clients,
        "tanks": snapshot.tanks,
        "projectiles": snapshot.projectiles,
        "entities": snapshot.entities,
        "cpu_pct": snapshot.cpu_pct.map(tenth),
        "rss_mb": snapshot.rss_bytes.map(|bytes| tenth(bytes as f64 / (1024.0 * 1024.0))),
        "physics_us": percentiles(snapshot.physics.as_ref()),
        "sleep_us": percentiles(snapshot.sleep.as_ref()),
        "collector_us": percentiles(snapshot.collector.as_ref()),
        "send_us": percentiles(snapshot.send.as_ref()),
        "throttled_usec": snapshot.throttled.map(|t| t.throttled_usec),
        "nr_throttled": snapshot.throttled.map(|t| t.nr_throttled),
        "husk_strikes": snapshot.husk_strikes,
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
        stages.add(
            crate::cost::stages::Work::ContactPoints,
            census.contact_points() as u64,
        );
        let work = stages.take_counts();
        let sample = census.sample(
            closed.us,
            starts.physics_us,
            starts.sleep_us,
            measured,
            phases.times(),
            fixed.timestep().as_secs_f64(),
        );
        let rows = [
            census.record(sample, work),
            load.as_deref_mut()
                .and_then(|load| load.record(sample, work)),
        ];
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
    let us = start.elapsed().as_secs_f64() * 1.0e6;
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
/// This counts attempted sends, not IP/UDP headers or delivery; only the load-test user pays it.
fn count_send_bytes(
    mut stats: ResMut<ServerStats>,
    mut links: Query<&mut Link>,
    mut worker_bytes: Local<Parallel<u64>>,
) {
    // Each link owns its FIFO. Integer subtotals commit once, before the transport flush.
    let worker_bytes = &mut *worker_bytes;
    links.par_iter_mut().for_each(|mut link| {
        worker_bytes.scope(|bytes| *bytes += queued_bytes(&mut link));
    });
    stats.send_bytes += worker_bytes.iter_mut().map(std::mem::take).sum::<u64>();
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
    clients: Query<'w, 's, (), (With<ClientOf>, With<Connected>)>,
    tanks: Query<'w, 's, (), With<Tank>>,
    projectiles: Query<'w, 's, (), With<Projectile>>,
    all: Query<'w, 's, Entity>,
    bodies: Query<'w, 's, &'static RigidBody, Without<Sleeping>>,
    pieces: Query<'w, 's, &'static Piece, With<Breakage>>,
    constructions: Query<'w, 's, (&'static MapInstance, &'static Breakage), With<Body>>,
    battle: Query<'w, 's, &'static super::battle::BattleState>,
}

/// `Last`: once the period has passed, print its line and open the next one.
fn emit_line(
    mut stats: ResMut<ServerStats>,
    fixed: Res<Time<Fixed>>,
    mut census: Census,
    debris: DebrisCensus,
    recorder: Res<ShotRecorder>,
    load: Option<ResMut<Loadtest>>,
    record: Res<Record>,
) {
    let now = Instant::now();
    let Some(elapsed) = stats.period.close(now) else {
        return;
    };
    let budget_us = fixed.timestep().as_secs_f64() * 1.0e6;
    let cpu = process_cpu();
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
    stats.sequence += 1;
    let workload = load.map(|mut load| {
        let mut value = load.json(&record, &census.constructions, now, elapsed);
        value["sequence"] = json!(stats.sequence);
        value["send_bytes"] = json!(std::mem::take(&mut stats.send_bytes));
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
        send: stats.send.drain(budget_us),
        debris: debris.json(),
        clients: census.clients.iter().count(),
        tanks: census.tanks.iter().count(),
        projectiles: census.projectiles.iter().count(),
        entities: census.all.iter().count(),
        cpu_pct,
        rss_bytes: resident_bytes(),
        throttled,
        husk_strikes,
        workload,
        construction,
        ram: census.ram.as_deref_mut().map(super::ram::Ram::json),
    };
    println!("{}", stats_line(&snapshot));
}

/// The user + system CPU time this process has used.
#[cfg(unix)]
fn process_cpu() -> Option<Duration> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: the pointer is valid for one `rusage`, and `getrusage` only writes through it.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status != 0 {
        return None;
    }
    // SAFETY: a zero status means `getrusage` filled the whole struct.
    let usage = unsafe { usage.assume_init() };
    let time = |tv: libc::timeval| {
        Duration::from_secs(tv.tv_sec as u64) + Duration::from_micros(tv.tv_usec as u64)
    };
    Some(time(usage.ru_utime) + time(usage.ru_stime))
}

#[cfg(not(unix))]
fn process_cpu() -> Option<Duration> {
    None
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
            send: TickWindow::default(),
            cpu_at_start: None,
            throttle_at_start: None,
            husk_strikes_at_start: 0,
            sequence: 0,
            send_bytes: 0,
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

    /// The game's destruction census counts authority-owned chunks. Pose carriers repeat Piece
    /// metadata, and retired carriers retain it; neither is additional physical matter.
    #[test]
    fn construction_census_excludes_live_and_retired_pose_carriers() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
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
        let full = Snapshot {
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
            sleep: None,
            collector: None,
            stages: std::array::from_fn(|_| None),
            phases: std::array::from_fn(|_| None),
            debris: json!({"live": 35, "sleeping": 140}),
            clients: 2,
            tanks: 6,
            projectiles: 3,
            entities: 1_234,
            cpu_pct: Some(37.56),
            rss_bytes: Some(200 * 1024 * 1024),
            throttled: Some(Throttle {
                nr_throttled: 3,
                throttled_usec: 45_000,
            }),
            husk_strikes: 4,
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
        assert_eq!(
            value,
            json!({
                "period_s": 10.0,
                "ticks": 640,
                "tick_hz": 64.0,
                "tick_us": {"p50": 812.3, "p95": 1500.0, "p99": 2000.0, "total": 600000.0, "max": 16000.0},
                "overruns": 1,
                "budget_us": 15625.0,
                "clients": 2,
                "tanks": 6,
                "projectiles": 3,
                "entities": 1234,
                "cpu_pct": 37.6,
                "rss_mb": 200.0,
                "sleep_us": null,
                "collector_us": null,
                "publish_us": null,
                "contacts_us": null,
                "intake_us": null,
                "structure_us": null,
                "refit_us": null,
                "sweep_us": null,
                "crumble_us": null,
                "belt_rubble_us": null,
                "phases_us": phases::NAMES.into_iter().map(|name| (name.to_owned(), Value::Null)).collect::<serde_json::Map<_, _>>(),
                "physics_us": {"p50": 301.0, "p95": 455.0, "p99": 600.0, "total": 200000.0, "max": 900.0},
                "send_us": {"p50": 120.0, "p95": 180.1, "p99": 200.0, "total": 80000.0, "max": 250.0},
                "throttled_usec": 45000,
                "nr_throttled": 3,
                "husk_strikes": 4,
                "debris": {"live": 35, "sleeping": 140},
                "construction": {"cracked_joints": 0, "separated_joints": 0, "departed_chunks": 0},
            })
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
