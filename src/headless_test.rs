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
    tanks: Query<(
        Has<TankCommand>,
        Has<crate::track::sim::TrackDrive>,
        Option<&crate::track::sim::TrackGripElements>,
    )>,
    blueprint: Option<Res<TankBlueprint>>,
) {
    let (command, drive, elements) = tanks
        .get(add.entity)
        .expect("a newly added Tank must still exist during its observer");
    assert!(
        command && drive,
        "TankCommand and TrackDrive must exist in the same insertion that adds Tank",
    );
    // The REV-14 fixed-size invariant at its source: every Tank is born with element slabs
    // pre-sized `link_count * 3` — never an empty vector awaiting a first-tick resize
    // (element-promotion-checklist.md §5 spawn fixture).
    let elements = elements.expect("TrackGripElements must exist in the same insertion as Tank");
    let expected = blueprint
        .expect("the blueprint bakes at Startup, before any Tank can spawn")
        .spec
        .track
        .link_count
        * 3;
    for side in &elements.sides {
        assert_eq!(
            (side.strain.len(), side.dwell.len()),
            (expected, expected),
            "a Tank spawned with wrong-sized element slabs (want link_count*3 = {expected})",
        );
    }
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
        .query_filtered::<(
            Has<TankCommand>,
            Has<crate::track::sim::TrackDrive>,
            Has<crate::track::sim::TrackGripElements>,
        ), With<Tank>>()
        .iter(world)
        .filter(|(command, drive, elements)| !command || !drive || !elements)
        .count();
    let weapon_tables: Vec<bool> = world
        .query_filtered::<Has<crate::firecontrol::RangeTable>, With<crate::tank::Weapon>>()
        .iter(world)
        .collect();
    assert_eq!(
        incomplete_tanks, 0,
        "a spawned Tank lacks command or drive state"
    );
    assert!(
        !weapon_tables.is_empty() && weapon_tables.iter().all(|present| *present),
        "a spawned Weapon lacks its RangeTable",
    );

    BootedSim { app, _lease: lease }
}

/// [`booted_sim`] with the sim clock started and the tanks settled onto their tracks — the
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
    // because settling onto the belt contacts from a standstill is part of what it proves.
    let mut app = booted_sim();

    // Start the clock (16 ms per `update()`, so the 64 Hz fixed sim ticks once per update) and let
    // the belt contacts ground and settle.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        16,
    )));
    let mut grounded = 0;
    for _ in 0..300 {
        app.update();
        let world = app.world_mut();
        grounded = world
            .query::<&crate::track::sim::TrackContacts>()
            .iter(world)
            .map(|c| c.0.iter().filter(|side| !side.is_empty()).count())
            .sum();
        if grounded >= 4 {
            break;
        }
    }
    assert!(
        grounded >= 4,
        "the belt field never grounded headless; contacting track sides: {grounded}"
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
    // holds; the command slew, belt forces, and drive all run on the fixed clock.
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

/// One scripted headless drive for the element-gate proof: boot the sim (the headless equivalent
/// of the `--offline` composition — [`headless_app`] mounts physics + `SimPlugin` + the SP duel
/// spawn, exactly what `GamePlugin` composes minus presentation), optionally latch
/// `ElementGripFeelTest`, settle, hold full throttle for ~4 sim-seconds, and return
/// `(horizontal metres moved, total element strain in metres)`.
fn element_gate_run(feel: bool) -> (f32, f32) {
    let mut app = booted_sim();
    if feel {
        // The offline latch, exactly as `run_offline` inserts it: present from before the
        // first sim tick, never toggled.
        app.init_resource::<crate::track::sim::ElementGripFeelTest>();
    }

    // Start the clock and let the belt contacts ground and settle (the
    // `sim_boots_and_drives_headless` scaffold).
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        16,
    )));
    let mut grounded = 0;
    for _ in 0..300 {
        app.update();
        let world = app.world_mut();
        grounded = world
            .query::<&crate::track::sim::TrackContacts>()
            .iter(world)
            .map(|c| c.0.iter().filter(|side| !side.is_empty()).count())
            .sum();
        if grounded >= 4 {
            break;
        }
    }
    assert!(grounded >= 4, "the belt field never grounded headless");
    for _ in 0..60 {
        app.update();
    }

    let mut tank_q = app
        .world_mut()
        .query_filtered::<(Entity, &Transform), (With<Tank>, With<Controlled>)>();
    let (tank, start) = tank_q.single(app.world()).expect("one controlled tank");
    let start = start.translation;

    // ~4 sim-seconds of full throttle, re-asserted every tick (no device gather headless).
    for _ in 0..250 {
        app.world_mut()
            .entity_mut(tank)
            .get_mut::<TankCommand>()
            .expect("tank carries a command")
            .throttle = 1.0;
        app.update();
    }

    let end = app
        .world()
        .get::<Transform>(tank)
        .expect("tank survived")
        .translation;
    let moved = Vec3::new(end.x - start.x, 0.0, end.z - start.z).length();
    let world = app.world_mut();
    let strain: f32 = world
        .query::<&crate::track::sim::TrackGripElements>()
        .iter(world)
        .flat_map(|elements| elements.sides.iter())
        .flat_map(|side| side.strain.iter())
        .map(|j| j.length())
        .sum();
    (moved, strain)
}

