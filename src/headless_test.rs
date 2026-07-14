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

#[derive(Resource, Default)]
struct ScriptedDeterminismRun {
    digests: Vec<Vec<(String, crate::trace::CanonicalTankStateDigest)>>,
    trajectory: Vec<(usize, Vec3, Quat)>,
    saw_airborne: bool,
    saw_grounded: bool,
    saw_brush_anchor: bool,
    saw_steering_slip: bool,
    saw_shot: bool,
    fire_shells: usize,
    saw_projectile_spawn: bool,
    saw_projectile_march: bool,
}

/// The observer is deliberately at the production `FireShell` seam: `rounds_fired > 0` proves
/// only root bookkeeping, while this proves the forward script actually crossed the shell-spawn
/// boundary. Bevy 0.19 applies `Commands::trigger` at its deferred barrier, where observers run.
fn count_scripted_fire_shells(
    _: On<crate::ballistics::FireShell>,
    mut run: ResMut<ScriptedDeterminismRun>,
) {
    run.fire_shells += 1;
}

fn capture_scripted_determinism_tick(
    roots: Query<
        (
            Entity,
            &Name,
            Has<Controlled>,
            &avian3d::prelude::Position,
            &avian3d::prelude::Rotation,
            &avian3d::prelude::LinearVelocity,
            &avian3d::prelude::AngularVelocity,
            &avian3d::prelude::ComputedCenterOfMass,
            &crate::driving::DriveState,
            &crate::tank::TankSim,
        ),
        With<Tank>,
    >,
    children: Query<&Children>,
    wheels: Query<&crate::driving::Suspension>,
    projectiles: Query<&crate::ballistics::ShellPath>,
    mut run: ResMut<ScriptedDeterminismRun>,
) {
    let tick = run.digests.len();
    let mut digests = Vec::with_capacity(roots.iter().len());
    let mut controlled = None;
    for (tank, name, is_controlled, position, rotation, linear, angular, com, drive, sim) in &roots
    {
        digests.push((
            name.as_str().to_owned(),
            crate::trace::canonical_tank_state_digest(
                position.0, rotation.0, linear.0, angular.0, drive, sim,
            ),
        ));
        if is_controlled {
            controlled = Some((
                tank,
                position.0,
                rotation.0,
                linear.0,
                angular.0,
                com.0,
                drive.steer(),
                sim,
            ));
        }
    }
    digests.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(digests.len(), 2, "the local duel has two simulation tanks");

    let (tank, position, rotation, linear, angular, local_com, steer, sim) =
        controlled.expect("one controlled tank");
    let grounded = children
        .iter_descendants(tank)
        .filter_map(|entity| wheels.get(entity).ok())
        .filter(|suspension| suspension.contact.is_some())
        .count();
    run.saw_airborne |= grounded == 0;
    run.saw_grounded |= grounded > 0;

    let anchors = sim.anchors.iter().filter(|anchor| anchor.is_some()).count();
    run.saw_brush_anchor |= anchors > 0;

    // Avian 0.7 `Forces::velocity_at_point`: v_point = v_linear + omega × (point − world_COM),
    // where world_COM = position + rotation * local_COM. Project onto the ground plane before
    // classifying with the exact production `static_weight` rule. A loaded anchor remains `Some`
    // while sliding, so anchor-count changes cannot witness this regime.
    let world_com = position + rotation * local_com;
    let loaded_contact_is_slipping = children
        .iter_descendants(tank)
        .filter_map(|entity| wheels.get(entity).ok())
        .filter_map(|suspension| {
            (suspension.load > 0.0)
                .then_some(suspension.contact)
                .flatten()
        })
        .any(|contact| {
            let point_velocity = linear + angular.cross(contact - world_com);
            let planar_speed = Vec2::new(point_velocity.x, point_velocity.z).length();
            crate::driving::static_weight_for_test(planar_speed) < 1.0
        });
    run.saw_steering_slip |=
        tick >= 240 && steer.abs() > f32::EPSILON && loaded_contact_is_slipping;
    run.saw_shot |= sim.weapons.iter().any(|weapon| weapon.rounds_fired > 0);
    run.saw_projectile_spawn |= !projectiles.is_empty();
    run.saw_projectile_march |= projectiles.iter().any(|path| path.points.len() > 1);
    if matches!(tick, 119 | 219 | 339) {
        run.trajectory.push((tick, position, rotation));
    }
    run.digests.push(digests);
}

