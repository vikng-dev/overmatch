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
fn headless_app_on(world: Option<crate::terrain_grid::HeightGrid>) -> App {
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

    finish_plugins(&mut app);
    app
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
    // Mechanism + suggested upstream fix: `.agents/docs/upstream/bevy-ktx2-uastc-fallback-length-panic.md`.
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
    booted_sim_on(None)
}

/// [`booted_sim`] over an explicitly chosen world — see [`headless_app_on`].
fn booted_sim_on(world: Option<crate::terrain_grid::HeightGrid>) -> BootedSim {
    let lease = BOOT_LEASE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut app = headless_app_on(world);

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

/// The established 20° park regression stays pinned while static breakaway and dynamic
/// dissipation become separate capacities. Teleport onto the 20° ramp mid-face (test course §1:
/// x = 0, z = −40, pitched about X), release all inputs, settle; the park latch must engage and
/// the hull must not back-drive over a sustained window.
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
    for _ in 0..(4 * FIXED_TICKS_PER_SECOND) {
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
    const MEASURED_DRIFT_M: f32 = 0.0928;
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
            // Same DERIVED 0.05 m compliance budget as `slope_park_holds_20_deg_tiger`: the
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

// --- Driving-feel probes ------------------------------------------------------------------------
//
// Reproducible, headless, deterministic driving experiments with per-tick telemetry: the three
// reported feel symptoms (uphill upshift refusal, downhill overrun, slow turning) as scenarios that
// write a CSV instead of asserting a threshold. They are `#[ignore]`d — they are INSTRUMENTS, not
// gates; a gate that fires on a feel number would freeze the very law under investigation.
//
//   cargo test --lib -- --ignored --nocapture probe_climb_10pct
//   cargo test --lib -- --ignored --nocapture probe_fire_backward_uphill
//   cargo test --lib -- --ignored --nocapture probe_descend_10pct
//   cargo test --lib -- --ignored --nocapture probe_turn_at_2ms
//   cargo test --lib -- --ignored --nocapture probe_turn_radius_sweep
//
// Telemetry lands in `$SPIKE_DRIVE_PROBE_DIR` (default `target/drive-probe`), one CSV per scenario.
// `SPIKE_DRIVE_PROBE_GRADE` overrides the ramp grade (rise/run, e.g. `0.15`) for a sweep, and
// `SPIKE_DRIVE_PROBE_TURN_THROTTLE` the throttle held through the turn (default full).
// The fire scenario takes `SPIKE_DRIVE_PROBE_FIRE_YAW_DEG` (turret lay, default 180 = over the
// rear deck), `SPIKE_DRIVE_PROBE_FIRE_SHOTS` (default 2; 0 runs the identical command stream
// WITHOUT pulling the trigger — the control), `SPIKE_DRIVE_PROBE_FIRE_SETTLE_S`,
// `SPIKE_DRIVE_PROBE_FIRE_GAP_S` and `SPIKE_DRIVE_PROBE_FIRE_RUN_S`.
// The turn scenarios take `SPIKE_DRIVE_PROBE_STEER` (steer magnitude, which SELECTS the L600
// detent: ≥ 0.15 wide, ≥ 0.55 tight); the sweep additionally takes `SPIKE_DRIVE_PROBE_GEARS` and
// `SPIKE_DRIVE_PROBE_STEERS` (comma lists) plus the tick budgets `SPIKE_DRIVE_PROBE_RUNUP_S`,
// `SPIKE_DRIVE_PROBE_TURN_S` and `SPIKE_DRIVE_PROBE_SETTLE_S`.
//
// Everything here is a READ-ONLY tap: the scenarios write only `TankCommand` and the initial pose
// (the same two seams the existing transmission gates use), and every derived number is recomputed
// through the production laws (`transmission::modeled_reserve_in_gear`,
// `transmission::predict_shift_landing_m`) rather than a local restatement of them.

/// Rise per unit run of the analytic ramp world, unless `SPIKE_DRIVE_PROBE_GRADE` overrides it.
fn probe_grade(default: f32) -> f32 {
    crate::env_parse("SPIKE_DRIVE_PROBE_GRADE").unwrap_or(default)
}

/// Steer magnitude held through a turn probe, unless `SPIKE_DRIVE_PROBE_STEER` overrides it.
/// The magnitude SELECTS the L600 detent (`|steer| ≥ 0.15` wide, `≥ 0.55` tight, with release
/// hysteresis); INSIDE a detent it does not scale the command at all — the geared constraint is
/// `d = sign(steer)·κ(gear, step)·|m|`, so 0.3 and 0.5 command the same wide radius.
fn probe_steer(default: f32) -> f32 {
    crate::env_parse("SPIKE_DRIVE_PROBE_STEER").unwrap_or(default)
}

/// A comma-separated numeric env list (`"1,2,4"`), falling back to `default` when unset or empty.
fn probe_list<T: std::str::FromStr + Copy>(name: &str, default: &[T]) -> Vec<T> {
    match crate::env_value(name) {
        Some(raw) if !raw.trim().is_empty() => raw
            .split(',')
            .filter(|entry| !entry.trim().is_empty())
            .map(|entry| {
                entry
                    .trim()
                    .parse::<T>()
                    .unwrap_or_else(|_| panic!("{name}: {entry:?} is not a number"))
            })
            .collect(),
        _ => default.to_vec(),
    }
}

/// The analytic constant-grade world: a plane inclined along +X through `y = 0` at `x = 0`, so the
/// origin sits where the flat slab put it and the spawn drop is unchanged. TWO samples per side is
/// EXACT for a plane — the grid interpolates linearly inside a cell — so the oracle, the collider
/// and the render mesh all read the authored grade everywhere, with no quantization to chase.
fn ramp_grid(grade: f32) -> crate::terrain_grid::HeightGrid {
    let half = crate::terrain_grid::WORLD_HALF_EXTENT;
    let (lo, hi) = (-grade * half, grade * half);
    crate::terrain_grid::HeightGrid::new(std::sync::Arc::from([lo, hi, lo, hi].as_slice()), 2)
}

/// Boot on [`ramp_grid`], settle onto the belt contacts, and hand back the controlled Tiger.
/// No `TransmissionFeelTest` is inserted: these probes run the SPEC's declared architecture, which
/// is what the reported symptoms were felt through.
fn booted_ramp_sim(grade: f32) -> (BootedSim, Entity, crate::terrain_grid::HeightGrid) {
    let grid = ramp_grid(grade);
    let mut app = booted_sim_on(Some(grid.clone()));
    start_fixed_clock(&mut app);
    // Ground the CONTROLLED tank only, with a 900-tick budget (the flat boots keep their
    // 300-tick all-tanks criterion): the landed spawn rule resolves height as the
    // footprint-square MAX plus clearance, so on a ramp both duel tanks drop farther and
    // bounce, and on steep grades the un-driven WINGMAN can toboggan far downhill before
    // (ever) parking — every probe re-poses and drives the controlled tank away from
    // spawn, so the wingman's settling is irrelevant to the traces.
    let mut grounded = 0;
    for _ in 0..900 {
        app.update();
        let world = app.world_mut();
        grounded = world
            .query_filtered::<&crate::track::sim::TrackContacts, With<Controlled>>()
            .iter(world)
            .map(|c| c.0.iter().filter(|side| !side.is_empty()).count())
            .sum();
        if grounded >= 2 {
            break;
        }
    }
    if grounded < 2 {
        let world = app.world_mut();
        let poses: Vec<(Vec3, usize)> = world
            .query::<(
                &avian3d::prelude::Position,
                &crate::track::sim::TrackContacts,
            )>()
            .iter(world)
            .map(|(p, c)| (p.0, c.0.iter().filter(|side| !side.is_empty()).count()))
            .collect();
        panic!(
            "the controlled tank never grounded on the ramp (its sides {grounded}, tanks \
             {poses:?}, surface under first {:?})",
            poses.first().map(|(p, _)| grid.height_at(p.x, p.z))
        );
    }
    let mut tank_q = app
        .world_mut()
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>();
    let tank = tank_q.single(app.world()).expect("one controlled tank");
    (app, tank, grid)
}

/// Yaw that points the hull's forward axis (local −Z) along `heading` in the XZ plane.
fn heading_yaw(heading: Vec3) -> Quat {
    Quat::from_rotation_arc(
        Vec3::NEG_Z,
        Vec3::new(heading.x, 0.0, heading.z).normalize(),
    )
}

/// Teleport onto the ramp already tilted into the surface, then settle under zero input. The
/// slope tilt is applied OUTSIDE the yaw (`tilt * yaw`) so the hull lies in the plane whichever
/// way it faces — a yaw-then-pitch composition would bank a cross-slope heading instead.
fn place_on_ramp(
    app: &mut App,
    tank: Entity,
    grid: &crate::terrain_grid::HeightGrid,
    grade: f32,
    x: f32,
    z: f32,
    heading: Vec3,
) {
    let normal = Vec3::new(-grade, 1.0, 0.0).normalize();
    let rotation = Quat::from_rotation_arc(Vec3::Y, normal) * heading_yaw(heading);
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::Position>().unwrap().0 =
            Vec3::new(x, grid.height_at(x, z) + 2.0, z);
        e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 = rotation;
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
    }
    // Drop, ring-down, and park latch — the same settle window the slope-park gates use.
    for _ in 0..256 {
        drive_tick(app, tank, 0.0, 0.0);
    }
}

/// One tick of telemetry, in the columns [`PROBE_HEADER`] names.
#[derive(Clone, Copy, Debug, Default)]
struct ProbeSample {
    speed: f32,
    /// Signed hull speed along the hull's forward axis (the CSV's `fwd_speed`) — the
    /// through-zero probes key on the HULL: the belts legitimately chatter tick-to-tick
    /// under saturated braking (stop-force vs grip shear), the hull does not.
    fwd_speed: f32,
    yaw_rate: f32,
    yaw_kinematic: f32,
    gear: u8,
    /// Which ladder is engaged (true = reverse) — the descent probes track the swap seam.
    reverse: bool,
    /// Parking-brake latch state — the park probe asserts engage/release edges.
    park: bool,
    /// The engaged L600 detent step: 0 straight, 1 wide, 2 tight.
    steer_step: u8,
    rpm: f32,
    demand_n: f32,
    reserve: f32,
    reserve_next: f32,
    belts: [f32; 2],
    /// Per-side longitudinal ground force sum (N), signed POSITIVE when the belt drives the
    /// hull forward and negative when it brakes — the same `belt_reaction` the transmission
    /// was handed this tick.
    belt_reaction: [f32; 2],
    side_commands: [f32; 2],
    position: Vec3,
    /// Summed normal load over every loaded contact, both sides (N) — the measured weight the
    /// footprint is carrying, so a turning moment can be normalized without an authored datum.
    support_n: f32,
    /// Fore-aft extent of the loaded contacts along the hull's forward axis (m) — the measured
    /// ground-contact length `L` of the classical `M_t = μ_t·W·L/4` turning-resistance form.
    footprint_m: f32,
    scheduler: crate::track::transmission::SchedulerState,
    /// The UNFILTERED demand sample the observer fed the EMA this tick: the production
    /// expression `max(0, dir·(R_l + R_r))`, so the CSV carries raw AND filtered load.
    demand_raw: f32,
    /// Consecutive-evidence counter behind the ordinary BAND upshift (`UPSHIFT_CONFIRM_TICKS`).
    band_confirm: u8,
    /// Remaining ticks of the declutched shift window (`st.shift_ticks`): non-zero means the
    /// demand observer is frozen and no scheduling decision runs.
    shift_ticks: u8,
    /// Turret-yaw servo angle (degrees, hull-local): 0 forward, ±180 rearward. NaN when the
    /// rig carries no `Turret_Yaw` servo.
    turret_yaw_deg: f32,
    /// `FireShell` events raised THIS tick — the production shot seam, not the command edge.
    fired: u32,
    /// The ordinary band upshift's PREDICTED landing rpm in the next gear — the fix-1a gate,
    /// which must clear `shift_down_rpm + POSTSHIFT_MARGIN_RPM`.
    landing_rpm: f32,
    /// `reserve_margin(demand)` — the headroom the next gear's reserve must clear.
    margin: f32,
}

/// Shot counter for the driving probes, incremented at the production `FireShell` seam so a
/// probe row records shots that actually left the bore (the command edge alone is only intent:
/// the reload gate, the crew requirement, or a corrupt bore pose all decline it silently).
#[derive(Resource, Default)]
struct ProbeShots(u32);

fn count_probe_fire_shells(_: On<crate::ballistics::FireShell>, mut shots: ResMut<ProbeShots>) {
    shots.0 += 1;
}

const PROBE_HEADER: &str = "tick,t_s,x,y,z,speed,fwd_speed,grade,pitch_deg,yaw_rate,yaw_kin,\
cmd_throttle,cmd_steer,drv_throttle,drv_steer,side_cmd_l,side_cmd_r,belt_l,belt_r,\
react_l,react_r,contacts_l,contacts_r,gear,reverse,shift_ticks,steer_step,scheduler,rpm,\
dec_shaft_rpm,demand_n,dec_reserve,dec_reserve_next,dec_margin,dec_landing_m,dec_landing_rpm,\
grade_confirm,grade_target,clutch_out,park,hill_hold,dwell,last_shift_dir,support_n,footprint_m,\
demand_raw,band_confirm,shift_window,turret_yaw_deg,fired,cmd_fire";

/// A running scenario: the booted sim plus the open telemetry sink. `tick` writes exactly one CSV
/// row per fixed tick, recomputing the shift scheduler's own decision inputs from the PRE-tick
/// state and THIS tick's ground reactions — the exact pair `transmission::step` consumed.
struct DriveProbe {
    app: BootedSim,
    tank: Entity,
    grid: crate::terrain_grid::HeightGrid,
    tp: crate::track::transmission::TransmissionParams,
    fp: crate::track::forces::ForceParams,
    half_tread: f32,
    out: std::io::BufWriter<std::fs::File>,
    path: std::path::PathBuf,
    tick: u32,
    /// `TankServos::states` slot of the turret-yaw servo, resolved once at boot by node name.
    turret_yaw_slot: Option<usize>,
}

/// Engine rad/s per rpm — the transmission module's own conversion, restated for the readout.
const PROBE_RPM_TO_RAD: f32 = std::f32::consts::TAU / 60.0;
const PROBE_DT: f32 = 1.0 / FIXED_TICKS_PER_SECOND as f32;

impl DriveProbe {
    fn open(scenario: &str, grade: f32) -> Self {
        let (mut app, tank, grid) = booted_ramp_sim(grade);
        // Read-only taps, installed for every probe: the production shot seam (`FireShell`) and
        // the turret-yaw servo slot. Neither writes sim state.
        app.init_resource::<ProbeShots>()
            .add_observer(count_probe_fire_shells);
        let turret_yaw_slot = {
            let world = app.world_mut();
            world
                .query::<(&Name, &crate::tank::ServoIndex, &crate::tank::TankRoot)>()
                .iter(world)
                .find(|(name, _, root)| root.0 == tank && name.as_str() == "Turret_Yaw")
                .map(|(_, slot, _)| slot.0)
        };
        let (tp, fp, half_tread) = {
            let gear = app.world().resource::<crate::track::sim::TrackGear>();
            (
                gear.trans()
                    .expect("the Tiger declares a transmission")
                    .clone(),
                gear.force_params().clone(),
                gear.half_tread(),
            )
        };
        let dir = std::path::PathBuf::from(
            crate::env_value("SPIKE_DRIVE_PROBE_DIR")
                .unwrap_or_else(|| "target/drive-probe".into()),
        );
        std::fs::create_dir_all(&dir).expect("the telemetry directory must be creatable");
        let path = dir.join(format!("{scenario}.csv"));
        let mut out = std::io::BufWriter::new(
            std::fs::File::create(&path).expect("the telemetry file must be creatable"),
        );
        {
            use std::io::Write as _;
            writeln!(out, "{PROBE_HEADER}").expect("telemetry header");
        }
        // The grounding/settle window is deliberately OUTSIDE the trace: `booted_ramp_sim` hands
        // back an already-grounded sim, and each scenario poses the hull before its first row.
        Self {
            app,
            tank,
            grid,
            tp,
            fp,
            half_tread,
            out,
            path,
            tick: 0,
            turret_yaw_slot,
        }
    }

    fn pose(&mut self, grade: f32, x: f32, z: f32, heading: Vec3) {
        let grid = self.grid.clone();
        place_on_ramp(&mut self.app, self.tank, &grid, grade, x, z, heading);
    }

    /// Advance one fixed tick under `(throttle, steer)` and append its telemetry row.
    fn tick(&mut self, throttle: f32, steer: f32) -> ProbeSample {
        self.tick_armed(throttle, steer, None, false)
    }

    /// [`Self::tick`] plus the turret/trigger seam: `aim` is the hull-local aim POINT the
    /// production `aim::drive_aim_servos` chases (held every tick, as the game layer re-authors
    /// it), and `fire` latches the one-tick primary edge `shooting::fire` consumes. Both are the
    /// SAME two command fields a player uses — no impulse is injected and no sim law is touched,
    /// so the recoil the trace records is the production `-mass·speed·RECOIL_FEEL` at the
    /// production bore pose.
    fn tick_armed(
        &mut self,
        throttle: f32,
        steer: f32,
        aim: Option<Vec3>,
        fire: bool,
    ) -> ProbeSample {
        let pre = {
            let world = self.app.world();
            (
                *world
                    .get::<crate::track::sim::TrackDrive>(self.tank)
                    .expect("tank drives"),
                world
                    .get::<crate::track::sim::TankTransmission>(self.tank)
                    .expect("tank carries transmission state")
                    .0,
            )
        };
        let shots_before = self.app.world().resource::<ProbeShots>().0;
        {
            let mut cmd = self
                .app
                .world_mut()
                .get_mut::<TankCommand>(self.tank)
                .expect("tank carries a command");
            cmd.aim = aim;
            cmd.fire_primary = fire;
        }
        drive_tick(&mut self.app, self.tank, throttle, steer);
        let fired = self.app.world().resource::<ProbeShots>().0 - shots_before;

        let world = self.app.world();
        let position = world
            .get::<avian3d::prelude::Position>(self.tank)
            .expect("tank has a position")
            .0;
        let rotation = world
            .get::<avian3d::prelude::Rotation>(self.tank)
            .expect("tank has a rotation")
            .0;
        let velocity = world
            .get::<avian3d::prelude::LinearVelocity>(self.tank)
            .expect("tank has velocity")
            .0;
        let angular = world
            .get::<avian3d::prelude::AngularVelocity>(self.tank)
            .expect("tank has angular velocity")
            .0;
        let drive = *world
            .get::<crate::track::sim::TrackDrive>(self.tank)
            .expect("tank drives");
        let st = world
            .get::<crate::track::sim::TankTransmission>(self.tank)
            .expect("tank carries transmission state")
            .0;
        let effect = *world
            .get::<crate::track::sim::TrackGripEffect>(self.tank)
            .expect("tank carries its traction effect");
        let contacts = world
            .get::<crate::track::sim::TrackContacts>(self.tank)
            .expect("tank carries its contact field");
        let contact_counts = [contacts.0[0].len(), contacts.0[1].len()];

        let forward = rotation * Vec3::NEG_Z;
        // The footprint the turning moment acts through, MEASURED: total carried load and the
        // fore-aft extent of the loaded contacts (the classical `L`), so `M_t = μ_t·W·L/4` can be
        // normalized against the sim's own geometry rather than an authored contact length.
        let (mut support_n, mut fore, mut aft) = (0.0f32, f32::NEG_INFINITY, f32::INFINITY);
        for side in &contacts.0 {
            for contact in side {
                if contact.load <= 0.0 {
                    continue;
                }
                support_n += contact.load;
                let along = (contact.point - position).dot(forward);
                fore = fore.max(along);
                aft = aft.min(along);
            }
        }
        let footprint_m = if fore > aft { fore - aft } else { 0.0 };
        let horizontal_forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        let speed = Vec3::new(velocity.x, 0.0, velocity.z).length();
        let forward_speed = velocity.dot(forward);
        let yaw_rate = angular.dot(rotation * Vec3::Y);
        let yaw_kinematic = (drive.sides[0].speed - drive.sides[1].speed) / (2.0 * self.half_tread);
        let pitch_deg = forward.y.clamp(-1.0, 1.0).asin().to_degrees();
        // The world's own grade along the heading, read through the surface the belts probe.
        let step = 2.0;
        let grade_here = if horizontal_forward == Vec3::ZERO {
            0.0
        } else {
            let ahead = self.grid.height_at(
                position.x + horizontal_forward.x * step,
                position.z + horizontal_forward.z * step,
            );
            let behind = self.grid.height_at(
                position.x - horizontal_forward.x * step,
                position.z - horizontal_forward.z * step,
            );
            (ahead - behind) / (2.0 * step)
        };

        // --- The scheduler's own decision inputs, recomputed from the PRE-tick transmission state
        // and THIS tick's belt reactions — exactly the pair `transmission::step` was handed.
        // DEMAND is the one exception: production updates the demand EMA BEFORE
        // computing reserves or evaluating shifts, and nothing later in the tick rewrites it, so
        // the POST-tick `st.demand_n` IS the value the scheduler priced this tick. Reading the
        // pre-tick EMA here blamed the wrong gate across the first-initialization transient.
        //
        // The LADDER is the second exception: production commits a direction
        // swap — reverse flipped, gear reset to 1 — BEFORE the demand observer and every
        // scheduling decision, so on the swap tick the decision state is the POST-swap
        // ladder at gear 1, not the pre-tick ladder. Pricing the swap row on the old ladder
        // put its shaft-rpm/reserve/landing columns on a gear the scheduler never consulted.
        let (pre_drive, pre_st) = pre;
        let dec_demand = st.demand_n;
        let swap_committed = st.reverse != pre_st.reverse;
        let (dec_reverse, dec_gear) = if swap_committed {
            (st.reverse, 1u8)
        } else {
            (pre_st.reverse, pre_st.gear)
        };
        let dir = if dec_reverse { -1.0 } else { 1.0 };
        let m = (pre_drive.sides[0].speed + pre_drive.sides[1].speed) / 2.0;
        let shaft = dir * m;
        let ladder: &[f32] = if dec_reverse {
            &self.tp.gears_rev
        } else {
            &self.tp.gears_fwd
        };
        let top = ladder.len() as u8;
        let current_ratio = ladder[(dec_gear.clamp(1, top) - 1) as usize];
        let shaft_rpm_of = |sh: f32, g: f32| sh * g / self.tp.sprocket_radius / PROBE_RPM_TO_RAD;
        let dec_shaft_rpm = shaft_rpm_of(shaft, current_ratio);
        let dec_reserve = crate::track::transmission::modeled_reserve_in_gear(
            &self.tp,
            &self.fp,
            shaft,
            current_ratio,
            dec_demand,
        );
        let dec_margin = crate::track::transmission::reserve_margin(dec_demand);
        let r_mean = (effect.belt_reaction[0] + effect.belt_reaction[1]) / 2.0;
        let landing = crate::track::transmission::predict_shift_landing_m(
            &self.tp, &self.fp, m, r_mean, PROBE_DT,
        );
        let (dec_reserve_next, dec_landing_rpm) = if dec_gear < top {
            let up = ladder[dec_gear as usize];
            (
                crate::track::transmission::modeled_reserve_in_gear(
                    &self.tp, &self.fp, shaft, up, dec_demand,
                ),
                shaft_rpm_of(dir * landing, up),
            )
        } else {
            (f32::NAN, f32::NAN)
        };

        let side_commands = crate::track::drive::DriveAxes {
            throttle: drive.throttle,
            steer: drive.steer,
        }
        .side_commands();
        let rpm = st.omega_e / PROBE_RPM_TO_RAD;
        let scheduler = match st.scheduler {
            crate::track::transmission::SchedulerState::Normal => "normal".to_string(),
            crate::track::transmission::SchedulerState::GradeShift { from, to } => {
                format!("grade{from}->{to}")
            }
            crate::track::transmission::SchedulerState::HillHold => "hillhold".to_string(),
            crate::track::transmission::SchedulerState::GradeLimit => "gradelimit".to_string(),
        };
        // The observer's UNFILTERED sample in the production expression. It is what the EMA
        // consumed on a decision tick and what it deliberately IGNORED inside a shift window
        // (`shift_window > 0`), so raw-vs-filtered stays readable across the cut.
        let demand_raw = (dir * (effect.belt_reaction[0] + effect.belt_reaction[1])).max(0.0);
        let turret_yaw_deg = self
            .turret_yaw_slot
            .and_then(|slot| {
                world
                    .get::<crate::tank::TankServos>(self.tank)
                    .and_then(|servos| servos.states.get(slot))
                    .map(|state| state.current().to_degrees())
            })
            .unwrap_or(f32::NAN);

        {
            use std::io::Write as _;
            writeln!(
                self.out,
                "{tick},{t:.5},{x:.4},{y:.4},{z:.4},{speed:.5},{fwd:.5},{grade:.6},{pitch:.4},\
{yaw:.6},{yaw_kin:.6},{cmd_t:.4},{cmd_s:.4},{dt_:.6},{ds:.6},{scl:.6},{scr:.6},{bl:.6},{br:.6},\
{rl:.2},{rr:.2},{cl},{cr},{gear},{rev},{shift},{detent},{sched},{rpm:.2},{dshaft:.2},{demand:.2},\
{res:.2},{resn:.2},{marg:.2},{land:.5},{landrpm:.2},{gc},{gt},{clutch},{park},{hold},{dwell},{lsd},\
{support:.1},{footprint:.4},{demand_raw:.2},{bc},{sw},{turret:.4},{fired},{cmd_fire}",
                tick = self.tick,
                t = self.tick as f32 / FIXED_TICKS_PER_SECOND as f32,
                x = position.x,
                y = position.y,
                z = position.z,
                speed = speed,
                fwd = forward_speed,
                grade = grade_here,
                pitch = pitch_deg,
                yaw = yaw_rate,
                yaw_kin = yaw_kinematic,
                cmd_t = throttle,
                cmd_s = steer,
                dt_ = drive.throttle,
                ds = drive.steer,
                scl = side_commands[0],
                scr = side_commands[1],
                bl = drive.sides[0].speed,
                br = drive.sides[1].speed,
                rl = effect.belt_reaction[0],
                rr = effect.belt_reaction[1],
                cl = contact_counts[0],
                cr = contact_counts[1],
                gear = st.gear,
                rev = u8::from(st.reverse),
                shift = st.shift_ticks,
                detent = st.steer_step,
                sched = scheduler,
                rpm = rpm,
                dshaft = dec_shaft_rpm,
                demand = st.demand_n,
                res = dec_reserve,
                resn = dec_reserve_next,
                marg = dec_margin,
                land = landing,
                landrpm = dec_landing_rpm,
                gc = st.grade_confirm_ticks,
                gt = st.grade_target,
                clutch = u8::from(st.clutch_out),
                park = u8::from(st.park),
                hold = u8::from(st.hill_hold),
                dwell = st.dwell_ticks,
                lsd = st.last_shift_dir,
                support = support_n,
                footprint = footprint_m,
                demand_raw = demand_raw,
                bc = st.band_confirm_ticks,
                sw = st.shift_ticks,
                turret = turret_yaw_deg,
                fired = fired,
                cmd_fire = u8::from(fire),
            )
            .expect("telemetry row");
        }
        self.tick += 1;

        ProbeSample {
            speed,
            fwd_speed: forward_speed,
            yaw_rate,
            yaw_kinematic,
            gear: st.gear,
            reverse: st.reverse,
            park: st.park,
            steer_step: st.steer_step,
            rpm,
            demand_n: st.demand_n,
            reserve: dec_reserve,
            reserve_next: dec_reserve_next,
            belts: [drive.sides[0].speed, drive.sides[1].speed],
            belt_reaction: effect.belt_reaction,
            support_n,
            footprint_m,
            side_commands,
            position,
            scheduler: st.scheduler,
            demand_raw,
            band_confirm: st.band_confirm_ticks,
            shift_ticks: st.shift_ticks,
            turret_yaw_deg,
            fired,
            landing_rpm: dec_landing_rpm,
            margin: dec_margin,
        }
    }

    fn finish(mut self) -> std::path::PathBuf {
        use std::io::Write as _;
        self.out.flush().expect("telemetry flush");
        self.path
    }
}

/// A gear trace as `(tick, gear)` transitions — the compact form the summaries print.
fn gear_transitions(samples: &[ProbeSample]) -> Vec<(usize, u8)> {
    let mut trace: Vec<(usize, u8)> = Vec::new();
    for (tick, sample) in samples.iter().enumerate() {
        if trace.last().map(|&(_, g)| g) != Some(sample.gear) {
            trace.push((tick, sample.gear));
        }
    }
    trace
}

/// SYMPTOM 1 — uphill upshift refusal. Full throttle from rest, pointed straight up a constant
/// grade. The trace answers whether the box ever leaves its launch gear, and (via `dec_reserve`,
/// `dec_reserve_next`, `dec_landing_rpm`) WHICH of the three upshift gates refuses.
#[test]
#[ignore = "instrument, not a gate: writes driving telemetry"]
fn probe_climb_10pct() {
    let grade = probe_grade(0.10);
    let mut probe = DriveProbe::open(&format!("climb-{:.0}pct", grade * 100.0), grade);
    probe.pose(grade, -200.0, 0.0, Vec3::X);
    let mut samples = Vec::new();
    for _ in 0..(40 * FIXED_TICKS_PER_SECOND) {
        samples.push(probe.tick(1.0, 0.0));
    }
    let last = *samples.last().expect("the climb recorded ticks");
    let max_rpm = samples.iter().fold(0.0f32, |a, s| a.max(s.rpm));
    let max_speed = samples.iter().fold(0.0f32, |a, s| a.max(s.speed));
    let governed = probe.tp.engine.governed_rpm;
    let up_band = probe.tp.shift_up_rpm;
    let ticks_at_band = samples.iter().filter(|s| s.rpm >= up_band).count();
    let mean_reserve_next = samples
        .iter()
        .filter(|s| s.reserve_next.is_finite())
        .map(|s| s.reserve_next)
        .sum::<f32>()
        / samples.len().max(1) as f32;
    println!(
        "probe climb {grade_pct:.1}% grade -> {path}\n  \
         gear trace (tick, gear): {trace:?}\n  \
         max rpm {max_rpm:.0} (up band {up_band:.0}, governed {governed:.0}), \
         ticks at/above up band {ticks_at_band}\n  \
         max speed {max_speed:.3} m/s, final speed {speed:.3} m/s, climbed {climb:.1} m of x\n  \
         final demand {demand:.0} N, reserve(cur) {res:.0} N, reserve(next) {resn:.0} N, \
         mean reserve(next) {meanresn:.0} N, scheduler {sched:?}",
        grade_pct = grade * 100.0,
        trace = gear_transitions(&samples),
        speed = last.speed,
        climb = last.position.x + 200.0,
        demand = last.demand_n,
        res = last.reserve,
        resn = last.reserve_next,
        meanresn = mean_reserve_next,
        sched = last.scheduler,
        path = probe.finish().display(),
    );
}

/// SYMPTOM 4 — the reported FIRE-BACKWARD-UPHILL gear oscillation (field report: climbing in F1,
/// each main-gun shot over the rear deck walks F1 → 2 → 3 → 1).
///
/// Full throttle straight up a constant grade with the turret laid at
/// `SPIKE_DRIVE_PROBE_FIRE_YAW_DEG` (default 180°, over the rear deck), pulling the primary trigger
/// once the climb has SETTLED and again after it re-settles. Both the lay and the trigger go
/// through the production command seam (`TankCommand::aim` chased by `aim::drive_aim_servos`, and
/// the one-tick `fire_primary` edge `shooting::fire` consumes) — NOTHING is injected, so the
/// recoil is `shooting::fire`'s own `−mass·speed·RECOIL_FEEL` impulse applied at the tick-truth
/// muzzle pose. Firing REARWARD while climbing therefore shoves the hull FORWARD, up the hill:
/// exactly the transient the `UPSHIFT_CONFIRM_TICKS` hard-reset window was written against.
///
/// The columns that decide it are `demand_raw`/`demand_n` (raw vs filtered load), `dec_shaft_rpm`
/// against the up band, `band_confirm` (the full-predicate upshift evidence counter), `dwell`,
/// `shift_window`, `dec_reserve`/`dec_reserve_next` and `fired` (the production shot seam, not the
/// command edge).
///
/// Knobs: `SPIKE_DRIVE_PROBE_GRADE`, `SPIKE_DRIVE_PROBE_FIRE_YAW_DEG`,
/// `SPIKE_DRIVE_PROBE_FIRE_SHOTS` (0 runs the identical command stream with the trigger held out —
/// the control), `SPIKE_DRIVE_PROBE_FIRE_SETTLE_S` (deadline after which the first shot goes
/// regardless), `SPIKE_DRIVE_PROBE_FIRE_GAP_S`, `SPIKE_DRIVE_PROBE_FIRE_RUN_S`.
#[test]
#[ignore = "instrument, not a gate: writes driving telemetry"]
fn probe_fire_backward_uphill() {
    let grade = probe_grade(0.10);
    let yaw_deg: f32 = crate::env_parse("SPIKE_DRIVE_PROBE_FIRE_YAW_DEG").unwrap_or(180.0);
    let shots: usize = crate::env_parse("SPIKE_DRIVE_PROBE_FIRE_SHOTS").unwrap_or(2);
    let settle_deadline_s: usize =
        crate::env_parse("SPIKE_DRIVE_PROBE_FIRE_SETTLE_S").unwrap_or(12);
    let gap_s: usize = crate::env_parse("SPIKE_DRIVE_PROBE_FIRE_GAP_S").unwrap_or(6);
    let run_s: usize = crate::env_parse("SPIKE_DRIVE_PROBE_FIRE_RUN_S").unwrap_or(40);
    let mut probe = DriveProbe::open(
        &format!("fire-{:.0}pct-yaw{yaw_deg:.0}-n{shots}", grade * 100.0),
        grade,
    );
    probe.pose(grade, -200.0, 0.0, Vec3::X);

    // The hull-local aim POINT whose azimuth is `yaw_deg`: `drive_aim_servos` lays yaw at
    // `(-x).atan2(-z)`, so a point at `(-R·sin θ, 0, -R·cos θ)` commands exactly θ. Level in the
    // hull frame (y = 0) keeps the gun inside its −8°..+15° travel on any probe grade, and R is
    // far enough away that each servo's own mount offset is irrelevant to the azimuth.
    const AIM_RANGE: f32 = 1000.0;
    let (sin, cos) = yaw_deg.to_radians().sin_cos();
    let aim = Some(Vec3::new(-AIM_RANGE * sin, 0.0, -AIM_RANGE * cos));

    // "Settled" = the climb has stopped changing: same gear, no shift window open, hull speed
    // steady, and the turret actually ON its commanded lay. Only then is a shot's transient
    // attributable to the shot.
    const STEADY_TICKS: usize = 48;
    let settled = |samples: &[ProbeSample]| {
        samples.len() > STEADY_TICKS && {
            let window = &samples[samples.len() - STEADY_TICKS..];
            let last = window[window.len() - 1];
            let turret_error = {
                let raw = (last.turret_yaw_deg - yaw_deg).rem_euclid(360.0);
                raw.min(360.0 - raw)
            };
            turret_error < 1.0
                && last.shift_ticks == 0
                && window
                    .iter()
                    .all(|s| s.gear == last.gear && s.shift_ticks == 0)
                && (last.fwd_speed - window[0].fwd_speed).abs() < 0.01
        }
    };

    let gap = gap_s * FIXED_TICKS_PER_SECOND;
    let deadline = settle_deadline_s * FIXED_TICKS_PER_SECOND;
    let mut samples: Vec<ProbeSample> = Vec::new();
    let mut shot_ticks: Vec<usize> = Vec::new();
    for tick in 0..(run_s * FIXED_TICKS_PER_SECOND) {
        let spaced = shot_ticks.last().is_none_or(|&t| tick >= t + gap);
        // The edge is asserted only when the run is armed; the reload gate may still decline it
        // (`fired` is the truth), in which case it is re-asserted on the next armed tick.
        let fire = shot_ticks.len() < shots
            && spaced
            && (settled(&samples) || (shot_ticks.is_empty() && tick >= deadline));
        let sample = probe.tick_armed(1.0, 0.0, aim, fire);
        if sample.fired > 0 {
            shot_ticks.push(tick);
        }
        samples.push(sample);
    }

    // Per-shot transient: what the shot did to speed and rpm, how long the upshift evidence held,
    // and how long the box stayed off the gear it was climbing in.
    let up_band = probe.tp.shift_up_rpm;
    // The fix-1a landing band, read from the production constants rather than restated.
    let landing_gate = probe.tp.shift_down_rpm + crate::track::transmission::POSTSHIFT_MARGIN_RPM;
    let mut shot_reports = Vec::new();
    for &shot in &shot_ticks {
        let pre = samples[shot.saturating_sub(1)];
        let window_end = (shot + 4 * FIXED_TICKS_PER_SECOND).min(samples.len());
        let window = &samples[shot..window_end];
        let peak_speed = window.iter().fold(f32::MIN, |a, s| a.max(s.fwd_speed));
        let min_speed = window.iter().fold(f32::MAX, |a, s| a.min(s.fwd_speed));
        let peak_rpm = window.iter().fold(f32::MIN, |a, s| a.max(s.rpm));
        let min_rpm = window.iter().fold(f32::MAX, |a, s| a.min(s.rpm));
        let wrong_gear = window.iter().filter(|s| s.gear != pre.gear).count();
        let recovered = window
            .iter()
            .enumerate()
            .skip(1)
            .find(|(i, s)| s.gear == pre.gear && window[..*i].iter().any(|w| w.gear != pre.gear))
            .map(|(i, _)| i);
        // The two gates the shot can flip, each stated as its DISTANCE before and one tick after
        // the shot: the fix-1a landing band (`landing_rpm` vs `shift_down_rpm +
        // POSTSHIFT_MARGIN_RPM`) and the stage-C reserve gate (`reserve_next` vs
        // `reserve_margin(demand)`). `band_confirm` is reported alongside; since the recoil
        // round it counts the FULL ordinary-upshift predicate (landing and reserve included),
        // so on a lugging climb it reads 0 instead of the old saturated 255 — the counter now
        // IS the gate under test.
        let post = window.get(1).copied().unwrap_or(pre);
        let confirm_needed = crate::track::transmission::UPSHIFT_CONFIRM_TICKS;
        shot_reports.push(format!(
            "  shot @tick {shot} (t {t:.2} s): gear F{gear} speed {v:.3} m/s rpm {rpm:.0}, \
             band_confirm {bc} (needs {confirm_needed}, up band {up_band:.0})\n    \
             speed {min_speed:.3}..{peak_speed:.3} m/s (Δ+{dv:.3}); rpm {min_rpm:.0}..{peak_rpm:.0} \
             (Δ{drpm:+.0})\n    \
             landing gate: {land_pre:.0} -> {land_post:.0} rpm against {land_gate:.0} \
             (distance {dpre:+.0} -> {dpost:+.0})\n    \
             reserve gate: {res_pre:.0} -> {res_post:.0} N against margin {marg_pre:.0} -> \
             {marg_post:.0} (distance {rpre:+.0} -> {rpost:+.0})\n    \
             off-gear ticks {wrong_gear} of {win}; gears {trace:?}{recov}",
            t = shot as f32 / FIXED_TICKS_PER_SECOND as f32,
            gear = pre.gear,
            v = pre.fwd_speed,
            rpm = pre.rpm,
            bc = pre.band_confirm,
            dv = peak_speed - pre.fwd_speed,
            drpm = peak_rpm - pre.rpm,
            land_pre = pre.landing_rpm,
            land_post = post.landing_rpm,
            land_gate = landing_gate,
            dpre = pre.landing_rpm - landing_gate,
            dpost = post.landing_rpm - landing_gate,
            res_pre = pre.reserve_next,
            res_post = post.reserve_next,
            marg_pre = pre.margin,
            marg_post = post.margin,
            rpre = pre.reserve_next - pre.margin,
            rpost = post.reserve_next - post.margin,
            win = window.len(),
            trace = gear_transitions(window),
            recov = match recovered {
                Some(i) => format!(", back in F{} after {i} ticks", pre.gear),
                None => String::new(),
            },
        ));
    }

    let last = *samples.last().expect("the fire probe recorded ticks");
    let weapon_note = {
        let world = probe.app.world_mut();
        world
            .query::<(&crate::tank::Weapon, &crate::tank::Muzzle)>()
            .iter(world)
            .find(|(w, _)| matches!(w.trigger, crate::spec::Trigger::Primary))
            .map(|(w, _)| {
                format!(
                    "{} — {:.1} kg at {:.0} m/s = {:.0} N·s of hull recoil (production \
                     `-mass*speed*RECOIL_FEEL`)",
                    w.name,
                    w.mass,
                    w.speed,
                    w.mass * w.speed,
                )
            })
            .unwrap_or_else(|| "no primary weapon found".into())
    };
    println!(
        "probe fire-backward {grade_pct:.1}% grade, turret {yaw_deg:.0}°, {n} shot(s) -> {path}\n  \
         weapon: {weapon_note}\n  \
         gear trace (tick, gear): {trace:?}\n  \
         shots landed at ticks {shot_ticks:?}\n\
{reports}\n  \
         final gear F{gear}, speed {speed:.3} m/s, rpm {rpm:.0}, climbed {climb:.1} m of x, \
         demand {demand:.0} N (raw {raw:.0} N), reserve(cur) {res:.0} N, scheduler {sched:?}",
        grade_pct = grade * 100.0,
        n = shot_ticks.len(),
        trace = gear_transitions(&samples),
        reports = shot_reports.join("\n"),
        gear = last.gear,
        speed = last.fwd_speed,
        rpm = last.rpm,
        climb = last.position.x + 200.0,
        demand = last.demand_n,
        raw = last.demand_raw,
        res = last.reserve,
        sched = last.scheduler,
        path = probe.finish().display(),
    );
}

/// SYMPTOM 2 — downhill overrun. Launch down the same grade, then release the throttle and coast:
/// the trace shows how far past the governor the crank is dragged. On overrun the
/// box HOLDS its gear (engine braking is the point) and the protective upshift is a last resort
/// past the max-curve ceiling — expect the dial to climb toward the curve top in the held gear,
/// not the old governed + 150 upshift walk.
#[test]
#[ignore = "instrument, not a gate: writes driving telemetry"]
fn probe_descend_10pct() {
    let grade = probe_grade(0.10);
    let mut probe = DriveProbe::open(&format!("descend-{:.0}pct", grade * 100.0), grade);
    probe.pose(grade, 300.0, 0.0, Vec3::NEG_X);
    let launch = 4 * FIXED_TICKS_PER_SECOND;
    let mut samples = Vec::new();
    for tick in 0..(40 * FIXED_TICKS_PER_SECOND) {
        let throttle = if tick < launch { 1.0 } else { 0.0 };
        samples.push(probe.tick(throttle, 0.0));
    }
    let coast = &samples[launch..];
    let max_rpm = coast.iter().fold(0.0f32, |a, s| a.max(s.rpm));
    let max_speed = coast.iter().fold(0.0f32, |a, s| a.max(s.speed));
    let governed = probe.tp.engine.governed_rpm;
    let rated = probe.tp.max_curve_rpm();
    let over = coast.iter().filter(|s| s.rpm > governed).count();
    let last = *coast.last().expect("the descent recorded coast ticks");
    println!(
        "probe descend {grade_pct:.1}% grade -> {path}\n  \
         gear trace (tick, gear): {trace:?}\n  \
         coast max rpm {max_rpm:.0} (governed {governed:.0}, curve top {rated:.0}) = \
         {over_pct:.1}% over governed; coast ticks above governed {over}\n  \
         coast max speed {max_speed:.3} m/s, final speed {speed:.3} m/s, final gear F{gear}, \
         final rpm {rpm:.0}, belts L {bl:.3} / R {br:.3}",
        grade_pct = grade * 100.0,
        trace = gear_transitions(&samples),
        over_pct = (max_rpm / governed - 1.0) * 100.0,
        speed = last.speed,
        gear = last.gear,
        rpm = last.rpm,
        bl = last.belts[0],
        br = last.belts[1],
        path = probe.finish().display(),
    );
}

/// SYMPTOM 3 — slow turning. Reach a low steady speed on the FLAT (grade 0), then hold full steer:
/// the trace pairs the commanded side split (`side_cmd_l/r`, and the belt difference it produced)
/// with the hull's measured yaw rate and the yaw rate that belt difference alone implies
/// (`yaw_kin = (v_l − v_r) / 2b`). The gap between them is scrub; the gap between `yaw_kin` and the
/// L600 detent's own `v / R` is the transmission's.
#[test]
#[ignore = "instrument, not a gate: writes driving telemetry"]
fn probe_turn_at_2ms() {
    const TARGET: f32 = 2.0;
    // Hold the SAME throttle through the turn as through the straight run-up, so the only changed
    // input is the steer command; `SPIKE_DRIVE_PROBE_TURN_THROTTLE` re-runs it part-throttle.
    let turn_throttle: f32 = crate::env_parse("SPIKE_DRIVE_PROBE_TURN_THROTTLE").unwrap_or(1.0);
    // Full lock by default (the tight detent); `SPIKE_DRIVE_PROBE_STEER=0.3` re-runs it wide.
    let steer = probe_steer(1.0);
    let mut probe = DriveProbe::open(&format!("turn-t{turn_throttle:.2}-s{steer:.2}"), 0.0);
    probe.pose(0.0, 0.0, 0.0, Vec3::X);
    // Phase 1: full throttle straight until the hull passes the target speed (bounded).
    let mut spin_up = 0;
    for tick in 0..(20 * FIXED_TICKS_PER_SECOND) {
        let sample = probe.tick(1.0, 0.0);
        spin_up = tick + 1;
        if sample.speed >= TARGET {
            break;
        }
    }
    // Phase 2: same throttle, steer held at the selected magnitude.
    let mut samples = Vec::new();
    for _ in 0..(20 * FIXED_TICKS_PER_SECOND) {
        samples.push(probe.tick(turn_throttle, steer));
    }
    // Read the settled turn from the last two seconds, clear of the steer slew and detent engage.
    let settled = &samples[samples.len() - 2 * FIXED_TICKS_PER_SECOND..];
    let mean =
        |f: fn(&ProbeSample) -> f32| settled.iter().map(f).sum::<f32>() / settled.len() as f32;
    let speed = mean(|s| s.speed);
    let yaw = mean(|s| s.yaw_rate);
    let yaw_kin = mean(|s| s.yaw_kinematic);
    let belt_l = mean(|s| s.belts[0]);
    let belt_r = mean(|s| s.belts[1]);
    let cmd_l = mean(|s| s.side_commands[0]);
    let cmd_r = mean(|s| s.side_commands[1]);
    let last = *settled.last().expect("the turn recorded settled ticks");
    let radii = &probe.tp.steer_radii_m;
    let (tight, wide) = radii[(last.gear.clamp(1, radii.len() as u8) - 1) as usize];
    println!(
        "probe turn: steer {steer:.2} engaged at {TARGET:.1} m/s, throttle {turn_throttle:.2} \
(flat) -> {path}\n  \
         spin-up took {spin_up} ticks; settled gear F{gear}, detent radii tight {tight:.2} m / \
         wide {wide:.2} m\n  \
         commanded side split L {cmd_l:.3} / R {cmd_r:.3} (difference {cmd_d:.3}); \
         belts L {belt_l:.3} / R {belt_r:.3} m/s (difference {belt_d:.3})\n  \
         speed {speed:.3} m/s; yaw MEASURED {yaw:.4} rad/s vs belt-kinematic {yaw_kin:.4} rad/s \
         vs detent v/R tight {yaw_tight:.4} / wide {yaw_wide:.4} rad/s\n  \
         implied turn radius {radius:.2} m; gear trace (tick, gear): {trace:?}",
        gear = last.gear,
        cmd_d = cmd_l - cmd_r,
        belt_d = belt_l - belt_r,
        yaw_tight = speed / tight,
        yaw_wide = speed / wide,
        radius = if yaw.abs() > 1e-6 {
            speed / yaw.abs()
        } else {
            f32::INFINITY
        },
        trace = gear_transitions(&samples),
        path = probe.finish().display(),
    );
}

// --- Turn-radius sweep ---------------------------------------------------------------------------
//
// The falsifiable experiment behind SYMPTOM 3. The shipped box is FixedRadii/L600: the commanded
// belt-speed half-difference is `d = sign(steer)·κ(gear, step)·|m|`, so gear × detent selects a
// COMMANDED radius `R_cmd = half_tread/κ` spanning 3.44 m (F1 tight) to 165 m (F8 wide). The
// per-element grip law underneath is an isotropic friction circle at a single μ.
//
// Primary-source research says the real Tiger's radius table is pure gear-train kinematics and that
// the physical turning resistance FALLS steeply with commanded radius (the empirical Nikitin curve:
// ~0.9·μ at R = 3.5 m down to ~0.1·μ at R = 173 m). The prediction that follows for THIS sim is
// that the scrub loss `1 − η`, with `η = ω·half_tread/d`, must be strongly radius-dependent — large
// in a tight low-gear turn, near zero in a high-gear sweeper. A FLAT loss across the whole radius
// span would instead indicate a defect in the force chain, not a friction curve.
//
// Secondary checksum: at the widest commanded radii the INNER track's longitudinal force must cross
// from braking (negative `belt_reaction`) to driving (positive) — the classical free turning radius
// — which validates the force balance end to end.
//
// The gear is reached HONESTLY: full throttle straight on the flat until the box lands the target
// gear, then the steer command goes in immediately. That ordering is what makes the point hold: the
// shift lands the gear with a torque-cut window of `shift_secs` (≈ 20 ticks) during which no further
// shift can be selected, and the steer slew ([`crate::track::drive::DRIVE_SLEW_PER_SECOND`] = 4/s)
// crosses the tight detent threshold in ~9 ticks — so the detent latches INSIDE the window, and an
// engaged detent defers every upshift thereafter. Nothing is pre-seeded; no sim law is touched.

/// One settled point of [`probe_turn_radius_sweep`] — everything the trend needs, per (gear, detent).
struct TurnPoint {
    target_gear: u8,
    steer_cmd: f32,
    /// Gear the box actually settled in (a bog downshift shows up here).
    gear: u8,
    /// Engaged detent step: 0 straight, 1 wide, 2 tight.
    step: u8,
    /// `κ = half_tread/R` for the settled (gear, step) — the table the constraint reads.
    kappa: f32,
    /// Commanded radius from the RON table for the settled (gear, step).
    r_cmd: f32,
    /// `v/|ω|` — the radius the hull actually described.
    r_achieved: f32,
    speed: f32,
    rpm: f32,
    /// `κ·|m|` — the half-difference the constraint asked for.
    d_cmd: f32,
    /// `(v_L − v_R)/2` — the half-difference the belts actually ran.
    d_actual: f32,
    /// Signed hull yaw rate (rad/s); negative is the right turn a positive steer commands.
    yaw: f32,
    /// `|ω|·half_tread/|d|` — how much of the belt difference became hull rotation.
    eta: f32,
    belts: [f32; 2],
    /// Per-side longitudinal ground force (N), `[outer, inner]` for the commanded turn side.
    reaction: [f32; 2],
    /// The yaw-resisting couple the footprint actually produced (N·m): `(F_outer − F_inner)·b`.
    /// This — not `1 − η` — is the sim's own analogue of a turning-resistance coefficient.
    yaw_moment: f32,
    /// `4·M/(W·L)` from the MEASURED carried load and contact length: the sim's emergent turning
    /// friction coefficient in the units the Nikitin curve is quoted in.
    mu_t_measured: f32,
    /// The Nikitin reference `μ/(0.925 + 0.15·R_cmd/B)` at this commanded radius — a REFERENCE
    /// CURVE for comparison only; no sim law reads it.
    mu_t_nikitin: f32,
    notes: Vec<String>,
    path: std::path::PathBuf,
}

/// Drive one sweep point: run up to `target_gear` on the flat, then hold `steer_cmd` until the turn
/// settles, and reduce the last `SPIKE_DRIVE_PROBE_SETTLE_S` seconds into a [`TurnPoint`].
fn probe_turn_point(target_gear: u8, steer_cmd: f32) -> TurnPoint {
    let run_up_secs: usize = crate::env_parse("SPIKE_DRIVE_PROBE_RUNUP_S").unwrap_or(120);
    let turn_secs: usize = crate::env_parse("SPIKE_DRIVE_PROBE_TURN_S").unwrap_or(40);
    let settle_secs: usize = crate::env_parse("SPIKE_DRIVE_PROBE_SETTLE_S").unwrap_or(3);
    let settle = settle_secs * FIXED_TICKS_PER_SECOND;
    assert!(
        turn_secs >= 2 * settle_secs && settle_secs > 0,
        "the turn window must hold two settle windows"
    );

    let mut probe = DriveProbe::open(&format!("turn-F{target_gear}-s{steer_cmd:.2}"), 0.0);
    // Start deep in the −X/−Z quadrant, heading +X: a positive steer curves toward hull-right
    // (+Z here), so the widest circle (R = 165 m) still closes well inside the world span.
    probe.pose(0.0, -1000.0, -300.0, Vec3::X);
    let mut notes = Vec::new();

    // Phase 1 — honest run-up: full throttle, straight, until the box lands the target gear.
    let mut run_up = 0usize;
    let mut top_seen = 1u8;
    let mut reached = false;
    for _ in 0..(run_up_secs * FIXED_TICKS_PER_SECOND) {
        let sample = probe.tick(1.0, 0.0);
        run_up += 1;
        top_seen = top_seen.max(sample.gear);
        if sample.gear >= target_gear {
            reached = true;
            break;
        }
        if sample.position.x.abs() > 1100.0 || sample.position.z.abs() > 1100.0 {
            notes.push("run-up hit the world guard before the gear landed".into());
            break;
        }
    }
    if !reached {
        notes.push(format!(
            "target gear NOT reached on the flat: topped out at F{top_seen} after {run_up} ticks"
        ));
    }

    // Phase 2 — steer in and hold. The first ~1 s covers the command slew and the detent latch.
    let mut turn = Vec::with_capacity(turn_secs * FIXED_TICKS_PER_SECOND);
    for _ in 0..(turn_secs * FIXED_TICKS_PER_SECOND) {
        turn.push(probe.tick(1.0, steer_cmd));
    }

    let mean = |window: &[ProbeSample], f: fn(&ProbeSample) -> f32| {
        window.iter().map(f).sum::<f32>() / window.len() as f32
    };
    let settled = &turn[turn.len() - settle..];
    let prior = &turn[turn.len() - 2 * settle..turn.len() - settle];
    let last = *settled.last().expect("the turn recorded settled ticks");

    let speed = mean(settled, |s| s.speed);
    let yaw = mean(settled, |s| s.yaw_rate);
    let belts = [mean(settled, |s| s.belts[0]), mean(settled, |s| s.belts[1])];
    let reaction_lr = [
        mean(settled, |s| s.belt_reaction[0]),
        mean(settled, |s| s.belt_reaction[1]),
    ];
    let m = (belts[0] + belts[1]) / 2.0;
    let d_actual = (belts[0] - belts[1]) / 2.0;
    let (tp, fp, half_tread) = (probe.tp.clone(), probe.fp.clone(), probe.half_tread);
    let idx = (last.gear.clamp(1, tp.steer_kappa.len() as u8) - 1) as usize;
    let (k_tight, k_wide) = tp.steer_kappa[idx];
    let (r_tight, r_wide) = tp.steer_radii_m[idx];
    let (kappa, r_cmd) = match last.steer_step {
        0 => (0.0, f32::INFINITY),
        1 => (k_wide, r_wide),
        _ => (k_tight, r_tight),
    };
    // The constraint's own target: sign(steer)·κ·|m|. Steer sign is the sweep's (positive).
    let d_cmd = kappa * m.abs();
    // The hull yaw sign is opposite the belt-difference convention (hull forward is −Z, so a
    // faster LEFT belt yaws negative about the hull's up axis); η is taken on magnitudes.
    let eta = if d_actual.abs() > 1e-6 {
        yaw.abs() * half_tread / d_actual.abs()
    } else {
        f32::NAN
    };
    let r_achieved = if yaw.abs() > 1e-6 {
        speed / yaw.abs()
    } else {
        f32::INFINITY
    };
    // The turning-resistance reading. The couple the two belts push through the tread arm is the
    // moment the footprint's lateral scrub is resisting; normalizing it by the MEASURED carried
    // load and contact length (`M_t = μ_t·W·L/4`) puts the sim's emergent number in the same units
    // as the empirical Nikitin curve `μ_t = μ/(0.925 + 0.15·R/B)`, quoted here purely as a
    // reference shape (B = tread = 2·half_tread).
    let yaw_moment = (reaction_lr[0] - reaction_lr[1]) * half_tread;
    let support_n = mean(settled, |s| s.support_n);
    let footprint_m = mean(settled, |s| s.footprint_m);
    let mu_t_measured = if support_n > 1.0 && footprint_m > 1e-3 {
        4.0 * yaw_moment / (support_n * footprint_m)
    } else {
        f32::NAN
    };
    let mu_t_nikitin = fp.mu / (0.925 + 0.15 * r_cmd / (2.0 * half_tread));
    if last.gear != target_gear {
        notes.push(format!(
            "settled in F{} — not the target F{target_gear}",
            last.gear
        ));
    }
    if last.steer_step == 0 {
        notes.push("the detent NEVER engaged (straight-gear constraint)".into());
    }
    let settled_rpm = mean(settled, |s| s.rpm);
    if settled_rpm < tp.engine.idle_rpm {
        notes.push(format!(
            "TURN BOG: the crank settled at {settled_rpm:.0} rpm, below idle \
             {:.0} — the turn load lugged the engine and no lower gear was available",
            tp.engine.idle_rpm
        ));
    }
    let trace = gear_transitions(&turn);
    if trace.len() > 1 {
        notes.push(format!("gear moved during the turn: {trace:?}"));
    }
    if d_cmd > 1e-4 && (d_actual - d_cmd).abs() / d_cmd > 0.05 {
        notes.push(format!(
            "λ SLIP: belt difference {d_actual:.3} vs commanded {d_cmd:.3} m/s \
             ({:+.1}%) — the geared constraint is not being met",
            (d_actual / d_cmd - 1.0) * 100.0
        ));
    }
    let drift = |f: fn(&ProbeSample) -> f32| {
        let (a, b) = (mean(settled, f), mean(prior, f));
        (a - b).abs() / a.abs().max(1e-3)
    };
    let (v_drift, w_drift) = (drift(|s| s.speed), drift(|s| s.yaw_rate));
    if v_drift > 0.02 || w_drift > 0.02 {
        notes.push(format!(
            "not fully settled over the last {}s: speed drift {:.1}%, yaw drift {:.1}%",
            2 * settle_secs,
            v_drift * 100.0,
            w_drift * 100.0
        ));
    }

    TurnPoint {
        target_gear,
        steer_cmd,
        gear: last.gear,
        step: last.steer_step,
        kappa,
        r_cmd,
        r_achieved,
        speed,
        rpm: settled_rpm,
        d_cmd,
        d_actual,
        yaw,
        eta,
        belts,
        // Positive steer runs the LEFT belt fast, so left is the OUTER track and right the inner.
        reaction: reaction_lr,
        yaw_moment,
        mu_t_measured,
        mu_t_nikitin,
        notes,
        path: probe.finish(),
    }
}

/// THE RADIUS SWEEP — every reachable forward gear × both steering detents, as steady-state turns
/// on the flat. Prints one table row per point (gear, detent, `R_cmd`, `R_achieved`, `v`, rpm,
/// `d_cmd`, `d_actual`, `ω`, `η`, loss, per-side force with the inner-track sign, and the yaw
/// moment against the Nikitin reference) and flags every point where the constraint slipped or the
/// box left the target gear instead of pretending it settled.
#[test]
#[ignore = "instrument, not a gate: writes driving telemetry"]
fn probe_turn_radius_sweep() {
    let gears = probe_list("SPIKE_DRIVE_PROBE_GEARS", &[1u8, 2, 3, 4, 5, 6, 7, 8]);
    // Tight then wide: `SPIKE_DRIVE_PROBE_STEER` re-points the tight entry, `..._STEERS` the list.
    let steers = probe_list("SPIKE_DRIVE_PROBE_STEERS", &[probe_steer(1.0), 0.3f32]);
    let mut points = Vec::new();
    for &steer in &steers {
        for &gear in &gears {
            points.push(probe_turn_point(gear, steer));
        }
    }

    println!(
        "\nturn-radius sweep — FixedRadii/L600, flat ground, full throttle\n\
         {:>4} {:>6} {:>7} {:>8} {:>9} {:>7} {:>7} {:>7} {:>7} {:>9} {:>6} {:>6} {:>10} {:>10} \
{:>9} {:>7} {:>7}",
        "gear",
        "detent",
        "kappa",
        "R_cmd_m",
        "R_achv_m",
        "v_m/s",
        "rpm",
        "d_cmd",
        "d_actl",
        "yaw_rad/s",
        "eta",
        "loss",
        "F_outer_N",
        "F_inner_N",
        "M_yaw_kNm",
        "mu_t",
        "nikitin",
    );
    for p in &points {
        let detent = match p.step {
            0 => "none",
            1 => "wide",
            _ => "tight",
        };
        println!(
            "  F{gear} {detent:>6} {kappa:>7.4} {r_cmd:>8.2} {r_achv:>9.2} {v:>7.3} {rpm:>7.0} \
{d_cmd:>7.3} {d_actual:>7.3} {yaw:>9.4} {eta:>6.3} {loss:>6.3} {fo:>10.0} {fi:>10.0} \
{moment:>9.1} {mu_t:>7.3} {nikitin:>7.3}",
            gear = p.gear,
            kappa = p.kappa,
            r_cmd = p.r_cmd,
            r_achv = p.r_achieved,
            v = p.speed,
            rpm = p.rpm,
            d_cmd = p.d_cmd,
            d_actual = p.d_actual,
            yaw = p.yaw,
            eta = p.eta,
            loss = 1.0 - p.eta,
            fo = p.reaction[0],
            fi = p.reaction[1],
            moment = p.yaw_moment / 1000.0,
            mu_t = p.mu_t_measured,
            nikitin = p.mu_t_nikitin,
        );
    }
    println!("\nper-point detail:");
    for p in &points {
        println!(
            "  target F{target} steer {steer:.2} -> settled F{gear} step {step}, belts \
             L {bl:.3} / R {br:.3} m/s, inner force {fi:.0} N ({sign}) -> {path}",
            target = p.target_gear,
            steer = p.steer_cmd,
            gear = p.gear,
            step = p.step,
            bl = p.belts[0],
            br = p.belts[1],
            fi = p.reaction[1],
            sign = if p.reaction[1] >= 0.0 {
                "DRIVING"
            } else {
                "braking"
            },
            path = p.path.display(),
        );
        for note in &p.notes {
            println!("      ! {note}");
        }
    }
}

// --- Descent-behavior probes ---------------------------------------------------------------------
//
// The rising motoring curve + the overrun gear hold give each gear a natural downhill
// equilibrium; the signed-intent contract flows a held S through the stop into reverse; the
// parking latch owns standstill. These probes are the slice's evidence: the per-gear
// equilibrium table, the through-zero flow (with the reverse-ladder boundary-cycle assert
// from the 2026-07-26 field report), and the latch hold/release. Same doctrine as the feel
// probes above: TankCommand + initial pose are the only writes, all derived numbers come
// through the production laws, and the CSVs land beside the others.

/// Signed forward proxy for the descent probes: the mean belt speed (positive = rolling the
/// F-ladder's way). The hull `speed` column is unsigned; the belts carry the sign.
fn mean_belt(sample: &ProbeSample) -> f32 {
    (sample.belts[0] + sample.belts[1]) / 2.0
}

/// Descent evidence (a): coast on the grade settles at a bounded per-gear equilibrium.
/// Launch downhill under W until the box lands the target gear, release, and coast 30 s:
/// the rising motoring curve balances the grade in mid-ladder gears (higher gear → higher
/// equilibrium speed), low gears over-brake and walk down to the F1 creep seam, and the rpm
/// stays bounded by the max-curve ceiling everywhere (the last-resort shift fires only past
/// it). Prints the equilibrium table.
#[test]
#[ignore = "instrument, not a gate: writes driving telemetry"]
fn probe_descend_equilibrium_gears() {
    let grade = probe_grade(0.10);
    let mut rows = Vec::new();
    for target in [3u8, 4, 5] {
        let mut probe = DriveProbe::open(
            &format!("descend-eq-F{target}-{:.0}pct", grade * 100.0),
            grade,
        );
        probe.pose(grade, 300.0, 0.0, Vec3::NEG_X);
        let mut landed = false;
        for _ in 0..(20 * FIXED_TICKS_PER_SECOND) {
            if probe.tick(1.0, 0.0).gear >= target {
                landed = true;
                break;
            }
        }
        assert!(landed, "the downhill launch never reached F{target}");
        let mut samples = Vec::new();
        for _ in 0..(30 * FIXED_TICKS_PER_SECOND) {
            samples.push(probe.tick(0.0, 0.0));
        }
        // Bounded rpm: the over-rev slip guard's end-of-tick clamp is an EXACT bound on
        // the raw crank state (ω units) — the crank may touch the ceiling (that fires
        // the last resort) and ride at most to `max_curve + OVERREV margin`, never past
        // it. The 0.01 rpm slack covers only the rad/s→rpm float round-trip, not physics.
        let guard = probe.tp.max_curve_rpm() + crate::track::transmission::OVERREV_MARGIN_RPM;
        let max_rpm = samples.iter().fold(0.0f32, |a, s| a.max(s.rpm));
        assert!(
            max_rpm <= guard + 0.01,
            "F{target} coast: crank ran away past the over-rev guard point \
             ({max_rpm:.0} vs {guard:.0} rpm)"
        );
        let tail = &samples[samples.len() - 2 * FIXED_TICKS_PER_SECOND..];
        let mean = |f: fn(&ProbeSample) -> f32| tail.iter().map(f).sum::<f32>() / tail.len() as f32;
        let (speed, rpm) = (mean(mean_belt), mean(|s| s.rpm));
        let (lo, hi) = tail
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |a, s| {
                (a.0.min(mean_belt(s)), a.1.max(mean_belt(s)))
            });
        // Bounded equilibrium: the settled window stays inside a narrow speed band (the F1
        // creep seam's clutch-hysteresis cycle is the widest legal band) and inside the
        // gearing-implied top speed.
        assert!(
            hi - lo < 0.3 && hi <= probe.tp.geared_top_speed(),
            "F{target} coast never settled (tail band {lo:.2}..{hi:.2} m/s)"
        );
        let last = tail.last().expect("tail has samples");
        let settled_gear = last.gear;
        assert!(
            tail.iter().all(|s| s.gear == settled_gear),
            "F{target} coast: gear still changing in the settled window"
        );
        rows.push((target, settled_gear, speed, rpm, probe.finish()));
    }
    println!("\ndescent equilibrium — 10% grade, coast from gear (release at landing):");
    println!(
        "  {:>9} {:>12} {:>11} {:>9}",
        "released", "settled gear", "speed m/s", "rpm"
    );
    for (target, gear, speed, rpm, path) in &rows {
        println!(
            "  {:>9} {:>12} {:>11.2} {:>9.0}   {}",
            format!("F{target}"),
            format!("F{gear}"),
            speed,
            rpm,
            path.display()
        );
    }
}