/// Phase-2 offline gate proof (element-promotion-checklist.md Q1). Two identical scripted drives:
///   * WITH `ElementGripFeelTest` latched (the `--offline` composition's gate): the tank drives
///     AND the per-element law actually engages — spawn-sized `TrackGripElements` strain becomes
///     nonzero.
///   * WITHOUT the resource (every MP-shaped composition): identical ticks, element strain stays
///     EXACTLY zero — the gate holds, and the unregistered element state provably cannot be
///     touched outside the offline route.
///
/// The spawn-sizing half of the checklist fixture lives in [`assert_tank_state_at_add`], which
/// every boot in this file runs.
#[test]
fn offline_element_gate_engages_only_under_feel_resource() {
    let (moved, strain) = element_gate_run(true);
    assert!(
        moved > 2.0,
        "the element regime should still drive the tank forward; moved {moved:.2} m"
    );
    assert!(
        strain > 0.0,
        "with ElementGripFeelTest present the element law must engage — strain stayed zero \
         (the gate never passed Some(&mut GripElements) through, or the slabs were mis-sized \
         and the invariant early-out silently skipped the regime)"
    );

    let (moved, strain) = element_gate_run(false);
    assert!(
        moved > 2.0,
        "the aggregate regime should drive the tank forward; moved {moved:.2} m"
    );
    assert_eq!(
        strain, 0.0,
        "without ElementGripFeelTest the element slabs must stay EXACTLY zero — something \
         outside the offline gate wrote element state"
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

// --- Tiger transmission gates -------------------------------------------------------------------
//
// The process fix behind the phase-2.5 postmortem: every physics gate ran on the sandbox's T-34
// lab vehicle, and vehicle-scaling defects (steering capacity vs footprint scrub) sailed through
// on the smaller tank. These gates drive the REAL Tiger blueprint through the offline
// composition — the same boot, spawn path, spec, and terrain the `--offline` feel session runs —
// with `TransmissionFeelTest` set per case. They are permanent `cargo test` members: the sandbox
// gates remain, but can never again be the only physics evidence.

/// [`booted_sim`] + the two offline feel gates exactly as `run_offline` mounts them
/// (`ElementGripFeelTest` latched, `TransmissionFeelTest(mode)`), clock started, tracks
/// grounded and settled. Returns the sim and the controlled Tiger.
fn booted_offline_sim(mode: crate::track::transmission::TransmissionMode) -> (BootedSim, Entity) {
    let mut app = booted_sim();
    app.init_resource::<crate::track::sim::ElementGripFeelTest>();
    app.insert_resource(crate::track::sim::TransmissionFeelTest(mode));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        16,
    )));
    let mut grounded = 0;
    for _ in 0..300 {
        app.update();
        let world = app.world_mut();
        grounded = world
            .query::<&crate::track::sim::TrackContacts>()
            .iter(world)
            .map(|c| c.0.iter().filter(|side| !side.is_empty()).count())
            .sum();
        if grounded >= 4 {
            break;
        }
    }
    assert!(grounded >= 4, "the belt field never grounded headless");
    for _ in 0..60 {
        app.update();
    }
    let mut tank_q = app
        .world_mut()
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>();
    let tank = tank_q.single(app.world()).expect("one controlled tank");
    (app, tank)
}

