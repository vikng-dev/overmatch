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
        "mgsc": crate::env_flag("SPIKE_MG_SHORTCIRCUIT", false),
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

/// The sim-tick BUDGET baseline: how long one authoritative fixed tick costs, wall-clock, on the
/// world we ship today.
///
/// This is an INSTRUMENT, not a gate. It asserts nothing about the number (it is machine-, load-
/// and thermal-dependent), it only measures and prints it, so it is `#[ignore]`d and run
/// deliberately:
///
/// ```text
/// cargo test --release sim_tick_cost_baseline -- --ignored --nocapture --test-threads=1
/// ```
///
/// `--test-threads=1` matters: a sibling headless boot running at the same time competes for the
/// same cores and inflates every percentile. `--release` matters too — the dev profile builds THIS
/// crate at `opt-level = 1` (only dependencies get 3), and the sim code is exactly this crate.
///
/// Why it lives here: this module already owns "what does a fixed tick cost" for the live game
/// (`SPIKE_COST_TRACE`), and the measurement below is the same question asked offline. It brackets
/// the fixed schedule with the same `FixedFirst`/`FixedLast` pair [`open_tick`]/[`close_tick`] use,
/// so the two numbers are comparable by construction.
#[cfg(test)]
mod baseline {
    use std::time::{Duration, Instant};

    use bevy::prelude::*;
    use bevy::time::TimeUpdateStrategy;

    use crate::SimPlugin;
    use crate::command::TankCommand;
    use crate::state::AppState;
    use crate::tank::{Controlled, Tank};

    /// Ticks measured after warmup. 640 ticks = 10 s of sim at 64 Hz — long enough for the
    /// percentiles to be stable, short enough that the whole run is a few seconds of wall clock.
    const MEASURED_TICKS: usize = 640;
    /// Ticks discarded before measuring: the belt contacts settle and the caches warm.
    const WARMUP_TICKS: usize = 192;
    /// Generous backstop so a wiring bug fails with a diagnosis instead of hanging the suite.
    const BOOT_DEADLINE: Duration = Duration::from_secs(120);

    /// Per-tick wall-clock samples, filled by the bracket below.
    #[derive(Resource, Default)]
    struct TickTimes {
        started: Option<Instant>,
        micros: Vec<f64>,
    }

    fn open(mut times: ResMut<TickTimes>) {
        times.started = Some(Instant::now());
    }

    fn close(mut times: ResMut<TickTimes>) {
        if let Some(started) = times.started.take() {
            let micros = started.elapsed().as_secs_f64() * 1.0e6;
            times.micros.push(micros);
        }
    }

    /// `sorted[p]` for a fraction `p` of the way through, nearest-rank. Empty input is impossible
    /// here (the caller measured [`MEASURED_TICKS`] > 0), but clamp rather than index blindly.
    fn percentile(sorted: &[f64], fraction: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
        sorted[index.min(sorted.len() - 1)]
    }

    /// The single-player composition, headless: no GPU, no window, no winit — the same shape
    /// `headless_test` boots, on the world we actually ship (the heightmap; NO `ForceFlatWorld`
    /// marker, so `terrain_grid::decode_height_grid` reads the real map). Deliberately a separate
    /// fixture from `headless_test`: a measurement rig must not silently inherit a gate's fixture
    /// choices.
    fn headless_sim() -> App {
        let mut app = App::new();
        app.add_plugins(crate::gpu_less_default_plugins(None))
            // Frozen clock during asset IO: colliderless tanks would otherwise free-fall for the whole
            // load. Started (one exact fixed loop per `update`) once the rig is bound.
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
        // UPSTREAM WORKAROUND, same as every other GPU-less boot of the tank glb: without it,
        // bevy_image 0.19 PANICS transcoding the Tiger's mipped UASTC KTX2 textures under
        // `CompressedImageFormats::NONE` ("range end index ... out of range" — MEASURED here
        // 2026-07-31, the baseline aborted at boot). Canonical mechanism comment + retirement
        // tripwire: the identical insertion in `headless_test` and `tests/bevy_ktx2_uastc_fallback.rs`.
        // Must precede `app.finish()` below: that is when the loaders read the resource.
        app.insert_resource(bevy::image::CompressedImageFormatSupport(
            bevy::image::CompressedImageFormats::ASTC_LDR,
        ));
        app.add_plugins((
            avian3d::prelude::PhysicsPlugins::default(),
            SimPlugin,
            crate::tank::sp_spawn_plugin,
        ))
        .init_resource::<TickTimes>()
        .add_systems(FixedFirst, open)
        .add_systems(FixedLast, close);
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app
    }

