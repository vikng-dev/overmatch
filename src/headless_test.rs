//! The dedicated-server guard: the whole sim must boot and run with **no GPU, no window, no
//! winit** — the M5 dedicated-server configuration. If a sim system grows a hard render
//! dependency, this test fails before a netcode integration ever would.
//!
//! (An earlier version hand-assembled `MinimalPlugins` + individual asset/scene/gltf plugins;
//! the gltf load never completed under that set. The canonical headless recipe — full
//! `DefaultPlugins` with `backends: None` and no window — is what real Bevy dedicated servers
//! use, and what the server binary will mount. Compile-out of render code is the later
//! crates-split step, per the client-server-organization decision.)

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use crate::SimPlugin;
use crate::command::TankCommand;
use crate::state::AppState;
use crate::tank::{Controlled, Tank};

/// Boot the sim headless, then drive the tank by writing its `TankCommand` directly — the exact
/// path a server takes applying a remote client's command (no device gather mounted).
#[test]
fn sim_boots_and_drives_headless() {
    let mut app = App::new();
    // The canonical Bevy dedicated-server configuration: full plugin registration (assets,
    // scenes, gltf, types) but **no GPU** (`backends: None` — wgpu never initializes), **no
    // window, no winit**. This is exactly what the M5 server binary will mount.
    app.add_plugins(
        DefaultPlugins
            .set(bevy::render::RenderPlugin {
                render_creation: bevy::render::settings::WgpuSettings {
                    backends: None,
                    ..default()
                }
                .into(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>(),
    )
    // Deterministic clock, in two phases: **zero** while assets load (asset IO is wall-clock; if
    // sim time advanced meanwhile, the collider-less tanks would free-fall through the terrain
    // for the whole load — the same spawn-before-bind race the game keeps to a frame or two),
    // then exactly 16 ms per `update()` so the 64 Hz fixed sim ticks once per update.
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    // Physics + the SP spawn scenario are composition-root choices (see lib.rs SimPlugin note);
    // this test exercises the single-player-shaped boot, headless.
    app.add_plugins((
        avian3d::prelude::PhysicsPlugins::default(),
        SimPlugin,
        crate::tank::sp_spawn_plugin,
    ));

    // `App::run` normally drives plugin finish/cleanup (some registration — e.g. Avian's
    // diagnostics resources — happens in `Plugin::finish`); a bare `update()` loop must do it.
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();

    // Boot: asset IO is genuinely async and runs on wall-clock IO threads (the tank spec RON +
    // tiger_1.glb), so poll until the spec loads and the app enters Playing. Each not-yet-Playing
    // iteration yields 1 ms to those IO threads — a bare CPU-bound spin (no yield) starves them on
    // a loaded 2-core CI runner and can burn through the whole bound before the glb finishes,
    // which was the headless-boot flake. The sleep is WALL-CLOCK only: the clock is still
    // `ManualDuration(ZERO)` here (Phase 1 below), so no sim tick advances and the frozen-load
    // invariant documented above holds untouched. A wall-clock deadline (not a fixed spin count)
    // makes the wait obviously bounded and deadlock-free — and the Playing early-exit keeps the
    // fast path (local machine, IO already done) spin-free.
    let boot_deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        app.update();
        if *app.world().resource::<State<AppState>>().get() == AppState::Playing {
            break;
        }
        assert!(
            std::time::Instant::now() < boot_deadline,
            "sim never reached Playing headless — spec or scene load failed"
        );
        std::thread::sleep(Duration::from_millis(1));
    }

    // Phase 1 (sim time frozen): poll real-time asset IO until both rigs are fully bound.
    let mut wheels = 0;
    for _ in 0..5000 {
        app.update();
        let world = app.world_mut();
        wheels = world.query::<&crate::tank::Roadwheel>().iter(world).count();
        if wheels >= 32 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        wheels >= 32,
        "rigs never bound headless (scene/spec bind failed?); roadwheels: {wheels}"
    );

    // Phase 2: start the clock and let the suspension ground and settle.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        16,
    )));
    let mut grounded = 0;
    for _ in 0..300 {
        app.update();
        let world = app.world_mut();
        grounded = world
            .query::<&crate::driving::Suspension>()
            .iter(world)
            .filter(|s| s.contact.is_some())
            .count();
        if grounded >= 8 {
            break;
        }
    }
    assert!(
        grounded >= 8,
        "suspension never grounded headless; grounded wheels: {grounded}"
    );
    // Settle for a second of sim time.
    for _ in 0..60 {
        app.update();
    }

    let mut tank_q = app
        .world_mut()
        .query_filtered::<(Entity, &Transform), (With<Tank>, With<Controlled>)>();
    let (tank, start) = tank_q.single(app.world()).expect("one controlled tank");
    let start = start.translation;

    // Full throttle via the command — the server's apply-remote-input path.
    app.world_mut()
        .entity_mut(tank)
        .get_mut::<TankCommand>()
        .expect("tank carries a command")
        .throttle = 1.0;

    // ~4 sim-seconds of driving. The command is level state (no gather to re-write it), so it
    // holds; the ramp, suspension, and drive all run on the fixed clock.
    for _ in 0..250 {
        app.update();
        app.world_mut()
            .entity_mut(tank)
            .get_mut::<TankCommand>()
            .unwrap()
            .throttle = 1.0;
    }

    let mut tank_q = app
        .world_mut()
        .query_filtered::<&Transform, (With<Tank>, With<Controlled>)>();
    let end = tank_q
        .single(app.world())
        .expect("tank survived")
        .translation;
    let horizontal = Vec3::new(end.x - start.x, 0.0, end.z - start.z).length();
    assert!(
        horizontal > 2.0,
        "full throttle for ~4 s should move the tank on flat ground; moved {horizontal:.2} m \
         (sim not actually running headless?)"
    );
}