/// Write the drive command (level state, re-asserted every tick like the other headless
/// drives — no device gather exists here) and advance one 16 ms update (= one 64 Hz tick).
fn drive_tick(app: &mut App, tank: Entity, throttle: f32, steer: f32) {
    {
        let mut cmd = app
            .world_mut()
            .get_mut::<TankCommand>(tank)
            .expect("tank carries a command");
        cmd.throttle = throttle;
        cmd.steer = steer;
    }
    app.update();
}

/// Horizontal hull speed (m/s) from the tick-truth velocity.
fn hull_speed(app: &mut App, tank: Entity) -> f32 {
    let v = app
        .world()
        .get::<avian3d::prelude::LinearVelocity>(tank)
        .expect("tank has velocity")
        .0;
    Vec3::new(v.x, 0.0, v.z).length()
}

/// Body-frame yaw rate (rad/s): world angular velocity projected on the hull's up axis
/// (world `av.y` lies on slopes — the harness's own rule).
fn yaw_rate(app: &mut App, tank: Entity) -> f32 {
    let world = app.world();
    let ang = world
        .get::<avian3d::prelude::AngularVelocity>(tank)
        .expect("tank has angular velocity")
        .0;
    let rot = world
        .get::<avian3d::prelude::Rotation>(tank)
        .expect("tank has rotation")
        .0;
    ang.dot(rot * Vec3::Y)
}

/// Point the hull down +Z (away from the SP duel partner at z = −12 and the −Z obstacle
/// course) and re-settle: the long straight-line gates need the ~490 m of flat ground the
/// +Z half of the map offers.
fn face_positive_z(app: &mut App, tank: Entity) {
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 =
            Quat::from_rotation_y(std::f32::consts::PI);
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
    }
    for _ in 0..120 {
        app.update();
    }
}

/// Full throttle until the hull reaches `target` m/s (bounded); returns ticks taken.
fn drive_to_speed(app: &mut App, tank: Entity, target: f32, max_ticks: usize) -> usize {
    for tick in 0..max_ticks {
        drive_tick(app, tank, 1.0, 0.0);
        if hull_speed(app, tank) >= target {
            return tick;
        }
    }
    panic!(
        "full throttle never reached {target} m/s in {max_ticks} ticks (speed {})",
        hull_speed(app, tank)
    );
}

/// The pivot gate body shared by the two steering laws: zero throttle, full steer, ≥ 4 s;
/// the mean yaw rate over the last second must clear the per-adapter floor (each adapter's
/// pivot scale is a different LAW — see the callers) and the belts must actually
/// counter-rotate. ZERO is the original bug this family pins: the Tiger's steering
/// capacity read on the wrong axis could not break its own footprint scrub.
fn tiger_pivot_gate(mode: crate::track::transmission::TransmissionMode, min_yaw: f32) {
    let (mut app, tank) = booted_offline_sim(mode);
    let mut yaw_sum = 0.0f32;
    let mut samples = 0u32;
    for tick in 0..320 {
        drive_tick(&mut app, tank, 0.0, 1.0);
        if tick >= 256 {
            yaw_sum += yaw_rate(&mut app, tank);
            samples += 1;
        }
    }
    let mean_yaw = yaw_sum / samples as f32;
    let drive = app
        .world()
        .get::<crate::track::sim::TrackDrive>(tank)
        .expect("tank drives");
    let (l, r) = (drive.sides[0].speed, drive.sides[1].speed);
    println!("tiger pivot [{mode:?}]: mean yaw {mean_yaw:.4} rad/s, belts L {l:.3} / R {r:.3}");
    assert!(
        l * r < 0.0,
        "[{mode:?}] a neutral pivot must counter-rotate the belts (L {l:.3}, R {r:.3})"
    );
    assert!(
        mean_yaw.abs() >= min_yaw,
        "[{mode:?}] pivot yaw {mean_yaw:.4} rad/s under full steer — gate ≥ {min_yaw} rad/s"
    );
}

/// Tiger pivot, L600 fixed-radius adapter (the vehicle's authored architecture): the
/// MARGINAL brake-gated neutral turn toward the DERIVED `neutral_d_full` = 0.2808 m/s
/// (fix 3 deleted the unprovenanced 0.75 fraction that used to shrink the target).
/// MEASURED on the declared data: 0.131 rad/s mean ground yaw, belts exactly ±0.281 m/s
/// (the belt-kinematic ceiling d/half-tread ≈ 0.188 rad/s, less scrub slip); gated at
/// ≥ 0.10 rad/s (margin for platform float drift — the restoration literature's
/// "technically yes, advisable no" crawl is exactly this regime).
#[test]
fn pivot_tiger_l600() {
    tiger_pivot_gate(
        crate::track::transmission::TransmissionMode::FixedRadii,
        0.10,
    );
}

