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

use std::time::Instant;

use bevy::prelude::*;
use serde_json::json;

use crate::ballistics::Projectile;
use crate::tank::Tank;
use crate::trace::{JsonlSink, role_path};

/// The fixed-tick timer, present only while some consumer has armed it.
#[derive(Resource, Default)]
pub(crate) struct TickClock {
    /// `Instant` captured at `FixedFirst` this tick; the whole-schedule timer's start.
    start: Option<Instant>,
    /// Accumulated `integrate_projectiles` wall micros this tick.
    march_us: f64,
    /// Number of projectile-march calls this tick.
    march_calls: u32,
    /// The tick [`close_tick`] closed this `FixedLast`; `None` when no tick was opened.
    closed: Option<TickCost>,
}

/// One closed fixed tick.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TickCost {
    /// Wall micros from `FixedFirst` to `FixedLast`.
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

    /// The tick closed this `FixedLast`, for a reader ordered after [`TickClosed`].
    pub(crate) fn closed(&self) -> Option<TickCost> {
        self.closed
    }
}

/// The `FixedLast` set holding the clock's close; every reader of [`TickClock::closed`] runs after
/// it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TickClosed;

/// Arm the clock. Idempotent, so each consumer arms it without knowing about the others.
pub(crate) fn arm_tick_clock(app: &mut App) {
    if app.world().contains_resource::<TickClock>() {
        return;
    }
    app.init_resource::<TickClock>();
    // Invariant: `FixedFirst` and `FixedLast` bracket only the complete fixed schedule.
    app.add_systems(FixedFirst, open_tick);
    app.add_systems(FixedLast, close_tick.in_set(TickClosed));
}

/// `FixedFirst`: open the per-tick timer and clear the march accumulator.
fn open_tick(mut clock: ResMut<TickClock>) {
    clock.start = Some(Instant::now());
    clock.march_us = 0.0;
    clock.march_calls = 0;
}

/// `FixedLast`: capture the elapsed time before any reader samples counts.
fn close_tick(mut clock: ResMut<TickClock>) {
    let closed = clock.start.take().map(|start| TickCost {
        us: start.elapsed().as_secs_f64() * 1.0e6,
        march_us: clock.march_us,
        march_calls: clock.march_calls,
    });
    clock.closed = closed;
}

/// Open JSONL sink and its warmup gate, present only when the recorder is armed.
#[derive(Resource)]
struct CostTrace {
    sink: JsonlSink,
    /// Monotone recorder tick index and warmup gate.
    tick: u64,
    /// Ticks to skip before writing rows.
    warmup: u64,
}

/// Open the role-qualified sink and register recorder systems when tracing is armed.
fn install(app: &mut App, role: &'static str) -> bool {
    let Some(path) = crate::env_value("SPIKE_COST_TRACE") else {
        return false;
    };
    let resolved = role_path(&path, role);
    let sink = match JsonlSink::create(&resolved) {
        Ok(sink) => sink,
        Err(err) => {
            error!("cost: cannot open {}: {err}", resolved.display());
            return false;
        }
    };
    let warmup: u64 = crate::env_parse("SPIKE_COST_WARMUP").unwrap_or(384);
    info!(
        "cost: recording {role} rows to {} (warmup {warmup} ticks)",
        resolved.display()
    );
    app.insert_resource(CostTrace {
        sink,
        tick: 0,
        warmup,
    });
    arm_tick_clock(app);
    app.add_systems(Startup, write_meta);
    app.add_systems(FixedLast, write_tick.after(TickClosed));
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
        "mgsc": crate::env_flag("SPIKE_MG_SHORTCIRCUIT", false),
    });
    cost.sink.write(&row);
}

/// Composition role for the startup metadata row.
#[derive(Resource)]
struct CostRole(&'static str);

/// `FixedLast`, after the clock closes: sample counts and write the row past warmup.
fn write_tick(
    mut cost: ResMut<CostTrace>,
    clock: Res<TickClock>,
    projectiles: Query<(), With<Projectile>>,
    tanks: Query<(), With<Tank>>,
    all: Query<Entity>,
) {
    let Some(closed) = clock.closed() else {
        return;
    };
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
