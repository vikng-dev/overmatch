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

    // Boot: asset IO is genuinely async, so poll until the spec loads and the app enters Playing.
    for _ in 0..3000 {
        app.update();
        if *app.world().resource::<State<AppState>>().get() == AppState::Playing {
            break;
        }
    }
    assert_eq!(
        *app.world().resource::<State<AppState>>().get(),
        AppState::Playing,
        "sim never reached Playing headless — spec or scene load failed"
    );

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