/// Tiger pivot, hybrid continuous adapter: POWER-limited (fix 2 — the standstill pivot
/// commands steer force up to capacity and the power-conservation scale is the binding
/// limiter, so the rate settles where engine power balances scrub dissipation; the old
/// neutral_d_full speed FLOOR used ~68 kW of the ~407 kW budget and pivoted at
/// 0.131 rad/s). MEASURED on the declared data: 0.654 rad/s mean ground yaw, belts
/// ±1.40 m/s; gated at ≥ 0.35 rad/s (margin, same policy as the L600 gate).
#[test]
fn pivot_tiger_hybrid() {
    tiger_pivot_gate(crate::track::transmission::TransmissionMode::Hybrid, 0.35);
}

/// The fix-1 smoking gun: a standstill full-throttle climb must walk the Tiger ladder
/// MONOTONICALLY. Pre-fix, every shift's own torque-cut window bled belt speed
/// (I·v̇ = Q − R keeps subtracting the ground reaction while Q is cut) and the low gears'
/// steep rpm-per-speed slope turned that into hundreds of rpm — the down band fired the
/// tick the freeze lifted (measured trace [1,2,1,2,1,2,3,2,3,4,3,4,5,6,7,8]). With the
/// predicted-landing gate + reversal dwell the gear sequence never decreases.
#[test]
fn gear_climb_monotone_tiger() {
    use crate::track::transmission::TransmissionMode;
    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    face_positive_z(&mut app, tank);
    let mut trace: Vec<u8> = vec![];
    let mut max_gear = 0u8;
    for _ in 0..(20 * 64) {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let st = app
            .world()
            .get::<crate::track::sim::TankTransmission>(tank)
            .expect("tank carries transmission state")
            .0;
        assert!(
            !st.reverse,
            "full forward throttle must stay on the F ladder"
        );
        if trace.last() != Some(&st.gear) {
            trace.push(st.gear);
        }
        assert!(
            st.gear >= max_gear,
            "gear decreased during the full-throttle climb — shift hunting is back \
             (trace {trace:?})"
        );
        max_gear = max_gear.max(st.gear);
    }
    println!("tiger full-throttle gear climb trace: {trace:?}");
    assert!(
        max_gear >= 6,
        "20 s of full throttle must climb well up the ladder (reached F{max_gear}, \
         trace {trace:?})"
    );
}

