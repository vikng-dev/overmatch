//! `SPIKE_COST_TRACE`: a per-fixed-tick SIM-COST recorder — the reusable sibling of the jitter
//! recorder (`crate::trace`), built to answer "what does machine-gun fire cost the FixedUpdate
//! tick?" It times the whole fixed schedule (`FixedFirst`→`FixedLast`) per tick, attributes the
//! projectile ray-march (`ballistics::integrate_projectiles`) inside that window, and stamps the
//! entity/projectile/tank counts so spawn-despawn churn is visible next to the compute cost.
//!
//! OFF unless `SPIKE_COST_TRACE=<path>` is set: the recorder resource + systems are REGISTERED
//! only in an armed run, so an unset var costs one `std::env::var` lookup at plugin-build and
//! nothing thereafter (same discipline as `crate::trace`). The sink path is role-qualified through
//! the shared `trace::role_path`, so a server and client launched from one shell with the same
//! `SPIKE_COST_TRACE` value write `<path>.server.jsonl` / `<path>.client.jsonl` and never truncate
//! each other. It is NOT de-conflicted against the jitter recorder, though: give `SPIKE_TRACE` and
//! `SPIKE_COST_TRACE` DIFFERENT base paths when arming both, or the two recorders role-qualify to
//! the SAME file and tear each other's lines (measured: two independent `File::create` handles on
//! one path → interleaved truncating writes, ~torn rows the parsers then skip).
//!
//! Rows (JSONL, one per fixed tick past the warmup):
//! - `meta`  — once at Startup: `role`, `tick_hz`, `ver` (crate version), `warmup` (skipped ticks),
//!   `mgsc` (was the experimental MG short-circuit A/B arm armed this process — see
//!   `ballistics::MgShortCircuit`).
//! - `tick`  — `t` fixed-tick index, `us` whole-fixed-schedule wall micros (the headline
//!   "FixedUpdate tick time"), `mus` the `integrate_projectiles` share of it (micros), `mc` how
//!   many times the march ran this tick (rollback replay can re-run the fixed loop), `np` live
//!   projectile count, `nt` tank count, `ne` total entity count.
//!
//! Timing is wall-clock `Instant` around the fixed schedule; on a shared box it carries scheduler
//! noise, which is exactly why the summarizer (`scripts/cost/analyze.py`) reports percentiles over
//! a long window rather than a mean. The warmup (`SPIKE_COST_WARMUP`, default 384 ticks ≈ 6 s)
//! drops the connect/spawn/asset-load transient so the reported window is steady state.

use std::time::Instant;

use bevy::prelude::*;
use serde_json::json;

use crate::ballistics::Projectile;
use crate::tank::Tank;
use crate::trace::{JsonlSink, role_path};

/// The open cost sink + the per-tick accumulator. Present iff `SPIKE_COST_TRACE` was set at
/// startup ([`install`] inserts it and returns whether it did, so the recorder systems gate on
/// that at registration time, never per-frame).
#[derive(Resource)]
pub(crate) struct CostTrace {
    sink: JsonlSink,
    /// Fixed-tick index (own counter — monotone across the whole run, unaffected by the net
    /// timeline's tick numbering), also the warmup gate.
    tick: u64,
    /// Ticks to skip before writing rows: drops the connect/spawn/asset-load transient.
    warmup: u64,
    /// `Instant` captured at `FixedFirst` this tick; the whole-schedule timer's start.
    tick_start: Option<Instant>,
    /// Accumulated `integrate_projectiles` wall-micros this tick (ballistics adds to it; a rollback
    /// replay that re-runs the fixed loop adds again, counted by `march_calls`).
    march_us: f64,
    /// How many times the march ran this tick (>1 only under a fixed-loop re-run / rollback replay).
    march_calls: u32,
}

impl CostTrace {
    /// Attribute `us` micros of `integrate_projectiles` compute to the current tick. Called from
    /// `ballistics::integrate_projectiles` behind an `Option<ResMut<CostTrace>>` — inert (the
    /// resource is absent) unless the recorder is armed.
    pub(crate) fn record_march(&mut self, us: f64) {
        self.march_us += us;
        self.march_calls += 1;
    }
}

/// Open the role-qualified sink and register the recorder systems — only when `SPIKE_COST_TRACE`
/// is set. Returns `true` iff armed, so each composition root registers its recorders only in a
/// traced run.
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
    // Meta row from Startup (not here) so `tick_hz` reads the app's actual `Time<Fixed>` timestep —
    // matches `crate::trace`'s ordering guarantee (Startup runs before any FixedUpdate recorder).
    app.add_systems(Startup, write_meta);
    // Bracket the whole fixed schedule: start the timer FIRST in `FixedFirst`, close it LAST in
    // `FixedLast`. The delta is the per-tick fixed-schedule compute (FixedPreUpdate/Update/PostUpdate
    // included) — precisely "FixedUpdate tick time" — and excludes the Main-schedule replication/render
    // that lives outside `FixedMain`.
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

/// Carries the composition role into `write_meta` (a `Res` reads cleaner than threading it through
/// the resource, and keeps `CostTrace` free of a field only Startup touches).
#[derive(Resource)]
struct CostRole(&'static str);

/// `FixedFirst`, first: open the per-tick timer and clear the march accumulator.
fn open_tick(mut cost: ResMut<CostTrace>) {
    cost.tick_start = Some(Instant::now());
    cost.march_us = 0.0;
    cost.march_calls = 0;
}

/// `FixedLast`, last: close the timer, sample the counts (AFTER the timing is captured, so counting
/// never inflates the reported tick time), and write the row once past the warmup.
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