/// The MG-tracer render gate, exercised on the real spawn path headless (the same dedicated-server
/// recipe as above). Firing the secondary trigger (the two 7.9 mm MGs) must, over a burst:
///   * spawn tracer STREAKS (`TracerStreak`) for the ~1-in-5 tracer rounds, and
///   * spawn NO `shell.glb` scene root on ANY MG round — the bug this slice fixes (MG bullets used to
///     render as full 88 mm shell scenes). A shell in flight carries `ShellPath`; only a
///     main-gun-calibre round also gets a `WorldAssetRoot` scene, so `ShellPath + WorldAssetRoot`
///     over an MG-only burst must stay empty while streaks appear.
#[test]
fn mg_rounds_stream_tracers_and_spawn_no_shell_scene() {
    use crate::ballistics::{ShellPath, TracerStreak};
    use bevy::world_serialization::WorldAssetRoot;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(bevy::render::RenderPlugin {
                render_creation: bevy::render::settings::WgpuSettings {
                    backends: None,
                    ..default()
                }
                .into(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>(),
    )
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    app.add_plugins((
        avian3d::prelude::PhysicsPlugins::default(),
        SimPlugin,
        crate::tank::sp_spawn_plugin,
    ));

    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();

    // Boot to Playing. Same spin-vs-async-IO race as the sibling test above: poll `app.update()`,
    // but yield 1 ms of WALL-CLOCK time (clock still `ManualDuration(ZERO)`, so no sim tick
    // advances) to the glb/RON IO threads each not-yet-Playing pass, bounded by a wall-clock
    // deadline. Without the yield, a loaded CI runner can starve the IO threads and exhaust the
    // bound before the assets load — the headless-boot flake. The Playing early-exit keeps the
    // fast path spin-free.
    let boot_deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        app.update();
        if *app.world().resource::<State<AppState>>().get() == AppState::Playing {
            break;
        }
        assert!(
            std::time::Instant::now() < boot_deadline,
            "sim never reached Playing headless",
        );
        std::thread::sleep(Duration::from_millis(1));
    }

    // Real-time asset IO (sim clock frozen) until the rig binds — the muzzles/weapons must exist for
    // `fire` to find a bore.
    let mut wheels = 0;
    for _ in 0..5000 {
        app.update();
        let world = app.world_mut();
        wheels = world.query::<&crate::tank::Roadwheel>().iter(world).count();
        if wheels >= 32 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        wheels >= 32,
        "rig never bound headless; roadwheels: {wheels}"
    );

    // Start the sim clock and let the tank settle a few ticks.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        16,
    )));
    for _ in 0..30 {
        app.update();
    }

    let mut tank_q = app
        .world_mut()
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>();
    let tank = tank_q.single(app.world()).expect("one controlled tank");

    // Hold the secondary trigger (the MGs) — a burst. Do NOT press primary, so no 88 round is fired.
    // The MGs are `Automatic(rpm: 750)` — a 0.08 s cyclic interval (~5 ticks) — so ~60 ticks yields
    // ~10 shots per MG across the two MGs, with the belt's tracer_every=5 giving several tracer
    // rounds. The 150-round belts stay far from dry (~12 rounds each), so no belt swap interrupts
    // the burst.
    let mut saw_streak = false;
    let mut saw_mg_shell_scene = false;
    let mut saw_shell = false;
    for _ in 0..60 {
        // Re-assert each tick (in its own scope so the command borrow ends before `update`): the
        // command layer clears edge fields, and there is no device gather to hold the level fields.
        {
            let mut entity = app.world_mut().entity_mut(tank);
            let mut cmd = entity
                .get_mut::<TankCommand>()
                .expect("tank carries a command");
            cmd.fire_secondary = true;
            cmd.fire_primary = false;
        }
        app.update();

        let world = app.world_mut();
        if world.query::<&TracerStreak>().iter(world).count() > 0 {
            saw_streak = true;
        }
        let world = app.world_mut();
        if world.query::<&ShellPath>().iter(world).count() > 0 {
            saw_shell = true;
        }
        let world = app.world_mut();
        if world
            .query_filtered::<(), (With<ShellPath>, With<WorldAssetRoot>)>()
            .iter(world)
            .count()
            > 0
        {
            saw_mg_shell_scene = true;
        }
    }

    assert!(
        saw_shell,
        "the MG burst never spawned a single shell — the fire gate, cyclic interval, or belt never \
         let it fire",
    );
    assert!(
        saw_streak,
        "MG tracer rounds spawned no TracerStreak — the streak visual never attached",
    );
    assert!(
        !saw_mg_shell_scene,
        "an MG round spawned a shell.glb scene root (WorldAssetRoot) — the very bug this fixes: MG \
         bullets must NOT render as 88 mm shell scenes",
    );
}
