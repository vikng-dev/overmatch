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

/// The simulation clock configured by the netcode composition. Fixtures advance it by one exact
/// fixed loop per [`App::update`], so every bound and elapsed-time report is stated in ticks.
const FIXED_TICKS_PER_SECOND: usize = 64;

fn start_fixed_clock(app: &mut App) {
    // Verified against Bevy 0.19: one `App::update` runs exactly one fixed loop.
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
}

fn elapsed_secs(ticks: usize) -> f32 {
    ticks as f32 / FIXED_TICKS_PER_SECOND as f32
}

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
    // pre-sized `link_count * 3` — never an empty vector awaiting a first-tick resize.
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
/// `world` selects the ground: `None` the flat slab + authored test course, `Some(grid)` a
/// synthetic height grid. Either marker is inserted before the first update, so
/// `terrain_grid::decode_height_grid` sees it and never decodes the shipped map.
///
/// `halves` selects how much of `ballistics` is composed. [`BallisticsHalves::SimOnly`] is the
/// DEDICATED SERVER's composition and the default: a shell is born bare, with no scene root, no
/// streak and no `Visibility`. A gate about what a round LOOKS like has to ask for the view half
/// explicitly, exactly as a client root does.
fn headless_app_on(
    world: Option<crate::terrain_grid::HeightGrid>,
    halves: BallisticsHalves,
) -> App {
    let mut app = headless_shell();
    // The gates are FIXTURED on the flat slab + authored test course (the 10°/20°/30° ramps,
    // the flat straight-line lanes): keep the heightmap world out even though the PNG ships in
    // assets/. The driving-feel probes pass their own analytic grid instead. Either marker only
    // has to be in place before the FIRST UPDATE, so inserting it after the plugin group is the
    // same thing to `terrain_grid::decode_height_grid`.
    match world {
        Some(grid) => {
            app.insert_resource(grid);
        }
        None => {
            app.insert_resource(crate::terrain_grid::ForceFlatWorld);
        }
    }
    // Physics + the SP spawn scenario are composition-root choices (see lib.rs SimPlugin note);
    // this exercises the single-player-shaped boot, headless.
    app.add_plugins((
        avian3d::prelude::PhysicsPlugins::default(),
        SimPlugin,
        crate::tank::sp_spawn_plugin,
    ))
    .add_observer(assert_tank_state_at_add)
    .add_observer(assert_range_table_at_add);
    if halves == BallisticsHalves::SimAndView {
        app.add_plugins(crate::ballistics::view_plugin);
    }

    finish_plugins(&mut app);
    app
}

/// Which halves of `ballistics` a fixture composes — see [`headless_app_on`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum BallisticsHalves {
    /// The dedicated server's composition: `SimPlugin` and nothing presentational.
    SimOnly,
    /// A client root's composition: `SimPlugin` plus `ballistics::view_plugin`.
    SimAndView,
}

/// The bare headless host every full-app fixture is built on: `DefaultPlugins` with no wgpu
/// backend, no primary window and no winit runner, and nothing of ours mounted yet. The caller adds
/// its own composition root and then calls [`finish_plugins`].
///
/// Split out of [`headless_app_on`] so a root OTHER than the game's — the dev sandboxes, which are
/// their own composition roots on `DefaultPlugins` (see [`boot_sandbox_headless`]) — can be booted
/// the same way instead of re-deriving this plugin-group surgery, the KTX2 workaround included.
///
/// The returned clock is FROZEN (`ManualDuration(ZERO)`); a caller that wants time picks its own
/// strategy, as [`start_fixed_clock`] does.
fn headless_shell() -> App {
    let mut app = App::new();
    app.add_plugins(crate::gpu_less_default_plugins(None))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    // UPSTREAM WORKAROUND — bevy_image 0.19 panics (not errors) transcoding UASTC KTX2 when NO
    // block-compressed format is supported: it sizes the SOURCE slice from the DESTINATION
    // format's block geometry, so the Rgba8 fallback reads 4x too far. `backends: None` means no
    // wgpu device, so nothing inserts `CompressedImageFormatSupport` and bevy_gltf resolves
    // `CompressedImageFormats::NONE` in its `finish()` — every UASTC texture in the tank glb
    // would abort the boot. Claiming ASTC 4x4 makes the arithmetic coincide and the transcode
    // exact; headless never uploads a texture, so this only decides which bytes sit in RAM (and
    // ASTC 4x4 is 8 bpp against RGBA8's 32 — it is also the cheaper lie). Must precede
    // `app.finish()` below: that is when the loaders read the resource.
    // Mechanism + suggested upstream fix: `upstream/bevy-ktx2-uastc-fallback-length-panic.md`.
    // DELETE THESE LINES when `tests/bevy_ktx2_uastc_fallback.rs` fails — that failure IS the
    // signal that bevy fixed the transcode and the workaround has become dead weight.
    app.insert_resource(bevy::image::CompressedImageFormatSupport(
        bevy::image::CompressedImageFormats::ASTC_LDR,
    ));
    app
}

/// `App::run` normally drives plugin finish/cleanup (some registration — e.g. Avian's diagnostics
/// resources — happens in `Plugin::finish`); a bare `update()` loop must do it.
fn finish_plugins(app: &mut App) {
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
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
            p.scene.as_ref().map_or_else(
                || "absent (sim-only composition)".to_string(),
                |scene| format!("{:?}", assets.load_state(scene)),
            ),
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
    booted_sim_on(None, BallisticsHalves::SimOnly)
}

/// [`booted_sim`] over an explicitly chosen world and ballistics composition — see
/// [`headless_app_on`].
fn booted_sim_on(
    world: Option<crate::terrain_grid::HeightGrid>,
    halves: BallisticsHalves,
) -> BootedSim {
    let lease = BOOT_LEASE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut app = headless_app_on(world, halves);

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
    booted_sp_app_with(BallisticsHalves::SimOnly)
}

/// [`booted_sp_app`] over an explicit ballistics composition — the shooting gates that are about
/// what a round LOOKS like ask for [`BallisticsHalves::SimAndView`].
fn booted_sp_app_with(halves: BallisticsHalves) -> BootedSim {
    let mut sim = booted_sim_on(None, halves);
    start_fixed_clock(&mut sim);
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

    // Start the clock at exactly one 64 Hz fixed tick per update, then let the belt contacts ground
    // and settle.
    start_fixed_clock(&mut app);
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
    // Settle for 60 exact ticks (DERIVED 60 / 64 = 0.9375 s).
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

    // 250 ticks (DERIVED 250 / 64 = 3.90625 s) of driving. The command is level state (no gather
    // to re-write it), so it holds; command slew, belt forces, and drive all run on the fixed clock.
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
    let current = app
        .world()
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("the tank carries transmission state");
    assert_ne!(
        *current,
        fresh_tank_transmission(&app),
        "without the offline dial, the declared transmission must advance instead of leaving its \
         spawn state inert; the net server and predicted client use this same SimPlugin path"
    );
}

