//! Headless boot regression tests.
//!
//! Invariant: simulation boots without GPU, window, or winit runtime initialization.

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use crate::SimPlugin;
use crate::bake::TankBlueprint;
use crate::command::TankCommand;
use crate::spec::TankSpec;
use crate::state::AppState;
use crate::tank::{Controlled, PendingTankAssets, TIGER_GLB_PATH, Tank};

/// Backstop only — NOT a performance budget. With the boots serialized (see [`BOOT_LEASE`]) a boot
/// has the whole box to itself, so it finishes in seconds; this bound exists purely so a genuine
/// hang (a wiring bug that never reaches `Playing`) fails with the diagnosis below instead of
/// sitting until the CI job timeout. It is generous on purpose: the loop exits the instant the sim
/// is up, so a wide bound costs a healthy run exactly nothing.
const BOOT_DEADLINE: Duration = Duration::from_secs(60);

/// Serializes full-app fixtures. The lease spans each test because booting and running apps compete
/// for the same host resources; mutex poisoning is irrelevant to this external resource.
static BOOT_LEASE: Mutex<()> = Mutex::new(());

fn assert_tank_state_at_add(
    add: On<Add, Tank>,
    tanks: Query<(Has<TankCommand>, Has<crate::driving::DriveState>)>,
) {
    let (command, drive) = tanks
        .get(add.entity)
        .expect("a newly added Tank must still exist during its observer");
    assert!(
        command && drive,
        "TankCommand and DriveState must exist in the same insertion that adds Tank",
    );
}

fn assert_suspension_at_add(
    add: On<Add, crate::tank::Roadwheel>,
    wheels: Query<Has<crate::driving::Suspension>>,
) {
    assert!(
        wheels.get(add.entity).is_ok_and(|present| present),
        "Suspension must exist in the same insertion that adds Roadwheel",
    );
}

fn assert_range_table_at_add(
    add: On<Add, crate::tank::Weapon>,
    weapons: Query<Has<crate::firecontrol::RangeTable>>,
) {
    assert!(
        weapons.get(add.entity).is_ok_and(|present| present),
        "RangeTable must exist in the same insertion that adds Weapon",
    );
}

/// A booted headless sim, plus the lease that serialized its boot. Derefs to the [`App`], so tests
/// use it exactly like one; keep it alive for the whole test (dropping it early releases the lease).
struct BootedSim {
    app: App,
    _lease: MutexGuard<'static, ()>,
}

impl std::ops::Deref for BootedSim {
    type Target = App;
    fn deref(&self) -> &App {
        &self.app
    }
}

impl std::ops::DerefMut for BootedSim {
    fn deref_mut(&mut self) -> &mut App {
        &mut self.app
    }
}

/// Full plugin registration without GPU, window, or winit runtime initialization.
///
/// The clock starts at `ManualDuration(ZERO)`: asset IO is wall-clock, and if sim time advanced
/// while it ran, the collider-less tanks would free-fall through the terrain for the whole load —
/// the same spawn-before-bind race the game keeps to a frame or two. Callers start the clock once
/// the rig is bound.
fn headless_app() -> App {
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
    // Physics + the SP spawn scenario are composition-root choices (see lib.rs SimPlugin note);
    // this exercises the single-player-shaped boot, headless.
    app.add_plugins((
        avian3d::prelude::PhysicsPlugins::default(),
        SimPlugin,
        crate::tank::sp_spawn_plugin,
    ))
    .add_observer(assert_tank_state_at_add)
    .add_observer(assert_suspension_at_add)
    .add_observer(assert_range_table_at_add);

    // `App::run` normally drives plugin finish/cleanup (some registration — e.g. Avian's
    // diagnostics resources — happens in `Plugin::finish`); a bare `update()` loop must do it.
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
    app
}

