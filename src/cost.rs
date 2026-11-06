//! The fixed-tick wall clock, and its optional JSONL recorder enabled by `SPIKE_COST_TRACE`.
//!
//! [`TickClock`] brackets the complete fixed schedule and the projectile-march share of it. It is
//! armed by whichever consumer asks first ([`arm_tick_clock`]): the JSONL rows here, or the
//! server's stats line (`net::server_stats`). Each consumer reads [`TickClock::closed`] after
//! [`TickClosed`], so every reader sees the one measurement.
//!
//! `scripts/cost/analyze.py` consumes the rows.
//! Invariant: use a distinct base path from `SPIKE_TRACE`; both recorders otherwise open the same
//! role-qualified file independently.

pub(crate) mod phases;
pub(crate) mod stages;

use std::time::Instant;

use bevy::{app::FixedMain, prelude::*};
use serde::Serialize;
use serde_json::json;
use stages::NamedTimes;

/// Shared per-tick wire schema for the ram and scheduled-collapse instruments.
#[derive(Serialize)]
pub(crate) struct TickSample {
    pub(crate) tick: u64,
    pub(crate) tick_us: f64,
    pub(crate) physics_us: f64,
    pub(crate) sleep_us: f64,
    #[serde(flatten)]
    pub(crate) stages: NamedTimes<{ stages::COUNT }>,
    pub(crate) phases_us: NamedTimes<{ phases::COUNT }>,
    pub(crate) collector_us: f64,
    pub(crate) awake: usize,
    pub(crate) tanks: usize,
    pub(crate) dt_s: f64,
}

#[derive(Serialize)]
pub(crate) struct SendSample {
    pub(crate) tick: u64,
    pub(crate) us: f64,
}

use crate::ballistics::Projectile;
use crate::state::MatchCommands;
use crate::tank::Tank;
use crate::trace::{JsonlSink, role_path};

/// The cost runner's shared start file. `scripts/cost/run.sh` creates it only after every
/// scripted client holds an input slot; scripts and recorders wait without delaying the Battle.
#[derive(Debug, Default)]
pub(crate) enum WorkloadStart {
    #[default]
    Ready,
    WaitingForFile(std::path::PathBuf),
}

impl WorkloadStart {
    pub(crate) fn from_env() -> Self {
        match crate::env_value("SPIKE_WORKLOAD_READY").unwrap_or_else(crate::refuse_start) {
            Some(path) => Self::WaitingForFile(path.into()),
            None => Self::Ready,
        }
    }

    pub(crate) fn ready(&mut self) -> bool {
        let Self::WaitingForFile(path) = self else {
            return true;
        };
        match path.try_exists() {
            Ok(false) => false,
            Ok(true) => {
                *self = Self::Ready;
                true
            }
            Err(error) => {
                eprintln!(
                    "SPIKE_WORKLOAD_READY: cannot inspect {}: {error}",
                    path.display()
                );
                std::process::exit(2);
            }
        }
    }
}

/// The fixed-tick timer, present only while some consumer has armed it.
#[derive(Resource, Default)]
pub(crate) struct TickClock {
    /// Start before `FixedMain::run_fixed_main`, including every fixed schedule's final flush.
    start: Option<Instant>,
    /// Accumulated `integrate_projectiles` wall micros this tick.
    march_us: f64,
    /// Number of projectile-march calls this tick.
    march_calls: u32,
    /// The tick [`close_tick`] closed in `FixedMain`; `None` when no tick was opened.
    closed: Option<TickCost>,
}

/// One closed fixed tick.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TickCost {
    /// Wall micros around `FixedMain::run_fixed_main`, excluding the stats/trace consumers.
    pub(crate) us: f64,
    /// The projectile-march share of `us`.
    march_us: f64,
    march_calls: u32,
}

impl TickClock {
    /// Attribute projectile-march wall micros to the current tick.
    pub(crate) fn record_march(&mut self, us: f64) {
        self.march_us += us;
        self.march_calls += 1;
    }