fn assert_simulation_mutators_are_ordered(app: &App) {
    let world = app.world();
    let schedules = world.resource::<bevy::ecs::schedule::Schedules>();
    let schedule = schedules
        .get(FixedUpdate)
        .expect("the full sim installs FixedUpdate");
    let names: std::collections::HashMap<_, _> = schedule
        .systems()
        .expect("FixedUpdate ran and initialized its systems")
        .map(|(key, system)| (key, system.name().to_string()))
        .collect();
    for expected in [
        "driving::traction::ramp_drive",
        "driving::suspension::apply_suspension",
        "driving::traction::apply_drive",
        "shooting::tick_reload",
        "shooting::fire",
        "shooting::apply_recoil",
        "ballistics::integrate_projectiles",
        "damage::process_cookoffs",
        "damage::kill_crew",
    ] {
        assert_eq!(
            names
                .values()
                .filter(|name| name.ends_with(expected))
                .count(),
            1,
            "the schedule guard must find exactly one `{expected}` system",
        );
    }
    let conflicts: Vec<_> = schedule
        .graph()
        .conflicting_systems()
        .iter()
        .filter_map(|(left, right, _)| Some((names.get(left)?, names.get(right)?)))
        .filter(|(left, right)| {
            let writes_physical_state = |name: &str| {
                name.contains("driving::traction::ramp_drive")
                    || name.contains("driving::suspension::apply_suspension")
                    || name.contains("driving::traction::apply_drive")
                    || name.contains("shooting::tick_reload")
                    || name.contains("shooting::fire")
                    || name.contains("shooting::apply_recoil")
                    || name.contains("ballistics::integrate_projectiles")
            };
            let force_conflict = writes_physical_state(left) && writes_physical_state(right);
            let projectile_damage_conflict = (left.contains("ballistics::integrate_projectiles")
                && right.contains("damage::"))
                || (right.contains("ballistics::integrate_projectiles")
                    && left.contains("damage::"));
            force_conflict || projectile_damage_conflict
        })
        .map(|(left, right)| (left.clone(), right.clone()))
        .collect();
    assert!(
        conflicts.is_empty(),
        "simulation mutators need an explicit order: {conflicts:#?}",
    );
}

const SCRIPT_TICKS: usize = 600;

fn scripted_determinism_run() -> ScriptedDeterminismRun {
    let mut app = booted_sim();
    app.init_resource::<ScriptedDeterminismRun>()
        .add_observer(count_scripted_fire_shells)
        .add_systems(FixedLast, capture_scripted_determinism_tick)
        // Verified against Bevy 0.19: one `App::update` runs exactly one fixed loop.
        .insert_resource(TimeUpdateStrategy::FixedTimesteps(1));

    let mut controlled = app
        .world_mut()
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>();
    let tank = controlled.single(app.world()).expect("one controlled tank");

    for tick in 0..SCRIPT_TICKS {
        {
            let mut command = app
                .world_mut()
                .get_mut::<TankCommand>(tank)
                .expect("controlled tank carries TankCommand");
            command.throttle = if (120..420).contains(&tick) { 1.0 } else { 0.0 };
            command.steer = if (240..420).contains(&tick) { 0.7 } else { 0.0 };
            command.fire_primary = tick == 220;
            command.fire_secondary = (360..420).contains(&tick);
        }
        app.update();
        if tick == 0 {
            assert_simulation_mutators_are_ordered(&app);
        }
    }

    app.world_mut()
        .remove_resource::<ScriptedDeterminismRun>()
        .expect("the scripted digest collector remains installed")
}