/// Reports each boot gate separately so a timeout identifies the unavailable prerequisite.
fn boot_diagnosis(app: &App, elapsed: Duration) -> String {
    let world = app.world();
    let state = *world.resource::<State<AppState>>().get();
    let assets = world.resource::<AssetServer>();
    let specs = world.resource::<Assets<TankSpec>>();
    let blueprint = world.get_resource::<TankBlueprint>().is_some();

    // The three gates `tank::spawn_tank_when_loaded` waits on, reported individually.
    let (spec_state, scene_state, spec_parsed) = match world.get_resource::<PendingTankAssets>() {
        Some(p) => (
            format!("{:?}", assets.load_state(&p.spec)),
            format!("{:?}", assets.load_state(&p.scene)),
            specs.get(&p.spec).is_some(),
        ),
        // Removed only by the spawn itself, which sets `Playing` in the same run — so if it is gone
        // while we are still Loading, the state machine, not the assets, is the suspect.
        None => {
            let gone = "<resource gone — the spawn already ran>".to_string();
            (gone.clone(), gone, false)
        }
    };

    // Size on disk catches the other way this can break: a Git LFS **pointer file** (~130 bytes of
    // text) instead of the 65 MB model, which is what a checkout without `lfs: true` leaves behind.
    let glb = crate::assets::asset_root().join(TIGER_GLB_PATH);
    let glb_report = match std::fs::metadata(&glb) {
        Ok(m) if m.len() < 1024 => format!(
            "{} — {} bytes: THIS IS A GIT LFS POINTER, not the model (checkout without `lfs: true`)",
            glb.display(),
            m.len()
        ),
        Ok(m) => format!("{} — {} bytes", glb.display(), m.len()),
        Err(e) => format!("{} — CANNOT STAT: {e}", glb.display()),
    };

    format!(
        "sim never reached AppState::Playing headless after {:.1} s (deadline {:?}).\n\
         \n\
         The boot waits on three gates (tank::spawn_tank_when_loaded); their state right now:\n  \
           AppState ............... {state:?}\n  \
           spec  (tiger_1.tank.ron) {spec_state}\n  \
           scene (tiger_1.glb) .... {scene_state}\n  \
           TankSpec parsed ........ {spec_parsed}\n  \
           TankBlueprint ......... {blueprint}  (bake::extract_at_startup, Startup)\n  \
           glb on disk ............ {glb_report}\n\
         \n\
         How to read this:\n  \
           * still `Loading` + a full-size glb -> the box was too slow or too contended to finish\n    \
             the asset IO in time. NOT a broken asset. Check whether several full apps booted at\n    \
             once (see BOOT_LEASE above — they are supposed to take turns).\n  \
           * `Failed(..)` -> a genuine load failure; the error is printed in the state above.\n  \
           * a ~130-byte glb -> a Git LFS pointer, not the model: the checkout ran without `lfs: true`.\n  \
           * `NotLoaded` -> `load_tank_assets` never ran: a plugin-wiring bug, not an asset problem.",
        elapsed.as_secs_f32(),
        BOOT_DEADLINE,
    )
}

/// Boot the sim headless and run it to a bound rig: `Playing` reached and both tanks' roadwheels
/// instantiated from the real Tiger scene. The sim clock is still FROZEN on return — callers start
/// it when they want time to pass.
///
/// Serialized against the other headless boots by [`BOOT_LEASE`]; the returned [`BootedSim`] holds
/// that lease, and the deadline clock only starts once the lease is in hand (a test queued behind a
/// sibling must not burn its own boot budget waiting its turn).
fn booted_sim() -> BootedSim {
    let lease = BOOT_LEASE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut app = headless_app();

    // Asset IO is genuinely async on wall-clock IO threads (the spec RON + tiger_1.glb), so poll
    // until the spawn gate opens and the app enters Playing. Each not-yet-Playing pass yields 1 ms
    // to those IO threads: a bare CPU-bound spin starves them. The sleep is WALL-CLOCK only — the
    // clock is `ManualDuration(ZERO)` here, so no sim tick advances and the frozen-load invariant
    // above holds untouched.
    let started = Instant::now();
    loop {
        app.update();
        if *app.world().resource::<State<AppState>>().get() == AppState::Playing {
            break;
        }
        let elapsed = started.elapsed();
        assert!(elapsed < BOOT_DEADLINE, "{}", boot_diagnosis(&app, elapsed));
        std::thread::sleep(Duration::from_millis(1));
    }

    // Still real-time asset IO (sim clock frozen): wait for the scene to instantiate and the rigs to
    // bind. Both tanks together carry 32 roadwheels; the muzzles/weapons land in the same bind, so
    // this is also what makes a bore available to `fire`.
    let mut wheels = 0;
    let started = Instant::now();
    while started.elapsed() < BOOT_DEADLINE {
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
        "the sim reached Playing but the rigs never bound headless — the Tiger scene instantiated \
         no roadwheels (expected 32 across the two tanks, saw {wheels}). The spec and scene both \
         loaded, so this is a scene-bind/spec-match failure, not an asset-IO one.",
    );

    // Final census complements the insertion-time observers above and catches any alternate
    // construction path that produced an incomplete entity without the expected marker.
    let world = app.world_mut();
    let incomplete_tanks = world
        .query_filtered::<(Has<TankCommand>, Has<crate::driving::DriveState>), With<Tank>>()
        .iter(world)
        .filter(|(command, drive)| !command || !drive)
        .count();
    let incomplete_wheels = world
        .query_filtered::<Has<crate::driving::Suspension>, With<crate::tank::Roadwheel>>()
        .iter(world)
        .filter(|suspension| !suspension)
        .count();
    let weapon_tables: Vec<bool> = world
        .query_filtered::<Has<crate::firecontrol::RangeTable>, With<crate::tank::Weapon>>()
        .iter(world)
        .collect();
    assert_eq!(
        incomplete_tanks, 0,
        "a spawned Tank lacks command or drive state"
    );
    assert_eq!(incomplete_wheels, 0, "a spawned Roadwheel lacks Suspension");
    assert!(
        !weapon_tables.is_empty() && weapon_tables.iter().all(|present| *present),
        "a spawned Weapon lacks its RangeTable",
    );

    BootedSim { app, _lease: lease }
}

