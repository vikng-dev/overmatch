//! Optional fixed-schedule cost recorder, enabled by `SPIKE_COST_TRACE`.
//!
//! It writes role-qualified JSONL rows for the complete fixed schedule and projectile-march share.
//! `scripts/cost/analyze.py` consumes the rows.
//! Invariant: use a distinct base path from `SPIKE_TRACE`; both recorders otherwise open the same
//! role-qualified file independently.

use std::time::Instant;

use bevy::prelude::*;
use serde_json::json;

use crate::ballistics::Projectile;
use crate::tank::Tank;
use crate::trace::{JsonlSink, role_path};

/// Open cost sink and per-tick accumulator, present only when tracing is armed.
#[derive(Resource)]
pub(crate) struct CostTrace {
    sink: JsonlSink,
    /// Monotone recorder tick index and warmup gate.
    tick: u64,
    /// Ticks to skip before writing rows.
    warmup: u64,
    /// `Instant` captured at `FixedFirst` this tick; the whole-schedule timer's start.
    tick_start: Option<Instant>,
    /// Accumulated `integrate_projectiles` wall micros this tick.
    march_us: f64,
    /// Number of projectile-march calls this tick.
    march_calls: u32,
}

impl CostTrace {
    /// Attribute projectile-march wall micros to the current tick.
    pub(crate) fn record_march(&mut self, us: f64) {
        self.march_us += us;
        self.march_calls += 1;
    }
}

/// Open the role-qualified sink and register recorder systems when tracing is armed.
fn install(app: &mut App, role: &'static str) -> bool {
    let Ok(path) = std::env::var("SPIKE_COST_TRACE") else {
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
    let warmup: u64 = std::env::var("SPIKE_COST_WARMUP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(384);
    info!(
        "cost: recording {role} rows to {} (warmup {warmup} ticks)",
        resolved.display()
    );
    app.insert_resource(CostTrace {
        sink,
        tick: 0,
        warmup,
        tick_start: None,
        march_us: 0.0,
        march_calls: 0,
    });
    app.add_systems(Startup, write_meta);
    // Invariant: `FixedFirst` and `FixedLast` bracket only the complete fixed schedule.
    app.add_systems(FixedFirst, open_tick);
    app.add_systems(FixedLast, close_tick);
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
        "mgsc": std::env::var("SPIKE_MG_SHORTCIRCUIT").is_ok(),
    });
    cost.sink.write(&row);
}

/// Composition role for the startup metadata row.
#[derive(Resource)]
struct CostRole(&'static str);

/// `FixedFirst`, first: open the per-tick timer and clear the march accumulator.
fn open_tick(mut cost: ResMut<CostTrace>) {
    cost.tick_start = Some(Instant::now());
    cost.march_us = 0.0;
    cost.march_calls = 0;
}

/// `FixedLast`: capture elapsed time before sampling counts and writing past warmup.
fn close_tick(
    mut cost: ResMut<CostTrace>,
    projectiles: Query<(), With<Projectile>>,
    tanks: Query<(), With<Tank>>,
    all: Query<Entity>,
) {
    let Some(start) = cost.tick_start.take() else {
        return;
    };
    let us = start.elapsed().as_secs_f64() * 1.0e6;
    let march_us = cost.march_us;
    let march_calls = cost.march_calls;
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
        "us": us,
        "mus": march_us,
        "mc": march_calls,
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
