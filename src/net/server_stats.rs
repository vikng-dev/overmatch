//! The dedicated server's stats line, armed by `OVERMATCH_SERVER_STATS_S`: once per period, one
//! line on stdout, `server_stats {json}`, for a host whose only window into the process is its
//! stdout (a container's logs). `println!` rather than the log macros, so the line reaches stdout
//! whatever `RUST_LOG` filters.
//!
//! The lever's value is the period in seconds. Present but empty or non-numeric, it is
//! [`DEFAULT_PERIOD`]; unset, `0` or `false`, the line is off.
//!
//! JSON fields:
//! - `period_s`: the wall time the line covers, which can exceed the period by one frame;
//! - `ticks`, `tick_hz`: fixed ticks run in the period, and per wall second;
//! - `tick_us`: `p50`, `p95` and `max` of the ticks' wall micros, null when no tick ran;
//! - `overruns`: ticks longer than the fixed timestep, `budget_us`;
//! - `clients` (connected links), `tanks`, `projectiles`, `entities`, sampled at the line;
//! - `cpu_pct`: the process's user + system CPU over the period, as % of one core;
//! - `rss_mb`: resident memory at the line;
//! - `physics_us`: `p50`, `p95` and `max` of avian's step (its `PhysicsSchedule`, broad phase
//!   through finalize), one sample per fixed tick, null when none ran;
//! - `send_us`: `p50`, `p95` and `max` of the outbound path, one sample per frame that replicated:
//!   the wall time from before replicon's `ServerSystems::Send` (the change scan and serialisation)
//!   to after lightyear's `LinkSystems::Send` (the socket flush), null when none ran. It is a
//!   bracket, so it also holds any unordered `PostUpdate` system the executor ran inside it;
//! - `debris`: present only while the debris stress lever is armed (`net::debris_stress`): its
//!   `live` and `frozen` pieces at the line, and its cumulative counts.
//!
//! `cpu_pct` and `rss_mb` are null where the platform gives no reading.
//!
//! The tick wall time is the cost recorder's measurement ([`crate::cost::TickClock`]), never a
//! second timer. `physics_us` sits inside it; `send_us` runs after the fixed loop, outside it.
//!
//! Not carried: bytes per client. lightyear 0.30.1 shows a link's queued payloads only through an
//! `iter` behind its `test_utils` feature, and counts bytes only in gauges behind its `metrics`
//! feature; neither is on.

use std::time::{Duration, Instant};

use avian3d::prelude::{PhysicsSchedule, PhysicsStepSystems};
use bevy::prelude::*;
use bevy_replicon::prelude::ServerSystems;
use bevy_replicon::server::server_tick::ServerTick;
use lightyear::link::LinkSystems;
use lightyear::prelude::Connected;
use lightyear::prelude::server::ClientOf;
use serde_json::{Value, json};

use super::debris_stress::DebrisCensus;
use crate::ballistics::Projectile;
use crate::cost::{TickClock, TickClosed, arm_tick_clock};
use crate::tank::Tank;

/// The lever: the period in seconds.
const LEVER: &str = "OVERMATCH_SERVER_STATS_S";

/// The period when the lever is present without a number.
const DEFAULT_PERIOD: Duration = Duration::from_secs(10);

/// The line's grep anchor.
const PREFIX: &str = "server_stats";

/// Arm the stats line when the lever is set.
pub(super) fn install(app: &mut App) {
    let Some(period) = period_from(std::env::var(LEVER).ok().as_deref()) else {
        return;
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
        send: TickWindow::default(),
        cpu_at_start: process_cpu(),
    });
    app.init_resource::<PhaseStarts>();
    app.add_systems(FixedLast, record_tick.after(TickClosed));
    app.add_systems(
        PhysicsSchedule,
        (
            open_physics.in_set(PhysicsStepSystems::First),
            close_physics.in_set(PhysicsStepSystems::Last),
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
    app.add_systems(Last, emit_line);
}

/// The lever's value as a period; `None` is off.
fn period_from(value: Option<&str>) -> Option<Duration> {
    let value = value?.trim();
    if value.eq_ignore_ascii_case("false") {
        return None;
    }
    match value.parse::<f64>() {
        Ok(secs) if secs.is_finite() && secs > 0.0 => Some(Duration::from_secs_f64(secs)),
        Ok(_) => None,
        Err(_) => Some(DEFAULT_PERIOD),
    }
}

#[derive(Resource)]
struct ServerStats {
    period: Period,
    ticks: TickWindow,
    /// The period's avian steps, wall micros.
    physics: TickWindow,
    /// The period's outbound brackets, wall micros.
    send: TickWindow,
    /// The process's CPU time when the current period opened.
    cpu_at_start: Option<Duration>,
}

/// The open phase brackets' starts.
#[derive(Resource, Default)]
struct PhaseStarts {
    physics: Option<Instant>,
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
    send: Option<TickSummary>,
    debris: Option<Value>,
    clients: usize,
    tanks: usize,
    projectiles: usize,
    entities: usize,
    cpu_pct: Option<f64>,
    rss_bytes: Option<u64>,
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
        "send_us": percentiles(snapshot.send.as_ref()),
    });
    if let Some(debris) = &snapshot.debris {
        body["debris"] = debris.clone();
    }
    format!("{PREFIX} {body}")
}

