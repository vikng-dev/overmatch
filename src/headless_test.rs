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

/// Boot the sim headless (the dedicated-server recipe above) with the single-player two-tank scenario,
/// run it to a bound, settled rig, and hand back the app — the shared scaffolding for the shooting
/// tests below, which need the REAL tiger geometry (a synthetic plate cannot reproduce a muzzle that
/// recoils behind its own mantlet).
fn booted_sp_app() -> App {
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

    // Boot to Playing on wall-clock asset IO with the sim clock frozen (see the boot notes above).
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        app.update();
        if *app.world().resource::<State<AppState>>().get() == AppState::Playing {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "sim never reached Playing headless",
        );
        std::thread::sleep(Duration::from_millis(1));
    }
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

    // Start the sim clock and let the tanks settle onto their suspension.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        16,
    )));
    for _ in 0..30 {
        app.update();
    }
    app
}

/// SHOOTER SELF-EXCLUSION on the REAL asset — the coax bug, at its root.
///
/// The tiger's coax muzzle clears its own `Gun_Mantlet_Ballistic` by ~7 cm at rest, and its recoil
/// spring retracts the barrel ~10 cm: from the second round of any burst on, the coax fires from INSIDE
/// its own mantlet. With no self-exclusion the authority march resolved that contact for real and the
/// 7.9 mm round EMBEDDED in its own mask a centimetre out of the barrel — the coax was a dud that shot
/// itself, in single-player and on the server alike (and on a net client the same contact fail-closed,
/// which is why the coax tracer never replicated while the bow MG — with no volume in front of it —
/// always did).
///
/// So: hold the secondary trigger for a sustained burst and assert NO round ever impacts a volume of
/// the tank that fired it, while rounds still reach the opposing tank downrange. The bow MG rides along
/// as the control that always worked.
#[test]
fn a_burst_never_shoots_its_own_tank() {
    use crate::ballistics::{BallisticVolume, Impact};
    use crate::damage::VolumeOf;
    use avian3d::prelude::{LayerMask, SpatialQuery, SpatialQueryFilter};

    /// Every MG impact, tagged with how far it landed from the firing tank's muzzle.
    #[derive(Resource, Default)]
    struct SelfHits {
        muzzle: Vec3,
        /// Impacts on a volume owned by the FIRING tank — must stay empty.
        own: Vec<f32>,
        /// Impacts anywhere else (the target, the terrain) — must NOT be empty, or the burst never flew.
        away: usize,
    }

    let mut app = booted_sp_app();
    app.init_resource::<SelfHits>();

    let mut tank_q = app
        .world_mut()
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>();
    let shooter = tank_q.single(app.world()).expect("one controlled tank");

    // The FIRING tank's own volumes — the set no round of its own may ever resolve against.
    let mut own_volumes = app.world_mut().query::<(Entity, &VolumeOf)>();
    let own: Vec<Entity> = own_volumes
        .iter(app.world())
        .filter(|(_, owner)| owner.tank() == shooter)
        .map(|(volume, _)| volume)
        .collect();
    assert!(
        own.len() > 20,
        "the firing tank should own its whole volume set; got {}",
        own.len()
    );
    app.world_mut().resource_mut::<SelfHits>().muzzle = Vec3::ZERO;

    // Classify every MG `Impact` by whether the struck geometry belongs to the shooter. The impact
    // carries no entity, so we re-resolve it the way the march does: cast a hair back along the
    // surface normal into whatever was struck and walk that hit's volume ancestry.
    app.add_observer(
        move |impact: On<Impact>,
              spatial: SpatialQuery,
              owners: Query<&VolumeOf>,
              volumes: Query<&BallisticVolume>,
              parents: Query<&ChildOf>,
              mut hits: ResMut<SelfHits>| {
            if impact.caliber > crate::ballistics::TRACER_MAX_CALIBER {
                return; // MG rounds only
            }
            let Ok(into) = Dir3::new(-impact.normal) else {
                return;
            };
            let probe = spatial.cast_ray(
                impact.position - Vec3::from(into) * 0.01,
                into,
                0.5,
                true,
                &SpatialQueryFilter::from_mask(
                    LayerMask::from(crate::Layer::Terrain) | LayerMask::from(crate::Layer::Armor),
                ),
            );
            let struck = probe
                .and_then(|hit| crate::damage::hit_ancestor(hit.entity, &volumes, &parents))
                .and_then(|(node, _)| owners.get(node).ok())
                .map(|owner| owner.tank());
            match struck {
                Some(tank) if tank == shooter => hits.own.push(impact.position.length()),
                _ => hits.away += 1,
            }
        },
    );

    // A sustained burst: both MGs are `Automatic(750 rpm)` — ~5 ticks apart — so 120 ticks is ~24
    // rounds per gun, far past the first-round-of-the-burst case that always worked.
    for _ in 0..120 {
        {
            let mut entity = app.world_mut().entity_mut(shooter);
            let mut cmd = entity
                .get_mut::<TankCommand>()
                .expect("tank carries a command");
            cmd.fire_secondary = true;
            cmd.fire_primary = false;
        }
        app.update();
    }

    let hits = app.world().resource::<SelfHits>();
    assert!(
        hits.own.is_empty(),
        "{} MG round(s) impacted the FIRING tank's own armour — a shell must be transparent to the \
         tank that fired it (`ballistics::not_own_volume`). The coax fires from inside its own \
         mantlet on every round after a burst's first: with no self-exclusion it embeds there, deals \
         no damage, and (on a net client) its tracer never appears.",
        hits.own.len(),
    );
    assert!(
        hits.away > 0,
        "the burst produced no impacts at all — the MGs never fired, so this test proves nothing",
    );
}