/// Descent evidence (b) + the 2026-07-26 field findings: hold S while descending. The tank
/// must brake to a stop with no free-roll gap (the swap window keeps braking — the old seam
/// released the brakes at the ladder swap and re-accelerated downhill), flow through zero
/// into reverse WITHOUT a re-press, and back up the slope steadily — the reverse-ladder
/// boundary limit cycle (R1→R2→R3→R1, same attractor as the measured F5↔F6 climb cycle)
/// must be gone: no gear revisited in the steady tail.
#[test]
#[ignore = "instrument, not a gate: writes driving telemetry"]
fn probe_descend_s_hold_flows_into_reverse_climb() {
    let grade = probe_grade(0.10);
    let mut probe = DriveProbe::open(&format!("descend-s-hold-{:.0}pct", grade * 100.0), grade);
    probe.pose(grade, 300.0, 0.0, Vec3::NEG_X);
    // Phase 1: the descend probe's own 4 s W launch downhill.
    for _ in 0..(4 * FIXED_TICKS_PER_SECOND) {
        probe.tick(1.0, 0.0);
    }
    // Phase 2: S held for 40 s — brake, cross zero, climb back up in reverse.
    let mut samples = Vec::new();
    for _ in 0..(40 * FIXED_TICKS_PER_SECOND) {
        samples.push(probe.tick(-1.0, 0.0));
    }

    // The command shaper slews +1 → −1 over 0.5 s; the drivetrain's own contract starts
    // once the brake command is actually in.
    let slew = FIXED_TICKS_PER_SECOND / 2;
    let crossing = samples
        .iter()
        .position(|s| s.fwd_speed < 0.0)
        .expect("held S must carry the tank through zero into reverse");
    assert!(crossing >= slew, "crossing cannot precede the command slew");
    // No free-roll gap: while still rolling forward under the settled S command, the HULL
    // never re-accelerates downhill (beyond suspension-pitch float noise) — including
    // through the swap window, which used to carry neither drive nor brake. The belts are
    // deliberately not the signal here: they chatter tick-to-tick under saturated braking
    // (stop-force vs grip shear), which is slip physics, not a free roll.
    let mut max_step = 0.0f32;
    for pair in samples[slew..=crossing].windows(2) {
        let dv = pair[1].fwd_speed - pair[0].fwd_speed;
        max_step = max_step.max(dv);
        assert!(
            dv <= 0.01,
            "free-roll gap: the hull re-accelerated {dv:.4} m/s in one tick while \
             braking downhill toward the crossing"
        );
    }
    // The swap engages the reverse ladder before the crossing completes and never flaps.
    let swap = samples
        .iter()
        .position(|s| s.reverse)
        .expect("the held S must engage the reverse ladder");
    assert!(
        samples[swap..].iter().all(|s| s.reverse),
        "the reverse ladder must stay engaged once swapped"
    );
    // No gear churn at the crossing: the braking-chain downshifts before it and the launch
    // upshifts after it are each legitimate, but no (ladder, gear) may be REVISITED inside
    // ±1 s — a revisit is the flapping the crossing must not have.
    let window = &samples
        [crossing.saturating_sub(FIXED_TICKS_PER_SECOND)..crossing + FIXED_TICKS_PER_SECOND];
    let mut crossing_seen: Vec<(bool, u8)> = Vec::new();
    for s in window {
        let state = (s.reverse, s.gear);
        if crossing_seen.last() != Some(&state) {
            assert!(
                !crossing_seen.contains(&state),
                "gear churn at the zero crossing: {}{} revisited (sequence {:?})",
                if state.0 { 'R' } else { 'F' },
                state.1,
                crossing_seen
            );
            crossing_seen.push(state);
        }
    }
    // Steady reverse climb: the field-reported boundary cycle means a gear REVISITED in the
    // tail. Assert every settled-tail transition lands a fresh gear.
    let tail = &samples[samples.len() - 15 * FIXED_TICKS_PER_SECOND..];
    let transitions = gear_transitions(tail);
    let mut seen = Vec::new();
    for &(_, gear) in &transitions {
        assert!(
            !seen.contains(&gear),
            "reverse-ladder boundary cycle: gear R{gear} revisited in the steady tail \
             (transitions {transitions:?})"
        );
        seen.push(gear);
    }
    let last = samples.last().expect("phase 2 recorded ticks");
    assert!(
        last.fwd_speed < -0.2,
        "the tank must be backing up the slope at the end (fwd {:.3})",
        last.fwd_speed
    );
    println!(
        "probe descend S-hold {grade_pct:.1}% -> {path}\n  \
         crossing at tick {crossing} ({t_cross:.2} s after S), swap at {swap}, max forward \
         re-accel step {max_step:.4} m/s/tick\n  \
         phase-2 gear trace (tick, gear): {trace:?}\n  \
         final: mean belt {belt:.2} m/s, gear R{gear}, rpm {rpm:.0}, x {x:.1}",
        grade_pct = grade * 100.0,
        t_cross = crossing as f32 / FIXED_TICKS_PER_SECOND as f32,
        trace = gear_transitions(&samples),
        belt = mean_belt(last),
        gear = last.gear,
        rpm = last.rpm,
        x = last.position.x,
        path = probe.finish().display(),
    );
}