/// `FixedLast`, after the clock closes: add the tick to the period.
fn record_tick(clock: Res<TickClock>, mut stats: ResMut<ServerStats>) {
    if let Some(closed) = clock.closed() {
        stats.ticks.push(closed.us);
    }
}

fn open_physics(mut starts: ResMut<PhaseStarts>) {
    starts.physics = Some(Instant::now());
}

fn close_physics(mut starts: ResMut<PhaseStarts>, mut stats: ResMut<ServerStats>) {
    if let Some(start) = starts.physics.take() {
        stats.physics.push(start.elapsed().as_secs_f64() * 1.0e6);
    }
}

fn open_send(mut starts: ResMut<PhaseStarts>) {
    starts.send = Some(Instant::now());
}

fn close_send(mut starts: ResMut<PhaseStarts>, mut stats: ResMut<ServerStats>) {
    if let Some(start) = starts.send.take() {
        stats.send.push(start.elapsed().as_secs_f64() * 1.0e6);
    }
}

/// `Last`: once the period has passed, print its line and open the next one.
fn emit_line(
    mut stats: ResMut<ServerStats>,
    fixed: Res<Time<Fixed>>,
    clients: Query<(), (With<ClientOf>, With<Connected>)>,
    tanks: Query<(), With<Tank>>,
    projectiles: Query<(), With<Projectile>>,
    all: Query<Entity>,
    debris: DebrisCensus,
) {
    let Some(elapsed) = stats.period.close(Instant::now()) else {
        return;
    };
    let budget_us = fixed.timestep().as_secs_f64() * 1.0e6;
    let cpu = process_cpu();
    let cpu_pct = cpu
        .zip(stats.cpu_at_start)
        .map(|(now, then)| now.saturating_sub(then).as_secs_f64() / elapsed.as_secs_f64() * 100.0);
    stats.cpu_at_start = cpu;
    let snapshot = Snapshot {
        elapsed,
        budget_us,
        ticks: stats.ticks.drain(budget_us),
        physics: stats.physics.drain(budget_us),
        send: stats.send.drain(budget_us),
        debris: debris.json(),
        clients: clients.iter().count(),
        tanks: tanks.iter().count(),
        projectiles: projectiles.iter().count(),
        entities: all.iter().count(),
        cpu_pct,
        rss_bytes: resident_bytes(),
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

    /// The lever's three readings. A mutant that turns an unset lever on prints a line from every
    /// server; one that ignores the number pins every host to the default.
    #[test]
    fn the_lever_is_a_period_in_seconds_present_means_on() {
        assert_eq!(period_from(None), None);
        assert_eq!(period_from(Some("0")), None);
        assert_eq!(period_from(Some("false")), None);
        assert_eq!(period_from(Some("2.5")), Some(Duration::from_millis(2500)));
        assert_eq!(period_from(Some("")), Some(DEFAULT_PERIOD));
        assert_eq!(period_from(Some("on")), Some(DEFAULT_PERIOD));
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
                max: 16_000.0,
                overruns: 1,
            }),
            physics: Some(TickSummary {
                ticks: 640,
                p50: 301.04,
                p95: 455.0,
                max: 900.0,
                overruns: 0,
            }),
            send: Some(TickSummary {
                ticks: 640,
                p50: 120.0,
                p95: 180.06,
                max: 250.0,
                overruns: 0,
            }),
            debris: Some(json!({"live": 35, "frozen": 140})),
            clients: 2,
            tanks: 6,
            projectiles: 3,
            entities: 1_234,
            cpu_pct: Some(37.56),
            rss_bytes: Some(200 * 1024 * 1024),
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
                "tick_us": {"p50": 812.3, "p95": 1500.0, "max": 16000.0},
                "overruns": 1,
                "budget_us": 15625.0,
                "clients": 2,
                "tanks": 6,
                "projectiles": 3,
                "entities": 1234,
                "cpu_pct": 37.6,
                "rss_mb": 200.0,
                "physics_us": {"p50": 301.0, "p95": 455.0, "max": 900.0},
                "send_us": {"p50": 120.0, "p95": 180.1, "max": 250.0},
                "debris": {"live": 35, "frozen": 140},
            })
        );

        let empty = Snapshot {
            ticks: None,
            physics: None,
            send: None,
            debris: None,
            cpu_pct: None,
            rss_bytes: None,
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
        assert!(
            value.get("debris").is_none(),
            "an unarmed debris lever adds no key, so a production line is unchanged by it"
        );
    }
}