/// THE REPLICA (OBSERVER) PATH — Yan's report: "the hull MG tracers replicate perfectly, but the
/// coaxial don't."
///
/// An observing client re-raises the shooter's shot as a local `FireShell` off the wire
/// (`net::client::receive_fire_events`) with a `catch_up_ticks` fast-forward, and `on_fire_shell` first
/// tests whether the round already landed during the skipped flight — a segment cast from the wire
/// origin, i.e. FROM THE SHOOTER'S MUZZLE. Mid-burst that muzzle sits inside the shooter's own mantlet,
/// so without self-exclusion that test reports an immediate hit and returns: the observer's shell is
/// never spawned at all (no tracer, plus a phantom armour spark at the shooter's mask).
///
/// Fire the exact shape the wire produces — origin 12 cm behind the coax muzzle (the recoil retraction),
/// `shooter` named and entity-mapped, `catch_up_ticks > 0`, `ClientReplica` present — and assert the
/// shell IS spawned and flies. The `shooter: None` control (the old code's shape) is what proves the
/// naming is load-bearing rather than incidental.
#[test]
fn a_replica_coax_shell_clears_the_shooters_mantlet() {
    use crate::ClientReplica;
    use crate::ShotId;
    use crate::ballistics::{FireShell, ShellPath, ShotSource};
    use crate::tank::{Muzzle, TankRoot, Weapon, WeaponIndex, rig_world_pose};
    use avian3d::prelude::{Position, Rotation};
    use bevy::ecs::system::RunSystemOnce;

    /// The coax's wire-shaped fire: where a mid-burst round's origin actually is, and which tank/slot
    /// the `FireEvent` names.
    #[derive(Resource, Clone, Copy)]
    struct CoaxShot {
        origin: Vec3,
        direction: Dir3,
        tank: Entity,
        slot: usize,
    }

    let mut app = booted_sp_app();
    // A net client is a REPLICA: it deposits no damage and fail-closes at armor contact. This is the
    // configuration in which a self-hit silently swallows the tracer.
    app.insert_resource(ClientReplica);

    // The coax's muzzle pose, then pushed 12 cm BACK down the bore — the recoil retraction that puts a
    // mid-burst round's origin inside `Gun_Mantlet_Ballistic` (the muzzle clears the mantlet by ~7 cm;
    // the coax recoil spring pulls it ~10 cm back). This is the origin the server puts on the wire.
    let shot = app
        .world_mut()
        .run_system_once(
            |muzzles: Query<(Entity, &Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
             controlled: Query<Entity, (With<Tank>, With<Controlled>)>,
             roots: Query<(&Position, &Rotation)>,
             parents: Query<&ChildOf>,
             locals: Query<&Transform>|
             -> CoaxShot {
                let tank = controlled.single().expect("one controlled tank");
                let (muzzle, _, slot, _) = muzzles
                    .iter()
                    .find(|(_, weapon, _, root)| weapon.name == "Coax" && root.0 == tank)
                    .expect("the tiger carries a coax");
                let (position, rotation) = roots.get(tank).expect("root pose");
                let (origin, rot) =
                    rig_world_pose(muzzle, tank, position.0, rotation.0, &parents, &locals)
                        .expect("muzzle pose");
                let bore = Dir3::new(rot * Vec3::NEG_Z).expect("bore");
                // Elevate the shot 20° so its ~47 m catch-up flies into open SKY, clearing the second
                // SP tank (14.8 m down the flat bore) and the ground. The catch-up's already-landed
                // test is honest — a round that really did land during the skipped flight must spawn no
                // tracer — so the only thing left in this shot's way is the shooter's OWN mantlet,
                // which is exactly what the test is about.
                let up = Quat::from_axis_angle(rot * Vec3::X, 20.0_f32.to_radians());
                let direction = Dir3::new(up * Vec3::from(bore)).expect("elevated bore");
                CoaxShot {
                    // The recoil retraction, down the BORE (the axis the barrel slides on) — the origin
                    // a mid-burst round is actually fired from, inside `Gun_Mantlet_Ballistic`.
                    origin: origin - Vec3::from(bore) * 0.12,
                    direction,
                    tank,
                    slot: slot.0,
                }
            },
        )
        .expect("probe the coax muzzle");

    // The shot as `receive_fire_events` builds it: the wire origin/bore, the shooter NAMED (entity-
    // mapped to this client's replica of that tank), a catch-up fast-forward, and the wire `ShotId`.
    let fire = |shooter: Option<ShotSource>| FireShell {
        origin: shot.origin,
        direction: shot.direction,
        speed: 755.0,
        caliber: 0.0079,
        mass: 0.0118,
        tracer: true,
        shooter,
        catch_up_ticks: 4,
        shot: Some(ShotId {
            shooter: shot.tank,
            weapon: shot.slot as u8,
            fire_tick: 1,
        }),
    };

    // CONTROL — the shape the old code raised (`shooter: None`). The already-landed test hits the
    // shooter's own mantlet a centimetre out and swallows the shell whole: no tracer, ever. This is the
    // bug, pinned.
    app.world_mut().trigger(fire(None));
    app.update();
    let mut shells = app.world_mut().query::<&ShellPath>();
    assert_eq!(
        shells.iter(app.world()).count(),
        0,
        "CONTROL: an un-attributed replica shell fired from inside the shooter's mantlet is swallowed \
         by the already-landed catch-up test — this is exactly the coax's missing tracer",
    );

    // THE FIX — the same shot, naming its shooter. The shooter's own volumes are transparent to it, so
    // the round is spawned and flies.
    app.world_mut().trigger(fire(Some(ShotSource {
        tank: shot.tank,
        weapon: shot.slot,
    })));
    app.update();
    let mut shells = app.world_mut().query::<(Entity, &Transform, &ShellPath)>();
    let (shell, transform, _) = shells
        .iter(app.world())
        .next()
        .map(|(e, t, p)| (e, *t, p.points.len()))
        .expect(
            "a replica coax shell naming its shooter must be spawned — the shooter's own mantlet is \
             transparent to its own round (`ballistics::not_own_volume`)",
        );
    let start = transform.translation;

    // …and keeps flying: it neither holds hidden at the mantlet nor dissolves. A held shell does not
    // advance, so distance travelled is the honest test (the catch-up already placed it downrange).
    for _ in 0..8 {
        app.update();
    }
    let flown = app
        .world()
        .get::<Transform>(shell)
        .map(|t| t.translation.distance(start))
        .unwrap_or(-1.0);
    assert!(
        flown > 10.0,
        "the replica coax shell must fly on, not freeze at the shooter's mantlet and dissolve; it \
         moved {flown:.2} m in 8 ticks (a ~755 m/s round covers ~90 m)",
    );
}