    /// The tick closed this `FixedMain`, for a reader ordered after [`TickClosed`].
    pub(crate) fn closed(&self) -> Option<TickCost> {
        self.closed
    }
}

/// The `FixedMain` set holding the clock's close; every reader of [`TickClock::closed`] runs after
/// it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TickClosed;

/// Arm the clock. Idempotent, so each consumer arms it without knowing about the others.
pub(crate) fn arm_tick_clock(app: &mut App) {
    if app.world().contains_resource::<TickClock>() {
        return;
    }
    app.init_resource::<TickClock>();
    // Bevy 0.19.1's runner returns only after each child schedule applies its final buffers.
    app.add_systems(FixedMain, open_tick.before(FixedMain::run_fixed_main));
    app.add_systems(
        FixedMain,
        close_tick
            .after(FixedMain::run_fixed_main)
            .in_set(TickClosed),
    );
}

fn open_tick(
    mut clock: ResMut<TickClock>,
    phases: Option<ResMut<phases::Phases>>,
    stages: Option<Res<stages::Stages>>,
) {
    let now = Instant::now();
    clock.start = Some(now);
    clock.march_us = 0.0;
    clock.march_calls = 0;
    if let Some(mut phases) = phases {
        phases.open(
            now,
            stages
                .as_deref()
                .expect("phase timing installs leaf timing"),
        );
    }
}

fn close_tick(
    mut clock: ResMut<TickClock>,
    phases: Option<ResMut<phases::Phases>>,
    stages: Option<Res<stages::Stages>>,
) {
    let now = Instant::now();
    if let Some(mut phases) = phases {
        phases.close(
            now,
            stages
                .as_deref()
                .expect("phase timing installs leaf timing"),
        );
    }
    let closed = clock.start.take().map(|start| TickCost {
        us: now.duration_since(start).as_secs_f64() * 1.0e6,
        march_us: clock.march_us,
        march_calls: clock.march_calls,
    });
    clock.closed = closed;
}

/// Open JSONL sink and its warmup gate, present only when the recorder is armed.
#[derive(Resource)]
struct CostTrace {
    sink: JsonlSink,
    /// Monotone recorder tick index and warmup gate: played ticks only, so in a match it counts
    /// from combat release ([`MatchCommands`]).
    tick: u64,
    /// Played ticks to skip before writing rows.
    warmup: u64,
    start: WorkloadStart,
}

/// Open the role-qualified sink and register recorder systems when tracing is armed.
fn install(app: &mut App, role: &'static str) -> bool {
    let Some(path) = crate::env_value("SPIKE_COST_TRACE").unwrap_or_else(crate::refuse_start)
    else {
        return false;
    };
    let resolved = role_path(&path, role);
    let sink = JsonlSink::create(&resolved).unwrap_or_else(crate::refuse_start);
    let warmup: u64 = crate::env_parse("SPIKE_COST_WARMUP")
        .unwrap_or_else(crate::refuse_start)
        .unwrap_or(384);
    info!(
        "cost: recording {role} rows to {} (warmup {warmup} ticks)",
        resolved.display()
    );
    app.insert_resource(CostTrace {
        sink,
        tick: 0,
        warmup,
        start: WorkloadStart::from_env(),
    });
    arm_tick_clock(app);
    app.add_systems(Startup, write_meta);
    app.add_systems(FixedMain, write_tick.after(TickClosed));
    true
}

fn write_meta(mut cost: ResMut<CostTrace>, fixed: Res<Time<Fixed>>, role: Res<CostRole>) {
    let tick_hz = 1.0 / fixed.timestep().as_secs_f64();
    let row = json!({
        "k": "meta",
        "role": role.0,
        "tick_hz": tick_hz,
        "ver": env!("CARGO_PKG_VERSION"),
        "warmup": cost.warmup,
    });
    cost.sink.write(&row);
}