/// One scripted headless drive for the element-law proof: boot the sim (the headless equivalent
/// of the `--offline` composition — [`headless_app`] mounts physics + `SimPlugin` + the SP duel
/// spawn, exactly what `GamePlugin` composes minus presentation), settle, hold full throttle for
/// ~4 sim-seconds, and return `(horizontal metres moved, total element strain in metres)`.
fn element_gate_run() -> (f32, f32) {
    let mut app = booted_sim();

    // Start the exact fixed clock and let the belt contacts ground and settle (the
    // `sim_boots_and_drives_headless` scaffold).
    start_fixed_clock(&mut app);
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

    // 250 ticks (DERIVED 250 / 64 = 3.90625 s) of full throttle, re-asserted every tick (no device
    // gather headless).
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

/// The element law is THE traction law — a scripted drive must both move the tank and put
/// real strain into the spawn-sized `TrackGripElements` slabs (a zero field would mean the
/// slabs were mis-sized and the invariant early-out silently skipped traction).
///
/// The spawn-sizing half of the fixture lives in [`assert_tank_state_at_add`], which every
/// boot in this file runs.
#[test]
fn the_element_law_drives_and_strains_on_a_scripted_drive() {
    let (moved, strain) = element_gate_run();
    assert!(
        moved > 2.0,
        "the element regime should drive the tank forward; moved {moved:.2} m"
    );
    assert!(
        strain > 0.0,
        "the element law must engage on a driving tank — strain stayed zero (mis-sized slabs \
         and the invariant early-out silently skipped the regime?)"
    );
}

/// The MG-tracer render gate, exercised on the real spawn path headless with a CLIENT's ballistics
/// composition (sim + view — the view half is what dresses a round at all). Firing the secondary
/// trigger must, over a burst:
///   * spawn tracer STREAKS (`TracerStreak`) for the ~1-in-5 tracer rounds, and
///   * spawn NO `shell.glb` scene root on ANY MG round. A shell in flight carries `ShellPath`; only a
///     main-gun-calibre round is dressed with `ShellVisual` + its `WorldAssetRoot` scene, so
///     `ShellPath + WorldAssetRoot` over an MG-only burst must stay empty while streaks appear.
///
/// The composition is load-bearing: on a server's `SimOnly` half the scene-root assertion would pass
/// for every calibre and prove nothing. [`a_server_composition_dresses_no_round_at_all`] is the gate
/// for that side.
#[test]
fn mg_rounds_stream_tracers_and_spawn_no_shell_scene() {
    use crate::ballistics::{ShellPath, ShellVisual, TracerStreak};
    use bevy::world_serialization::WorldAssetRoot;

    // A booted, settled rig: the muzzles/weapons must exist for `fire` to find a bore.
    let mut app = booted_sp_app_with(BallisticsHalves::SimAndView);

    let mut tank_q = app
        .world_mut()
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>();
    let tank = tank_q.single(app.world()).expect("one controlled tank");

    // Hold the secondary trigger (the MGs) — a burst. Do NOT press primary, so no 88 round is fired.
    // DERIVED: the MGs' authored 750 rpm interval is 5.12 ticks at 64 Hz, which ceilings to 6 ticks
    // (640 rpm effective), so 60 ticks yields about 10 shots per MG. Across the two MGs, the belt's
    // tracer_every=5 gives several tracer rounds. The 150-round belts stay far from dry (about 10
    // rounds each), so no belt swap interrupts the burst.
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

    // The other half of the same law, on the same rig: the MAIN GUN round IS dressed. Without it the
    // "no scene root" assertion above could be satisfied by a view plugin that dresses nothing.
    let mut saw_dressed_shell = false;
    for tick in 0..30 {
        {
            let mut entity = app.world_mut().entity_mut(tank);
            let mut cmd = entity
                .get_mut::<TankCommand>()
                .expect("tank carries a command");
            cmd.fire_secondary = false;
            // A single trigger edge; the gun is loaded from the boot settle above.
            cmd.fire_primary = tick == 0;
        }
        app.update();

        let world = app.world_mut();
        if world
            .query_filtered::<(), (With<ShellPath>, With<ShellVisual>, With<WorldAssetRoot>)>()
            .iter(world)
            .count()
            > 0
        {
            saw_dressed_shell = true;
            break;
        }
    }
    assert!(
        saw_dressed_shell,
        "the 88's round was never dressed — a main-gun shell must get ShellVisual + its shell.glb \
         scene root from ballistics::view_plugin",
    );
}

/// SERVER-COMPOSITION HONESTY — the structural invariant the ballistics sim/view split buys.
///
/// `SimPlugin` alone (what `net::server::run` composes) fires an 88 and an MG burst and marches them.
/// Not one projectile may carry presentation state: no scene root, no streak child, no mesh, no
/// `Visibility`, and no `ShellVisual` — the classification marker is the view's own interface to
/// `vfx`, so a server that attached it would be deciding what a round IS to the renderer. The server
/// does not decide what a round looks like, so it must not be able to.
#[test]
fn a_server_composition_dresses_no_round_at_all() {
    use crate::ballistics::{Projectile, ShellVisual, TracerStreak};
    use bevy::world_serialization::WorldAssetRoot;

    // SimOnly: the dedicated server's ballistics composition, on the real spawn path.
    let mut app = booted_sp_app_with(BallisticsHalves::SimOnly);

    let mut tank_q = app
        .world_mut()
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>();
    let tank = tank_q.single(app.world()).expect("one controlled tank");

    // BOTH calibres have to actually fly, or the gate is vacuous on the branch that matters: the
    // main gun is the only round that was ever dressed with a scene, the MG the only one with a
    // streak.
    let mut saw_main_gun = false;
    let mut saw_mg = false;
    let mut dressed = Vec::new();
    // DERIVED: long enough for the crew to finish the main gun's opening load (the MG cycles from
    // tick one, the 88 does not) — the loop breaks as soon as both calibres have flown.
    for _ in 0..200 {
        if saw_main_gun && saw_mg {
            break;
        }
        {
            let mut entity = app.world_mut().entity_mut(tank);
            let mut cmd = entity
                .get_mut::<TankCommand>()
                .expect("tank carries a command");
            // Both triggers held: the 88 fires the moment its gate opens, under a continuous MG burst.
            cmd.fire_primary = true;
            cmd.fire_secondary = true;
        }
        app.update();

        let world = app.world_mut();
        for projectile in world.query::<&Projectile>().iter(world) {
            if projectile.caliber() >= crate::ballistics::TRACER_MAX_CALIBER {
                saw_main_gun = true;
            } else {
                saw_mg = true;
            }
        }
        // Every render component the old shared spawn used to emit, plus the marker the view half
        // classifies on — all checked on the projectile itself.
        let world = app.world_mut();
        for (root, mesh, material, visibility, classified) in world
            .query_filtered::<(
                Has<WorldAssetRoot>,
                Has<Mesh3d>,
                Has<MeshMaterial3d<StandardMaterial>>,
                Has<Visibility>,
                Has<ShellVisual>,
            ), With<Projectile>>()
            .iter(world)
        {
            for (present, name) in [
                (root, "WorldAssetRoot"),
                (mesh, "Mesh3d"),
                (material, "MeshMaterial3d"),
                (visibility, "Visibility"),
                (classified, "ShellVisual"),
            ] {
                if present && !dressed.contains(&name) {
                    dressed.push(name);
                }
            }
        }
        // The streak is a CHILD, so it is counted on its own rather than on the projectile.
        let world = app.world_mut();
        if world.query::<&TracerStreak>().iter(world).count() > 0
            && !dressed.contains(&"TracerStreak")
        {
            dressed.push("TracerStreak");
        }
    }

    assert!(
        saw_main_gun && saw_mg,
        "the burst never put both calibres in flight (main gun {saw_main_gun}, MG {saw_mg}) — the \
         gate below would then be vacuously true for the branch that never fired",
    );
    assert!(
        dressed.is_empty(),
        "a server-composed projectile carried presentation state ({dressed:?}). SimPlugin must not \
         name a render component: dressing belongs to ballistics::view_plugin, which no server root \
         mounts",
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

    // DERIVED: both MGs' authored 750 rpm cadence ceilings to 6 ticks at 64 Hz (640 rpm effective),
    // so 120 ticks is about 20 rounds per gun — far past the first-round-of-the-burst case that
    // always worked.
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

    // The coax's muzzle pose, then pushed 20 cm BACK down the bore — the recoil retraction that puts a
    // mid-burst round's origin inside `Gun_Mask` as well as the coax barrel. RE-MEASURED against the
    // 2026-08-07 model (was 12 cm against the pre-restructure one, where `Gun_Mantlet_Ballistic`
    // reached the barrel at 8 cm): the coax muzzle moved 5.6 cm rearward and the mask now starts
    // 20 cm back down the bore. The number is a fact about the geometry, so it moves with it — what
    // the test asserts is unchanged: the origin is INSIDE the shooter's own armour.
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
                    // a mid-burst round is actually fired from, inside `Gun_Mask`.
                    origin: origin - Vec3::from(bore) * 0.20,
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

    // Control: omitting `shooter` holds the catch-up shell at the armor candidate. `Held` IS the
    // hidden stop — this fixture is a server's `SimOnly` composition, so nothing here draws at all
    // and the view's `Visibility::Hidden` is derived from exactly this fact on a client.
    app.world_mut().trigger(fire(None));
    app.update();
    let mut shells = app
        .world_mut()
        .query_filtered::<Entity, (With<ShellPath>, With<crate::ballistics::Held>)>();
    let control = shells.iter(app.world()).next().expect(
        "CONTROL: an un-attributed replica shell fired from inside the shooter's mantlet must be \
             HELD there — it cannot honestly fly or render a tracer",
    );
    app.world_mut().despawn(control);

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

/// [`booted_sim`] + the offline transmission dial exactly as `run_offline` mounts it
/// (`TransmissionFeelTest(mode)`), clock started, tracks grounded and settled. Returns the
/// sim and the controlled Tiger.
fn booted_offline_sim(mode: crate::track::transmission::TransmissionMode) -> (BootedSim, Entity) {
    let mut app = booted_sim();
    app.insert_resource(crate::track::sim::TransmissionFeelTest(mode));
    start_fixed_clock(&mut app);
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

fn fresh_tank_transmission(app: &BootedSim) -> crate::track::sim::TankTransmission {
    let params = app
        .world()
        .resource::<crate::track::sim::TrackGear>()
        .trans()
        .expect("the Tiger declares a transmission");
    crate::track::sim::TankTransmission::from_spec(params)
}

/// Write the drive command (level state, re-asserted every tick like the other headless
/// drives — no device gather exists here) and advance one exact 64 Hz tick.
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
/// MARGINAL brake-gated neutral turn toward the DERIVED `neutral_d_full` = 0.2885 m/s
/// (0.2808 before half_tread went 1.4904 → 1.5312; no unprovenanced 0.75 fraction shrinks
/// the target any more). MEASURED on the declared data: 0.131 rad/s
/// mean ground yaw, belts exactly ±neutral_d_full
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

/// Tiger pivot, hybrid continuous adapter: POWER-limited (the standstill pivot
/// commands steer force up to capacity and the power-conservation scale is the binding
/// limiter, so the rate settles where engine power balances scrub dissipation; the old
/// neutral_d_full speed FLOOR used ~68 kW of the ~407 kW budget and pivoted at
/// 0.131 rad/s). MEASURED on the declared data: 0.654 rad/s mean ground yaw pre-stage-B;
/// 0.646 rad/s with the stage-B crank (the declutched steer demand parks the crank at the
/// same peak-torque operating point the old rev floor used, minus the rev-governor's ~30
/// rpm taper droop — steady rate preserved by design); gated at ≥ 0.35 rad/s (margin,
/// same policy as the L600 gate).
#[test]
fn pivot_tiger_hybrid() {
    tiger_pivot_gate(crate::track::transmission::TransmissionMode::Hybrid, 0.35);
}

/// Stage B pivot SPIN-UP gate (new): the standstill pivot's power budget now follows the
/// CRANK, not the input slew. MEASURED on the declared data: 0.95 s to 90% of steady yaw —
/// essentially the old 0.94 s, NOT the memo's expected 1.2–1.5 s, and the reason is honest
/// physics of this model: the power gate cannot bind at v ≈ 0 (delivered power is F·v),
/// so the early pivot phase is CAPACITY-limited while the crank spool (idle → ~2100 rpm at
/// τ/J ≈ 400 rad/s² ≈ 0.4 s, J = 4 kg·m²) completes underneath the ~0.5 s steer input
/// slew — by the time the belts are fast enough for power to bind, the crank has arrived.
/// The yaw-time gate therefore pins the measured 0.95 s with margin, and the CRANK STATE
/// itself is what discriminates stage B from the rpm-floor hack: ω_e must still be LOW
/// shortly after the command (a floor would teleport it) and must park at the peak-torque
/// operating point at steady state.
#[test]
fn pivot_spin_up_tiger_hybrid() {
    use crate::track::transmission::TransmissionMode;
    let (mut app, tank) = booted_offline_sim(TransmissionMode::Hybrid);
    let total = 8 * FIXED_TICKS_PER_SECOND;
    let mut yaws = Vec::with_capacity(total);
    let mut early_rpm = 0.0f32;
    for tick in 0..total {
        drive_tick(&mut app, tank, 0.0, 1.0);
        yaws.push(yaw_rate(&mut app, tank));
        if tick == 6 {
            // ~0.1 s in: the crank must still be climbing (idle + a few hundred rpm).
            early_rpm = app
                .world()
                .get::<crate::track::sim::TankTransmission>(tank)
                .expect("tank carries transmission state")
                .0
                .omega_e
                / (std::f32::consts::TAU / 60.0);
        }
    }
    let steady: f32 =
        yaws[total - FIXED_TICKS_PER_SECOND..].iter().sum::<f32>() / FIXED_TICKS_PER_SECOND as f32;
    assert!(
        steady.abs() > 0.35,
        "the steady pivot must be live for the spin-up measurement (got {steady:.3})"
    );
    let target = 0.9 * steady.abs();
    let rise_tick = yaws
        .iter()
        .position(|y| y.abs() >= target)
        .expect("yaw must reach 90% of steady inside the run");
    let secs = elapsed_secs(rise_tick + 1);
    let steady_rpm = app
        .world()
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("tank carries transmission state")
        .0
        .omega_e
        / (std::f32::consts::TAU / 60.0);
    println!(
        "tiger hybrid pivot spin-up: {secs:.2} s to 90% of steady {steady:.3} rad/s; \
         crank {early_rpm:.0} rpm @ 0.1 s -> {steady_rpm:.0} rpm steady"
    );
    assert!(
        (0.6..=1.6).contains(&secs),
        "pivot spin-up {secs:.2} s outside the pinned band around the measured 0.95 s"
    );
    assert!(
        early_rpm < 1_500.0,
        "0.1 s after the command the crank must still be spooling ({early_rpm:.0} rpm) — \
         an instant high rpm means the rpm-floor hack is back"
    );
    assert!(
        (1_900.0..=2_200.0).contains(&steady_rpm),
        "the steady pivot crank must park at the peak-torque operating point \
         (~2100 rpm), got {steady_rpm:.0}"
    );
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
    for _ in 0..(20 * FIXED_TICKS_PER_SECOND) {
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
/// * RELEASE (coast): engine drag from the RISING motoring curve
///   (`drag_fraction × peak` anchored at mid-band 1550 rpm, growing linearly with crank
///   speed), stage B: at the CRANK, reaching the belt through the engaged coupling — so
///   the drag torque decelerates crank AND belt together, and the belt's share is the old
///   force × `I_m/(I_m + k²J)` (F7: 32 000/(32 000 + 37.1²·4) ≈ 0.85), plus the shift
///   windows are genuinely drag-free (declutched). MEASURED on the declared data:
///   6 → 2 m/s in 12.3 s (12.2 s under the old flat 462 N·m drag — the mid-band anchor is
///   the point: the coast's downshift chain spends most of its time near mid-band where
///   the curve reads ≈ 1×, so flat-ground coast feel is deliberately unchanged while
///   overrun drag above governed grew; the gate's ≤ 14 s absorbs it with margin for float
///   drift, nothing else). The fix-round brief hoped for 8 s — unreachable without
///   rolling resistance, WHICH THE CONTACT MODEL DOES NOT HAVE (a real Tiger's ~25–35 kN
///   of rolling drag would dominate its own engine braking; ground resistance belongs to
///   the terrain/ground-type mechanic, ADR-0007 bucket 3 — not to the drivetrain, and not
///   tunable-by-feel here). Also pinned: past the command-shaper's release slew, coasting
///   never accelerates (the old code ACCELERATED on opposite input — the regression this
///   kills).
/// * OPPOSITE THROTTLE: service brakes at the declared capacity, DUAL-anchored
///   (96 kN/side: the settled 20° park hold at 95.6 kN/side demand,
///   0.343 g total service decel inside the 0.2–0.35 g WWII heavy-tank band; the old
///   250 kN was the circular grip-limit sizing — 1.17 s from 6 m/s was the
///   energy-impossible tell). Analytic prediction: 2 × 96 kN / 57 t = 3.37 m/s² in the
///   full phase, plus engine drag (~17 kN in F7, growing through downshifts)
///   ≈ 3.6+ m/s², plus the command shaper's ~0.5 s press slew dead time → from 6.0 m/s
///   ≈ 0.5 + 5.0/3.6 ≈ 1.9 s to 1 m/s. MEASURED: 2.36 s. Gate ≤ 3 s (margin for
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
    for tick in 0..(14 * FIXED_TICKS_PER_SECOND) {
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
        elapsed_secs(coast_ticks)
    );

    // Phase 2 — service brakes: opposite throttle from ≥ 6 m/s. Budget 3 s: the
    // dual-anchored capacity predicts ≈ 1.9 s including the input slew dead time (see
    // the doc comment's arithmetic).
    drive_to_speed(&mut app, tank, 6.0, 2400);
    released = hull_speed(&mut app, tank);
    let mut brake_ticks = None;
    for tick in 0..(3 * FIXED_TICKS_PER_SECOND) {
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
        elapsed_secs(brake_ticks)
    );
}

/// BENCHMARK, not a requirement: what the Tiger's holding power measures on the course's 30° face
/// right now (Yan ruling, 2026-08-07 — *"there is no requirement for a tiger to hold on 30 deg, it
/// should be purely emergent — the holding power is purely a benchmark of how much it can hold
/// right now"*). Nothing in the design says a 57-tonne tank with 1940s Argus discs must hold half a
/// g of slope; the number is an OUTPUT of the brake model, the geometry and the contact law, and it
/// is recorded here so that it cannot move without someone saying so.
///
/// The park latch itself IS still a requirement — zero input at rest must latch, whatever the
/// latch then holds — so that assertion keeps its teeth below.
///
/// ANCHOR: the 2026-08-07 wheel redesign. The road wheels became five-shell mixed-substance objects
/// and their tread radius measured 0.386441 m against the old 0.386968 — 0.53 mm smaller — which is
/// the entire cause of the drift moving 0.0055 → 0.0928 m. Confirmed by A/B: forcing the OLD radius
/// into this same branch's geometry returns the drift to 0.0055 m, and a sweep between them is FLAT
/// at ~0.005 m until it falls off a cliff inside a 10 µm window (0.386470 → 0.0063 m, 0.386460 →
/// 0.0561 m). The shipped radius sits ~30 µm the wrong side of that cliff, so this is a discrete
/// contact-regime change — a road wheel's engagement flipping — and NOT a gradual weakening.
///
/// That is also why the band is tight rather than generous. The measurement is bit-stable here
/// (0.0928 m on five consecutive runs), so the tolerance exists to absorb platform float variation,
/// not run-to-run noise; and the regime it guards is 15× away (0.006 m if the geometry crosses back
/// over the cliff), so ±0.005 m cannot mask a regime change while still catching any real drift in
/// the brake model or the contact law.
///
/// RE-PIN CONSCIOUSLY. When the geometry or the brake model changes on purpose, this number moves
/// with it — read the new value off the printed MEASURED line, put it here, and say why in the
/// commit. A silent re-pin turns the tripwire back into the acceptance gate it used to be.
#[test]
fn slope_park_benchmarks_the_30_deg_holding_power_tiger() {
    use crate::track::transmission::TransmissionMode;
    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    let course_rotation = Quat::from_rotation_x(30.0_f32.to_radians());
    let (mass, dynamic_capacity, static_factor) = {
        let world = app.world();
        let mass = world.resource::<TankBlueprint>().spec.mass;
        let tp = world
            .resource::<crate::track::sim::TrackGear>()
            .trans()
            .expect("the Tiger declares a transmission");
        (mass, tp.brake_capacity_n, tp.brake_static_factor)
    };
    let demand = mass * 9.81 * 30.0_f32.to_radians().sin();
    let static_capacity = 2.0 * dynamic_capacity * static_factor;
    let margin = static_capacity - demand;
    assert!(
        margin > 0.0,
        "30-degree static-hold fixture needs positive capacity margin (capacity \
         {static_capacity:.0} N, demand {demand:.0} N)"
    );

    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::Position>().unwrap().0 = Vec3::new(14.0, 3.4, -40.0);
        e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 = course_rotation;
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
    }
    for _ in 0..256 {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    let p0 = app
        .world()
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has a position")
        .0;
    for _ in 0..(4 * FIXED_TICKS_PER_SECOND) {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    let world = app.world();
    let p1 = world
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has a position")
        .0;
    let st = world
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("tank carries transmission state")
        .0;
    let drift = (p1 - p0).length();
    println!(
        "tiger 30-deg slope park: MEASURED drift {drift:.4} m over 4 s, park latch {}; \
         DERIVED static {static_capacity:.0} N, demand {demand:.0} N, margin {margin:.0} N",
        st.park
    );
    assert!(
        st.park,
        "zero input at rest on the 30-degree ramp must latch the park brake"
    );
    // The BENCHMARK. See the doc above: this asserts what the holding power IS, not what it owes.
    // 0.0035, was 0.0928: re-pinned 2026-08-08 against the rebuilt track shoe. The 26x drop in
    // drift is NOT explained by the 1.256 mm the shoe narrowed, and is on the open queue rather
    // than accounted for here (Yan's ruling, same day). The benchmark records what the holding
    // power IS; it is pinned again so the NEXT move is caught, not because this one is understood.
    const MEASURED_DRIFT_M: f32 = 0.0035;
    const BAND_M: f32 = 0.005;
    assert!(
        (drift - MEASURED_DRIFT_M).abs() < BAND_M,
        "30-degree holding power moved: drifted {drift:.4} m over 4 s against the pinned \
         {MEASURED_DRIFT_M:.4} m ± {BAND_M}. This is a benchmark, not a requirement — if the \
         geometry or the brake model changed on purpose, re-pin to the measured value and say so \
         in the commit; if nothing changed on purpose, something regressed."
    );
}

/// Beyond-capability inverse gate: keep the real Tiger geometry and dynamic brakes, but author a
/// valid synthetic 1.1× static factor. The DERIVED 211,200 N static capacity remains below the
/// DERIVED 279,585 N 30° demand, so a zero-input parking latch must breach. Once either belt leaves
/// the at-rest band, query the production capacity seam and assert that the latch has dropped to
/// 96,000 N/side dynamic braking while the tank continues downhill.
#[test]
fn synthetic_30_deg_park_breaches_to_dynamic_braking() {
    use crate::track::transmission::{
        PARK_ENGAGE_SPEED, TransmissionMode, brake_capacity_for_regime,
    };
    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    let course_rotation = Quat::from_rotation_x(30.0_f32.to_radians());
    let uphill = course_rotation * Vec3::NEG_Z;
    let mass = app.world().resource::<TankBlueprint>().spec.mass;
    let (dynamic_capacity, static_factor) = {
        let mut gear = app
            .world_mut()
            .resource_mut::<crate::track::sim::TrackGear>();
        let tp = gear.trans_mut().expect("the Tiger declares a transmission");
        tp.brake_static_factor = 1.1;
        (tp.brake_capacity_n, tp.brake_static_factor)
    };
    let demand = mass * 9.81 * 30.0_f32.to_radians().sin();
    let static_capacity = 2.0 * dynamic_capacity * static_factor;
    assert!(
        static_capacity < demand,
        "synthetic park must be beyond static capability (capacity {static_capacity:.0} N, \
         demand {demand:.0} N)"
    );

    let fresh_transmission = fresh_tank_transmission(&app);
    let start_position = {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::Position>().unwrap().0 = Vec3::new(14.0, 3.4, -40.0);
        e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 = course_rotation;
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
        *e.get_mut::<crate::track::sim::TankTransmission>().unwrap() = fresh_transmission;
        let mut drive = e.get_mut::<crate::track::sim::TrackDrive>().unwrap();
        drive.throttle = 0.0;
        drive.steer = 0.0;
        drive.sides[0].speed = 0.0;
        drive.sides[1].speed = 0.0;
        e.get::<avian3d::prelude::Position>().unwrap().0
    };

    let mut moving_capacity = None;
    let mut final_belt_m = 0.0;
    for _ in 0..(2 * FIXED_TICKS_PER_SECOND) {
        // Inspect the PRE-TICK belt state that the production brake law will consume below. The
        // first tick after breakaway may cross the threshold while still using static capacity;
        // this pins the following tick, whose input is already outside the band, to dynamic.
        {
            let world = app.world();
            let drive = world
                .get::<crate::track::sim::TrackDrive>(tank)
                .expect("tank drives");
            if moving_capacity.is_none()
                && drive
                    .sides
                    .iter()
                    .any(|side| side.speed.abs() >= PARK_ENGAGE_SPEED)
            {
                let tp = world
                    .resource::<crate::track::sim::TrackGear>()
                    .trans()
                    .expect("the Tiger declares a transmission");
                moving_capacity = Some(brake_capacity_for_regime(
                    tp,
                    true,
                    0.0,
                    drive.sides[0].speed,
                ));
            }
        }
        drive_tick(&mut app, tank, 0.0, 0.0);
        let world = app.world();
        let drive = world
            .get::<crate::track::sim::TrackDrive>(tank)
            .expect("tank drives");
        final_belt_m = (drive.sides[0].speed + drive.sides[1].speed) / 2.0;
    }

    let world = app.world();
    let end_position = world
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has a position")
        .0;
    let state = world
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("tank carries transmission state")
        .0;
    let downhill_distance = -(end_position - start_position).dot(uphill);
    let moving_capacity = moving_capacity.expect("the breached park never left the at-rest band");
    println!(
        "synthetic 30-deg park: DERIVED static {static_capacity:.0} N vs demand {demand:.0} N; \
         MEASURED moving cap {moving_capacity:.0} N/side, belt_m {final_belt_m:.3} m/s, \
         downhill {downhill_distance:.3} m"
    );
    assert!(
        state.park,
        "zero input must retain the parking latch after breach"
    );
    assert_eq!(
        moving_capacity, dynamic_capacity,
        "a breached moving latch must drop to dynamic braking that same tick"
    );
    assert!(
        final_belt_m < -PARK_ENGAGE_SPEED,
        "the beyond-capability park must keep sliding (belt_m {final_belt_m:.3} m/s)"
    );
    assert!(
        downhill_distance > 0.25,
        "the beyond-capability park must move downhill (moved {downhill_distance:.3} m)"
    );
}

/// Stage A (signed shaft) grade gate: from REST mid-face on the course's 20° ramp, held
/// full W on the real Tiger (L600). Two assertions the `|m|` shaft made impossible:
///
/// * the box must NEVER walk the gear ladder UPWARD while the hull is moving backward —
///   pre-fix a backslide read as high FORWARD rpm, the governor cut drive to zero, and
///   the scheduler laddered 1→6 while the tank slid backward at −2..−3 m/s off the ramp;
/// * the tank must either CREST the ramp or hold position — it must never end up sliding
///   backward off the ramp in a forward gear with W held.
///
/// MEASURED post-fix (recorded per the stage-A brief): from rest at z = −40 the Tiger
/// launches in F1 with no backward roll beyond the settle jitter and CRESTS (hull past
/// the high edge at z ≈ −44.7, ~4.9 m along the face) in 7.1 s — mean ~0.7 m/s climb
/// including the ~0.5 s input slew — with gear trace [1] the whole way: F1 holds 20°,
/// and no upshift is predicted to land, so none is attempted. Budget 30 s with a hold
/// fallback so grade-scheduling changes in later stages don't spuriously fail the gate.
#[test]
fn ramp_climb_20_deg_never_upshifts_backward_tiger() {
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
    // Settle onto the face under zero input (drop + ring-down + park latch) — the same
    // seat the park gate uses; the climb starts from a genuine held rest.
    for _ in 0..256 {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    let z0 = app
        .world()
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has a position")
        .0
        .z;
    let mut prev_gear = app
        .world()
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("tank carries transmission state")
        .0
        .gear;
    let mut trace = vec![prev_gear];
    let mut crest_tick = None;
    // Stage-B launch grip-utilization measurement (the slope-investigation wheelspin): max
    // belt-vs-hull slip during the first 3 s of the from-rest grade launch. Pre-stage-B the
    // rev floor held peak-torque force (~747 kN) against the ~473 kN on-slope grip ceiling
    // for the whole launch (MEASURED baseline: 0.370 m/s max slip); the clutch-limited
    // launch locks the belt to the crank within ticks and the reflected crank inertia
    // (k₁²·J ≈ 20× the belt inertia in F1) pins it there — MEASURED stage B: 0.155 m/s,
    // a 58% cut. Printed, not gated — the crest/no-rollback asserts are the gate.
    let mut max_launch_slip = 0.0f32;
    for tick in 0..(30 * FIXED_TICKS_PER_SECOND) {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let world = app.world();
        let v = world
            .get::<avian3d::prelude::LinearVelocity>(tank)
            .expect("tank has velocity")
            .0;
        let rot = world
            .get::<avian3d::prelude::Rotation>(tank)
            .expect("tank has rotation")
            .0;
        // Signed hull speed along the hull's forward axis (−Z local; uphill here).
        let v_fwd = v.dot(rot * Vec3::NEG_Z);
        if tick < 3 * FIXED_TICKS_PER_SECOND {
            let drive = world
                .get::<crate::track::sim::TrackDrive>(tank)
                .expect("tank drives");
            let belt_m = (drive.sides[0].speed + drive.sides[1].speed) / 2.0;
            max_launch_slip = max_launch_slip.max(belt_m - v_fwd);
        }
        let st = world
            .get::<crate::track::sim::TankTransmission>(tank)
            .expect("tank carries transmission state")
            .0;
        assert!(!st.reverse, "held W must stay on the F ladder");
        if st.gear > prev_gear {
            assert!(
                v_fwd >= -0.05,
                "tick {tick}: upshift {prev_gear} -> {} committed while the hull was \
                 moving BACKWARD ({v_fwd:.2} m/s) — the signed-shaft scheduler must make \
                 this impossible",
                st.gear
            );
        }
        if trace.last() != Some(&st.gear) {
            trace.push(st.gear);
        }
        prev_gear = st.gear;
        let z = world
            .get::<avian3d::prelude::Position>(tank)
            .expect("tank has a position")
            .0
            .z;
        assert!(
            z < -36.5,
            "tick {tick}: the tank slid backward off the ramp under held W (z {z:.1}, \
             started {z0:.1}, gear trace {trace:?})"
        );
        if z <= -44.6 {
            crest_tick = Some(tick + 1);
            break;
        }
    }
    println!(
        "tiger 20-deg ramp launch: max belt-vs-hull slip {max_launch_slip:.3} m/s (first 3 s)"
    );
    match crest_tick {
        Some(t) => println!(
            "tiger 20-deg ramp climb from rest: CRESTED in {:.1} s, gear trace {trace:?}",
            elapsed_secs(t)
        ),
        None => {
            // Not cresting is acceptable ONLY as a hold: no net rollback, not sliding.
            let world = app.world();
            let z1 = world
                .get::<avian3d::prelude::Position>(tank)
                .expect("tank has a position")
                .0
                .z;
            let v = world
                .get::<avian3d::prelude::LinearVelocity>(tank)
                .expect("tank has velocity")
                .0;
            println!(
                "tiger 20-deg ramp climb from rest: HELD at z {z1:.2} (from {z0:.2}), \
                 gear trace {trace:?}"
            );
            assert!(
                z1 <= z0 + 0.5 && v.length() < 0.3,
                "30 s of held W on the 20-deg face must crest or HOLD — not roll back \
                 (z {z0:.2} -> {z1:.2}, |v| {:.2}, gear trace {trace:?})",
                v.length()
            );
        }
    }
}

#[derive(Debug)]
struct GradeApproachResult {
    crest_secs: f32,
    gear_trace: Vec<u8>,
    grade_shift: Option<(u8, u8)>,
    hill_hold_ticks: usize,
    min_uphill_speed: f32,
    max_rollback_m: f32,
}

/// Stage-C approach fixture: place the already-rolling Tiger on the lower 20-degree face in F6,
/// with belt and hull speeds matched at a DERIVED 4.0 m/s (about 1722 rpm DERIVED in F6, above
/// the ordinary down band) and W already shaped to full. This removes spawn slew/wheelspin from
/// the question and isolates the scheduler under the DERIVED 191.2 kN grade demand. The only
/// variant datum changed is shift addressing.
///
/// SCOPE: the fixture settles onto the face first and SEEDS the demand
/// EMA at its declared demand, so it proves scheduler behavior GIVEN that demand — it is not
/// evidence about contact-driven EMA acquisition. That acquisition (first-sample seed +
/// convergence under sustained reactions) is pinned at unit level by
/// `transmission::tests::demand_ema_seeds_from_first_sample_and_converges`.
fn run_grade_approach_20_deg(
    addressing: crate::track::transmission::ShiftAddressing,
) -> GradeApproachResult {
    use crate::track::transmission::{SchedulerState, TransmissionMode};
    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    app.world_mut()
        .resource_mut::<crate::track::sim::TrackGear>()
        .trans_mut()
        .expect("the Tiger declares a transmission")
        .shift_addressing = addressing;

    let rot = Quat::from_rotation_x(20.0_f32.to_radians());
    let approach_speed = 4.0;
    let mut transmission = fresh_tank_transmission(&app);
    transmission.0.gear = 6;
    // Seed the demand EMA at the fixture's own DERIVED grade demand:
    // a teleport-spawn otherwise starts the EMA from near-zero settle reactions and the
    // scheduler's first ~13 confirm ticks argue against a load the fixture has DECLARED.
    transmission.0.demand_n = 57_000.0 * 9.81 * 20.0_f32.to_radians().sin();
    transmission.0.demand_initialized = true;
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::Position>().unwrap().0 = Vec3::new(0.0, 1.50, -37.0);
        e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 = rot;
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
    }
    // Land the contact field BEFORE injecting the rolling approach:
    // the old single-teleport start left the belts airborne and free-spinning for ~0.6 s —
    // the spin blipped over the up band with the demand observer honestly reading "no
    // load", committing a reserve-legal-looking F6→F7 the moment before ground contact.
    // That is exactly the spawn-slew/wheelspin artifact this fixture documents removing
    // (the old frozen landing predictor masked it by over-refusing every loaded upshift).
    for _ in 0..128 {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 =
            rot * Vec3::NEG_Z * approach_speed;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
        let mut drive = e.get_mut::<crate::track::sim::TrackDrive>().unwrap();
        drive.throttle = 1.0;
        drive.steer = 0.0;
        drive.sides[0].speed = approach_speed;
        drive.sides[1].speed = approach_speed;
        *e.get_mut::<crate::track::sim::TankTransmission>().unwrap() = transmission;
    }

    let z0 = app
        .world()
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has position")
        .0
        .z;
    let mut previous_gear = 6u8;
    let mut trace = vec![6];
    let mut grade_shift = None;
    let mut hill_hold_ticks = 0;
    let mut min_uphill_speed = f32::INFINITY;
    let mut furthest_uphill_z = z0;
    let mut max_rollback_m = 0.0f32;
    for tick in 0..(20 * FIXED_TICKS_PER_SECOND) {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let world = app.world();
        let state = world
            .get::<crate::track::sim::TankTransmission>(tank)
            .expect("tank carries transmission state")
            .0;
        if trace.last() != Some(&state.gear) {
            trace.push(state.gear);
        }
        if let SchedulerState::GradeShift { from, to } = state.scheduler {
            grade_shift.get_or_insert((from, to));
        }
        if state.hill_hold {
            hill_hold_ticks += 1;
        }
        match addressing {
            crate::track::transmission::ShiftAddressing::Direct => {}
            crate::track::transmission::ShiftAddressing::Sequential => assert!(
                previous_gear.abs_diff(state.gear) <= 1,
                "Sequential skipped F{previous_gear} -> F{} (trace {trace:?})",
                state.gear
            ),
        }
        previous_gear = state.gear;

        let position = world
            .get::<avian3d::prelude::Position>(tank)
            .expect("tank has position")
            .0;
        let velocity = world
            .get::<avian3d::prelude::LinearVelocity>(tank)
            .expect("tank has velocity")
            .0;
        let belt = world
            .get::<crate::track::sim::TrackDrive>(tank)
            .expect("tank drives");
        let belt_m = (belt.sides[0].speed + belt.sides[1].speed) / 2.0;
        // Measure motion against the COURSE tangent, not the hull's springing pitch: projecting
        // heave onto an oscillating body-forward axis produced a false ~0.07 m/s MEASURED
        // "rollback" during fixture calibration.
        let forward_speed = velocity.dot(rot * Vec3::NEG_Z);
        min_uphill_speed = min_uphill_speed.min(forward_speed);
        furthest_uphill_z = furthest_uphill_z.min(position.z);
        max_rollback_m = max_rollback_m.max(position.z - furthest_uphill_z);
        let rollback_limit = match addressing {
            crate::track::transmission::ShiftAddressing::Direct => 0.02,
            // DERIVED 0.05 m compliance budget: the
            // sequential cascade may settle its static grip anchors under hill hold, but may not
            // slide off backward.
            crate::track::transmission::ShiftAddressing::Sequential => 0.05,
        };
        assert!(
            max_rollback_m <= rollback_limit && position.z <= z0 + 0.10,
            "{addressing:?} tick {tick}: hull rolled backward on the 20-degree face \
             (v_fwd {forward_speed:.3}, rollback {max_rollback_m:.4} m, z {:.3}, \
             trace {trace:?}, scheduler {:?}, \
             belt_m {belt_m:.3}, demand {:.0}, hill-hold ticks {hill_hold_ticks})",
            position.z,
            state.scheduler,
            state.demand_n,
        );
        if position.z <= -44.6 {
            return GradeApproachResult {
                crest_secs: elapsed_secs(tick + 1),
                gear_trace: trace,
                grade_shift,
                hill_hold_ticks,
                min_uphill_speed,
                max_rollback_m,
            };
        }
    }
    panic!(
        "{addressing:?} F6 approach did not crest in 20 s (trace {trace:?}, \
         grade shift {grade_shift:?}, hill-hold ticks {hill_hold_ticks})"
    );
}

/// Stage C high-gear grade scheduling on the real Tiger/contact course. Direct must perform one
/// reserve-commanded skip and crest; Sequential must pay adjacent windows, also never roll back,
/// and expose the honest cost as a slower crest or a nonzero hill-hold interval.
#[test]
fn grade_approach_20_deg_direct_vs_sequential_tiger() {
    use crate::track::transmission::ShiftAddressing;
    let direct = run_grade_approach_20_deg(ShiftAddressing::Direct);
    let sequential = run_grade_approach_20_deg(ShiftAddressing::Sequential);
    println!(
        "tiger 20-deg F6 approach: Direct {:.3} s {:?}, shift {:?}, hold {} ticks, \
         min {:.3} m/s, rollback {:.4} m; Sequential {:.3} s {:?}, shift {:?}, \
         hold {} ticks, min {:.3} m/s, rollback {:.4} m",
        direct.crest_secs,
        direct.gear_trace,
        direct.grade_shift,
        direct.hill_hold_ticks,
        direct.min_uphill_speed,
        direct.max_rollback_m,
        sequential.crest_secs,
        sequential.gear_trace,
        sequential.grade_shift,
        sequential.hill_hold_ticks,
        sequential.min_uphill_speed,
        sequential.max_rollback_m,
    );
    let (from, to) = direct
        .grade_shift
        .expect("Direct must expose a reserve-commanded shift");
    assert!(
        from.abs_diff(to) >= 2,
        "Direct must skip at least one intermediate gear"
    );
    assert!(
        sequential.crest_secs > direct.crest_secs || sequential.hill_hold_ticks > 0,
        "Sequential must expose the paid-window cost (Direct {direct:?}, Sequential {sequential:?})"
    );
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct StageCReplayTick {
    state: crate::track::transmission::TransmissionState,
    belt_speed_bits: [u32; 2],
}

/// A slope script that exercises stage-C memory in the real FixedRadii offline composition. The
/// Sequential F6 approach pays adjacent windows, accumulating the demand EMA and confirmation
/// evidence, retaining a target, and entering hill hold before it crests.
fn scripted_stage_c_replay_run() -> Vec<StageCReplayTick> {
    use crate::track::transmission::{ShiftAddressing, TransmissionMode};

    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    app.world_mut()
        .resource_mut::<crate::track::sim::TrackGear>()
        .trans_mut()
        .expect("the Tiger declares a transmission")
        .shift_addressing = ShiftAddressing::Sequential;

    let rot = Quat::from_rotation_x(20.0_f32.to_radians());
    let approach_speed = 4.0;
    let mut transmission = fresh_tank_transmission(&app);
    transmission.0.gear = 6;
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::Position>().unwrap().0 = Vec3::new(0.0, 1.50, -37.0);
        e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 = rot;
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 =
            rot * Vec3::NEG_Z * approach_speed;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
        let mut drive = e.get_mut::<crate::track::sim::TrackDrive>().unwrap();
        drive.throttle = 1.0;
        drive.steer = 0.0;
        drive.sides[0].speed = approach_speed;
        drive.sides[1].speed = approach_speed;
        *e.get_mut::<crate::track::sim::TankTransmission>().unwrap() = transmission;
    }

    let mut ticks = Vec::with_capacity(512);
    for _ in 0..512 {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let world = app.world();
        let state = world
            .get::<crate::track::sim::TankTransmission>(tank)
            .expect("tank carries transmission state")
            .0;
        let drive = world
            .get::<crate::track::sim::TrackDrive>(tank)
            .expect("tank carries belt state");
        ticks.push(StageCReplayTick {
            state,
            belt_speed_bits: [
                drive.sides[0].speed.to_bits(),
                drive.sides[1].speed.to_bits(),
            ],
        });
    }
    ticks
}

/// D-replay: two fresh FixedRadii offline worlds must reproduce every stage-C state field and both
/// belt speeds bit-for-bit on every scripted slope tick. The witnesses prevent a vacuous pass that
/// never exercised the EMA, counter, held target, or hill-hold latch.
#[test]
fn stage_c_slope_replay_is_bit_exact_every_tick() {
    let first = scripted_stage_c_replay_run();
    let max_demand = first
        .iter()
        .map(|tick| tick.state.demand_n)
        .fold(0.0f32, f32::max);
    let max_counter = first
        .iter()
        .map(|tick| tick.state.grade_confirm_ticks)
        .max()
        .unwrap_or(0);
    let target_ticks = first
        .iter()
        .filter(|tick| tick.state.grade_target > 0)
        .count();
    let hold_ticks = first.iter().filter(|tick| tick.state.hill_hold).count();
    assert!(
        max_demand > 0.0,
        "slope script never exercised the demand EMA"
    );
    assert!(
        max_counter > 0,
        "slope script never accumulated deficit evidence"
    );
    assert!(
        target_ticks > 0,
        "slope script never retained a sequential target"
    );
    assert!(hold_ticks > 0, "slope script never latched hill hold");

    let second = scripted_stage_c_replay_run();
    assert_eq!(first.len(), second.len());
    if let Some((tick, (left, right))) = first
        .iter()
        .zip(&second)
        .enumerate()
        .find(|(_, (left, right))| left != right)
    {
        panic!(
            "stage-C replay first differs at slope tick {tick}:\nleft:  {left:#?}\nright: {right:#?}"
        );
    }
    println!(
        "stage-C bit replay: {}/{} ticks exact; max demand {:.0} N, max counter {}, target {} ticks, hold {} ticks",
        first.len(),
        second.len(),
        max_demand,
        max_counter,
        target_ticks,
        hold_ticks,
    );
}

/// Stage-C hill hold on the real 20-degree face. After the normal zero-input settle, force the
/// preselector into F5 at rest and hold W: F5 cannot launch against the DERIVED 191.2 kN slope
/// demand, so hill hold must engage, directly select a capable lower gear, and then release into
/// uphill travel without more than the established 5 cm DERIVED static-compliance gate bound.
#[test]
fn hill_hold_20_deg_engages_and_pulls_away_tiger() {
    use crate::track::transmission::{SchedulerState, TransmissionMode};
    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::Position>().unwrap().0 = Vec3::new(0.0, 2.6, -40.0);
        e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 =
            Quat::from_rotation_x(20.0_f32.to_radians());
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
    }
    for _ in 0..256 {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    let z0 = app
        .world()
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has position")
        .0
        .z;
    let mut transmission = fresh_tank_transmission(&app);
    transmission.0.gear = 5;
    {
        let mut e = app.world_mut().entity_mut(tank);
        *e.get_mut::<crate::track::sim::TankTransmission>().unwrap() = transmission;
        let mut drive = e.get_mut::<crate::track::sim::TrackDrive>().unwrap();
        drive.throttle = 1.0;
        drive.sides[0].speed = 0.0;
        drive.sides[1].speed = 0.0;
    }

    let mut saw_hold = false;
    let mut release_tick = None;
    let mut min_z = z0;
    let mut max_rollback = 0.0f32;
    let mut launch_tick = None;
    for tick in 0..(12 * FIXED_TICKS_PER_SECOND) {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let world = app.world();
        let state = world
            .get::<crate::track::sim::TankTransmission>(tank)
            .expect("tank has transmission state")
            .0;
        if state.hill_hold {
            saw_hold = true;
            assert!(
                matches!(state.scheduler, SchedulerState::HillHold),
                "a capable grade uses HILL HOLD, not GRADE LIMIT"
            );
        } else if saw_hold && release_tick.is_none() {
            release_tick = Some(tick);
        }
        let z = world
            .get::<avian3d::prelude::Position>(tank)
            .expect("tank has position")
            .0
            .z;
        min_z = min_z.min(z);
        max_rollback = max_rollback.max(z - min_z);
        assert!(
            max_rollback <= 0.05,
            "20-degree hill hold exceeded static compliance ({max_rollback:.4} m)"
        );
        if z <= z0 - 0.5 {
            launch_tick = Some(tick + 1);
            break;
        }
    }
    println!(
        "tiger 20-deg hill hold: engaged {saw_hold}, release {:.3} s, pulled 0.5 m in {:.3} s, \
         rollback {max_rollback:.4} m",
        elapsed_secs(release_tick.expect("capable launch gear must release the hold")),
        elapsed_secs(launch_tick.expect("capable launch gear must pull uphill")),
    );
    assert!(saw_hold, "F5 at rest on 20 degrees must engage hill hold");
}

/// D4 honest 30-degree capability gate using the REAL Tiger blueprint values. The prior fixture
/// manufactured `GRADE LIMIT` with DERIVED test overrides of 100 N m engine/clutch torque and
/// 160 kN/side brake force. The shipped Tiger's MEASURED blueprint values instead author a
/// 250 kN/side force cap and 96 kN/side brake: its F1 launch capability exceeds the DERIVED
/// 30-degree demand plus scheduler margin, so truthful
/// selection must NOT report `GRADE LIMIT`; held W climbs. This gate prints and pins those numbers
/// so a future fixture cannot mask another engage/release capability mismatch.
#[test]
fn real_tiger_30_deg_reports_capability_truthfully() {
    use crate::track::transmission::{SchedulerState, TransmissionMode};
    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    let mass = app.world().resource::<TankBlueprint>().spec.mass;
    let demand = mass * 9.81 * 30.0_f32.to_radians().sin();
    // Fixture-validity precondition, deliberately restated with LOCAL literals (10% +
    // 10 kN) rather than the crate's reserve-margin constants: if the shipped policy
    // values drift, this precondition diverges from the scheduler and fails loudly here.
    // Residual implementation-as-oracle (documented): `torque_at` and the ratio/radius
    // reads below consume AUTHORED VEHICLE DATA through the params accessor, not
    // scheduler logic — an independent restatement of the torque interpolation would
    // re-implement the curve, not strengthen the test.
    let scheduler_margin = demand * 0.10 + 10_000.0;
    let max_launch_force = {
        let gear = app.world().resource::<crate::track::sim::TrackGear>();
        let tp = gear.trans().expect("the Tiger declares a transmission");
        let force_cap = 2.0
            * app
                .world()
                .resource::<TankBlueprint>()
                .spec
                .track
                .powertrain
                .force;
        tp.gears_fwd
            .iter()
            .map(|&ratio| (tp.torque_at(0.0) * ratio / tp.sprocket_radius).min(force_cap))
            .fold(0.0f32, f32::max)
    };
    assert!(
        max_launch_force >= demand + scheduler_margin,
        "real Tiger fixture must be capable on 30 degrees: force {max_launch_force:.0}, demand \
         {demand:.0}, margin {scheduler_margin:.0}"
    );
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::Position>().unwrap().0 = Vec3::new(14.0, 3.4, -40.0);
        e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 =
            Quat::from_rotation_x(30.0_f32.to_radians());
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
    }
    for _ in 0..256 {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    let p0 = app
        .world()
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has position")
        .0;
    let fresh_transmission = fresh_tank_transmission(&app);
    {
        let mut e = app.world_mut().entity_mut(tank);
        *e.get_mut::<crate::track::sim::TankTransmission>().unwrap() = fresh_transmission;
        let mut drive = e.get_mut::<crate::track::sim::TrackDrive>().unwrap();
        drive.throttle = 1.0;
        drive.sides[0].speed = 0.0;
        drive.sides[1].speed = 0.0;
    }

    let mut grade_limit_ticks = 0usize;
    for _ in 0..(6 * FIXED_TICKS_PER_SECOND) {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let state = app
            .world()
            .get::<crate::track::sim::TankTransmission>(tank)
            .expect("tank has transmission state")
            .0;
        if state.scheduler == SchedulerState::GradeLimit {
            grade_limit_ticks += 1;
            assert!(
                state.hill_hold,
                "GRADE LIMIT must retain the modeled brake hold"
            );
        }
    }
    let world = app.world();
    let p1 = world
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has position")
        .0;
    let drive = world
        .get::<crate::track::sim::TrackDrive>(tank)
        .expect("tank drives");
    let belt_m = (drive.sides[0].speed + drive.sides[1].speed) / 2.0;
    let uphill_progress = (p1 - p0).dot(Quat::from_rotation_x(30.0_f32.to_radians()) * Vec3::NEG_Z);
    println!(
        "30-deg real Tiger: modeled max launch {max_launch_force:.0} N, demand {demand:.0} N, \
         margin {scheduler_margin:.0} N; GRADE LIMIT {grade_limit_ticks}/384 ticks, uphill \
         {uphill_progress:.4} m, belt_m {belt_m:.4} m/s"
    );
    assert_eq!(
        grade_limit_ticks, 0,
        "a capable real Tiger must never expose GRADE LIMIT on 30 degrees"
    );
    assert!(
        uphill_progress > 0.5,
        "the capable real Tiger must pull uphill (progress {uphill_progress:.4} m)"
    );
    assert!(
        belt_m > 0.0,
        "the capable real Tiger must drive its belts uphill (m = {belt_m})"
    );
}

/// Regression: the REAL Tiger starts on the course's 30-degree face in F8, already rolling
/// backward faster than the 0.25 m/s DERIVED hill-hold threshold with W held. Its shipped
/// 96 kN/side brakes cannot arrest the 279.6 kN DERIVED grade demand by themselves. At the
/// DERIVED 317.5 kN selection threshold, only F1-F2 are capable launch gears: F1 is capped at
/// 500 kN DERIVED, F2 makes 341.9 kN DERIVED, and F3's 237.1 kN DERIVED fails. The flow:
/// the moving rollback is first braked CONTINUOUSLY by the `back_driven_intent` service
/// envelope (the latch is near-rest-only — no grab at speed); once the hull decelerates
/// into the engagement zone the hold latches, the Direct preselector rescues through a paid
/// shift exposing HILL HOLD, arrests the hull, and resumes uphill travel (measured: latch
/// t114, capable F2 t142, arrest t134, +0.5 m t255).
#[test]
fn real_tiger_f8_30_deg_rollback_rescues_to_capable_gear() {
    use crate::track::transmission::{SchedulerState, TransmissionMode};
    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    let course_rotation = Quat::from_rotation_x(30.0_f32.to_radians());
    let uphill = course_rotation * Vec3::NEG_Z;
    let initial_rollback_speed = 0.5;
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::Position>().unwrap().0 = Vec3::new(14.0, 3.4, -40.0);
        e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 = course_rotation;
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
    }
    // Seat the real suspension/contact field on the face before injecting the rollback. Starting
    // the belt in mid-air would exercise an unloaded free-rev, not the 30-degree rescue.
    for _ in 0..64 {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    let grounded_sides = app
        .world()
        .get::<crate::track::sim::TrackContacts>(tank)
        .expect("tank has contact telemetry")
        .0
        .iter()
        .filter(|side| !side.is_empty())
        .count();
    assert_eq!(grounded_sides, 2, "rollback fixture must start grounded");
    let mut transmission = fresh_tank_transmission(&app);
    transmission.0.gear = 8;
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 =
            -uphill * initial_rollback_speed;
        *e.get_mut::<crate::track::sim::TankTransmission>().unwrap() = transmission;
        let mut drive = e.get_mut::<crate::track::sim::TrackDrive>().unwrap();
        drive.throttle = 1.0;
        drive.steer = 0.0;
        drive.sides[0].speed = -initial_rollback_speed;
        drive.sides[1].speed = -initial_rollback_speed;
    }

    let mut gear_path = vec![8u8];
    let mut state_trace = Vec::new();
    let mut previous_state = None;
    let mut reached_capable_tick = None;
    let mut arrest_tick = None;
    let mut arrest_position = None;
    let mut progress_tick = None;
    for tick in 0..(12 * FIXED_TICKS_PER_SECOND) {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let world = app.world();
        let state = world
            .get::<crate::track::sim::TankTransmission>(tank)
            .expect("tank has transmission state")
            .0;
        let marker = (state.gear, state.scheduler, state.hill_hold);
        if previous_state != Some(marker) {
            state_trace.push((tick + 1, marker));
            previous_state = Some(marker);
        }
        if gear_path.last() != Some(&state.gear) {
            gear_path.push(state.gear);
        }
        if state.hill_hold {
            assert_eq!(
                state.scheduler,
                SchedulerState::HillHold,
                "tick {tick}: a capable rollback rescue must expose HILL HOLD (trace \
                 {state_trace:?})"
            );
        }
        assert_ne!(
            state.scheduler,
            SchedulerState::GradeLimit,
            "tick {tick}: the real Tiger has a capable launch gear (trace {state_trace:?})"
        );
        if state.gear <= 2 {
            reached_capable_tick.get_or_insert(tick + 1);
        }

        let position = world
            .get::<avian3d::prelude::Position>(tank)
            .expect("tank has position")
            .0;
        let course_speed = world
            .get::<avian3d::prelude::LinearVelocity>(tank)
            .expect("tank has velocity")
            .0
            .dot(uphill);
        if course_speed >= 0.0 && arrest_tick.is_none() {
            arrest_tick = Some(tick + 1);
            arrest_position = Some(position);
        }
        if let Some(p_arrest) = arrest_position
            && (position - p_arrest).dot(uphill) >= 0.5
        {
            progress_tick = Some(tick + 1);
            break;
        }
    }

    let reached_capable_tick =
        reached_capable_tick.expect("F8 rollback rescue never reached a capable F1-F2 gear");
    let arrest_tick = arrest_tick.expect("capable launch gear never arrested the rollback");
    let progress_tick = progress_tick.expect("the rescued Tiger never made 0.5 m uphill progress");
    println!(
        "30-deg real Tiger F8 rollback rescue: capable tick {reached_capable_tick}, arrest tick \
         {arrest_tick}, +0.5 m tick {progress_tick}, gears {gear_path:?}, states {state_trace:?}"
    );
    assert!(
        reached_capable_tick <= 192,
        "the Direct preselector must not remain silently stuck in F8 — the rescue flow \
         brakes to the near-rest zone first (measured latch ≈ t114, capable ≈ t142), so \
         the bound covers braking + one paid rescue window (trace {state_trace:?})"
    );
    assert!(
        arrest_tick <= 4 * FIXED_TICKS_PER_SECOND,
        "the capable gear must arrest rollback within 4 s (trace {state_trace:?})"
    );
    assert!(
        progress_tick <= 12 * FIXED_TICKS_PER_SECOND,
        "the rescued Tiger must make uphill progress within 12 s (trace {state_trace:?})"
    );
}

/// Shared fixture for the two synthetic weak-powertrain 30-degree regressions: the REAL
/// Tiger geometry/contact model on the course's 30-degree face, seated and settled, then
/// the engine curve and clutch nerfed to a 100 N m DERIVED test value so every gear is
/// incapable and the DYNAMIC brakes alone are unarrestable (static breakaway still holds
/// at rest — the shipped 1.5x factor clears the 279.6 kN demand by the RON's authored
/// margin). Returns `(app, tank, demand, uphill)`.
fn synthetic_weak_30deg_fixture() -> (BootedSim, Entity, f32, Vec3) {
    use crate::track::transmission::TransmissionMode;
    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    let course_rotation = Quat::from_rotation_x(30.0_f32.to_radians());
    let uphill = course_rotation * Vec3::NEG_Z;
    let mass = app.world().resource::<TankBlueprint>().spec.mass;
    let demand = mass * 9.81 * 30.0_f32.to_radians().sin();
    let force_cap = 2.0
        * app
            .world()
            .resource::<TankBlueprint>()
            .spec
            .track
            .powertrain
            .force;
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::Position>().unwrap().0 = Vec3::new(14.0, 3.4, -40.0);
        e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 = course_rotation;
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
    }
    // Seat the suspension, ring down, and PARK — 256 ticks like the ramp probes' settle:
    // 64 was not enough on the 30-degree face, and an unparked fixture starts its W tick
    // already sliding, outside the near-rest zone the at-rest test is about. The parked
    // belt reactions also seed the demand EMA with the real static grade load.
    for _ in 0..256 {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    let parked = app
        .world()
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("tank has transmission state")
        .0
        .park;
    assert!(
        parked,
        "the fixture must reach a genuine park before the nerf"
    );
    let (max_launch_force, brake_capacity) = {
        let mut gear = app
            .world_mut()
            .resource_mut::<crate::track::sim::TrackGear>();
        let tp = gear.trans_mut().expect("the Tiger declares a transmission");
        for (_, torque) in &mut tp.engine.torque_nm {
            *torque = 100.0;
        }
        tp.peak_torque_nm = 100.0;
        tp.clutch_capacity = 100.0;
        let max_launch_force = tp
            .gears_fwd
            .iter()
            .map(|&ratio| (tp.torque_at(0.0) * ratio / tp.sprocket_radius).min(force_cap))
            .fold(0.0f32, f32::max);
        (max_launch_force, tp.brake_capacity_n)
    };
    assert!(
        max_launch_force < demand,
        "synthetic fixture must leave every gear incapable: max {max_launch_force:.0} N, demand \
         {demand:.0} N"
    );
    assert!(
        2.0 * brake_capacity < demand,
        "synthetic fixture must be unarrestable on DYNAMIC brakes alone: brakes {:.0} N, demand \
         {demand:.0} N",
        2.0 * brake_capacity
    );
    (app, tank, demand, uphill)
}

/// Restored 30-degree protection (a): GRADE LIMIT ENTRY observed on the
/// live sim, not seeded by hand. At rest on the face with the demand EMA already carrying
/// the parked grade load, held W must latch the hill hold near rest, and the launch
/// selector — finding NO capable gear in the nerfed powertrain — must expose GRADE LIMIT
/// while the full brake envelope (static breakaway at rest) holds the hull to
/// centimeter-class drift.
#[test]
fn synthetic_weak_powertrain_30_deg_at_rest_enters_grade_limit() {
    use crate::track::transmission::SchedulerState;
    let (mut app, tank, _demand, _uphill) = synthetic_weak_30deg_fixture();
    let start = app
        .world()
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has position")
        .0;
    let mut entered_at = None;
    for tick in 0..96 {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let state = app
            .world()
            .get::<crate::track::sim::TankTransmission>(tank)
            .expect("tank has transmission state")
            .0;

        if entered_at.is_none() && state.hill_hold && state.scheduler == SchedulerState::GradeLimit
        {
            entered_at = Some(tick);
        }
        if let Some(entry) = entered_at {
            assert!(
                state.hill_hold && state.scheduler == SchedulerState::GradeLimit,
                "tick {tick}: GRADE LIMIT must persist once entered (entry {entry})"
            );
        }
    }
    let entered_at =
        entered_at.expect("held W at rest on an impossible grade must ENTER GRADE LIMIT");
    assert!(
        entered_at <= 16,
        "entry must be prompt from the settled at-rest state (took {entered_at} ticks)"
    );
    let end = app
        .world()
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has position")
        .0;
    let drift = (end - start).length();
    assert!(
        drift < 0.05,
        "the latched full envelope (static breakaway at rest) must hold the hull \
         (drifted {drift:.3} m)"
    );
}

/// Restored 30-degree protection (b): a MOVING weak-powertrain
/// rollback with W held keeps sliding (dynamic brakes unarrestable by construction), the
/// near-rest-only latch never grabs it, its status stays Normal — AND the
/// `back_driven_intent` service braking is PROVEN by a deceleration bound: over the
/// 1-second window the slide may gain at most 2.5 m/s where a free roll on 30 degrees
/// would gain g.sin(30) ~ 4.9 m/s. If the cross-motion braking seam vanishes, this bound
/// fails.
#[test]
fn synthetic_weak_powertrain_30_deg_moving_slide_brakes_without_latch() {
    let (mut app, tank, demand, uphill) = synthetic_weak_30deg_fixture();
    // Genuinely MOVING: from the parked fixture, a 0.5 m/s slide turned out to be
    // arrestable by kinetic grip + the service envelope within the window (measured
    // −0.006 m/s at the end — a knife-edge fixture); 2.0 m/s keeps the slide alive
    // through the whole window while the braking bound still bites.
    let initial_rollback_speed = 2.0;
    let mut transmission = fresh_tank_transmission(&app);
    transmission.0.gear = 8;
    let start_position = {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 =
            -uphill * initial_rollback_speed;
        *e.get_mut::<crate::track::sim::TankTransmission>().unwrap() = transmission;
        let mut drive = e.get_mut::<crate::track::sim::TrackDrive>().unwrap();
        drive.throttle = 1.0;
        drive.steer = 0.0;
        drive.sides[0].speed = -initial_rollback_speed;
        drive.sides[1].speed = -initial_rollback_speed;
        e.get::<avian3d::prelude::Position>().unwrap().0
    };

    let mut final_course_speed = -initial_rollback_speed;
    for tick in 0..64 {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let world = app.world();
        let state = world
            .get::<crate::track::sim::TankTransmission>(tank)
            .expect("tank has transmission state")
            .0;
        let drive = world
            .get::<crate::track::sim::TrackDrive>(tank)
            .expect("tank drives");
        let belt_m = (drive.sides[0].speed + drive.sides[1].speed) / 2.0;
        // The latch domain is the SHAFT (belt), not the hull: the service envelope can
        // legitimately arrest the BELTS while the hull skids on kinetic grip, and a
        // belt inside the near-rest zone latching (then reporting GRADE LIMIT — locked
        // belts, still sliding, no capable gear) is designed truth-telling. What must
        // NEVER happen is a latch while the belt itself is moving.
        if belt_m.abs() >= 0.25 {
            assert!(
                !state.hill_hold,
                "tick {tick}: the near-rest-only latch must never grab a MOVING shaft \
                 (belt {belt_m:.2} m/s — cross/rolling motion belongs to \
                 back_driven_intent braking)"
            );
        }
        assert!(
            !state.park,
            "tick {tick}: no parking latch may appear under held W either"
        );
        final_course_speed = world
            .get::<avian3d::prelude::LinearVelocity>(tank)
            .expect("tank has velocity")
            .0
            .dot(uphill);
        assert!(
            final_course_speed < 0.0,
            "tick {tick}: deliberately insufficient brakes and power must not arrest the rollback"
        );
    }
    // The braking bound: back_driven_intent's service envelope plus belt-locked kinetic
    // grip must cap the 1-second speed change far under a free roll's +4.9 m/s gain
    // (measured: the braked slide DEcelerates). A vanished cross-motion braking seam
    // fails here.
    assert!(
        final_course_speed > -(initial_rollback_speed + 2.5),
        "the slide must be continuously service-braked: reached {final_course_speed:.2} m/s \
         where a free roll would pass {:.2}",
        -(initial_rollback_speed + 4.9)
    );
    let end_position = app
        .world()
        .get::<avian3d::prelude::Position>(tank)
        .expect("tank has position")
        .0;
    let downhill_distance = -(end_position - start_position).dot(uphill);
    println!(
        "synthetic 30-deg weak-powertrain slide: max launch nerfed, demand {demand:.0} N; \
         unlatched slide 64/64 ticks, final speed {final_course_speed:.3} m/s, downhill \
         {downhill_distance:.3} m"
    );
    assert!(
        downhill_distance > 0.25,
        "the incapable synthetic fixture must continue downhill (moved {downhill_distance:.3} m)"
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
    let total = 30 * FIXED_TICKS_PER_SECOND;
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
    let state = world
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("the controlled tank carries transmission state");
    let tp = world
        .resource::<crate::track::sim::TrackGear>()
        .trans()
        .expect("the Tiger blueprint declares a transmission");
    // Stage B: the readout is the crank state directly — no belt speeds involved.
    let readout = transmission::readout(&state.0, tp);
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
            &crate::track::sim::TrackGripElements,
            &crate::track::sim::TankTransmission,
            (&crate::tank::WeaponGate, &crate::ballistics::HullShock),
            &crate::track::sim::TrackContacts,
            (&crate::tank::TankServos, &crate::tank::TankSim),
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
        elements,
        transmission,
        (weapon_gate, shock),
        contacts,
        (servos, sim),
    ) in &roots
    {
        digests.push((
            name.as_str().to_owned(),
            crate::trace::canonical_tank_state_digest(
                position.0,
                rotation.0,
                linear.0,
                angular.0,
                drive,
                grip,
                elements,
                transmission,
                weapon_gate,
                Some(shock),
                servos,
                sim,
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
        tick >= 300 && drive.steer.abs() > f32::EPSILON && loaded_contact_is_slipping;
    run.saw_shot |= sim.weapons.iter().any(|weapon| weapon.rounds_fired > 0);
    run.saw_projectile_spawn |= !projectiles.is_empty();
    run.saw_projectile_march |= projectiles.iter().any(|path| path.points.len() > 1);
    if matches!(tick, 119 | 299 | 479) {
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
        "shooting::tick_weapon_gate",
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
                    || name.contains("shooting::tick_weapon_gate")
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
            // The straight run-up is long because the upshift confirmation
            // dwell (UPSHIFT_CONFIRM_TICKS per shift) slows the flat gear walk, and the
            // steering-slip witness needs the hull fast enough that the tight detent
            // scrubs its loaded contacts past the slip band.
            command.throttle = if (120..480).contains(&tick) { 1.0 } else { 0.0 };
            command.steer = if (300..480).contains(&tick) { 0.7 } else { 0.0 };
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
        [119, 299, 479],
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

// ---------------------------------------------------------------------------------------------
// Dev-sandbox boot guards
// ---------------------------------------------------------------------------------------------
//
// WHY THIS SECTION EXISTS. The two sandboxes are composition roots of their own — each mounts
// `DefaultPlugins` in its `bin/` shell and then assembles a DIFFERENT subset of the game's plugins
// than `ClientPlugin` does. Nothing else in the suite ever composed one, and CI cannot: the bins
// carry `required-features = ["dev_ui"]`, so `cargo clippy --all-targets` (no `dev_ui`) SKIPS them
// entirely and even the compile is unproven there. That left the sandboxes' wiring checked by
// exactly one thing — a human running `cargo armor` / `cargo track` — and it rotted:
// `sandbox::plugin` mounted `crew_ui`, whose status panel takes `Res<WeaponClock>`, while the
// resource is inserted by `shooting::plugin` (and by the net composition), which the sandbox
// deliberately does not mount. Every `armor_sandbox` boot died on its first `Update` with
// "Parameter `Res<WeaponClock>` failed validation: Resource does not exist".
//
// WHAT THESE GUARD, therefore, is the SHAPE and not that one resource: booting a root and running
// frames validates the parameters of every system the root schedules, so ANY resource a sandbox
// forgets to provide for a plugin it borrowed from the game fails here — as does any observer or
// startup system that faults on the sandbox's world. Frames are run past the deferred build both
// roots do (`bake` inserts `TankBlueprint` with `Commands` at `Startup`, so the tank/rig only
// materialize from the next schedule on), because the systems gated on that build are exactly the
// ones a "did it survive one update" check would never reach.
//
// A THIRD windowed root belongs here the day it is added.

/// Frames to run past `Startup`. Both roots need two updates to get their subject up (the `Startup`
/// command flush, then the `Update` that consumes `TankBlueprint`); the rest are slack, so systems
/// gated on the built rig/tank run for several ticks rather than once.
const SANDBOX_BOOT_FRAMES: usize = 8;

/// Boot one sandbox root headless and run [`SANDBOX_BOOT_FRAMES`] frames of it.
///
/// The clock is the fixed-tick one (one exact fixed loop per update), so `FixedUpdate` — physics,
/// the belt field, the weapon-shaped systems — is exercised too, not just `Update`.
///
/// Serialized against every other full-app fixture by [`BOOT_LEASE`]: these boots parse the whole
/// Tiger glb (`bake`) and run Avian, and the suite's rule is that full apps take turns.
fn boot_sandbox_headless(root: fn(&mut App)) -> (App, MutexGuard<'static, ()>) {
    let lease = BOOT_LEASE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut app = headless_shell();
    app.add_plugins(root);
    finish_plugins(&mut app);
    start_fixed_clock(&mut app);
    for _ in 0..SANDBOX_BOOT_FRAMES {
        app.update();
    }
    (app, lease)
}

/// Every string rendered by a `Text` node this frame — the sandboxes' HUD surface, read without
/// reaching into another module's private marker components.
fn rendered_text(app: &mut App) -> Vec<String> {
    let mut texts = app.world_mut().query::<&Text>();
    texts.iter(app.world()).map(|text| text.0.clone()).collect()
}

/// The armour/penetration sandbox boots and keeps running.
///
/// The regression this pins is the `Res<WeaponClock>` panic described above, but the assertions
/// deliberately go one step further than "it didn't panic": the target tank must actually be up and
/// the shared status panel must have RENDERED it. The panicking system is `update_status_panel`, so
/// a guard that only proved the app survived could be satisfied by a panel that silently drew
/// nothing — which is the state the "just make the resource optional" fix would have produced.
#[test]
fn armor_sandbox_boots_headless() {
    let (mut app, _lease) = boot_sandbox_headless(crate::sandbox::plugin);

    let mut targets = app
        .world_mut()
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>();
    assert_eq!(
        targets.iter(app.world()).count(),
        1,
        "the armor sandbox must spawn exactly one Controlled target tank from the blueprint \
         within {SANDBOX_BOOT_FRAMES} frames — the crew bar and the status panel are both scoped \
         to `Controlled`, so without it they render nothing and this guard proves nothing",
    );

    let texts = rendered_text(&mut app);
    assert!(
        texts.iter().any(|text| text.contains("Weapons:")),
        "the shared status panel (crew_ui::update_status_panel) rendered no weapons row in the \
         armor sandbox; text nodes were {texts:?}",
    );

    // The frozen weapon clock the sandbox root inserts BECAUSE it does not simulate the gun: no
    // system here advances it, and none arms a `WeaponGate` either, so the readouts derived
    // against it stay true of a tank that has never fired. A clock that has moved means something
    // started ticking it — at which point this sandbox is claiming to simulate a gun it does not
    // have, and the comment on that `insert_resource` is a lie.
    assert_eq!(
        app.world().resource::<crate::WeaponClock>().0,
        0,
        "the armor sandbox's weapon clock advanced — it mounts no gun simulation, so a moving \
         clock means the panel's reload readouts are now measured against a tick nothing else here \
         respects",
    );
}

/// The track-model sandbox boots and keeps running, through the deferred rig build.
///
/// `RigGeom` is the root's own "the rig is up" gate — nearly every system it schedules is
/// `run_if(resource_exists::<RigGeom>)`, so a boot that never reaches it would exercise almost
/// nothing and this guard would go quietly blind.
#[test]
fn track_sandbox_boots_headless() {
    let (app, _lease) = boot_sandbox_headless(crate::track_sandbox::plugin);

    assert!(
        app.world()
            .get_resource::<crate::track::rig_geom::RigGeom>()
            .is_some(),
        "the track sandbox never built its rig within {SANDBOX_BOOT_FRAMES} frames (build_rig \
         waits on bake's TankBlueprint) — everything downstream is gated on RigGeom, so nothing \
         past the plugin build was actually exercised",
    );
}

/// THE SHIPPED MAP, END TO END: a main-gun round fired between two tanks standing on the real
/// kalinovo terrain must reach the target's armour and take HP off it.
///
/// Every other ballistics gate in the tree runs on a FIXTURE world — a synthetic plate, a flat
/// slab, an analytic ramp. None of those carries the one thing the shipped map has: a heightfield
/// terrain collider whose AABB is the entire world box, and which therefore sits in the broad-phase
/// candidate set of every corridor a shell ever flies. That difference alone silently disarmed the
/// main gun on the real map while the whole fixture suite stayed green (`cast_disc_segment`'s
/// broad-phase gate was ASSIGNED per candidate instead of accumulated, and avian walks its collider
/// trees with `for_each` — so the terrain tree, visited after the armour tree, overwrote every
/// armour candidate back to "no armour near this corridor"). Two tanks on the map, unable to
/// scratch each other, for a whole session.
///
/// So this test is deliberately the SLOW, WHOLE one: the shipped heightmap, the real Tiger
/// geometry, the live march, the real damage deposit. The claim it makes cannot be made anywhere
/// cheaper, because the thing that broke was exactly the gap between a fixture world and the
/// shipped one.
#[test]
fn a_main_gun_round_damages_a_tank_on_the_shipped_map() {
    use crate::ballistics::{ComponentHealth, FireShell, FireShellOrigin, Impact, ShotSource};
    use crate::damage::VolumeOf;
    use avian3d::prelude::{Collider, Position, Rotation, SimpleCollider};

    /// Every armour impact the round produced.
    #[derive(Resource, Default)]
    struct ArmorImpacts(Vec<bool>);

    let mut app = {
        let grid = crate::terrain_grid::tests::shipped_grid();
        let mut sim = booted_sim_on(Some(grid), BallisticsHalves::SimOnly);
        start_fixed_clock(&mut sim);
        // Let both tanks settle onto the terrain before anything is fired at them.
        for _ in 0..30 {
            sim.update();
        }
        sim
    };
    app.init_resource::<ArmorImpacts>();
    app.add_observer(|impact: On<Impact>, mut log: ResMut<ArmorImpacts>| {
        if impact.surface == crate::ballistics::ImpactSurface::Armor {
            log.0.push(impact.penetrated);
        }
    });

    let world = app.world_mut();
    let tanks: Vec<(Entity, Vec3)> = world
        .query_filtered::<(Entity, &Position), With<Tank>>()
        .iter(world)
        .map(|(tank, pos)| (tank, pos.0))
        .collect();
    assert_eq!(tanks.len(), 2, "the duel scenario spawns two tanks");
    let (shooter, shooter_pos) = tanks[0];
    let (target, _) = tanks[1];

    // Aim at the target's OWN armour, read off its bound colliders rather than guessed from the
    // hull origin: a hand-picked offset drifts the moment the rig or the spawn pose changes, and a
    // shot that sails over the turret roof would pass this test for the wrong reason.
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    let world = app.world_mut();
    for (owner, pos, rot, collider) in world
        .query::<(&VolumeOf, &Position, &Rotation, &Collider)>()
        .iter(world)
    {
        if owner.tank() == target {
            let aabb = collider.aabb(pos.0, rot.0);
            lo = lo.min(aabb.min);
            hi = hi.max(aabb.max);
        }
    }
    assert!(
        lo.x.is_finite(),
        "the target tank bound no armour colliders"
    );
    let aim = (lo + hi) * 0.5;

    let total_hp = |app: &mut App| -> f32 {
        let world = app.world_mut();
        world
            .query::<&ComponentHealth>()
            .iter(world)
            .map(|health| health.current)
            .sum()
    };
    let before = total_hp(&mut app);
    assert!(before > 0.0, "the target carries no HP pools to lower");

    // Above the shooter's own hull, so the round leaves its own geometry the way a bore round does.
    let origin = shooter_pos + Vec3::Y * 1.2;
    let direction = Dir3::new(aim - origin).expect("the two tanks are not co-located");
    // The terrain between them is not in the way — asserted, so a later spawn move that DOES put a
    // ridge on the sight line fails here saying so, instead of failing the damage claim below.
    {
        let grid = app.world().resource::<crate::terrain_grid::HeightGrid>();
        assert!(
            grid.cast_ray(origin, Vec3::from(direction), (aim - origin).length())
                .is_none(),
            "the shipped terrain crosses the sight line between the two duel spawns — this test \
             can no longer say anything about armour",
        );
    }

    app.world_mut().trigger(FireShell {
        origin,
        direction,
        speed: 755.0,
        caliber: 0.088,
        mass: 10.2,
        mechanism: crate::spec::FireMechanism::Single,
        shooter: Some(ShotSource {
            tank: shooter,
            weapon: 0,
        }),
        tracer: false,
        shot_origin: FireShellOrigin::Local,
        catch_up_ticks: 0,
        shot: None,
    });
    // Generous: the round covers the ~17 m gap inside one 64 Hz tick, and the interior march and
    // spall resolve within a few more.
    for _ in 0..20 {
        app.update();
    }

    let impacts = std::mem::take(&mut app.world_mut().resource_mut::<ArmorImpacts>().0);
    assert!(
        !impacts.is_empty(),
        "the round produced NO armour impact on the shipped map — it flew through the target and \
         went on to the ground, exactly as it did in the live session. The corridor's broad-phase \
         armour gate is being masked by the world-spanning terrain collider again \
         (`ballistics::cast_disc_segment`).",
    );
    assert!(
        impacts.iter().any(|penetrated| *penetrated),
        "the round reached the target's armour but never penetrated it — {} impact(s), all \
         defeated. A point-blank 88 mm round into a Tiger's side must get through.",
        impacts.len(),
    );
    let after = total_hp(&mut app);
    assert!(
        after < before,
        "armour was penetrated but no HP pool moved: {before} before, {after} after. The march \
         resolved geometry without depositing damage.",
    );
}