fn assert_scripted_determinism_witnesses(run: &ScriptedDeterminismRun, label: &str) {
    assert_eq!(
        run.digests.len(),
        SCRIPT_TICKS,
        "{label} produced one digest per fixed tick",
    );
    assert!(run.saw_airborne, "{label} crossed an airborne state");
    assert!(run.saw_grounded, "{label} reached ground contact");
    assert!(run.saw_brush_anchor, "{label} established a brush anchor");
    assert!(
        run.saw_steering_slip,
        "{label} put a loaded wheel in the blended/kinetic regime while steering",
    );
    assert!(run.saw_shot, "{label} fired at least one weapon");
    assert!(
        run.fire_shells > 0,
        "{label} reached shooting::fire's FireShell spawn seam",
    );
    assert!(
        run.saw_projectile_spawn,
        "{label} spawned a projectile entity from FireShell",
    );
    assert!(
        run.saw_projectile_march,
        "{label} marched a projectile beyond its spawn point",
    );
}

/// Two fresh, full simulation compositions must replay one command script bit-for-bit. The witness
/// assertions keep this from passing because the scenario never reached contact, brush traction,
/// steering slip, or fire.
#[test]
fn full_simulation_replay_is_bit_exact_for_six_hundred_ticks() {
    let first = scripted_determinism_run();
    assert_scripted_determinism_witnesses(&first, "first fresh sim");

    // MEASURED 2026-07-14 on macOS arm64. These characterize the current driving trajectory; they
    // do not claim that its feel is correct. DERIVED: tick 119 is the last settle-only sample before
    // throttle starts at tick 120; tick 219 follows 100 throttle ticks and precedes the shot at 220.
    let expected_trajectory = [
        (
            119,
            Vec3::new(8.702614, -0.05056709, 4.9854083),
            Quat::from_xyzw(0.002687655, 0.023880916, 0.016786428, 0.99957025),
        ),
        (
            219,
            Vec3::new(8.549372, 0.023869634, 2.10651),
            Quat::from_xyzw(0.005728841, 0.024727648, -0.00014382847, 0.9996778),
        ),
        // MEASURED 2026-07-14 on macOS arm64. DERIVED: tick 339 is 100 fixed steps after steer
        // begins and 21 before the MG hold starts, so this checkpoint characterizes steering rather
        // than a later burst.
        (
            339,
            Vec3::new(8.392248, 0.022434652, -6.159646),
            Quat::from_xyzw(0.0053370306, -0.015307999, 0.0013887828, 0.9998676),
        ),
    ];
    // DERIVED tolerances: tight enough to expose a material force-law change while allowing the
    // deferred cross-platform determinism work to land without rewriting a platform-specific bit
    // snapshot. Position may drift by one centimetre; orientation by two milliradians.
    const POSITION_TOLERANCE_M: f32 = 0.01;
    const ROTATION_TOLERANCE_RAD: f32 = 0.002;
    assert_eq!(
        first.trajectory.len(),
        expected_trajectory.len(),
        "every driving checkpoint was observed",
    );
    for ((tick, position, rotation), (expected_tick, expected_position, expected_rotation)) in
        first.trajectory.iter().zip(expected_trajectory)
    {
        assert_eq!(*tick, expected_tick, "the scripted checkpoint tick moved");
        let position_error = position.distance(expected_position);
        assert!(
            position_error <= POSITION_TOLERANCE_M,
            "MEASURED driving position changed at tick {tick}: error {position_error} m, actual \
             {position:?}, expected {expected_position:?}",
        );
        let rotation_error = rotation.angle_between(expected_rotation);
        assert!(
            rotation_error <= ROTATION_TOLERANCE_RAD,
            "MEASURED driving rotation changed at tick {tick}: error {rotation_error} rad, actual \
             {rotation:?}, expected {expected_rotation:?}",
        );
    }

    let second = scripted_determinism_run();
    assert_scripted_determinism_witnesses(&second, "second fresh sim");
    if let Some((tick, (left, right))) = first
        .digests
        .iter()
        .zip(&second.digests)
        .enumerate()
        .find(|(_, (left, right))| left != right)
    {
        panic!(
            "fresh full-sim worlds first differ at scripted tick {tick}:\nleft:  {left:#?}\nright: {right:#?}",
        );
    }
}