    /// MEASURE the current per-tick sim cost and print it. See the module-level doc above for how
    /// to run it and why it never fails on the number.
    #[test]
    #[ignore = "measurement instrument, not a gate — machine-dependent; run explicitly"]
    fn sim_tick_cost_baseline() {
        let mut app = headless_sim();

        // Asset IO is genuinely async (spec RON + tiger_1.glb); yield to those threads each pass.
        let started = Instant::now();
        while *app.world().resource::<State<AppState>>().get() != AppState::Playing {
            app.update();
            assert!(
                started.elapsed() < BOOT_DEADLINE,
                "the sim never reached Playing headless — see headless_test::boot_diagnosis for \
                 the usual causes (a Git LFS pointer instead of the glb is the common one)",
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut wheels = 0;
        let started = Instant::now();
        while started.elapsed() < BOOT_DEADLINE && wheels < 32 {
            app.update();
            let world = app.world_mut();
            wheels = world.query::<&crate::tank::Roadwheel>().iter(world).count();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            wheels >= 32,
            "the rigs never bound headless ({wheels} wheels)"
        );

        // Real time now: exactly one 64 Hz fixed loop per `update`.
        app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));

        let mut tank_q = app
            .world_mut()
            .query_filtered::<Entity, (With<Tank>, With<Controlled>)>();
        let tank = tank_q.single(app.world()).expect("one controlled tank");

        // Warmup, then measurement — both DRIVING. An idle tank measures the cheap case; the
        // number that has to scale is the one with belts loaded, the transmission scheduling and
        // the contact solve doing work. The command is level state and there is no device gather
        // headless, so it is re-asserted every tick.
        let drive = |app: &mut App| {
            app.world_mut()
                .get_mut::<TankCommand>(tank)
                .expect("tank carries a command")
                .throttle = 1.0;
            app.update();
        };
        for _ in 0..WARMUP_TICKS {
            drive(&mut app);
        }
        app.world_mut().resource_mut::<TickTimes>().micros.clear();
        for _ in 0..MEASURED_TICKS {
            drive(&mut app);
        }

        let tanks = {
            let world = app.world_mut();
            world.query_filtered::<(), With<Tank>>().iter(world).count()
        };
        let entities = {
            let world = app.world_mut();
            world.query::<Entity>().iter(world).count()
        };
        let mut micros = app.world().resource::<TickTimes>().micros.clone();
        assert!(
            micros.len() >= MEASURED_TICKS,
            "the FixedFirst/FixedLast bracket recorded {} ticks, expected {MEASURED_TICKS} — one \
             `App::update` must run exactly one fixed loop under FixedTimesteps(1)",
            micros.len(),
        );
        let mean = micros.iter().sum::<f64>() / micros.len() as f64;
        micros.sort_by(f64::total_cmp);
        let (p50, p95, p99) = (
            percentile(&micros, 0.50),
            percentile(&micros, 0.95),
            percentile(&micros, 0.99),
        );
        // The tick budget at 64 Hz. Reported as a fraction so the headroom question ("what does
        // this cost 30x over?") is answered on the line rather than in someone's head.
        let budget_us = 1.0e6 / 64.0;
        println!(
            "\nsim tick cost baseline (MEASURED, headless, driving, release build recommended)\n  \
               world ................ shipped heightmap, SP duel spawn\n  \
               tanks / entities ..... {tanks} / {entities}\n  \
               ticks measured ....... {MEASURED_TICKS} (after {WARMUP_TICKS} warmup)\n  \
               mean ................. {mean:8.1} us/tick  ({:.1}% of the {budget_us:.0} us 64 Hz budget)\n  \
               p50 / p95 / p99 ...... {p50:8.1} / {p95:.1} / {p99:.1} us\n  \
               per tank (mean) ...... {:8.1} us\n  \
               DERIVED 15v15 (30 tanks, linear) ... {:.1} us/tick ({:.1}% of budget)\n\
             The linear extrapolation is a FLOOR, not a prediction: broad-phase pairs and the\n\
             contact solve grow superlinearly with vehicle count, and it ignores netcode entirely.\n",
            100.0 * mean / budget_us,
            mean / tanks.max(1) as f64,
            30.0 * mean / tanks.max(1) as f64,
            100.0 * (30.0 * mean / tanks.max(1) as f64) / budget_us,
        );
        assert!(mean > 0.0, "the tick timer recorded no time at all");
    }
}