/// Descent evidence (c): the parking latch holds the grade and releases cleanly. Settled at
/// standstill on the 10% grade under zero input, the latch must be engaged and hold the
/// hull for 600 ticks with centimeter-class drift; the first W tick releases it and the
/// tank pulls away uphill with no residual brake drag.
#[test]
#[ignore = "instrument, not a gate: writes driving telemetry"]
fn probe_park_latch_10pct() {
    let grade = probe_grade(0.10);
    let mut probe = DriveProbe::open(&format!("park-latch-{:.0}pct", grade * 100.0), grade);
    // Facing uphill; `pose` already settles 256 zero-input ticks, which latches the park.
    probe.pose(grade, 0.0, 0.0, Vec3::X);
    // Phase 1: 600 held ticks at zero input — the latch must hold the grade.
    let mut hold = Vec::new();
    for _ in 0..600 {
        hold.push(probe.tick(0.0, 0.0));
    }
    let first = hold.first().expect("hold recorded ticks");
    let last = hold.last().expect("hold recorded ticks");
    assert!(
        hold.iter().all(|s| s.park),
        "the parking latch must stay engaged through the whole zero-input hold"
    );
    let drift = (last.position - first.position).length();
    assert!(
        drift < 0.03,
        "the latch must hold the 10% grade (drifted {drift:.4} m over 600 ticks)"
    );
    // Phase 2: hold W — the latch releases on the first shaped-command tick and the tank
    // pulls uphill promptly (residual brake drag would show up as a failure to launch).
    let mut launch = Vec::new();
    for _ in 0..(3 * FIXED_TICKS_PER_SECOND) {
        launch.push(probe.tick(1.0, 0.0));
    }
    let release = launch
        .iter()
        .position(|s| !s.park)
        .expect("W must release the parking latch");
    assert!(
        release <= 2,
        "the latch must release within the first shaped ticks of W (took {release})"
    );
    assert!(
        launch[release..].iter().all(|s| !s.park),
        "the latch must not re-engage under held W"
    );
    let to_speed = launch.iter().position(|s| mean_belt(s) >= 1.0);
    assert!(
        to_speed.is_some_and(|t| t <= 2 * FIXED_TICKS_PER_SECOND),
        "residual brake drag: the released tank never reached 1 m/s uphill within 2 s \
         (got {:?})",
        to_speed
    );
    let end = launch.last().expect("launch recorded ticks");
    println!(
        "probe park latch {grade_pct:.1}% -> {path}\n  \
         hold: drift {drift:.4} m over 600 ticks, park held\n  \
         release at W tick {release}; 1 m/s at tick {t1:?}; end speed {speed:.2} m/s, \
         gear F{gear}, rpm {rpm:.0}",
        grade_pct = grade * 100.0,
        t1 = to_speed,
        speed = mean_belt(end),
        gear = end.gear,
        rpm = end.rpm,
        path = probe.finish().display(),
    );
}