/// Deceleration on the real Tiger (L600, the authored architecture), both driver intents:
///
/// * RELEASE (coast): engine drag at the declared `drag_fraction` (0.25 of peak torque)
///   through the CURRENT gear, downshifting on the authored bands as speed falls — drag at
///   6 m/s (F7) is ≈ 17 kN, growing through the downshifts, plus the grip law's slip
///   losses. MEASURED on the declared data: 6 → 2 m/s in 10.6 s; the gate pins ≤ 14 s
///   (margin for platform float drift, nothing else). The fix-round brief hoped for 8 s —
///   that is unreachable without rolling resistance, WHICH THE CONTACT MODEL DOES NOT
///   HAVE (a real Tiger's ~25–35 kN of rolling drag would dominate its own engine
///   braking; ground resistance belongs to the terrain/ground-type mechanic, ADR-0007
///   bucket 3 — not to the drivetrain, and not tunable-by-feel here). Also pinned: past
///   the command-shaper's release slew, coasting never accelerates (the old code
///   ACCELERATED on opposite input — the regression this kills).
/// * OPPOSITE THROTTLE: service brakes at the declared capacity, DUAL-anchored by fix 4
///   and the review round (96 kN/side: the settled 20° park hold at 95.6 kN/side demand,
///   0.343 g total service decel inside the 0.2–0.35 g WWII heavy-tank band; the old
///   250 kN was the circular grip-limit sizing — 1.17 s from 6 m/s was the
///   energy-impossible tell). Analytic prediction: 2 × 96 kN / 57 t = 3.37 m/s² in the
///   full phase, plus engine drag (~17 kN in F7, growing through downshifts)
///   ≈ 3.6+ m/s², plus the command shaper's ~0.5 s press slew dead time → from 6.0 m/s
///   ≈ 0.5 + 5.0/3.6 ≈ 1.9 s to 1 m/s. MEASURED: 2.23 s. Gate ≤ 3 s (margin for
///   platform float drift, nothing else). The coast leg above is UNCHANGED (no brake in
///   the release intent).
#[test]
fn decel_tiger() {
    use crate::track::transmission::TransmissionMode;
    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    face_positive_z(&mut app, tank);

    // Phase 1 — coast from ≥ 6 m/s.
    drive_to_speed(&mut app, tank, 6.0, 2400);
    let mut released = hull_speed(&mut app, tank);
    let mut coast_ticks = None;
    let mut peak = 0.0f32;
    for tick in 0..(14 * 64) {
        drive_tick(&mut app, tank, 0.0, 0.0);
        let v = hull_speed(&mut app, tank);
        // The command SHAPER slews the released throttle to zero over ~0.5 s (the same
        // ramp a lifted key gets); the drivetrain's own no-acceleration guarantee starts
        // once the drive signal is actually zero.
        if tick >= 48 {
            peak = peak.max(v);
        }
        if v <= 2.0 {
            coast_ticks = Some(tick + 1);
            break;
        }
    }
    let coasting_from = peak;
    assert!(
        coasting_from <= released + 0.15,
        "released throttle must not meaningfully accelerate past the slew window \
         (peak {coasting_from:.2} from {released:.2})"
    );
    let coast_ticks = coast_ticks.unwrap_or_else(|| {
        panic!(
            "coast never reached 2 m/s in 14 s (speed {:.2})",
            hull_speed(&mut app, tank)
        )
    });
    println!(
        "tiger decel: released at {released:.2} m/s, coast to 2 m/s in {:.1} s",
        coast_ticks as f32 / 64.0
    );

    // Phase 2 — service brakes: opposite throttle from ≥ 6 m/s. Budget 3 s: the
    // dual-anchored capacity predicts ≈ 1.9 s including the input slew dead time (see
    // the doc comment's arithmetic).
    drive_to_speed(&mut app, tank, 6.0, 2400);
    released = hull_speed(&mut app, tank);
    let mut brake_ticks = None;
    for tick in 0..(3 * 64) {
        drive_tick(&mut app, tank, -1.0, 0.0);
        if hull_speed(&mut app, tank) <= 1.0 {
            brake_ticks = Some(tick + 1);
            break;
        }
    }
    let brake_ticks = brake_ticks.unwrap_or_else(|| {
        panic!(
            "service brakes never reached 1 m/s within 3 s from {released:.2} m/s \
             (speed {:.2})",
            hull_speed(&mut app, tank)
        )
    });
    println!(
        "tiger decel: service brakes {released:.2} -> 1 m/s in {:.2} s",
        brake_ticks as f32 / 64.0
    );
}