/// [`booted_sim`] with the sim clock started and the tanks settled onto their suspension — the
/// shared scaffolding for the shooting tests, which need the REAL tiger geometry (a synthetic plate
/// cannot reproduce a muzzle that recoils behind its own mantlet).
fn booted_sp_app() -> BootedSim {
    let mut sim = booted_sim();
    sim.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        16,
    )));
    for _ in 0..30 {
        sim.update();
    }
    sim
}

/// Boot the sim headless, then drive the tank by writing its `TankCommand` directly — the exact
/// path a server takes applying a remote client's command (no device gather mounted).
#[test]
fn sim_boots_and_drives_headless() {
    // Boot to a bound rig with the sim clock still frozen — this test then starts the clock itself,
    // because grounding the suspension from a standstill is part of what it proves.
    let mut app = booted_sim();

    // Start the clock (16 ms per `update()`, so the 64 Hz fixed sim ticks once per update) and let
    // the suspension ground and settle.
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

/// The MG-tracer render gate, exercised on the real spawn path headless. Firing the secondary trigger
/// must, over a burst:
///   * spawn tracer STREAKS (`TracerStreak`) for the ~1-in-5 tracer rounds, and
///   * spawn NO `shell.glb` scene root on ANY MG round. A shell in flight carries `ShellPath`; only a
///     main-gun-calibre round also gets a `WorldAssetRoot` scene, so `ShellPath + WorldAssetRoot`
///     over an MG-only burst must stay empty while streaks appear.
#[test]
fn mg_rounds_stream_tracers_and_spawn_no_shell_scene() {
    use crate::ballistics::{ShellPath, TracerStreak};
    use bevy::world_serialization::WorldAssetRoot;

    // A booted, settled rig: the muzzles/weapons must exist for `fire` to find a bore.
    let mut app = booted_sp_app();

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

/// Shooter self-exclusion regression on the real asset.
///
/// A sustained MG burst must not impact the firing tank, while still reaching other geometry.
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

/// Replica catch-up regression: a named shooter remains excluded from its own collision volumes.
/// The control omits `shooter` and must remain held at the armor candidate.
#[test]
fn a_replica_coax_shell_clears_the_shooters_mantlet() {
    use crate::ClientReplica;
    use crate::ShotId;
    use crate::ballistics::{FireShell, FireShellOrigin, ShellPath, ShotSource};
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
        mechanism: crate::spec::FireMechanism::Automatic,
        tracer: true,
        shot_origin: FireShellOrigin::Reconstructed,
        shooter,
        catch_up_ticks: 4,
        shot: Some(ShotId {
            combatant: crate::CombatantId(1),
            weapon: shot.slot as u8,
            fire_tick: 1,
        }),
    };

    // Control: omitting `shooter` holds the catch-up shell at the armor candidate.
    app.world_mut().trigger(fire(None));
    app.update();
    let mut shells = app.world_mut().query::<(Entity, &Visibility, &ShellPath)>();
    let control = shells
        .iter(app.world())
        .next()
        .map(|(entity, visibility, _)| (entity, *visibility))
        .expect("the keyed control shell should survive as an authority-waiting candidate");
    assert_eq!(
        control.1,
        Visibility::Hidden,
        "CONTROL: an un-attributed replica shell fired from inside the shooter's mantlet must be held \
         hidden there — it cannot honestly fly or render a tracer",
    );
    app.world_mut().despawn(control.0);

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