/// Field regression (Yan, rescaled 1.7×-steeper world): driving downhill the crank
/// followed the belt to ~9000 rpm — three times its mechanical limit. Steep descents at
/// 25% and 35%, BOTH facings: forward coast (the ceiling-rescue ladder walk is the
/// containment) and backing down under held S (reverse-ladder territory: R4 tops out and
/// the over-rev slip guard is the ONLY bound). Post-fix the SUSTAINED crank must stay at
/// or below the guard point `max_curve_rpm + OVERREV_MARGIN_RPM`; brief transients above
/// the CEILING during paid windows are physical and bounded by the same guard.
#[test]
#[ignore = "instrument, not a gate: writes driving telemetry"]
fn probe_steep_descent_crank_bound() {
    let mut rows = Vec::new();
    for grade in [0.25f32, 0.35] {
        for reverse in [false, true] {
            let facing = if reverse { "reverse" } else { "forward" };
            let mut probe =
                DriveProbe::open(&format!("steep-{:.0}pct-{facing}", grade * 100.0), grade);
            let guard_rpm =
                probe.tp.max_curve_rpm() + crate::track::transmission::OVERREV_MARGIN_RPM;
            let mut samples = Vec::new();
            if reverse {
                // Facing uphill; held S backs the tank down the slope on the R ladder.
                probe.pose(grade, 400.0, 0.0, Vec3::X);
                for _ in 0..(25 * FIXED_TICKS_PER_SECOND) {
                    samples.push(probe.tick(-1.0, 0.0));
                }
            } else {
                // Facing downhill; a short W launch releases the park latch, then coast.
                probe.pose(grade, 400.0, 0.0, Vec3::NEG_X);
                for _ in 0..(2 * FIXED_TICKS_PER_SECOND) {
                    probe.tick(1.0, 0.0);
                }
                for _ in 0..(23 * FIXED_TICKS_PER_SECOND) {
                    samples.push(probe.tick(0.0, 0.0));
                }
            }
            let max_rpm = samples.iter().fold(0.0f32, |a, s| a.max(s.rpm));
            // The guard's end-of-tick clamp is an EXACT bound on the raw crank state (ω
            // units); 0.01 rpm covers only the rad/s→rpm float round-trip.
            assert!(
                max_rpm <= guard_rpm + 0.01,
                "steep {facing} {:.0}%: crank hit {max_rpm:.0} rpm past the guard point \
                 {guard_rpm:.0} — the field 9000-rpm runaway is back",
                grade * 100.0
            );
            let last = *samples.last().expect("descent recorded ticks");
            rows.push((
                grade,
                facing,
                gear_transitions(&samples),
                max_rpm,
                last,
                probe.finish(),
            ));
        }
    }
    println!("\nsteep-descent crank bound — guard = curve top + over-rev margin:");
    for (grade, facing, trace, max_rpm, last, path) in &rows {
        println!(
            "  {:>3.0}% {facing:>7}: max rpm {max_rpm:.0}, final {gear}{g} @ {rpm:.0} rpm, \
             fwd {fwd:.2} m/s\n      gear trace {trace:?}\n      {p}",
            grade * 100.0,
            gear = if last.reverse { 'R' } else { 'F' },
            g = last.gear,
            rpm = last.rpm,
            fwd = last.fwd_speed,
            p = path.display(),
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