/// The brake datum's own regression gate (review round): the Tiger parks on the course's
/// 20° ramp and HOLDS. `brake_force` is dual-anchored on exactly this capability —
/// W·sin 20°/2 ≈ 95.6 kN/side demand against the 96 kN/side capacity — so the settled
/// ADR-0026 hill-hold behavior is pinned by test, not by comment. Teleport onto the 20°
/// ramp mid-face (test course §1: x = 0, z = −40, pitched about X), release all inputs,
/// settle; the park latch must engage and the hull must not back-drive over a sustained
/// window. 30° is now genuinely beyond capacity (139.8 kN/side demand) and is NOT gated —
/// it back-drives honestly under the capacity-breach law.
#[test]
fn slope_park_holds_20_deg_tiger() {
    use crate::track::transmission::TransmissionMode;
    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::Position>().unwrap().0 = Vec3::new(0.0, 2.6, -40.0);
        e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 =
            Quat::from_rotation_x(20.0_f32.to_radians());
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
    }
    // Settle onto the face under zero input (drop + suspension ring-down + latch).
    for _ in 0..256 {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    let p0 = app
        .world()
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has a position")
        .0;
    for _ in 0..(4 * 64) {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    let p1 = app
        .world()
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has a position")
        .0;
    let st = app
        .world()
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("tank carries transmission state")
        .0;
    let drift = (p1 - p0).length();
    println!(
        "tiger 20-deg slope park: drift {drift:.4} m over 4 s, park latch {}",
        st.park
    );
    assert!(
        st.park,
        "zero input at rest on the ramp must latch the park brake"
    );
    assert!(
        drift < 0.05,
        "the latched park must hold the 20-deg ramp (drifted {drift:.3} m over 4 s)"
    );
}

/// The gearing-emergence check on the REAL vehicle: 30 s of full throttle on flat ground
/// must land inside [10.0, 11.0] m/s — the authored ladder's F8 at the governed 2500 rpm
/// is 10.48 m/s (matching the spec's max_speed 10.5), so both a broken ladder (too slow)
/// and a governor that no longer binds (too fast) fail.
#[test]
fn top_speed_tiger() {
    use crate::track::transmission::TransmissionMode;
    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    face_positive_z(&mut app, tank);
    let mut speed_sum = 0.0f32;
    let mut samples = 0u32;
    let total = 30 * 64;
    for tick in 0..total {
        drive_tick(&mut app, tank, 1.0, 0.0);
        if tick >= total - 128 {
            speed_sum += hull_speed(&mut app, tank);
            samples += 1;
        }
    }
    let mean = speed_sum / samples as f32;
    println!("tiger top speed: {mean:.2} m/s over the last 2 s");
    assert!(
        (10.0..=11.0).contains(&mean),
        "30 s of full throttle must land the geared top speed (10.0–11.0 m/s), got {mean:.2}"
    );
}

/// The offline drive HUD's readout fn, exercised on the REAL Tiger through the offline
/// composition: after driving forward under the L600 adapter, [`transmission::readout`] must
/// report a sane geared operating point — the engaged FORWARD gear label and an rpm inside the
/// engine's idle..governed band (never below idle, never past the governor). This pins the one
/// place the HUD reads gear/rpm from, on the same tick-truth components the HUD queries.
#[test]
fn drive_readout_reports_sane_operating_point() {
    use crate::track::transmission::{self, TransmissionMode};
    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    face_positive_z(&mut app, tank);
    drive_to_speed(&mut app, tank, 6.0, 2400);
    // A second more at full throttle so the box has settled onto a gear/rpm.
    for _ in 0..64 {
        drive_tick(&mut app, tank, 1.0, 0.0);
    }

    let world = app.world();
    let drive = world
        .get::<crate::track::sim::TrackDrive>(tank)
        .expect("the controlled tank drives");
    let state = world
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("the controlled tank carries transmission state");
    let tp = world
        .resource::<crate::track::sim::TrackGear>()
        .trans()
        .expect("the Tiger blueprint declares a transmission");
    let readout = transmission::readout(&state.0, tp, [drive.sides[0].speed, drive.sides[1].speed]);
    println!(
        "drive readout: gear {} rpm {:.0} (idle {}, governed {})",
        readout.gear_label, readout.rpm, tp.engine.idle_rpm, tp.engine.governed_rpm
    );

    assert!(
        (tp.engine.idle_rpm..=tp.engine.governed_rpm).contains(&readout.rpm),
        "a driven Tiger's readout rpm {} must lie in idle..governed [{}, {}]",
        readout.rpm,
        tp.engine.idle_rpm,
        tp.engine.governed_rpm,
    );
    assert!(
        !state.0.reverse,
        "the tank drove forward — the state must be on the forward ladder"
    );
    assert_eq!(
        readout.gear_label,
        format!("F{}", state.0.gear),
        "the label must name the actually-engaged forward gear",
    );
}

#[derive(Resource, Default)]
struct ScriptedDeterminismRun {
    digests: Vec<Vec<(String, crate::trace::CanonicalTankStateDigest)>>,
    checkpoints: Vec<ScriptedPose>,
    saw_airborne: bool,
    saw_grounded: bool,
    saw_steering_slip: bool,
    saw_shot: bool,
    fire_shells: usize,
    saw_projectile_spawn: bool,
    saw_projectile_march: bool,
}

#[derive(Clone, Copy)]
struct ScriptedPose {
    tick: usize,
    position: Vec3,
    rotation: Quat,
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
            &crate::track::sim::TrackDrive,
            &crate::track::sim::TrackGrip,
            &crate::track::sim::TrackContacts,
            &crate::tank::TankSim,
        ),
        With<Tank>,
    >,
    projectiles: Query<&crate::ballistics::ShellPath>,
    mut run: ResMut<ScriptedDeterminismRun>,
) {
    let tick = run.digests.len();
    let mut digests = Vec::with_capacity(roots.iter().len());
    let mut controlled = None;
    for (
        _,
        name,
        is_controlled,
        position,
        rotation,
        linear,
        angular,
        com,
        drive,
        grip,
        contacts,
        sim,
    ) in &roots
    {
        digests.push((
            name.as_str().to_owned(),
            crate::trace::canonical_tank_state_digest(
                position.0, rotation.0, linear.0, angular.0, drive, grip, sim,
            ),
        ));
        if is_controlled {
            controlled = Some((
                position.0, rotation.0, linear.0, angular.0, com.0, drive, contacts, sim,
            ));
        }
    }
    digests.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(digests.len(), 2, "the local duel has two simulation tanks");

    let (position, rotation, linear, angular, local_com, drive, contacts, sim) =
        controlled.expect("one controlled tank");
    let grounded = contacts.0.iter().filter(|side| !side.is_empty()).count();
    run.saw_airborne |= grounded == 0;
    run.saw_grounded |= grounded > 0;

    // Avian 0.7 `Forces::velocity_at_point`: v_point = v_linear + omega × (point − world_COM),
    // where world_COM = position + rotation * local_COM. Slip is witnessed directly from the
    // belt model's contact telemetry: a loaded contact whose longitudinal slip is past the
    // near-rest band while steer is commanded.
    let _ = (position, rotation, linear, angular, local_com);
    let loaded_contact_is_slipping = contacts
        .0
        .iter()
        .flatten()
        .any(|c| c.load > 0.0 && c.slip.abs() > 0.3);
    run.saw_steering_slip |=
        tick >= 240 && drive.steer.abs() > f32::EPSILON && loaded_contact_is_slipping;
    run.saw_shot |= sim.weapons.iter().any(|weapon| weapon.rounds_fired > 0);
    run.saw_projectile_spawn |= !projectiles.is_empty();
    run.saw_projectile_march |= projectiles.iter().any(|path| path.points.len() > 1);
    if matches!(tick, 119 | 219 | 339) {
        run.checkpoints.push(ScriptedPose {
            tick,
            position,
            rotation,
        });
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
        "track::sim::apply_track_forces",
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
                name.contains("track::sim::apply_track_forces")
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
    assert!(
        run.saw_steering_slip,
        "{label} put a loaded belt contact in the slipping regime while steering",
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

    let [settled, powered, steered] = run.checkpoints.as_slice() else {
        panic!("{label} did not capture the three scripted driving checkpoints");
    };
    assert_eq!(
        [settled.tick, powered.tick, steered.tick],
        [119, 219, 339],
        "{label} driving checkpoint ticks moved",
    );

    // DERIVED broad semantic bounds: reject a deterministic broken drivetrain or reversed steering
    // without treating one platform's floating-point trajectory as the portable contract.
    const MIN_PROGRESS_M: f32 = 1.0;
    const MIN_RIGHT_TURN_COMPONENT: f32 = 0.02;
    let settled_forward = settled.rotation * Vec3::NEG_Z;
    let straight_progress = (powered.position - settled.position).dot(settled_forward);
    assert!(
        straight_progress > MIN_PROGRESS_M,
        "{label} did not drive forward during straight throttle: {straight_progress} m",
    );

    let powered_forward = powered.rotation * Vec3::NEG_Z;
    let powered_right = powered.rotation * Vec3::X;
    let steering_progress = (steered.position - powered.position).dot(powered_forward);
    assert!(
        steering_progress > MIN_PROGRESS_M,
        "{label} stopped progressing when steering began: {steering_progress} m",
    );
    let right_turn_component = (steered.rotation * Vec3::NEG_Z).dot(powered_right);
    assert!(
        right_turn_component > MIN_RIGHT_TURN_COMPONENT,
        "{label} positive steer did not turn the hull right: component {right_turn_component}",
    );
}

/// Two fresh, full simulation compositions must replay one command script bit-for-bit. The witness
/// assertions keep this from passing because the scenario never reached contact, slip traction,
/// steering slip, or fire.
#[test]
fn full_simulation_replay_is_bit_exact_for_six_hundred_ticks() {
    let first = scripted_determinism_run();
    assert_scripted_determinism_witnesses(&first, "first fresh sim");

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