/// Composition role for the startup metadata row.
#[derive(Resource)]
struct CostRole(&'static str);

/// `FixedMain`, after the clock closes: sample counts and write the row past warmup. A held tick
/// (the empty server or results, [`MatchCommands::Held`]) is neither counted nor written: the
/// workload under measurement starts at combat release and any runner signal, so tables hold no held
/// ticks. Both recorder compositions install the match-command publication.
fn write_tick(
    mut cost: ResMut<CostTrace>,
    clock: Res<TickClock>,
    match_commands: Res<MatchCommands>,
    projectiles: Query<(), With<Projectile>>,
    tanks: Query<(), With<Tank>>,
    all: Query<Entity>,
) {
    let Some(closed) = clock.closed() else {
        return;
    };
    if *match_commands == MatchCommands::Held || !cost.start.ready() {
        return;
    }
    let tick = cost.tick;
    cost.tick += 1;
    if tick < cost.warmup {
        return;
    }
    let np = projectiles.iter().count();
    let nt = tanks.iter().count();
    let ne = all.iter().count();
    let row = json!({
        "k": "tick",
        "t": tick,
        "us": closed.us,
        "mus": closed.march_us,
        "mc": closed.march_calls,
        "np": np,
        "nt": nt,
        "ne": ne,
    });
    cost.sink.write(&row);
}

/// MP server: fixed-tick cost rows (the authoritative tick — the headline server number).
pub fn server_plugin(app: &mut App) {
    if !install(app, "server") {
        return;
    }
    app.insert_resource(CostRole("server"));
}

/// MP client: fixed-tick cost rows (the client's cosmetic-march share of the sim).
pub fn client_plugin(app: &mut App) {
    if !install(app, "client") {
        return;
    }
    app.insert_resource(CostRole("client"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Cost tables must contain played ticks only: a hold neither spends warmup nor emits a row,
    /// including a results hold after recording has begun. Fixture costs are not measurements.
    #[test]
    fn held_ticks_neither_advance_warmup_nor_write_rows() {
        let path =
            std::env::temp_dir().join(format!("overmatch-held-cost-{}.jsonl", std::process::id()));
        let mut world = World::new();
        world.insert_resource(CostTrace {
            sink: JsonlSink::create(&path).unwrap(),
            tick: 0,
            warmup: 1,
            start: WorkloadStart::default(),
        });
        world.insert_resource(TickClock {
            closed: Some(TickCost {
                us: 1.0,
                march_us: 0.0,
                march_calls: 0,
            }),
            ..default()
        });
        world.insert_resource(MatchCommands::Held);
        for _ in 0..3 {
            world.run_system_once(write_tick).unwrap();
        }
        assert_eq!(
            world.resource::<CostTrace>().tick,
            0,
            "waiting never spends warmup"
        );
        world.insert_resource(MatchCommands::Live);
        let ready = path.with_extension("ready");
        assert!(!ready.exists(), "fixture start file must be absent");
        world.resource_mut::<CostTrace>().start = WorkloadStart::WaitingForFile(ready.clone());
        world.run_system_once(write_tick).unwrap();
        assert_eq!(
            world.resource::<CostTrace>().tick,
            0,
            "a live partial cohort spends no warmup"
        );
        std::fs::write(&ready, "").unwrap();
        world.run_system_once(write_tick).unwrap();
        std::fs::remove_file(ready).unwrap();
        world.run_system_once(write_tick).unwrap();
        assert_eq!(world.resource::<CostTrace>().tick, 2);
        world.insert_resource(MatchCommands::Held);
        world.run_system_once(write_tick).unwrap();
        assert_eq!(
            world.resource::<CostTrace>().tick,
            2,
            "results never advance the recorder"
        );
        world.insert_resource(MatchCommands::Live);
        world.run_system_once(write_tick).unwrap();
        drop(world.remove_resource::<CostTrace>().unwrap());
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            rows.iter()
                .map(|row| row["t"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2],
            "only the played ticks past warmup reach the persisted table"
        );
        std::fs::remove_file(path).unwrap();
    }
}
