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
use crate::tank::{Controlled, PendingTankAssets, TANK_GLB_PATH, Tank};

/// Resolution: backstop for async asset/state polling after synchronous Startup admission,
/// never a performance budget. A missing gate fails with its asset diagnosis rather than
/// waiting until the CI timeout. Bounded construction preparation is synchronous work.
const BOOT_DEADLINE: Duration = Duration::from_secs(60);

/// The simulation clock configured by the netcode composition. Fixtures advance it by one exact
/// fixed loop per [`App::update`], so every bound and elapsed-time report is stated in ticks.
const FIXED_TICKS_PER_SECOND: usize = 64;

pub(crate) fn start_fixed_clock(app: &mut App) {
    // Verified against Bevy 0.19: one `App::update` runs exactly one fixed loop.
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
}

/// Serializes full-app fixtures. The lease spans each test because booting and running apps compete
/// for the same host resources; mutex poisoning is irrelevant to this external resource.
pub(crate) static BOOT_LEASE: Mutex<()> = Mutex::new(());

fn assert_tank_state_at_add(
    add: On<Add, Tank>,
    tanks: Query<(
        Has<TankCommand>,
        Has<crate::track::sim::TrackDrive>,
        Option<&crate::track::sim::TrackGripElements>,
        (
            Has<avian3d::prelude::TranslationInterpolation>,
            Has<avian3d::prelude::RotationInterpolation>,
        ),
    )>,
    blueprint: Option<Res<TankBlueprint>>,
) {
    let (command, drive, elements, eased) = tanks
        .get(add.entity)
        .expect("a newly added Tank must still exist during its observer");
    assert!(
        command && drive,
        "TankCommand and TrackDrive must exist in the same insertion that adds Tank",
    );
    // The offline physics eases only the bodies that opt in (`crate::offline_physics`): a scenario
    // tank is born an `eased_dynamic_body`, or the drawn hull steps at the fixed rate.
    assert_eq!(
        eased,
        (true, true),
        "a scenario tank must be born with translation and rotation easing",
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
/// `ballistics` is composed as the DEDICATED SERVER composes it, without the view half: a shell is
/// born bare, with no scene root, no streak and no `Visibility`.
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
    // this exercises the single-player-shaped boot, headless. Without render easing: the scenario's
    // tanks still carry their easing components, which no system reads here, so every reader sees
    // the fixed-step pose the gates are stated in.
    app.add_plugins((
        crate::offline_physics().disable::<avian3d::prelude::PhysicsInterpolationPlugin>(),
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
pub(crate) fn headless_shell() -> App {
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
    // DELETE THESE LINES when `src/tests/bevy_ktx2_uastc_fallback.rs` fails — that failure IS the
    // signal that bevy fixed the transcode and the workaround has become dead weight.
    app.insert_resource(bevy::image::CompressedImageFormatSupport(
        bevy::image::CompressedImageFormats::ASTC_LDR,
    ));
    app
}

/// `App::run` normally drives plugin finish/cleanup (some registration — e.g. Avian's diagnostics
/// resources — happens in `Plugin::finish`); a bare `update()` loop must do it.
pub(crate) fn finish_plugins(app: &mut App) {
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
    let glb = crate::assets::asset_root().join(TANK_GLB_PATH);
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
    booted_sim_on_map(world, None)
}

/// Analytical driving worlds declare an empty placement as well as their surface. Unrelated
/// construction dynamics cannot be a premise of a brake, traction or drivetrain witness.
/// The populated-map composition gates select their own placement through `booted_sim_on`.
fn booted_driving_grid(grid: crate::terrain_grid::HeightGrid) -> BootedSim {
    let extent = grid.extent();
    let manifest = crate::map::tests::fixture_manifest(
        extent.half_extent(),
        extent.height_offset_m,
        extent.height_span_m,
    );
    booted_sim_on_map(Some(grid), Some(manifest))
}

/// [`booted_sim_on`] with the map's placement chosen too: a manifest inserted before the first
/// update is what `terrain_grid::decode_height_grid` publishes in place of the shipped file.
fn booted_sim_on_map(
    world: Option<crate::terrain_grid::HeightGrid>,
    manifest: Option<crate::map::MapManifest>,
) -> BootedSim {
    let lease = BOOT_LEASE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut app = headless_app_on(world);
    if let Some(manifest) = manifest {
        app.insert_resource(manifest);
    }

    // Asset IO is genuinely async on wall-clock IO threads (the spec RON + tiger_1.glb), so poll
    // until the spawn gate opens and the app enters Playing. Each not-yet-Playing pass yields 1 ms
    // to those IO threads: a bare CPU-bound spin starves them. The sleep is WALL-CLOCK only — the
    // clock is `ManualDuration(ZERO)` here, so no sim tick advances and the frozen-load invariant
    // above holds untouched.
    // Startup admits construction data synchronously; the watchdog measures the subsequent
    // async asset/state wait, with the simulation clock still frozen.
    app.update();
    let started = Instant::now();
    loop {
        if *app.world().resource::<State<AppState>>().get() == AppState::Playing {
            break;
        }
        let elapsed = started.elapsed();
        assert!(elapsed < BOOT_DEADLINE, "{}", boot_diagnosis(&app, elapsed));
        std::thread::sleep(Duration::from_millis(1));
        app.update();
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

/// The round-dressing law of `ballistics::view_plugin`'s `Add` observer, on rounds spawned bare into
/// a host that mounts the view half alone, each dressed from its authored visuals and never from
/// its calibre (FIXTURE DATA: every round flies at one calibre):
///   * a round authoring a body is dressed with `ShellVisual` + its model's scene root
///     (`WorldAssetRoot`), whether or not its belt marks it a tracer;
///   * a tracer round authoring a streak streams a `TracerStreak` child and gets NO scene root;
///   * the same streak-authoring round without a tracer gets neither.
///
/// The body arm is load-bearing: without it the "no scene root" assertions could be satisfied by a
/// view plugin that dresses nothing. [`a_server_composition_dresses_no_round_at_all`] is the gate
/// for the server side, which mounts no view at all.
#[test]
fn a_round_is_dressed_from_its_authored_body_and_streak() {
    use crate::ballistics::{Projectile, ShellPath, ShellVisual, TracerRound, TracerStreak};
    use crate::rounds::{BodyModel, FlightEffects, StreakEmission};
    use bevy::world_serialization::WorldAssetRoot;

    let mut app = headless_shell();
    app.add_plugins(crate::ballistics::view_plugin);
    finish_plugins(&mut app);
    // `Startup` builds the shell scene handle and the streak mesh/material the observer clones.
    app.update();

    let streaked_round = FlightEffects {
        streak: Some(StreakEmission {
            radius_m: 0.02,
            length_per_speed_s: 0.02,
            min_length_m: 2.0,
            emissive: [1.0, 1.0, 1.0],
        }),
        ..FlightEffects::NONE
    };
    let bodied_round = FlightEffects {
        body: Some(&BodyModel::shipped()[0]),
        ..FlightEffects::NONE
    };
    let round = |effects: FlightEffects| {
        (
            Projectile::view_test_round(0.05),
            effects,
            Transform::default(),
            ShellPath::default(),
        )
    };
    let world = app.world_mut();
    let main_gun = world.spawn((round(bodied_round), TracerRound)).id();
    let tracer = world.spawn((round(streaked_round), TracerRound)).id();
    let plain = world.spawn(round(streaked_round)).id();
    app.update();

    let world = app.world_mut();
    let streaked: Vec<Entity> = world
        .query_filtered::<&ChildOf, With<TracerStreak>>()
        .iter(world)
        .map(ChildOf::parent)
        .collect();
    let world = app.world();
    let scene = |round: Entity| world.get::<WorldAssetRoot>(round).is_some();
    assert_eq!(
        streaked,
        vec![tracer],
        "the streak-authoring tracer round, and only it, must stream a TracerStreak",
    );
    assert!(
        !scene(tracer) && !scene(plain),
        "a round authoring no body spawned a scene root (WorldAssetRoot)",
    );
    assert!(
        world.get::<ShellVisual>(main_gun).is_some() && scene(main_gun),
        "a body-authoring round was never dressed — it must get ShellVisual + its model's scene \
         root from ballistics::view_plugin",
    );
}

/// The game's damage-model composition gate samples projectile births, frame ends and removals.
/// Deferred dressing is visible either on the surviving round or before its removal.
#[derive(Resource, Default)]
struct ServerRoundSamples {
    seen: bool,
    dressed: Vec<&'static str>,
}

impl ServerRoundSamples {
    fn note(&mut self, facets: (bool, bool, bool, bool, bool)) {
        self.seen = true;
        let (scene, mesh, material, visibility, marker) = facets;
        for (present, name) in [
            (scene, "WorldAssetRoot"),
            (mesh, "Mesh3d"),
            (material, "MeshMaterial3d"),
            (visibility, "Visibility"),
            (marker, "ShellVisual"),
        ] {
            if present && !self.dressed.contains(&name) {
                self.dressed.push(name);
            }
        }
    }
}

type ServerRoundFacets = (
    Has<bevy::world_serialization::WorldAssetRoot>,
    Has<Mesh3d>,
    Has<MeshMaterial3d<StandardMaterial>>,
    Has<Visibility>,
    Has<crate::ballistics::ShellVisual>,
);

type ServerRoundViews<'w, 's> = Query<'w, 's, ServerRoundFacets>;

fn sample_server_round_frame(
    views: Query<ServerRoundFacets, With<crate::ballistics::Projectile>>,
    mut samples: ResMut<ServerRoundSamples>,
) {
    for facets in &views {
        samples.note(facets);
    }
}

fn sample_server_round_birth(
    born: On<Add, crate::ballistics::Projectile>,
    views: ServerRoundViews,
    mut samples: ResMut<ServerRoundSamples>,
) {
    samples.note(
        views
            .get(born.entity)
            .expect("a projectile birth is sampled before it can despawn"),
    );
}

fn sample_server_round_removal(
    gone: On<Remove, crate::ballistics::Projectile>,
    views: ServerRoundViews,
    mut samples: ResMut<ServerRoundSamples>,
) {
    samples.note(
        views
            .get(gone.entity)
            .expect("a projectile removal observer reads its live entity"),
    );
}

/// The game's damage model: a server composition fires through the real spawn path and carries
/// no render components. The invariant is independent of calibre, weapon count and how long a
/// round survives: the birth/removal observers also see a round stopping in its first tick.
#[test]
fn a_server_composition_dresses_no_round_at_all() {
    let mut app = booted_sp_app();
    app.init_resource::<ServerRoundSamples>()
        .add_observer(sample_server_round_birth)
        .add_observer(sample_server_round_removal)
        .add_systems(Last, sample_server_round_frame);
    let world = app.world_mut();
    let tank = world
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>()
        .single(world)
        .expect("one controlled tank");
    // FIXTURE DATA: a bounded wait for the manual clock to consume the command. Weapons spawn
    // loaded (WeaponGateState::for_cycle); this is no per-weapon reload or calibre requirement.
    for _ in 0..200 {
        if app.world().resource::<ServerRoundSamples>().seen {
            break;
        }
        let mut command = app
            .world_mut()
            .get_mut::<TankCommand>(tank)
            .expect("tank carries its command");
        command.fire_primary = true;
        command.fire_secondary = true;
        app.update();
    }
    let samples = app.world().resource::<ServerRoundSamples>();
    assert!(
        samples.seen,
        "the server composition emitted no round, so the gate would be vacuous"
    );
    assert!(
        samples.dressed.is_empty(),
        "a server-composed round carried render state: {:?}",
        samples.dressed
    );
    let world = app.world_mut();
    assert_eq!(
        world
            .query_filtered::<Entity, With<crate::ballistics::TracerStreak>>()
            .iter(world)
            .count(),
        0,
        "a server composition spawned a tracer child"
    );
}

/// The game's damage-model composition gate must catch deferred dressing even when its round
/// survives the frame. Birth alone can precede another observer's component insertion.
#[test]
fn the_server_round_gate_catches_dressing_after_birth() {
    let mut app = App::new();
    app.init_resource::<ServerRoundSamples>()
        .add_observer(sample_server_round_birth)
        .add_observer(sample_server_round_removal)
        .add_systems(Last, sample_server_round_frame);
    // FIXTURE DATA: an arbitrary undrawn round; its diameter does not select dressing.
    let round = app
        .world_mut()
        .spawn(crate::ballistics::Projectile::view_test_round(0.05))
        .id();
    assert!(
        app.world()
            .resource::<ServerRoundSamples>()
            .dressed
            .is_empty()
    );
    app.world_mut()
        .entity_mut(round)
        .insert(Visibility::Inherited);
    app.update();
    assert!(
        app.world()
            .resource::<ServerRoundSamples>()
            .dressed
            .contains(&"Visibility"),
        "the gate must see late dressing on a surviving round"
    );
}

// Full-app driving gates mount the shipped blueprint through the offline composition.

/// [`booted_sim`] on the vehicle's declared transmission, clock started, tracks grounded and
/// settled. Returns the sim and the controlled tank.
fn booted_offline_sim() -> (BootedSim, Entity) {
    let mut app = booted_sim();
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
        .trans();
    crate::track::sim::TankTransmission::from_spec(params)
}

/// Construction pressure used by expected rolling loads in the full-app fixtures.
fn nominal_pressure(app: &App) -> f32 {
    crate::track::rig_geom::tank_ground_pressure_kpa(
        app.world().resource::<crate::track::rig_geom::RigGeom>(),
    )
}

/// Write the drive command and advance one exact fixed tick; this harness has no device gather.
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

/// A neutral pivot must counter-rotate its belts and deliver an ASSUMED class acceptance
/// bound: at least half of the commanded belt-kinematic yaw reaches the ground after settling.
/// The ceiling derives from the vehicle's own neutral differential and measured half-tread;
/// driving relies on the steering capacity overcoming the footprint's scrub.
fn neutral_pivot_gate(app: &mut App, tank: Entity) {
    let gear = app.world().resource::<crate::track::sim::TrackGear>();
    let half_tread = app
        .world()
        .resource::<crate::track::rig_geom::RigGeom>()
        .plane_x;
    let ceiling = gear.trans().neutral_d_full / half_tread;
    let min_yaw = 0.5 * ceiling;
    let mut yaw_sum = 0.0f32;
    let mut samples = 0u32;
    for tick in 0..320 {
        drive_tick(app, tank, 0.0, 1.0);
        if tick >= 256 {
            yaw_sum += yaw_rate(app, tank);
            samples += 1;
        }
    }
    let mean_yaw = yaw_sum / samples as f32;
    let drive = app
        .world()
        .get::<crate::track::sim::TrackDrive>(tank)
        .expect("tank drives");
    let (l, r) = (drive.sides[0].speed, drive.sides[1].speed);
    println!(
        "pivot: mean yaw {mean_yaw:.4} rad/s, kinematic ceiling {ceiling:.4}, ratio {:.4}, belts L {l:.3} / R {r:.3}",
        mean_yaw.abs() / ceiling
    );
    assert!(
        l * r < 0.0,
        "a neutral pivot must counter-rotate the belts (L {l:.3}, R {r:.3})"
    );
    assert!(
        mean_yaw.abs() >= min_yaw,
        "pivot yaw {mean_yaw:.4} rad/s under full steer — gate ≥ {min_yaw} rad/s"
    );
}

/// The declared transmission's neutral turn clears the class yaw bound in the game's
/// authoritative simulation. FIXTURE DATA: the shipped vehicle supplies the geometry and tables;
/// no measured yaw of that vehicle defines the pass condition.
#[test]
fn pivot_clears_its_own_kinematic_yaw_floor() {
    let (mut app, tank) = booted_offline_sim();
    neutral_pivot_gate(&mut app, tank);
}

/// Full pedal on firm level ground must reach the declared ladder top one gear at a time.
/// Fixture: the shipped construction, 20 simulated seconds; the Driver relies on no hunting.
#[test]
fn full_pedal_climbs_the_declared_ladder_gear_by_gear() {
    let (mut app, tank) = booted_offline_sim();
    face_positive_z(&mut app, tank);
    let top = app
        .world()
        .resource::<crate::track::sim::TrackGear>()
        .trans()
        .gears_fwd
        .len() as u8;
    let mut trace = Vec::new();
    for _ in 0..20 * FIXED_TICKS_PER_SECOND {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let state = app
            .world()
            .get::<crate::track::sim::TankTransmission>(tank)
            .unwrap()
            .0;
        assert!(
            !state.reverse,
            "forward intent must retain the forward ladder"
        );
        if trace.last() != Some(&state.gear) {
            trace.push(state.gear);
        }
    }
    let first = *trace
        .first()
        .expect("the full-pedal fixture samples at least one tick");
    println!("MEASURED shipped construction gear walk {trace:?}");
    assert_eq!(
        trace,
        (first..=top).collect::<Vec<_>>(),
        "full pedal must climb every gear through the declared top"
    );
}

/// No authored speed cap bounds the game's tanks: the top gear at the fuel governor does. Full
/// throttle on lossless flat ground must reach the ladder's top gear and settle within its
/// governor ramp: construction drag balances fuel before the complete cut. FIXTURE acceptance:
/// 1 % below governed for settling and 0.1 % above cut-off for integration and ride; neither
/// margin represents rolling loss. A
/// governor or ladder mutant that lets the hull run away, or stalls it short of top gear, fails.
/// The ground takes no rolling resistance, so the governor and the ladder are what this measures:
/// on a ground that does, the ground's resistance may hold the tank below its governed speed
/// (design doc, `track-model/architecture.md` §5a). FIXTURE DATA: the base pack's friction, no
/// rolling resistance.
#[test]
fn full_throttle_reaches_and_holds_the_governed_top_speed() {
    let (mut app, tank) = booted_offline_sim();
    let base = crate::ground_types::GroundTypes::load_shipped()
        .get(crate::terrain_blend::BASE_GROUND)
        .contact();
    let road = crate::ground_field::ContactGround {
        // Fixture: invert the pressure law to make this governor witness lossless.
        rolling_resistance: crate::track::forces::rigid_rolling_resistance()
            * (1.0 - crate::track::forces::rolling_reference_kpa() / nominal_pressure(&app)),
        ..base
    };
    {
        let mut track = app
            .world_mut()
            .resource_mut::<crate::track::terrain::TrackField>();
        let field = track
            .field
            .take()
            .expect("the booted world built its track field");
        track.field =
            Some(field.with_ground(Some(crate::ground_field::GroundField::uniform(road))));
    }
    face_positive_z(&mut app, tank);
    let trans = app
        .world()
        .resource::<crate::track::sim::TrackGear>()
        .trans()
        .clone();
    let governed = trans.governed_speed(false);
    let governed_rpm = trans.engine.governed_rpm;
    let cut_rpm = (0..10_000)
        .map(|step| governed_rpm + step as f32)
        .find(|&rpm| trans.torque_at(rpm) == 0.0)
        .expect("the fuel governor cuts torque to zero past governed rpm");
    let ceiling = governed * cut_rpm / governed_rpm;
    let top = trans.gears_fwd.len() as u8;
    let mut last_second = vec![];
    let ticks = 40 * FIXED_TICKS_PER_SECOND;
    for tick in 0..ticks {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let world = app.world();
        let speed = world
            .get::<avian3d::prelude::LinearVelocity>(tank)
            .expect("tank has linear velocity")
            .0
            .length();
        assert!(
            speed <= ceiling * 1.001,
            "tick {tick}: hull speed {speed:.3} m/s past the governor's torque cut-off \
             {ceiling:.3} m/s"
        );
        if tick >= ticks - FIXED_TICKS_PER_SECOND {
            last_second.push(speed);
        }
    }
    let gear = app
        .world()
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("tank carries transmission state")
        .0
        .gear;
    let mean = last_second.iter().sum::<f32>() / last_second.len() as f32;
    let spread = last_second.iter().copied().fold(f32::MIN, f32::max)
        - last_second.iter().copied().fold(f32::MAX, f32::min);
    println!(
        "full throttle: F{gear}, mean {mean:.3} m/s over the last second (spread {spread:.4}), \
         governed {governed:.3}, cut-off {ceiling:.3}"
    );
    assert_eq!(gear, top, "40 s of full throttle must reach the top gear");
    assert!(
        mean >= 0.99 * governed,
        "top speed {mean:.3} m/s short of the governed {governed:.3} m/s"
    );
    assert!(
        spread <= 0.01 * governed,
        "top speed must hold steady (spread {spread:.4} m/s over the last second)"
    );
}

/// Every ground/entry must reach a supported, moving equilibrium before its gear can be
/// called converged or a C1 vehicle-truth split. Mutants which strand a launch, lose support,
/// exhaust the plane or retain a force deficit fail independently of the selector's answer.
/// Fixture: fresh app per entry, a 90-second traverse inside the uninterrupted flat plane.
#[cfg(feature = "bitprobe")]
#[test]
fn contact_law_shipped_ground_speeds() {
    use crate::track::sim::{TankTransmission, TrackContacts, TrackGripEffect};
    for (name, ground) in crate::ground_types::GroundTypes::load_shipped().types {
        let mut outcomes = Vec::new();
        for rolling in [false, true] {
            let (mut app, tank) = booted_offline_sim();
            let extent = crate::terrain_grid::FIXTURE_EXTENT;
            {
                let mut track = app
                    .world_mut()
                    .resource_mut::<crate::track::terrain::TrackField>();
                let field = track.field.take().expect("boot builds terrain");
                track.field = Some(field.with_ground(Some(
                    crate::ground_field::GroundField::uniform(ground.contact()),
                )));
            }
            let params = app
                .world()
                .resource::<crate::track::sim::TrackGear>()
                .trans();
            let (speed, gear) = if rolling {
                (params.governed_speed(false), params.gears_fwd.len() as u8)
            } else {
                (0.0, 1)
            };
            let rest = app
                .world()
                .resource::<crate::track::rig_geom::RigGeom>()
                .hull_rest_y;
            // Fixture: the maximum governed traverse fits within the 1 km plane from this end.
            let start = Vec3::new(100.0, rest, -480.0);
            drive_load::place(
                &mut app,
                tank,
                start,
                Quat::from_rotation_y(std::f32::consts::PI),
                speed,
                gear,
            );
            let weight =
                app.world().resource::<TankBlueprint>().spec.mass * crate::track::derive::G;
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            let mut sum = 0.0;
            let mut spare_max = 0.0_f32;
            let mut next_max = f32::NEG_INFINITY;
            for tick in 0..90 * FIXED_TICKS_PER_SECOND {
                drive_tick(&mut app, tank, 1.0, 0.0);
                let position = app
                    .world()
                    .get::<avian3d::prelude::Position>(tank)
                    .unwrap()
                    .0;
                assert!(
                    position.x.abs() < extent.half_extent() - 10.0
                        && position.z.abs() < extent.half_extent() - 10.0,
                    "{name} rolling={rolling} must remain on the uninterrupted plane: {position:?}"
                );
                let probe = app
                    .world()
                    .get::<TrackGripEffect>(tank)
                    .unwrap()
                    .transmission_probe;
                assert!(
                    probe.grip_limit_n > 0.0,
                    "{name} rolling={rolling} needs geometric support"
                );
                if tick >= 89 * FIXED_TICKS_PER_SECOND {
                    let contacts = &app.world().get::<TrackContacts>(tank).unwrap().0;
                    assert!(
                        contacts
                            .iter()
                            .all(|side| side.iter().any(|c| c.load_elastic > 0.0))
                    );
                    let speed = app
                        .world()
                        .get::<avian3d::prelude::LinearVelocity>(tank)
                        .unwrap()
                        .0
                        .z;
                    assert!(speed > crate::track::transmission::PARK_ENGAGE_SPEED);
                    let state = &app.world().get::<TankTransmission>(tank).unwrap().0;
                    let construction = app.world().resource::<crate::track::sim::TrackGear>();
                    assert_eq!(state.shift_ticks, 0, "equilibrium cannot be a paid cut");
                    spare_max =
                        spare_max.max(construction.landed_spare(state, probe, state.gear).abs());
                    for higher in state.gear + 1
                        ..=state
                            .gear
                            .saturating_add(construction.trans().max_step)
                            .min(construction.trans().gears_fwd.len() as u8)
                    {
                        next_max = next_max.max(construction.landed_spare(state, probe, higher));
                    }
                    lo = lo.min(speed);
                    hi = hi.max(speed);
                    sum += speed;
                }
            }
            let state = &app.world().get::<TankTransmission>(tank).unwrap().0;
            assert_eq!(state.clutch, crate::track::transmission::ClutchState::In);
            let construction = app.world().resource::<crate::track::sim::TrackGear>();
            assert!(
                state.omega_e / crate::track::transmission::RPM_TO_RAD
                    >= construction.trans().engine.torque_nm[0].0,
                "a sustainable C1 split cannot be an engaged lug"
            );
            let margin =
                crate::track::load::DriverResolution::load().margin_n(weight, construction.trans());
            let mean = sum / FIXED_TICKS_PER_SECOND as f32;
            println!(
                "MEASURED S4 ground={name} rolling={rolling} gear={} mean_m_s={mean:.6} spread_m_s={:.6} abs_current_spare_n={spare_max:.6} best_higher_spare_n={next_max:.6} margin_n={margin:.6}",
                state.gear,
                hi - lo
            );
            assert!(
                hi - lo < 0.01,
                "{name} rolling={rolling} must settle in speed"
            );
            // Fixture observation precision: 0.01% of weight, much smaller than one ground row's load.
            assert!(
                spare_max < weight * 0.0001,
                "{name} rolling={rolling}: equilibrium spare {spare_max} N"
            );
            assert!(
                next_max <= margin,
                "a sustainable better higher gear must be selected"
            );
            let contacts = &app.world().get::<TrackContacts>(tank).unwrap().0;
            let capacity: f32 = contacts
                .iter()
                .flatten()
                .map(|c| c.mu * c.load_elastic)
                .sum();
            let supported_load: f32 = contacts.iter().flatten().map(|c| c.load_elastic).sum();
            let net_pull = capacity - ground.contact().rolling_at(nominal_pressure(&app)) * weight;
            println!(
                "DERIVED ground={name} rolling={rolling} contact_capacity_n={capacity:.3} supported_elastic_n={supported_load:.3} capacity_less_drag_n={net_pull:.3}"
            );
            outcomes.push((state.gear, mean));
        }
        println!(
            "MEASURED S4 {name}: {}",
            if outcomes[0].0 == outcomes[1].0 && (outcomes[0].1 - outcomes[1].1).abs() < 0.01 {
                "converged"
            } else {
                "C1 vehicle-truth split: both entries are sustainable, neither can afford a higher landing"
            }
        );
    }
}

/// The production app must publish the capacity its contact law uses while stopping.
/// FIXTURE: the shipped vehicle on flat mud at 4 m/s, held service brakes, 20 simulated
/// seconds. Drivers rely on recoverable soil travel being bounded by K after any skid.
#[cfg(feature = "bitprobe")]
#[test]
fn contact_law_service_stop_publishes_capacity_and_reports_rollback() {
    use crate::track::sim::{TrackContacts, TrackGripEffect};
    use avian3d::prelude::{LinearVelocity, Position};
    let (mut app, tank) = booted_offline_sim();
    let ground = crate::ground_types::GroundTypes::load_shipped()
        .get("mud")
        .contact();
    {
        let mut track = app
            .world_mut()
            .resource_mut::<crate::track::terrain::TrackField>();
        let field = track.field.take().unwrap();
        track.field =
            Some(field.with_ground(Some(crate::ground_field::GroundField::uniform(ground))));
    }
    let rest = app
        .world()
        .resource::<crate::track::rig_geom::RigGeom>()
        .hull_rest_y;
    let entry_gear = app
        .world()
        .resource::<crate::track::sim::TrackGear>()
        .trans()
        .gears_fwd
        .len() as u8;
    drive_load::place(
        &mut app,
        tank,
        Vec3::new(100.0, rest, 0.0),
        Quat::from_rotation_y(std::f32::consts::PI),
        4.0,
        entry_gear,
    );
    app.add_systems(
        FixedUpdate,
        (|mut commands: Query<&mut crate::command::ParsedTankCommand, With<Controlled>>| {
            for mut command in &mut commands {
                *command = crate::command::ParsedTankCommand::hold();
            }
        })
        .after(crate::command::ParseCommands)
        .before(crate::state::GameplaySet),
    );
    let mut peak = 0.0_f32;
    let mut reverse = 0.0_f32;
    let mut last = 0.0_f32;
    let mut capacity = 0.0_f32;
    for _ in 0..20 * FIXED_TICKS_PER_SECOND {
        app.update();
        let world = app.world();
        last = world.get::<Position>(tank).unwrap().0.z;
        peak = peak.max(last);
        reverse = reverse.min(world.get::<LinearVelocity>(tank).unwrap().0.z);
        let effect = world.get::<TrackGripEffect>(tank).unwrap();
        let expected: f32 = world
            .get::<TrackContacts>(tank)
            .unwrap()
            .0
            .iter()
            .flatten()
            .map(|c| c.mu * c.load_elastic)
            .sum();
        assert!(
            (effect.traction_capacity_n - expected).abs() < 1.0,
            "published capacity must match the force pass"
        );
        capacity = expected;
    }
    // FIXTURE rest observation: below 1 mm/s before comparing recovered travel.
    assert!(
        app.world().get::<LinearVelocity>(tank).unwrap().0.z.abs() < 0.001,
        "the hull must stop before its rebound is read"
    );
    assert!(
        capacity > 0.0,
        "the stopped fixture remains supported on its flat soil"
    );
    println!(
        "MEASURED full-app mud service stop: capacity={capacity:.3} N peak={peak:.6} m rollback={:.6} m reverse={reverse:.6} m/s",
        peak - last
    );
    assert!(
        peak - last <= crate::track::forces::GRIP_SHEAR_FLOOR_M,
        "a stop must not return the skid as a spring: {} m",
        peak - last
    );
}

/// The shipped brakes/transmission must hold below both static limits and move above the
/// lower one. The force pad cannot witness brake breakaway. Fixture: longitudinal tilted
/// planes and fresh parked tank. Hold for 60 seconds; observe sliding for 8 seconds so even
/// free acceleration cannot exhaust the plane. Angle margin is one tenth of the limit.
#[test]
fn contact_law_shipped_brakes_hold_below_both_limits_and_move_above_the_lower() {
    use avian3d::prelude::{LinearVelocity, Position};
    let spec = crate::track::rig_geom::tiger_spec();
    let tr = &spec.track.powertrain.transmission;
    let weight = spec.mass * crate::track::derive::G;
    let brake_limit = (2.0 * tr.brake_force * tr.brake_static_factor / weight)
        .min(1.0)
        .asin();
    for (name, ground) in crate::ground_types::GroundTypes::load_shipped().types {
        let ground_limit = ground.grip.value.atan();
        let limit = ground_limit.min(brake_limit);
        for above in [false, true] {
            if limit == 0.0 && !above {
                continue; // Zero grip has no positive holding-slope interval.
            }
            let margin = if limit > 0.0 {
                (limit * 0.1).min((std::f32::consts::FRAC_PI_2 - limit) * 0.5)
            } else {
                1.0_f32.to_radians() // Fixture sliding slope when no static interval exists.
            };
            let angle = limit + if above { margin } else { -margin };
            let extent = crate::terrain_grid::FIXTURE_EXTENT;
            let height = extent.half_extent() * angle.tan();
            let grid = crate::terrain_grid::HeightGrid::new(
                vec![height, height, -height, -height].into(),
                2,
                extent,
            );
            let mut app = booted_driving_grid(grid);
            start_fixed_clock(&mut app);
            app.update();
            let tank = app
                .world_mut()
                .query_filtered::<Entity, (With<Tank>, With<Controlled>)>()
                .single(app.world())
                .expect("one controlled tank");
            {
                let mut track = app
                    .world_mut()
                    .resource_mut::<crate::track::terrain::TrackField>();
                let field = track.field.take().expect("boot builds terrain");
                track.field = Some(field.with_ground(Some(
                    crate::ground_field::GroundField::uniform(ground.contact()),
                )));
            }
            let rotation = Quat::from_rotation_x(angle);
            let downhill = rotation * Vec3::Z;
            let rest = app
                .world()
                .resource::<crate::track::rig_geom::RigGeom>()
                .hull_rest_y;
            let start = Vec3::new(100.0, 0.0, 0.0) + rotation * Vec3::Y * rest;
            drive_load::place(&mut app, tank, start, rotation, 0.0, 1);
            let (mut last, mut late_start, mut late_max_speed) = (0.0, 0.0, 0.0_f32);
            let seconds = if above { 8 } else { 60 }; // Fixture horizons, bounded by plane extent.
            for tick in 0..seconds * FIXED_TICKS_PER_SECOND {
                drive_tick(&mut app, tank, 0.0, 0.0);
                last = (app.world().get::<Position>(tank).unwrap().0 - start).dot(downhill);
                if tick == (seconds - 1) * FIXED_TICKS_PER_SECOND - 1 {
                    late_start = last;
                }
                if tick >= (seconds - 1) * FIXED_TICKS_PER_SECOND {
                    late_max_speed = late_max_speed.max(
                        app.world()
                            .get::<LinearVelocity>(tank)
                            .unwrap()
                            .0
                            .dot(downhill)
                            .abs(),
                    );
                }
            }
            // Resolution: 0.1% of a tick's gravitational speed permits force/pose roundoff.
            // A persistent 1 m/min drift is over two orders of magnitude larger.
            let speed_precision = crate::track::derive::G / FIXED_TICKS_PER_SECOND as f32 * 1e-3;
            let travel = last - late_start; // Last second, so the same precision has units m.
            println!(
                "MEASURED full-app slope ground={name} above={above} angle_deg={:.6} grip_limit_deg={:.6} brake_limit_deg={:.6} binding={} settle_m={last:.8} late_travel_m={travel:.8} late_speed_m_s={late_max_speed:.8}",
                angle.to_degrees(),
                ground_limit.to_degrees(),
                brake_limit.to_degrees(),
                if ground_limit < brake_limit {
                    "ground"
                } else {
                    "brake"
                }
            );
            if above {
                assert!(
                    travel > speed_precision && late_max_speed > speed_precision,
                    "{name}: above the lower static limit must keep moving"
                );
            } else {
                assert!(
                    travel.abs() < speed_precision && late_max_speed < speed_precision,
                    "{name}: below both limits must stop and stay, travel={travel}, speed={late_max_speed}"
                );
            }
            let contacts = app
                .world()
                .get::<crate::track::sim::TrackContacts>(tank)
                .unwrap();
            assert!(
                contacts
                    .0
                    .iter()
                    .all(|side| side.iter().any(|c| c.load_elastic > 0.0)),
                "the slope witness must retain support"
            );
        }
    }
}

/// A soft ground that a declared launch gear can out-pull must not strand the vehicle in
/// first. Fixture: friction 0.4 and rolling resistance 0.12; full pedal for 40 seconds.
#[test]
fn a_tank_leaves_first_gear_from_rest_on_a_soft_ground() {
    let (mut app, tank) = booted_offline_sim();
    {
        let mut track = app
            .world_mut()
            .resource_mut::<crate::track::terrain::TrackField>();
        let field = track.field.take().expect("boot builds terrain");
        track.field = Some(
            field.with_ground(Some(crate::ground_field::GroundField::uniform(
                crate::ground_field::ContactGround {
                    grip: 0.4,
                    rolling_resistance: 0.12,
                },
            ))),
        );
    }
    face_positive_z(&mut app, tank);
    let mut gear = 1;
    let mut gears = Vec::new();
    for _ in 0..40 * FIXED_TICKS_PER_SECOND {
        drive_tick(&mut app, tank, 1.0, 0.0);
        gear = app
            .world()
            .get::<crate::track::sim::TankTransmission>(tank)
            .unwrap()
            .0
            .gear;
        if gears.last() != Some(&gear) {
            gears.push(gear);
        }
    }
    println!("MEASURED soft-ground launch and climb {gears:?}");
    assert!(
        gears.windows(2).all(|pair| pair[1] > pair[0]),
        "a uniform-ground launch must not undo its preselection while spooling: {gears:?}"
    );
    #[cfg(feature = "bitprobe")]
    {
        let construction = app.world().resource::<crate::track::sim::TrackGear>();
        let state = &app
            .world()
            .get::<crate::track::sim::TankTransmission>(tank)
            .unwrap()
            .0;
        if usize::from(gear) < construction.trans().gears_fwd.len() {
            let probe = app
                .world()
                .get::<crate::track::sim::TrackGripEffect>(tank)
                .unwrap()
                .transmission_probe;
            let next = construction.landed_spare(state, probe, gear + 1);
            let margin = crate::track::load::DriverResolution::load().margin_n(
                app.world().resource::<TankBlueprint>().spec.mass * crate::track::derive::G,
                construction.trans(),
            );
            println!(
                "MEASURED soft-ground final gear={gear} next_landed_spare_n={next} margin_n={margin}"
            );
            assert!(
                next < margin,
                "the next landed gear must lack sustainable spare"
            );
        }
    }
    let speed = app
        .world()
        .get::<avian3d::prelude::LinearVelocity>(tank)
        .expect("tank has linear velocity")
        .0
        .length();
    assert!(
        gear > 1,
        "40 s of full throttle from rest must leave first gear (F{gear} at {speed:.4} m/s)"
    );
}

/// A parked hull's roll kick must decay rather than sustain a side-swap limit cycle. Drivers
/// rely on a settled ride. Fixture: 0.3 rad/s about the hull's longitudinal axis, 3 s observation;
/// third-second RMS must be below 1 % of the kick. Peak side-load change covers all three seconds.
#[test]
fn contact_law_roll_kick_decays() {
    let (mut app, tank) = booted_offline_sim();
    app.world_mut()
        .get_mut::<avian3d::prelude::AngularVelocity>(tank)
        .unwrap()
        .0 = Vec3::Z * 0.3;
    let mut square_sum = 0.0;
    let mut peak_swing = 0.0_f32;
    let mut previous_difference: Option<f32> = None;
    let mut previous_swing = 0.0_f32;
    let mut alternations = 0;
    for tick in 0..3 * FIXED_TICKS_PER_SECOND {
        app.update();
        let contacts = &app
            .world()
            .get::<crate::track::sim::TrackContacts>(tank)
            .unwrap()
            .0;
        let loads = contacts
            .each_ref()
            .map(|side| side.iter().map(|c| c.load).sum::<f32>());
        let difference = loads[0] - loads[1];
        if let Some(previous) = previous_difference {
            let swing = difference - previous;
            peak_swing = peak_swing.max(swing.abs());
            if swing * previous_swing < 0.0 {
                alternations += 1;
            }
            previous_swing = swing;
        }
        previous_difference = Some(difference);
        if tick >= 2 * FIXED_TICKS_PER_SECOND {
            let rate = app
                .world()
                .get::<avian3d::prelude::AngularVelocity>(tank)
                .unwrap()
                .0
                .z;
            square_sum += rate * rate;
        }
    }
    let rms = (square_sum / FIXED_TICKS_PER_SECOND as f32).sqrt();
    println!(
        "MEASURED ride: third-second roll RMS {rms:.8} rad/s, peak tick side-load swing {peak_swing:.3} N, alternation {:.5}",
        alternations as f32 / (3 * FIXED_TICKS_PER_SECOND - 2) as f32
    );
    assert!(rms < 0.003, "roll kick must decay; third-second RMS {rms}");
}

/// A hull must climb a block step under forward drive. The trace isolates the game's contact
/// support and traction from collider solver impulses. Fixture: the course's 0.4 m curb,
/// hull facing -z at z=-63, from rest at full throttle, stopping once its centre clears the lip.
#[test]
fn contact_law_forward_step_trace() {
    use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
    let (mut app, tank) = booted_offline_sim();
    let rest_y = app
        .world()
        .resource::<crate::track::rig_geom::RigGeom>()
        .hull_rest_y;
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<Position>().unwrap().0 = Vec3::new(0.0, rest_y, -63.0);
        e.get_mut::<Rotation>().unwrap().0 = Quat::IDENTITY;
        e.get_mut::<LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<AngularVelocity>().unwrap().0 = Vec3::ZERO;
    }
    for _ in 0..120 {
        app.update();
    }
    let initial_y = app.world().get::<Position>(tank).unwrap().0.y;
    let mut max_y = initial_y;
    let mut peak_rearward = 0.0_f32;
    let mut peak_delta = 0.0_f32;
    let mut previous_force = 0.0_f32;
    let mut max_damping_excess = 0.0_f32;
    let mut last_bin = i32::MIN;
    let mut end_z = -63.0;
    for tick in 0..12 * FIXED_TICKS_PER_SECOND {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let position = app.world().get::<Position>(tank).unwrap().0;
        end_z = position.z;
        max_y = max_y.max(position.y);
        let contacts = &app
            .world()
            .get::<crate::track::sim::TrackContacts>(tank)
            .unwrap()
            .0;
        let force = contacts
            .iter()
            .flatten()
            .map(|c| (c.normal * c.load + c.tangential_force()).z)
            .sum::<f32>();
        for contact in contacts.iter().flatten() {
            // Belt motion is tangent to the contact plane, so the reported belt velocity
            // carries the force pass's exact normal velocity without a tick-phase mismatch.
            let closing = -contact.normal.dot(contact.belt_velocity);
            let bound = contact.load_elastic + contact.damping * closing.abs();
            max_damping_excess =
                max_damping_excess.max((contact.load - bound) / contact.load.max(1.0));
        }
        peak_rearward = peak_rearward.max(force);
        peak_delta = peak_delta.max(force - previous_force);
        previous_force = force;
        let bin = (position.z * 10.0).floor() as i32;
        if bin != last_bin {
            let speed = -app.world().get::<LinearVelocity>(tank).unwrap().0.z;
            println!(
                "MEASURED hull lip tick={tick} z={:.5} y={:.5} speed={speed:.5} F_z={force:.3} N",
                position.z, position.y
            );
            last_bin = bin;
        }
        if position.z < -70.0 {
            break;
        }
    }
    println!(
        "MEASURED hull lip summary: end_z {end_z:.5}, rise {:.5} m, peak rearward {peak_rearward:.3} N, peak rearward force jump {peak_delta:.3} N",
        max_y - initial_y
    );
    println!("MEASURED hull lip maximum damping bound excess {max_damping_excess:.8}");
    // Fixture tolerance: f32 arithmetic in the load assembly and normal projection.
    assert!(
        max_damping_excess < 1e-5,
        "contact damping amplified the point speed: {max_damping_excess}"
    );
    assert!(
        end_z < -69.0 && max_y - initial_y > 0.2,
        "forward drive must climb the fixture step"
    );
}

/// The world height (m) of the lowest vertex of the bump-stop colliders `tank`'s body carries.
fn lowest_bump_stop_y(app: &mut App, tank: Entity) -> f32 {
    use avian3d::prelude::{Collider, Position, Rotation};
    let world = app.world_mut();
    let (position, rotation) = (
        world.get::<Position>(tank).expect("tank has a position").0,
        world.get::<Rotation>(tank).expect("tank has a rotation").0,
    );
    world
        .query_filtered::<(&Collider, &ChildOf), With<crate::track::bump_stop::BumpStop>>()
        .iter(world)
        .filter(|(_, parent)| parent.parent() == tank)
        .flat_map(|(collider, _)| {
            collider
                .shape()
                .as_convex_polyhedron()
                .expect("a bump stop is a convex hull")
                .points()
                .to_vec()
        })
        .map(|p| (position + rotation * p).y)
        .fold(f32::INFINITY, f32::min)
}

/// A tank whose springs are too soft to hold it bottoms on its bump stops: it comes to rest on
/// them, not through them, and their contacts reach the contact reader as any collider's do. On
/// its own springs the same tank at rest touches nothing with them: the stop must never stand in
/// for the belt. FIXTURE DATA: the support springs at 2 % of the tank's own, far softer than its
/// stop's clearance can hold; the solver's resting penetration bound (3 cm) is ASSUMED.
#[test]
fn a_tank_too_soft_for_its_springs_rests_on_its_bump_stops() {
    use crate::contact_transfer::{ContactTransfers, StructureBody};
    use crate::track::bump_stop::BumpStop;
    use avian3d::prelude::{Collider, LinearVelocity, Position, RigidBody};

    let (mut app, tank) = booted_offline_sim();
    let rest_clearance = lowest_bump_stop_y(&mut app, tank);
    assert!(
        rest_clearance > 0.0,
        "on its own springs the stop must clear flat ground at rest (lowest vertex at \
         {rest_clearance:.4} m)"
    );
    let stops = {
        let world = app.world_mut();
        world
            .query_filtered::<(Entity, &ChildOf), With<BumpStop>>()
            .iter(world)
            .filter(|(_, parent)| parent.parent() == tank)
            .count()
    };
    assert_eq!(stops, 2, "every tank spawns one bump stop per side");
    let plates = {
        let world = app.world_mut();
        world
            .query_filtered::<(Entity, &ChildOf), With<crate::track::bump_stop::SidePlate>>()
            .iter(world)
            .filter(|(_, parent)| parent.parent() == tank)
            .count()
    };
    assert_eq!(plates, 4, "every tank spawns a plate at each shoe edge");

    // Instrumented flat ground: StructureBody lets the reader see the stop's press. Its Terrain
    // membership excludes side plates, exactly as the coincident ordinary ground does. FIXTURE
    // slab top at y = 0 m; the dedicated plate gate covers real standing structure contacts.
    let at = app
        .world()
        .get::<Position>(tank)
        .expect("tank has a position")
        .0;
    let slab = app
        .world_mut()
        .spawn((
            Transform::from_xyz(at.x, -0.5, at.z),
            RigidBody::Static,
            StructureBody,
        ))
        .id();
    app.world_mut().spawn((
        ChildOf(slab),
        Transform::IDENTITY,
        Collider::cuboid(30.0, 1.0, 30.0),
        crate::track::terrain::FieldSurface,
        avian3d::prelude::CollisionLayers::new(
            [crate::Layer::Terrain],
            avian3d::prelude::LayerMask::ALL,
        ),
    ));
    app.world_mut()
        .resource_mut::<crate::track::sim::TrackGear>()
        .params_mut()
        .support_stiffness_per_m *= 0.02;
    // Dropped from 0.3 m above its rest pose.
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<Position>().expect("tank has a position").0.y += 0.3;
        e.get_mut::<LinearVelocity>()
            .expect("tank has a velocity")
            .0 = Vec3::ZERO;
    }

    let ticks = 5 * FIXED_TICKS_PER_SECOND;
    let mut lowest_late = f32::INFINITY;
    let mut fastest_late = 0.0f32;
    let mut press_up = 0.0f32;
    let mut pressed_ticks = 0;
    for tick in 0..ticks {
        drive_tick(&mut app, tank, 0.0, 0.0);
        if tick < ticks - FIXED_TICKS_PER_SECOND {
            continue;
        }
        lowest_late = lowest_late.min(lowest_bump_stop_y(&mut app, tank));
        let world = app.world();
        let speed = world
            .get::<LinearVelocity>(tank)
            .expect("tank has a velocity")
            .0
            .length();
        fastest_late = fastest_late.max(speed);
        let press: f32 = world
            .resource::<ContactTransfers>()
            .split()
            .filter(|(point, _, _, _)| {
                point.body == tank
                    && point
                        .body_collider
                        .is_some_and(|collider| world.get::<BumpStop>(collider).is_some())
            })
            .map(|(_, _, press, _)| -press.y)
            .sum();
        if press > 0.0 {
            pressed_ticks += 1;
            press_up += press;
        }
    }
    let weight = app
        .world()
        .get::<avian3d::prelude::Mass>(tank)
        .expect("tank has mass")
        .0
        * crate::track::derive::G;
    println!(
        "bump stop: rest clearance {rest_clearance:.4} m; soft springs: lowest vertex \
         {lowest_late:.4} m, fastest {fastest_late:.4} m/s over the last second; the structure \
         read a press on the stops in {pressed_ticks}/{FIXED_TICKS_PER_SECOND} ticks, mean \
         {:.0} N ({:.3} of the weight)",
        press_up / FIXED_TICKS_PER_SECOND as f32,
        press_up / FIXED_TICKS_PER_SECOND as f32 / weight,
    );
    assert!(
        lowest_late >= -0.03,
        "the tank sank {:.4} m through its bump stops",
        -lowest_late
    );
    assert!(
        lowest_late <= 0.03,
        "springs this soft cannot hold the tank clear of its stops (lowest vertex \
         {lowest_late:.4} m)"
    );
    assert!(
        fastest_late < 0.05,
        "the tank must come to rest on its stops (moving {fastest_late:.4} m/s)"
    );
    assert_eq!(
        pressed_ticks, FIXED_TICKS_PER_SECOND,
        "the contact reader must read the stops' press on the structure every tick at rest"
    );
}

/// THE GAME'S DRIVING ADAPTER LOADS FIXED STRUCTURE THROUGH THE BELT. A tank parked on standing
/// structural construction bears on it through its belt alone, and the reaction staged by
/// `track::sim::apply_track_forces` reaches the structure reading that wall and trunk laws judge.
/// Dropping the adapter's staging, or a structure taking no share of the hull's weight, fails it.
/// FIXTURE DATA: a 30 m slab of Terrain membership (which keeps the side plates off it, as on
/// ordinary ground), its top 5 cm above the flat ground. The observed vertical momentum change
/// closes the gravity/reaction budget; a settling time does not imply zero acceleration.
#[test]
fn a_parked_tank_loads_fixed_structure_through_its_belt() {
    use crate::contact_transfer::{ContactTransfers, StructureBody};
    use avian3d::prelude::{Collider, CollisionLayers, LayerMask, Mass, Position, RigidBody};

    let (mut app, tank) = booted_offline_sim();
    let at = app
        .world()
        .get::<Position>(tank)
        .expect("tank has a position")
        .0;
    let slab = app
        .world_mut()
        .spawn((
            Transform::from_xyz(at.x, -0.45, at.z),
            RigidBody::Static,
            StructureBody,
        ))
        .id();
    app.world_mut().spawn((
        ChildOf(slab),
        Transform::IDENTITY,
        Collider::cuboid(30.0, 1.0, 30.0),
        CollisionLayers::new([crate::Layer::Terrain], LayerMask::ALL),
    ));
    let ticks = 3 * FIXED_TICKS_PER_SECOND;
    let (mut lift, mut elapsed, mut read_ticks) = (0.0_f64, 0.0_f64, 0);
    let mass = app.world().get::<Mass>(tank).expect("tank has mass").0;
    let gravity = app.world().resource::<avian3d::prelude::Gravity>().0.y;
    let mut initial_velocity = None;
    for tick in 0..ticks {
        if tick == ticks - FIXED_TICKS_PER_SECOND {
            initial_velocity = Some(
                app.world()
                    .get::<avian3d::prelude::LinearVelocity>(tank)
                    .unwrap()
                    .0
                    .y,
            );
        }
        drive_tick(&mut app, tank, 0.0, 0.0);
        if tick < ticks - FIXED_TICKS_PER_SECOND {
            continue;
        }
        let transfers = app.world().resource::<ContactTransfers>();
        let dt = transfers.dt();
        let mut tick_lift = 0.0;
        for (point, struck, press, _) in transfers.split() {
            if point.body == tank && point.body_collider.is_none() {
                assert_eq!(point.structure, slab, "the belt bears on the slab alone");
                tick_lift -= struck.y / dt + press.y;
            }
        }
        if tick_lift > 0.0 {
            read_ticks += 1;
        }
        lift += f64::from(tick_lift) * f64::from(dt);
        elapsed += f64::from(dt);
    }
    let weight_impulse = -f64::from(mass) * f64::from(gravity) * elapsed;
    let final_velocity = app
        .world()
        .get::<avian3d::prelude::LinearVelocity>(tank)
        .unwrap()
        .0
        .y;
    let momentum_change = f64::from(mass)
        * f64::from(
            final_velocity
                - initial_velocity.expect("the observation window starts inside the scripted run"),
        );
    let share = lift / weight_impulse;
    println!(
        "parked on structure: the slab read the belt's reaction in {read_ticks}/\
         {FIXED_TICKS_PER_SECOND} ticks, mean {share:.4} of the weight"
    );
    assert_eq!(
        read_ticks, FIXED_TICKS_PER_SECOND,
        "the structure must read the belt's reaction every tick"
    );
    // Resolution: f32 force assembly and substep integration, one part in ten thousand of
    // the accumulated gravity impulse; this bounds numerical closure, not a settling outcome.
    assert!(
        (lift - weight_impulse - momentum_change).abs() <= 1e-4 * weight_impulse,
        "vertical momentum must close: reaction={lift}, gravity={weight_impulse}, delta_p={momentum_change}"
    );
}

/// Belt support lifts a driving hull onto loose pieces and carries it after stopping.
/// This witnesses the hull's support path; recipient impulse delivery is outside its claim.
/// The pieces lie between the rigid side slices: neither the slices nor the compression stops
/// may supply their lift. FIXTURE DATA: two long pieces, height 0.1 m, each
/// one eighth of the hull's mass; dimensions and positions otherwise come from its running gear.
/// Positive lift and settled piece support follow from the raised support partition;
/// the lift magnitude remains an observation.
#[test]
fn a_driving_tank_rides_onto_loose_pieces_through_its_belt_support() {
    use crate::track::side::Side;
    use crate::track::sim::TrackContacts;
    use avian3d::prelude::{
        AngularInertia, CenterOfMass, Collider, LinearVelocity, Mass, NoAutoAngularInertia,
        NoAutoCenterOfMass, NoAutoMass, Position, RigidBody, Rotation,
    };

    let (mut app, tank) = booted_offline_sim();
    face_positive_z(&mut app, tank);
    let at = app.world().get::<Position>(tank).unwrap().0;
    let rotation = app.world().get::<Rotation>(tank).unwrap().0;
    let mass = app.world().get::<Mass>(tank).unwrap().0 / 8.0;
    let geom = app.world().resource::<crate::track::rig_geom::RigGeom>();
    let points = &geom.rest.get(Side::Right);
    let z_min = points
        .iter()
        .map(|(p, _)| p.x)
        .fold(f32::INFINITY, f32::min);
    let z_max = points
        .iter()
        .map(|(p, _)| p.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let span = z_max - z_min + geom.model.pin_to_outer * 2.0;
    let height = 0.1;
    let length = span * 3.0;
    let front = at.z + z_min.abs().max(z_max.abs()) + geom.pitch * 4.0;
    let dimensions = Side::ALL.map(|side| {
        let columns = geom.grip_columns(side);
        let width = (columns[2].0 - columns[0].0).abs() - geom.thickness * 4.0;
        assert!(
            width > 0.0,
            "the fixture must fit between the rigid shoe-edge slices"
        );
        let x = (columns[0].0 + columns[2].0) / 2.0;
        let center = at + rotation * Vec3::X * x;
        (
            Vec3::new(center.x, height / 2.0, front + length / 2.0),
            width,
        )
    });
    let pieces = dimensions.map(|(position, width)| {
        app.world_mut()
            .spawn((
                crate::debris::DebrisPiece,
                Transform::from_translation(position),
                Position(position),
                Rotation::default(),
                RigidBody::Dynamic,
                Collider::cuboid(width, height, length),
                Mass(mass),
                NoAutoMass,
                AngularInertia::from_shape(&Cuboid::new(width, height, length), mass),
                NoAutoAngularInertia,
                CenterOfMass(Vec3::ZERO),
                NoAutoCenterOfMass,
                crate::substances::contact_friction(0.6),
                crate::construction::breakage::piece_layers(),
            ))
            .id()
    });
    let mut carried = false;
    let mut arrived = false;
    for _ in 0..(20 * FIXED_TICKS_PER_SECOND) {
        // Full requested power gives this support fixture its declared launch capability.
        drive_tick(&mut app, tank, 1.0, 0.0);
        let world = app.world();
        carried |= world
            .get::<TrackContacts>(tank)
            .unwrap()
            .0
            .iter()
            .flatten()
            .any(|c| {
                c.collider.is_some_and(|e| pieces.contains(&e)) && c.normal.y > 0.5 && c.load > 0.0
            });
        if world.get::<Position>(tank).unwrap().0.z > front + span {
            arrived = true;
            break;
        }
    }
    assert!(arrived, "the tank never drove onto the fixture pieces");
    // Lift-off coasts in the retained gear. Service braking keeps the settled support
    // witness inside these finite pieces, independent of the Driver's arrival gear.
    for _ in 0..(4 * FIXED_TICKS_PER_SECOND) {
        let moving_forward = app.world().get::<LinearVelocity>(tank).unwrap().0.z
            > crate::track::transmission::PARK_ENGAGE_SPEED;
        drive_tick(&mut app, tank, if moving_forward { -1.0 } else { 0.0 }, 0.0);
    }
    let settled = app.world().get::<Position>(tank).unwrap().0;
    assert!(
        (front + span * 0.5..front + length - span * 0.5).contains(&settled.z),
        "the settled footprint must remain inside the fixture pieces: z={}, front={front}, length={length}, span={span}",
        settled.z
    );
    let rise = settled.y - at.y;
    // The support law spreads each column's spring over its share of the shoe width, and the strip
    // beside a piece keeps pressing the ground while the rise stays under the free travel. A piece
    // covering the share `s` of every grounded shoe then lifts the hull by `s·h` (DERIVED, level
    // hull, ignoring the zero-travel sprocket and idler arcs).
    let share = {
        let geom = app.world().resource::<crate::track::rig_geom::RigGeom>();
        let columns = geom.grip_columns(Side::Right);
        dimensions[Side::Right.index()].1 / (columns[2].0 - columns[0].0).abs()
    };
    println!(
        "MEASURED driven hull rise {rise:.6} m over {height} m pieces; DERIVED coverage share {share:.4}, share-law rise {:.6} m",
        share * height
    );
    assert!(
        carried,
        "the game's track reports never carried the tank on a loose piece"
    );
    // Resolution: 0.1% of the raised surface's height excludes position roundoff.
    assert!(rise > height * 1e-3, "raised support must lift the hull");
    let piece_support: f32 = app
        .world()
        .get::<TrackContacts>(tank)
        .unwrap()
        .0
        .iter()
        .flatten()
        .filter(|c| c.collider.is_some_and(|e| pieces.contains(&e)))
        .map(|c| c.normal.y * c.load)
        .sum();
    assert!(
        piece_support > 0.0,
        "the settled hull must still load the pieces"
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
    /// The controlled tank's track elements carried strain on some tick.
    saw_element_strain: bool,
    /// The controlled tank's transmission left its spawn state over the flat script.
    transmission_advanced: bool,
    /// Slope-segment witness of support-priced gear changes.
    saw_slope_shift: bool,
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
            (
                &crate::track::sim::TrackDrive,
                &crate::track::sim::DriveSignal,
            ),
            &crate::track::sim::TrackGrip,
            &crate::track::sim::TrackGripElements,
            &crate::track::sim::TankTransmission,
            (&crate::tank::WeaponGate, &crate::tank::WeaponLoads),
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
        (drive, signal),
        grip,
        elements,
        transmission,
        (weapon_gate, weapon_loads),
        contacts,
        (servos, sim),
    ) in &roots
    {
        digests.push((
            name.as_str().to_owned(),
            crate::trace::canonical_tank_state_digest(&crate::trace::TankState {
                position: position.0,
                rotation: rotation.0,
                linvel: linear.0,
                angvel: angular.0,
                drive,
                signal: Some(signal),
                grip,
                elements: Some(elements),
                transmission: Some(transmission),
                weapon_gate: Some(weapon_gate),
                loads: Some(weapon_loads),
                servos: &servos.states,
                sim,
            }),
        ));
        if is_controlled {
            controlled = Some((
                position.0, rotation.0, linear.0, angular.0, com.0, signal, contacts, sim,
            ));
            run.saw_element_strain |= elements
                .sides
                .iter()
                .flat_map(|side| side.strain.iter())
                .any(|strain| strain.length() > 0.0);
        }
    }
    digests.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(digests.len(), 2, "the local duel has two simulation tanks");

    let (position, rotation, linear, angular, local_com, signal, contacts, sim) =
        controlled.expect("one controlled tank");
    let grounded = contacts.0.iter().filter(|side| !side.is_empty()).count();
    run.saw_airborne |= grounded == 0;
    run.saw_grounded |= grounded > 0;

    // Avian 0.7 `Forces::velocity_at_point`: v_point = v_linear + omega × (point − world_COM),
    // where world_COM = position + rotation * local_COM. Slip is witnessed directly from the
    // belt model's contact telemetry: a loaded contact whose longitudinal slip is past the
    // near-rest band while steer is commanded. FIXTURE DATA: the band is half the sim's 0.4 m/s
    // slip-saturation scale; the script's steer peaks at 0.28 m/s of slip (MEASURED).
    let _ = (position, rotation, linear, angular, local_com);
    let loaded_contact_is_slipping = contacts
        .0
        .iter()
        .flatten()
        .any(|c| c.load > 0.0 && c.slip.abs() > 0.2);
    run.saw_steering_slip |=
        tick >= 300 && signal.steer.abs() > f32::EPSILON && loaded_contact_is_slipping;
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

/// The sim's mutators run in two fixed schedules: driving, the weapon gate, firing and recoil in
/// `FixedUpdate`; the projectile walk, after the physics step it reads, and the damage chain it
/// feeds in `FixedPostUpdate`. Each schedule must hold its systems exactly once, and no two of them
/// that write physical state, nor the walk and a damage system, nor the walk and a physics system,
/// may run in an order the schedule leaves open.
fn assert_simulation_mutators_are_ordered(app: &App) {
    assert_mutators_are_ordered(
        app,
        FixedUpdate,
        &[
            "track::sim::apply_track_forces",
            "shooting::tick_weapon_gate",
            "shooting::fire",
            "shooting::apply_recoil",
        ],
    );
    assert_mutators_are_ordered(
        app,
        FixedPostUpdate,
        &[
            "ballistics::integrate_projectiles",
            "damage::process_cookoffs",
            "damage::kill_crew",
        ],
    );
}

fn assert_mutators_are_ordered(
    app: &App,
    label: impl bevy::ecs::schedule::ScheduleLabel,
    expected: &[&str],
) {
    const WALK: &str = "ballistics::integrate_projectiles";
    let schedule_name = format!("{label:?}");
    let schedules = app.world().resource::<bevy::ecs::schedule::Schedules>();
    let schedule = schedules
        .get(label)
        .unwrap_or_else(|| panic!("the full sim installs {schedule_name}"));
    let names: std::collections::HashMap<_, _> = schedule
        .systems()
        .unwrap_or_else(|_| panic!("{schedule_name} ran and initialized its systems"))
        .map(|(key, system)| (key, system.name().to_string()))
        .collect();
    for expected in expected {
        assert_eq!(
            names
                .values()
                .filter(|name| name.ends_with(expected))
                .count(),
            1,
            "the schedule guard must find exactly one `{expected}` system in {schedule_name}",
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
                    || name.contains(WALK)
            };
            let walk_beside = |prefix: &str| {
                (left.contains(WALK) && right.contains(prefix))
                    || (right.contains(WALK) && left.contains(prefix))
            };
            (writes_physical_state(left) && writes_physical_state(right))
                || walk_beside("damage::")
                || walk_beside("avian3d::")
        })
        .map(|(left, right)| (left.clone(), right.clone()))
        .collect();
    assert!(
        conflicts.is_empty(),
        "simulation mutators in {schedule_name} need an explicit order: {conflicts:#?}",
    );
}

const SCRIPT_TICKS: usize = 600;
/// Ticks of the slope segment that follows the flat script.
const SLOPE_TICKS: usize = 512;

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
            // The steering-slip witness needs enough run-up for the tight detent to scrub
            // its loaded contacts past the slip band.
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
    let transmission_advanced = *app
        .world()
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("the controlled tank carries transmission state")
        != fresh_tank_transmission(&app);

    // Fixture: a rolling grade entry exercises support-priced gear choices and paid cuts.
    let rot = Quat::from_rotation_x(20.0_f32.to_radians());
    // Fixture: the rolling entry keeps the front contact on the slope face during gear response.
    let approach_speed = 3.0;
    // DERIVED fixture placement: put the rest envelope on the actual terrain face along its
    // normal, rather than dropping the rig from an unrelated world height.
    use crate::track::oracle::DepthOracle as _;
    let normal = rot * Vec3::Y;
    let rest_height = app
        .world()
        .resource::<crate::track::rig_geom::RigGeom>()
        .hull_rest_y;
    // FIXTURE: query seed over the face. Its cast foot lands DERIVED 2.15 m up the face from
    // where the face leaves the slab (`world::spawn_test_course`), and the rest envelope's rear-
    // most loaded contact sits MEASURED 1.88 m behind the foot, so no station starts on the slab.
    let mut approach = Vec3::new(0.0, 1.50, -39.45);
    let clearance = app
        .world()
        .resource::<crate::track::terrain::TrackField>()
        .field
        .as_ref()
        .expect("the booted fixture has its terrain field")
        .depth_along(approach - normal * rest_height, -normal, 8.0); // FIXTURE: cast reach above the course.
    approach += normal * clearance;
    let foot = approach - normal * rest_height;
    // The largest distance of a loaded contact from the face's plane through the foot. A contact
    // on the slab, or a station buried in it, lies off that plane by up to the envelope's length
    // times sin 20°; zero says every station presses the face.
    let off_face = |app: &BootedSim| {
        let contacts = &app
            .world()
            .get::<crate::track::sim::TrackContacts>(tank)
            .expect("the controlled tank carries contact telemetry")
            .0;
        let loaded = contacts
            .iter()
            .flatten()
            .filter(|contact| contact.load > 0.0);
        let off = loaded
            .clone()
            .map(|contact| normal.dot(contact.point - foot).abs())
            .fold(0.0_f32, f32::max);
        (loaded.count(), off)
    };
    // FIXTURE: a contact point lies on the terrain surface, so only roundoff separates it.
    const ON_FACE_M: f32 = 0.01;
    let mut transmission = fresh_tank_transmission(&app);
    transmission.0.gear = 6;
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<avian3d::prelude::Position>().unwrap().0 = approach;
        e.get_mut::<avian3d::prelude::Rotation>().unwrap().0 = rot;
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 =
            rot * Vec3::NEG_Z * approach_speed;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
        *e.get_mut::<crate::track::sim::DriveSignal>().unwrap() = crate::track::sim::DriveSignal {
            throttle: 1.0,
            steer: 0.0,
        };
        let mut drive = e.get_mut::<crate::track::sim::TrackDrive>().unwrap();
        drive.sides[0].speed = approach_speed;
        drive.sides[1].speed = approach_speed;
        *e.get_mut::<crate::track::sim::TankTransmission>().unwrap() = transmission;
    }
    let mut shifted = false;
    for slope_tick in 0..SLOPE_TICKS {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let state = app
            .world()
            .get::<crate::track::sim::TankTransmission>(tank)
            .expect("the controlled tank carries transmission state")
            .0;
        if slope_tick == 0 {
            let (loaded, off) = off_face(&app);
            assert!(
                loaded > 0 && off <= ON_FACE_M,
                "slope tick {slope_tick}: the grade-entry fixture must be supported on the 20° \
                 face alone (rolling at the start, before the grade entry proceeds); {loaded} loaded \
                 contacts lie up to {off} m off the face",
            );
        }
        shifted |= state.gear != 6;
    }

    let mut run = app
        .world_mut()
        .remove_resource::<ScriptedDeterminismRun>()
        .expect("the scripted digest collector remains installed");
    run.transmission_advanced = transmission_advanced;
    run.saw_slope_shift = shifted;
    run
}

fn assert_scripted_determinism_witnesses(run: &ScriptedDeterminismRun, label: &str) {
    assert_eq!(
        run.digests.len(),
        SCRIPT_TICKS + SLOPE_TICKS,
        "{label} produced one digest per fixed tick",
    );
    assert!(run.saw_airborne, "{label} crossed an airborne state");
    assert!(run.saw_grounded, "{label} reached ground contact");
    assert!(
        run.saw_element_strain,
        "{label} put strain into the driven tank's track elements (the element law engaged)",
    );
    assert!(
        run.transmission_advanced,
        "{label} advanced the declared transmission from its spawn state",
    );
    assert!(
        run.saw_slope_shift,
        "{label} slope segment exercised a gear decision"
    );
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

/// Two fresh, full simulation compositions must replay one command script bit-for-bit, every tick:
/// the flat drive-steer-fire script, then the slope segment. The digest hashes the transmission's
/// exhaustive field projection (`transmission_state_projection`) and both belt speeds, so the
/// support-priced gear decisions, park lever and reversal pause replay exactly too. The witnesses
/// keep this from passing because the scenario never reached contact, element strain, slip
/// traction, steering slip, fire, a transmission off its spawn state, or that grade response.
#[test]
fn full_simulation_replay_is_bit_exact_through_the_flat_and_slope_scripts() {
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
#[cfg(feature = "dev_ui")]
const SANDBOX_BOOT_FRAMES: usize = 8;

/// Boot one sandbox root headless and run [`SANDBOX_BOOT_FRAMES`] frames of it.
///
/// The clock is the fixed-tick one (one exact fixed loop per update), so `FixedUpdate` — physics,
/// the belt field, the weapon-shaped systems — is exercised too, not just `Update`.
///
/// Serialized against every other full-app fixture by [`BOOT_LEASE`]: these boots parse the whole
/// Tiger glb (`bake`) and run Avian, and the suite's rule is that full apps take turns.
#[cfg(feature = "dev_ui")]
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

/// Every string a `Text` node shows this frame — its own `Visibility` and every ancestor's other
/// than `Hidden`: the sandboxes' HUD surface, read without reaching into another module's private
/// marker components.
#[cfg(feature = "dev_ui")]
fn shown_text(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    let mut texts = world.query::<(Entity, &Text)>();
    let world = &*world;
    let hidden = |node: Entity| world.get::<Visibility>(node) == Some(&Visibility::Hidden);
    texts
        .iter(world)
        .filter(|&(entity, _)| {
            !std::iter::successors(Some(entity), |&node| {
                world.get::<ChildOf>(node).map(ChildOf::parent)
            })
            .any(hidden)
        })
        .map(|(_, text)| text.0.clone())
        .collect()
}

/// The armour/penetration sandbox boots and keeps running, its view's text at a glance.
///
/// The regression this pins is the `Res<WeaponClock>` panic described above; past surviving, the
/// target tank must be up, the free shooter must hold the range gun, and the weapon clock must
/// advance. The view shows one line at boot, the selected round's name: the legend waits behind a
/// key and no game HUD (crew bar, status card, nameplate) is mounted, since the look shows the
/// target's state. A mounted HUD, a shown legend or a label carrying more than the name fails it.
#[cfg(feature = "dev_ui")]
#[test]
fn armor_sandbox_boots_headless() {
    let (mut app, _lease) = boot_sandbox_headless(crate::sandbox::plugin);

    let mut targets = app.world_mut().query_filtered::<Entity, With<Tank>>();
    assert_eq!(
        targets.iter(app.world()).count(),
        1,
        "the armor sandbox must spawn exactly one target tank from the blueprint within \
         {SANDBOX_BOOT_FRAMES} frames",
    );

    // The range: the camera is the one free shooter, and exactly one weapon names it as its root.
    let shooter = app
        .world_mut()
        .query_filtered::<Entity, With<crate::FreeShooter>>()
        .single(app.world())
        .expect("the armor sandbox's free-fly camera is its one free shooter");
    let mut weapons = app
        .world_mut()
        .query::<(&crate::tank::Weapon, &crate::tank::TankRoot)>();
    let guns: Vec<_> = weapons
        .iter(app.world())
        .filter(|(_, root)| root.0 == shooter)
        .map(|(gun, _)| gun.rounds[0].name.clone())
        .collect();
    let [first_round] = guns.as_slice() else {
        panic!("the free shooter holds no range gun (or more than one)");
    };

    // The weapon clock advances one tick per fixed step, as the game's does: each range round's
    // `ShotId` names its tick.
    assert!(
        app.world().resource::<crate::WeaponClock>().0.raw() > 0,
        "the armor sandbox's weapon clock never advanced — every range round would share tick 0",
    );

    let shown = shown_text(&mut app);
    assert_eq!(
        shown,
        std::slice::from_ref(first_round),
        "the armour sandbox's view shows only the selected round's name at boot",
    );
}

/// A range round on the armour sandbox's target is explained where the look reads it, and opens no
/// corner: through the real trigger, the offline delivery stores the shooter's projection of the
/// round's record for the free shooter, naming its shot, the target's life and the gun's selected
/// round, and no camera opens beside the range's own while it plays. The subject is the game's
/// damage-model tooling.
#[cfg(feature = "dev_ui")]
#[test]
fn a_range_round_on_the_target_is_explained_and_opens_no_corner() {
    use crate::damage_cam::delivery::ShotExplanations;
    use crate::damage_cam::wire::{Audience, TankLife};

    let (mut app, _lease) = boot_sandbox_headless(crate::sandbox::plugin);
    let world = app.world_mut();
    let (struck, target) = {
        let mut targets = world
            .query_filtered::<(Entity, &crate::CombatantId, &crate::CombatantLife), With<Tank>>();
        let (entity, &combatant, &life) = targets.single(world).expect("one target");
        (TankLife { combatant, life }, entity)
    };
    let (shooter, shooter_id) = {
        let mut shooters =
            world.query_filtered::<(Entity, &crate::CombatantId), With<crate::FreeShooter>>();
        let (entity, &id) = shooters.single(world).expect("one free shooter");
        (entity, id)
    };
    let round = {
        let mut weapons = world.query::<(&crate::tank::Weapon, &crate::tank::TankRoot)>();
        let (gun, _) = weapons
            .iter(world)
            .find(|(_, root)| root.0 == shooter)
            .expect("the range gun");
        gun.rounds[0].key.clone()
    };

    // Broadside through the centre of the target's colliders, from outside them: the line crosses
    // the tank's side, clear of the plates downrange of the camera's spawn.
    let mut colliders = world.query::<(Entity, &avian3d::prelude::ColliderAabb)>();
    let (min, max) = colliders
        .iter(world)
        .filter(|&(entity, _)| {
            std::iter::successors(Some(entity), |&node| {
                world.get::<ChildOf>(node).map(ChildOf::parent)
            })
            .any(|node| node == target)
        })
        .fold((Vec3::MAX, Vec3::MIN), |(min, max), (_, aabb)| {
            (min.min(aabb.min), max.max(aabb.max))
        });
    assert!(min.cmple(max).all(), "the target has colliders");
    let aim = (min + max) / 2.0;
    let side = world
        .get::<GlobalTransform>(target)
        .expect("the target is posed")
        .right();
    let from = aim + side * ((max - min).length() + 2.0);
    *world.get_mut::<Transform>(shooter).expect("the camera") =
        Transform::from_translation(from).looking_at(aim, Vec3::Y);
    world.spawn(bevy::window::CursorOptions {
        grab_mode: bevy::window::CursorGrabMode::Locked,
        ..default()
    });
    let crate::command::ButtonBinding::Mouse(button) =
        world.resource::<crate::command::Bindings>().fire_primary
    else {
        panic!("the fire binding is a mouse button");
    };
    world.write_message(bevy::input::mouse::MouseButtonInput {
        button,
        state: bevy::input::ButtonState::Pressed,
        window: shooter,
    });
    let mut cameras = world.query_filtered::<(), With<Camera>>();
    let range_cameras = cameras.iter(world).count();

    let explained = |app: &App| {
        app.world()
            .resource::<ShotExplanations>()
            .iter()
            .any(|(_, stored)| {
                stored.explanation.audience == Audience::Shooter
                    && stored.explanation.shot.combatant == shooter_id
                    && stored.explanation.struck == struck
                    && stored.report.round == round
            })
    };
    // MEASURED: the record lands on frame 2, and with the corner admitting every hit its camera
    // opens on that same frame; the watch runs well past both.
    const CORNER_WATCH_FRAMES: usize = 120;
    for frame in 0..CORNER_WATCH_FRAMES {
        app.update();
        assert_eq!(
            cameras.iter(app.world()).count(),
            range_cameras,
            "a camera opened beside the range's own on frame {frame}: the pen cam corner",
        );
    }
    assert!(
        explained(&app),
        "no explanation of the range round reached the free shooter",
    );
}

/// The track-model sandbox boots and keeps running, through the deferred rig build.
///
/// `RigGeom` is the root's own "the rig is up" gate — nearly every system it schedules is
/// `run_if(resource_exists::<RigGeom>)`, so a boot that never reaches it would exercise almost
/// nothing and this guard would go quietly blind.
#[cfg(feature = "dev_ui")]
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

/// A STRAIGHT DRIVE ON THE SHIPPED HEIGHTFIELD — a measurement bed, never a gate (`-- --ignored
/// heightfield_drive_bed --nocapture`), in the game's tank driving physics (game state only): the
/// controlled tank settles on the shipped terrain, turns away from its duel partner and settles
/// again, then drives at full throttle for 10 s. Per phase it prints a digest of every tick's hull
/// pose bits and the end pose, so two builds compare bit for bit and a difference shows in the
/// phase that makes it, and where the tank's colliders touched with a normal impulse.
#[test]
#[ignore = "a measurement bed, run on demand"]
fn heightfield_drive_bed() {
    use avian3d::prelude::{ColliderOf, ContactGraph, Position, Rotation};
    use std::collections::BTreeMap;

    /// The tank's collider contacts that carried a normal impulse, for one collider node and
    /// what it touched.
    #[derive(Default)]
    struct Seen {
        points: usize,
        /// Σ normal impulse (N·s).
        impulse: f32,
        /// Hull-local |x| range and lowest y of the contact points (m).
        x: (f32, f32),
        y_lo: f32,
        /// First and last phase tick with a touch.
        ticks: (usize, usize),
    }

    /// One phase's pose-bit digest, its end pose, and what the tank's colliders touched.
    fn phase(
        app: &mut App,
        label: &str,
        ticks: usize,
        tank: Entity,
        mut drive: impl FnMut(&mut App),
    ) {
        let mut digest = 0xcbf2_9ce4_8422_2325_u64;
        let mut touches: BTreeMap<(String, &str), Seen> = BTreeMap::new();
        for tick in 0..ticks {
            drive(app);
            let world = app.world();
            let p = world.get::<Position>(tank).unwrap().0;
            let r = world.get::<Rotation>(tank).unwrap().0;
            for w in p.to_array().into_iter().chain(r.to_array()) {
                for b in w.to_bits().to_le_bytes() {
                    digest = (digest ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
                }
            }
            let to_local = Transform::from_translation(p)
                .with_rotation(r)
                .to_matrix()
                .inverse();
            let body = |c: Entity| world.get::<ColliderOf>(c).map_or(c, |of| of.body);
            let node = |c: Entity| {
                world
                    .get::<ChildOf>(c)
                    .and_then(|parent| world.get::<Name>(parent.parent()))
                    .map_or_else(|| "?".to_owned(), |name| name.as_str().to_owned())
            };
            for pair in world.resource::<ContactGraph>().iter_active() {
                let (ours, other) = if body(pair.collider1) == tank {
                    (pair.collider1, pair.collider2)
                } else if body(pair.collider2) == tank {
                    (pair.collider2, pair.collider1)
                } else {
                    continue;
                };
                let kind = if world.get::<Tank>(body(other)).is_some() {
                    "a tank"
                } else {
                    "the world"
                };
                for point in pair.manifolds.iter().flat_map(|m| m.points.iter()) {
                    if point.normal_impulse > 0.0 {
                        let local = to_local.transform_point3(point.point);
                        let seen = touches.entry((node(ours), kind)).or_insert(Seen {
                            x: (f32::INFINITY, 0.0),
                            y_lo: f32::INFINITY,
                            ticks: (tick, tick),
                            ..Default::default()
                        });
                        seen.points += 1;
                        seen.impulse += point.normal_impulse;
                        seen.x = (seen.x.0.min(local.x.abs()), seen.x.1.max(local.x.abs()));
                        seen.y_lo = seen.y_lo.min(local.y);
                        seen.ticks.1 = tick;
                    }
                }
            }
        }
        let p = app.world().get::<Position>(tank).unwrap().0;
        println!("heightfield {label}: digest {digest:016x}, ends at {p:?}");
        for ((node, other), seen) in touches {
            println!(
                "  {node} on {other}: {} points, hull-local |x| {:.3}..{:.3} m, lowest y {:.3} m, \
                 Σ normal impulse {:.0} N·s, phase ticks {}..={}",
                seen.points,
                seen.x.0,
                seen.x.1,
                seen.y_lo,
                seen.impulse,
                seen.ticks.0,
                seen.ticks.1,
            );
        }
    }

    let mut app = booted_sim_on(Some(crate::terrain_grid::tests::shipped_grid()));
    start_fixed_clock(&mut app);
    let tank = app
        .world_mut()
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>()
        .single(app.world())
        .expect("one controlled tank");
    phase(&mut app, "settle after spawn", 120, tank, |app| {
        app.update()
    });
    // Away from the duel partner (`face_positive_z`), so the drive meets only the terrain.
    {
        let mut e = app.world_mut().entity_mut(tank);
        e.get_mut::<Rotation>().unwrap().0 = Quat::from_rotation_y(std::f32::consts::PI);
        e.get_mut::<avian3d::prelude::LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<avian3d::prelude::AngularVelocity>().unwrap().0 = Vec3::ZERO;
    }
    phase(&mut app, "settle after the turn", 120, tank, |app| {
        app.update()
    });
    phase(
        &mut app,
        "straight drive, full throttle",
        640,
        tank,
        |app| drive_tick(app, tank, 1.0, 0.0),
    );
}

/// The game's damage model: live terrain must not mask armour in the round's broad phase.
/// A clear sight line resolves a material contact whether that construction skids, stops or
/// admits the round. Mutant: the world-spanning terrain collider hides the armour query.
#[test]
fn shipped_terrain_does_not_hide_armour_from_a_round() {
    use crate::ballistics::{FireShell, FireShellOrigin, Impact, ShotSource};
    use crate::damage::VolumeOf;
    use avian3d::prelude::{Collider, Position, Rotation, SimpleCollider};

    #[derive(Resource, Default)]
    struct ArmorImpacts(usize);

    let mut app = {
        let grid = crate::terrain_grid::tests::shipped_grid();
        let mut sim = booted_sim_on(Some(grid));
        start_fixed_clock(&mut sim);
        // Let both tanks settle onto the terrain before anything is fired at them.
        for _ in 0..30 {
            sim.update();
        }
        sim
    };
    app.init_resource::<ArmorImpacts>();
    app.add_observer(|impact: On<Impact>, mut log: ResMut<ArmorImpacts>| {
        if matches!(
            impact.surface,
            crate::ballistics::ImpactSurface::Material(_)
        ) {
            log.0 += 1;
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
        effects: crate::rounds::FlightEffects::fixture_traced(),
        muzzle: crate::guns::MuzzleEmission::fixture_cannon(),
        birth: origin,
        origin,
        direction,
        speed: 755.0,
        caliber: 0.088,
        mass: 10.2,
        drag_k: crate::ballistics::fixture_drag_k(0.088, 10.2),
        // Fixture hit nose: a hemispherical cap. Contact reads the separate type preset.
        nose_factor: 0.5,
        penetrator_caliber: 0.088,
        penetrator_mass: 10.2,
        wound: 30.0,
        burst: None,
        fuze_mode: 0,
        contact: crate::rounds::ContactPreset::fixture(),
        mechanism: crate::spec::FireMechanism::Single,
        shooter: Some(ShotSource {
            tank: shooter,
            weapon: 0,
            round: 0,
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
        impacts > 0,
        "the round produced NO armour impact on the shipped map — it flew through the target and \
         went on to the ground, exactly as it did in the live session. The corridor's broad-phase \
         armour gate is being masked by the world-spanning terrain collider again \
         (`ballistics::cast_disc_segment`).",
    );
}

/// THE REVERSE THROW (#188) — a measurement bed, never a gate (`-- --ignored reverse_throw_bed
/// --nocapture`): the controlled tank reverses at full throttle down a slope onto flat ground and
/// over one raised line of grid samples, on a synthetic grid at the shipped map's 1.465 m spacing.
/// Per variant it prints the root's peak upward speed and pitch rate and the largest single-tick
/// hull-terrain impulse, with that contact's normal, depth and hull-frame point: a rigid hull
/// backstop, the heightfield's internal-edge normals and the overlap push-out each leave a
/// different signature. The variants that clear the collider's `FIX_INTERNAL_EDGES` rebuild the
/// heightfield avian's own constructor builds. Fixture numbers (slope, ridge height, spawn) are
/// fixture data.
mod reverse_throw {
    use super::*;
    use avian3d::dynamics::solver::SolverConfig;
    use avian3d::parry::shape::{HeightFieldFlags, SharedShape};
    use avian3d::prelude::{
        AngularVelocity, Collider, Collisions, LinearVelocity, Position, Rotation,
    };
    use bevy::ecs::system::RunSystemOnce;

    use crate::terrain_grid::{FIXTURE_EXTENT, HeightGrid, TerrainExtent};

    const SIZE: u32 = 129;
    const SPACING_M: f32 = 1.465;
    const FOOT_J: u32 = 64;
    const RIDGE_J: u32 = FOOT_J + 3;
    const SLOPE_CELLS: u32 = 20;
    const SPAWN_Z: f32 = -10.0;
    const DRIVE_TICKS: usize = 8 * FIXED_TICKS_PER_SECOND;

    #[derive(Clone, Copy)]
    enum Ridge {
        None,
        /// One row of samples along world X, `j` constant: the crest is a grid line.
        Row,
        /// The samples with `i + j` constant: the crest runs along cell diagonals.
        Diagonal,
    }

    #[derive(Clone, Copy)]
    struct Variant {
        label: &'static str,
        slope_deg: f32,
        ridge: Ridge,
        ridge_m: f32,
        clear_internal_edges: bool,
        max_overlap_solve_speed: Option<f32>,
    }

    /// One hull-terrain manifold: `normal` points from the terrain into the tank, `depth` is the
    /// deepest point's penetration (negative: a speculative contact, still apart), `impulse` the
    /// tick's summed normal impulse (N s), `local` the points' mean in the root's frame.
    struct HullHit {
        tick: usize,
        normal: Vec3,
        depth: f32,
        impulse: f32,
        local: Vec3,
    }

    impl HullHit {
        fn text(&self) -> String {
            let (n, l) = (self.normal, self.local);
            format!(
                "n ({:5.2} {:5.2} {:5.2}) d {:6.3} J {:7.0} at ({:5.2} {:5.2} {:5.2})",
                n.x, n.y, n.z, self.depth, self.impulse, l.x, l.y, l.z,
            )
        }
    }

    /// A slope rising toward −Z that meets flat ground at row `FOOT_J`, plus the variant's ridge.
    fn grid(v: Variant) -> HeightGrid {
        let rise_per_cell = SPACING_M * v.slope_deg.to_radians().tan();
        let mut samples = Vec::with_capacity((SIZE * SIZE) as usize);
        for j in 0..SIZE {
            for i in 0..SIZE {
                let slope_h = rise_per_cell * FOOT_J.saturating_sub(j).min(SLOPE_CELLS) as f32;
                let on_ridge = match v.ridge {
                    Ridge::None => false,
                    Ridge::Row => j == RIDGE_J,
                    Ridge::Diagonal => i + j == SIZE / 2 + RIDGE_J,
                };
                samples.push(slope_h + if on_ridge { v.ridge_m } else { 0.0 });
            }
        }
        let extent = TerrainExtent {
            world_size_m: (SIZE - 1) as f32 * SPACING_M,
            ..FIXTURE_EXTENT
        };
        HeightGrid::new(samples.into(), SIZE, extent)
    }

    /// The terrain collider's entity, with the variant's collider and solver overrides applied.
    fn terrain(app: &mut App, v: Variant) -> Entity {
        let world = app.world_mut();
        let terrain = world
            .query::<(Entity, &Collider)>()
            .iter(world)
            .find(|(_, c)| c.shape().as_heightfield().is_some())
            .map(|(e, _)| e)
            .expect("the heightfield terrain collider");
        if v.clear_internal_edges {
            let mut collider = world.get_mut::<Collider>(terrain).unwrap();
            let mut field = collider.shape().as_heightfield().unwrap().clone();
            field.set_flags(HeightFieldFlags::empty());
            collider.set_shape(SharedShape::new(field));
        }
        if let Some(speed) = v.max_overlap_solve_speed {
            world.resource_mut::<SolverConfig>().max_overlap_solve_speed = speed;
        }
        terrain
    }

    fn place(app: &mut App, entity: Entity, at: Vec3, rot: Quat) {
        let mut e = app.world_mut().entity_mut(entity);
        e.get_mut::<Position>().unwrap().0 = at;
        e.get_mut::<Rotation>().unwrap().0 = rot;
        e.get_mut::<LinearVelocity>().unwrap().0 = Vec3::ZERO;
        e.get_mut::<AngularVelocity>().unwrap().0 = Vec3::ZERO;
    }

    /// Every manifold between `tank`'s colliders and the terrain after the last step.
    fn hull_hits(app: &mut App, tank: Entity, terrain: Entity, tick: usize) -> Vec<HullHit> {
        let pos = app.world().get::<Position>(tank).unwrap().0;
        let rot = app.world().get::<Rotation>(tank).unwrap().0;
        app.world_mut()
            .run_system_once(move |collisions: Collisions| {
                collisions
                    .collisions_with(terrain)
                    .filter(|pair| pair.body1 == Some(tank) || pair.body2 == Some(tank))
                    .flat_map(|pair| {
                        let flip = if pair.collider1 == terrain { 1.0 } else { -1.0 };
                        pair.manifolds.iter().map(move |m| HullHit {
                            tick,
                            normal: m.normal * flip,
                            depth: m
                                .points
                                .iter()
                                .map(|p| p.penetration)
                                .fold(f32::MIN, f32::max),
                            impulse: m.points.iter().map(|p| p.normal_impulse).sum(),
                            local: rot.inverse()
                                * (m.points.iter().map(|p| p.point).sum::<Vec3>()
                                    / m.points.len().max(1) as f32
                                    - pos),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap()
    }

    /// The tick's kinematics and belt loads (total, and behind 1.8 m in the hull frame: the idler
    /// end), one trace line.
    fn kinematics(app: &App, tank: Entity, tick: usize) -> (Vec3, f32, String) {
        let world = app.world();
        let pos = world.get::<Position>(tank).unwrap().0;
        let rot = world.get::<Rotation>(tank).unwrap().0;
        let vel = world.get::<LinearVelocity>(tank).unwrap().0;
        let pitch_rate = world
            .get::<AngularVelocity>(tank)
            .unwrap()
            .0
            .dot(rot * Vec3::X);
        let pitch_deg = (rot * Vec3::NEG_Z).y.asin().to_degrees();
        let (rear, all) =
            world
                .get::<crate::track::sim::TrackContacts>(tank)
                .map_or((0.0, 0.0), |sides| {
                    sides.0.iter().flatten().fold((0.0, 0.0), |(rear, all), c| {
                        let z = (rot.inverse() * (c.point - pos)).z;
                        (rear + if z > 1.8 { c.load } else { 0.0 }, all + c.load)
                    })
                });
        let line = format!(
            "t{tick:3} z {:6.2} y {:5.2} vz {:5.2} vy {:5.2} pitch {pitch_deg:6.2} rate \
             {pitch_rate:5.2} belt {:5.0} kN rear {:4.0} kN",
            pos.z,
            pos.y,
            vel.z,
            vel.y,
            all / 1000.0,
            rear / 1000.0,
        );
        (vel, pitch_rate, line)
    }

    fn run(v: Variant) {
        let ground = grid(v);
        let mut app = booted_sim_on(Some(ground.clone()));
        let terrain = terrain(&mut app, v);
        start_fixed_clock(&mut app);
        let world = app.world_mut();
        let tank = world
            .query_filtered::<Entity, (With<Tank>, With<Controlled>)>()
            .single(world)
            .expect("one controlled tank");
        let other = world
            .query_filtered::<Entity, (With<Tank>, Without<Controlled>)>()
            .single(world)
            .expect("the duel partner");
        place(&mut app, other, Vec3::new(50.0, 3.0, 40.0), Quat::IDENTITY);
        // The ride height on flat ground, read rather than assumed.
        place(&mut app, tank, Vec3::new(0.0, 3.0, 40.0), Quat::IDENTITY);
        for _ in 0..240 {
            app.update();
        }
        let rest = app.world().get::<Position>(tank).unwrap().0.y - ground.height_at(0.0, 40.0);
        // Nose up the slope (hull forward is −Z), so full reverse runs down it.
        let pitch = Quat::from_rotation_x(v.slope_deg.to_radians());
        let start = Vec3::new(0.0, ground.height_at(0.0, SPAWN_Z) + rest + 0.3, SPAWN_Z);
        place(&mut app, tank, start, pitch);
        for _ in 0..120 {
            app.update();
        }

        let mut peak_vy = (0.0_f32, 0);
        let mut peak_pitch_rate = (0.0_f32, 0);
        let mut peak_hit: Option<HullHit> = None;
        let mut deepest = (f32::MIN, 0);
        let mut trace: Vec<String> = Vec::new();
        for tick in 0..DRIVE_TICKS {
            drive_tick(&mut app, tank, -1.0, 0.0);
            let (vel, pitch_rate, mut line) = kinematics(&app, tank, tick);
            if vel.y > peak_vy.0 {
                peak_vy = (vel.y, tick);
            }
            if pitch_rate.abs() > peak_pitch_rate.0.abs() {
                peak_pitch_rate = (pitch_rate, tick);
            }
            for hit in hull_hits(&mut app, tank, terrain, tick) {
                line += &format!(" | {}", hit.text());
                if hit.depth > deepest.0 {
                    deepest = (hit.depth, tick);
                }
                if peak_hit.as_ref().is_none_or(|p| hit.impulse > p.impulse) {
                    peak_hit = Some(hit);
                }
            }
            trace.push(line);
        }

        eprintln!(
            "reverse throw [{}]: peak vy {:.2} m/s at t{}; peak pitch rate {:.2} rad/s at t{}; \
             deepest hull contact {:.3} m at t{}",
            v.label,
            peak_vy.0,
            peak_vy.1,
            peak_pitch_rate.0,
            peak_pitch_rate.1,
            deepest.0,
            deepest.1,
        );
        match &peak_hit {
            Some(h) => eprintln!("  largest hull impulse at t{}: {}", h.tick, h.text()),
            None => eprintln!("  no hull-terrain contact"),
        }
        let centre = peak_hit.as_ref().map_or(peak_vy.1, |h| h.tick);
        for line in &trace[centre.saturating_sub(16)..(centre + 8).min(trace.len())] {
            eprintln!("  {line}");
        }
    }

    #[test]
    #[ignore = "a measurement bed, run on demand"]
    fn reverse_throw_bed() {
        let base = Variant {
            label: "",
            slope_deg: 12.0,
            ridge: Ridge::Row,
            ridge_m: 0.25,
            clear_internal_edges: false,
            max_overlap_solve_speed: None,
        };
        let cleared = Variant {
            clear_internal_edges: true,
            ..base
        };
        let variants = [
            Variant {
                label: "A shipped collider",
                ..base
            },
            Variant {
                label: "B internal-edge fix cleared",
                ..cleared
            },
            Variant {
                label: "C fix cleared, max_overlap_solve_speed 0.5",
                max_overlap_solve_speed: Some(0.5),
                ..cleared
            },
            Variant {
                label: "D diagonal ridge",
                ridge: Ridge::Diagonal,
                ..base
            },
            Variant {
                label: "D' diagonal ridge, fix cleared",
                ridge: Ridge::Diagonal,
                ..cleared
            },
            Variant {
                label: "E no ridge",
                ridge: Ridge::None,
                ..base
            },
            Variant {
                label: "E' no ridge, fix cleared",
                ridge: Ridge::None,
                ..cleared
            },
        ];
        let only = std::env::var("OVERMATCH_THROW_VARIANT").ok();
        for v in variants {
            if only.as_deref().is_none_or(|o| v.label.starts_with(o)) {
                run(v);
            }
        }
    }
}

/// The game's environment destruction model: the shipped simulation must activate every map
/// construction through the common spawn, with complete state before any contact or view load.
/// Every placed construction contributes its declared roots and intact element colliders.
#[test]
fn shipped_map_constructions_use_complete_common_spawn_state() {
    use crate::construction::{Body, Element, breakage::Breakage};
    use crate::map::Prototype;
    use avian3d::prelude::Collider;
    let mut app = booted_sim_on(Some(crate::terrain_grid::tests::shipped_grid()));
    let manifest = app.world().resource::<crate::map::MapManifest>();
    let mut expected = std::collections::BTreeMap::<String, (usize, usize)>::new();
    for instance in &manifest.instances {
        match &manifest.prototypes[&instance.prototype] {
            Prototype::Construction { asset } => {
                let kind = manifest.constructions.get(asset).unwrap();
                let count = expected.entry(asset.clone()).or_default();
                count.0 += 1;
                count.1 += kind.asset.elements.len();
            }
            Prototype::Block { .. } => {}
        }
    }
    let world = app.world_mut();
    let roots: Vec<_> = world
        .query_filtered::<(&crate::scatter::MapInstance, &Breakage), With<Body>>()
        .iter(world)
        .collect();
    for (identity, body) in &roots {
        assert_eq!(body.construction, identity.ordinal);
        assert_eq!(
            body.live.state.pool,
            body.live
                .structure
                .chunks
                .iter()
                .map(crate::construction::fracture::full_pool)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            body.live.state.fate,
            vec![crate::construction::fracture::Fate::Standing; body.live.structure.chunks.len()]
        );
        assert_eq!(
            body.live.state.cracked,
            vec![false; body.live.structure.joints.len()]
        );
        assert_eq!(
            body.live.swapped,
            vec![false; body.kind.asset.elements.len()]
        );
        assert!(body.live.holes.iter().all(Vec::is_empty));
    }
    let roots = roots.len();
    assert_eq!(roots, expected.values().map(|count| count.0).sum::<usize>());
    let elements = world
        .query_filtered::<Entity, (With<Element>, With<Collider>)>()
        .iter(world)
        .count();
    assert_eq!(
        elements,
        expected.values().map(|count| count.1).sum::<usize>()
    );
    for (asset, (instances, colliders)) in expected {
        println!(
            "MEASURED live map {asset}: {instances} instances, {colliders} intact element colliders"
        );
    }
}

/// ADR-0014, SERVER-COMPOSITION HONESTY for the track prints (the game's cosmetic terrain marks):
/// the dedicated server's composition, on the shipped map's height grid with its tanks driving,
/// builds none of them — no stores, no stamp log, no upload queue. `world::spawn_environment`'s
/// heightmap branch is shared with the clients and builds the prints beside the ground's material,
/// so the window gate there and the track view owning the mount are what keep them off the server:
/// a mount moved into a shared plugin, or a store built outside the gate, fails here. The control
/// mounts the prints' plugin on a bare host, so the check cannot pass by looking for nothing.
#[test]
fn a_server_composition_builds_no_track_prints() {
    let mut control = headless_shell();
    control.add_plugins(crate::track_prints::plugin);
    finish_plugins(&mut control);
    assert!(
        crate::track_prints::composed(control.world()),
        "the prints' plugin leaves a footprint the check can see",
    );

    let grid = crate::terrain_grid::tests::shipped_grid();
    let mut app = booted_sim_on(Some(grid));
    start_fixed_clock(&mut app);
    for _ in 0..64 {
        let world = app.world_mut();
        for mut command in world
            .query_filtered::<&mut TankCommand, With<Controlled>>()
            .iter_mut(world)
        {
            command.throttle = 1.0;
        }
        app.update();
    }
    assert!(
        app.world()
            .contains_resource::<crate::terrain_grid::HeightGrid>(),
        "the heightmap world is the branch under test",
    );
    assert!(
        !crate::track_prints::composed(app.world()),
        "the server composition built track prints",
    );
}

/// EVERY COLLIDER THAT CAN TOUCH ANOTHER CARRIES ITS MATERIAL'S FRICTION UNDER THE ONE RULE, in
/// the game's physics (game state only): on the shipped map, headless, each contact-capable
/// collider (a filter that admits some layer) resolves, as Avian's narrow phase resolves it (its
/// own `Friction`, else its body's, else `DefaultFriction`), to its material's row under
/// `substances::PAIR_RULE`. The material is found by class, never by name: a ballistic volume's
/// substance, a vehicle proxy's `colliders` entry, a rigid track collider's (a bump stop or a side
/// plate) the sheet's `track.substance`, a graybox prototype's substance, and for the terrain the base ground
/// type. A collider of no class, one falling back to Avian's default, or
/// one under another rule (a frictionless `Min` or a `Max`, either of which makes a pair's value
/// one side's) fails here, naming it.
#[test]
fn every_collider_carries_its_materials_friction_under_the_one_rule() {
    use crate::ballistics::BallisticVolume;
    use crate::map::Prototype;
    use crate::scatter::{MapInstance, ScatterProxy};
    use crate::substances::{PAIR_RULE, SubstanceRegistry};
    use avian3d::prelude::{
        Collider, ColliderOf, CollisionLayers, DefaultFriction, Friction, LayerMask,
    };

    let mut app = booted_sim_on(Some(crate::terrain_grid::tests::shipped_grid()));
    let registry = SubstanceRegistry::shipped_static();
    let terrain = crate::ground_types::GroundTypes::load_shipped().terrain_friction();
    let spec = app
        .world()
        .resource::<crate::bake::TankBlueprint>()
        .spec
        .clone();
    let manifest = app.world().resource::<crate::map::MapManifest>().clone();
    let default = app.world().resource::<DefaultFriction>().0;
    let world = app.world_mut();
    let mut colliders = world.query_filtered::<(
        Entity,
        &CollisionLayers,
        Option<&Friction>,
        Option<&ColliderOf>,
        Option<&BallisticVolume>,
        Option<&ChildOf>,
        Has<ScatterProxy>,
        Option<&MapInstance>,
    ), With<Collider>>();
    let rows: Vec<_> = colliders
        .iter(world)
        .map(|(e, layers, own, of, volume, parent, scatter, instance)| {
            (
                e,
                *layers,
                own.copied(),
                of.map(|of| of.body),
                volume.map(|v| v.substance.clone()),
                parent.map(ChildOf::parent),
                scatter,
                instance.map(|i| i.id.clone()),
            )
        })
        .collect();
    let mut checked = 0;
    for (collider, layers, own, body, substance, parent, scatter, instance) in rows {
        if layers.filters == LayerMask::NONE {
            continue;
        }
        let resolved = own
            .or_else(|| body.and_then(|body| world.get::<Friction>(body).copied()))
            .unwrap_or(default);
        let name = world.get::<Name>(collider).map(|n| n.as_str().to_owned());
        let expected = if let Some(substance) = substance {
            registry.contact_friction(&substance).unwrap()
        } else if scatter {
            let id = instance.expect("a scatter proxy carries its instance");
            let record = manifest.instances.iter().find(|i| i.id == id).unwrap();
            match &manifest.prototypes[&record.prototype] {
                Prototype::Block { substance, .. } => registry.contact_friction(substance).unwrap(),
                other => panic!("a scatter proxy of {other:?}"),
            }
        } else if world
            .get::<crate::track::bump_stop::BumpStop>(collider)
            .is_some()
            || world
                .get::<crate::track::bump_stop::SidePlate>(collider)
                .is_some()
        {
            registry.contact_friction(&spec.track.substance).unwrap()
        } else if layers.memberships.has_all(crate::Layer::Vehicle) {
            let node = parent
                .and_then(|p| world.get::<Name>(p))
                .expect("a proxy hull hangs under its named node");
            crate::tank::proxy_friction(&spec, node.as_str())
        } else if layers.memberships.has_all(crate::Layer::Terrain) {
            terrain
        } else {
            panic!("collider {collider} ({name:?}, {layers:?}) is of no material class");
        };
        assert_eq!(
            resolved, expected,
            "collider {collider} ({name:?}) carries {resolved:?}, its material states {expected:?}"
        );
        assert_eq!(resolved.combine_rule, PAIR_RULE);
        checked += 1;
    }
    assert!(checked > 0, "the shipped map spawned no contact collider");
    println!("{checked} contact colliders carry their material's friction");
}

/// The game's environment destruction, game state only: a shipped-data hull pushes the
/// shipped house with the normal driving and contact systems. ASSUMED fixture placement and
/// initial closing speed; report topology, never tick time or an asset-specific pass threshold,
/// so it pins no law (the weight check's own fixtures do) and runs on demand.
#[test]
#[ignore = "a measurement bed, run on demand"]
fn a_shipped_data_hull_push_reports_environment_release_groups() {
    use crate::construction::breakage::Piece;
    use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
    let (mut app, tank) = booted_offline_sim();
    for _ in 0..128 {
        app.update();
    }
    let y = app.world().get::<Position>(tank).unwrap().y;
    let house_at = Vec3::new(20.0, 0.0, 20.0);
    let kind = crate::construction::tests::shipped_house();
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, app.world());
    crate::construction::spawn(
        &mut commands,
        &kind,
        crate::scatter::MapInstance {
            id: "push_fixture".to_owned(),
            ordinal: 60000,
        },
        Transform::from_translation(house_at),
    );
    queue.apply(app.world_mut());
    app.world_mut().entity_mut(tank).insert((
        Position(house_at + Vec3::new(-5.1, y, 0.0)),
        Rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)),
        LinearVelocity(Vec3::X * 1.2),
        AngularVelocity::ZERO,
    ));
    let intake = crate::construction::breakage::watch_contact_intake(&mut app, 60000, tank);
    for _ in 0..512 {
        drive_tick(&mut app, tank, 1.0, 0.0);
    }
    let groups: Vec<usize> = app
        .world_mut()
        .query::<&Piece>()
        .iter(app.world())
        .filter(|piece| piece.construction == 60000)
        .map(|piece| piece.chunks.len())
        .collect();
    assert!(
        intake.load(std::sync::atomic::Ordering::Relaxed),
        "the driving fixture must deliver a queued production contact to the wall"
    );
    println!(
        "MEASURED shipped-data hull push: {} chunks freed in {} groups, sizes {:?}",
        groups.iter().sum::<usize>(),
        groups.len(),
        groups
    );
}

/// Median, 95th percentile and mean of `values` (µs), with their count.
fn tick_price(values: &[f64]) -> String {
    if values.is_empty() {
        return "no ticks".to_owned();
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    format!(
        "{} ticks, p50 {:.0} p95 {:.0} mean {:.0}",
        sorted.len(),
        at(0.5),
        at(0.95),
        sorted.iter().sum::<f64>() / sorted.len() as f64
    )
}

/// The wall time of the last tick's driving phase, from a stamp ordered before
/// `SimPhase::DrivingForces` to one ordered after it (µs). An upper bound on the phase: the first
/// stamp may run before systems the phase waits on.
#[derive(Resource, Default)]
struct DrivePhaseClock {
    start: Option<Instant>,
    last: f64,
}

fn stamp_drive_phase_start(mut clock: ResMut<DrivePhaseClock>) {
    clock.start = Some(Instant::now());
}

fn stamp_drive_phase_end(mut clock: ResMut<DrivePhaseClock>) {
    let start = clock
        .start
        .take()
        .expect("the start stamp is ordered before the driving phase");
    clock.last = start.elapsed().as_secs_f64() * 1e6;
}

/// A PRICING BED, NOT A GATE (#202): the offline sim's wall time per fixed tick with loose pieces,
/// in the game's driving model and environment destruction (game state only), each window beside
/// the same sim without them. Windows, in order: the controlled tank parked on flat ground; parked
/// beside a settled, sleeping pile 40 m ahead (200 pieces, then 1000); parked again after each pile
/// is despawned; then driving at a fixed throttle over a one-layer carpet of pieces, every tick
/// classed by whether a belt bore on a piece; then parked on that carpet. FIXTURE DATA: each piece
/// a 0.5 × 0.25 × 0.5 m box at 1800 kg/m³, the rubble yard's brick density. Tick times (µs) are
/// the whole `App::update`, with the driving phase's share ([`DrivePhaseClock`]); a window's price
/// is its difference from the parked baseline. It asserts nothing about time; run it under the
/// `perf` profile with the machine quiet.
#[test]
#[ignore = "a pricing bed, run on demand"]
fn loose_pieces_priced_per_tick() {
    use crate::state::SimPhase;
    use crate::track::sim::TrackContacts;
    use avian3d::prelude::{
        AngularInertia, CenterOfMass, Collider, Mass, NoAutoAngularInertia, NoAutoCenterOfMass,
        NoAutoMass, Position, RigidBody, Rotation, Sleeping,
    };

    const WINDOW: usize = 640;
    let size = Vec3::new(0.5, 0.25, 0.5);
    let mass = 1800.0 * size.x * size.y * size.z;
    let (mut app, tank) = booted_offline_sim();
    app.init_resource::<DrivePhaseClock>().add_systems(
        FixedUpdate,
        (
            stamp_drive_phase_start.before(SimPhase::DrivingForces),
            stamp_drive_phase_end.after(SimPhase::DrivingForces),
        ),
    );
    face_positive_z(&mut app, tank);
    let phase = |app: &App| app.world().resource::<DrivePhaseClock>().last;
    let at = app.world().get::<Position>(tank).unwrap().0;

    let spawn = |app: &mut App, centres: &[Vec3]| -> Vec<Entity> {
        centres
            .iter()
            .map(|&centre| {
                app.world_mut()
                    .spawn((
                        crate::debris::DebrisPiece,
                        Transform::from_translation(centre),
                        Position(centre),
                        Rotation::default(),
                        RigidBody::Dynamic,
                        Collider::cuboid(size.x, size.y, size.z),
                        Mass(mass),
                        NoAutoMass,
                        AngularInertia::from_shape(&Cuboid::new(size.x, size.y, size.z), mass),
                        NoAutoAngularInertia,
                        CenterOfMass(Vec3::ZERO),
                        NoAutoCenterOfMass,
                        crate::substances::contact_friction(0.6),
                        crate::construction::breakage::piece_layers(),
                    ))
                    .id()
            })
            .collect()
    };
    // A stacked pile on the ground under `centre`: `count` pieces on a square grid 0.55 m apart,
    // five layers.
    let pile = |count: usize, centre: Vec3| -> Vec<Vec3> {
        let per_layer = count.div_ceil(5);
        let side = (per_layer as f32).sqrt().ceil() as usize;
        (0..count)
            .map(|i| {
                let (layer, k) = (i / per_layer, i % per_layer);
                let (ix, iz) = (k % side, k / side);
                Vec3::new(
                    centre.x + (ix as f32 - side as f32 / 2.0) * 0.55,
                    0.13 + layer as f32 * 0.26,
                    centre.z + (iz as f32 - side as f32 / 2.0) * 0.55,
                )
            })
            .collect()
    };
    let asleep = |app: &App, pieces: &[Entity]| {
        pieces
            .iter()
            .filter(|&&p| app.world().get::<Sleeping>(p).is_some())
            .count()
    };
    let settle = |app: &mut App, pieces: &[Entity]| {
        for tick in 0..(30 * FIXED_TICKS_PER_SECOND) {
            drive_tick(app, tank, 0.0, 0.0);
            if asleep(app, pieces) == pieces.len() {
                return tick;
            }
        }
        30 * FIXED_TICKS_PER_SECOND
    };
    let parked = |app: &mut App, label: &str, pieces: &[Entity]| {
        let mut micros = Vec::with_capacity(WINDOW);
        let mut driving = Vec::with_capacity(WINDOW);
        let mut awake = 0;
        for _ in 0..WINDOW {
            let start = Instant::now();
            drive_tick(app, tank, 0.0, 0.0);
            micros.push(start.elapsed().as_secs_f64() * 1e6);
            driving.push(phase(app));
            awake = awake.max(pieces.len() - asleep(app, pieces));
        }
        println!(
            "MEASURED {label}: tick {}; driving phase {}; {} pieces, at most {awake} awake",
            tick_price(&micros),
            tick_price(&driving),
            pieces.len()
        );
    };
    let clear = |app: &mut App, pieces: &[Entity]| {
        for &piece in pieces {
            app.world_mut().despawn(piece);
        }
        for _ in 0..FIXED_TICKS_PER_SECOND {
            drive_tick(app, tank, 0.0, 0.0);
        }
    };

    parked(&mut app, "parked, no pieces", &[]);
    for count in [200, 1000] {
        let pieces = spawn(&mut app, &pile(count, at + Vec3::Z * 40.0));
        let ticks = settle(&mut app, &pieces);
        println!(
            "MEASURED {count}-piece pile settled: {} of {count} asleep after {ticks} ticks",
            asleep(&app, &pieces)
        );
        parked(
            &mut app,
            &format!("parked beside a {count}-piece pile"),
            &pieces,
        );
        clear(&mut app, &pieces);
        parked(&mut app, "parked, no pieces", &[]);
    }

    // One layer under both belts: 8 columns across 4.4 m, 15 rows over 8.25 m, from 10 m ahead.
    let carpet: Vec<Vec3> = (0..15)
        .flat_map(|row| {
            (0..8).map(move |column| {
                Vec3::new(
                    at.x + (column as f32 - 3.5) * 0.55,
                    0.13,
                    at.z + 10.0 + row as f32 * 0.55,
                )
            })
        })
        .collect();
    let pieces = spawn(&mut app, &carpet);
    let ticks = settle(&mut app, &pieces);
    println!(
        "MEASURED carpet settled: {} of {} asleep after {ticks} ticks",
        asleep(&app, &pieces),
        pieces.len()
    );
    // Per class (flat before, a belt on a piece, flat after): tick and driving-phase times.
    let mut classes: [(Vec<f64>, Vec<f64>); 3] = Default::default();
    let mut contacts = Vec::new();
    let end = at.z + 10.0 + 15.0 * 0.55 + 12.0;
    for _ in 0..(40 * FIXED_TICKS_PER_SECOND) {
        let start = Instant::now();
        drive_tick(&mut app, tank, 0.35, 0.0);
        let micros = start.elapsed().as_secs_f64() * 1e6;
        let bearing = app
            .world()
            .get::<TrackContacts>(tank)
            .unwrap()
            .0
            .iter()
            .flatten()
            .filter(|c| c.collider.is_some_and(|e| pieces.contains(&e)) && c.load > 0.0)
            .count();
        let class = if bearing > 0 {
            contacts.push(bearing as f64);
            1
        } else if contacts.is_empty() {
            0
        } else {
            2
        };
        classes[class].0.push(micros);
        classes[class].1.push(phase(&app));
        if app.world().get::<Position>(tank).unwrap().0.z > end {
            break;
        }
    }
    for (label, (ticks, driving)) in [
        "driving on flat ground before the carpet",
        "driving with a belt on a piece",
        "driving on flat ground after the carpet",
    ]
    .iter()
    .zip(&classes)
    {
        println!(
            "MEASURED {label}: tick {}; driving phase {}",
            tick_price(ticks),
            tick_price(driving)
        );
    }
    println!(
        "MEASURED belt contacts on pieces per tick on the carpet: {}",
        tick_price(&contacts)
    );

    // Back onto the carpet's middle, then parked on it.
    let middle = at.z + 10.0 + 7.0 * 0.55;
    for _ in 0..(40 * FIXED_TICKS_PER_SECOND) {
        drive_tick(&mut app, tank, -0.35, 0.0);
        if app.world().get::<Position>(tank).unwrap().0.z < middle {
            break;
        }
    }
    for _ in 0..(3 * FIXED_TICKS_PER_SECOND) {
        drive_tick(&mut app, tank, 0.0, 0.0);
    }
    parked(&mut app, "parked on the carpet", &pieces);
    clear(&mut app, &pieces);
    parked(&mut app, "parked, no pieces", &[]);
}

#[cfg(feature = "bitprobe")]
mod drive_load;
#[cfg(feature = "bitprobe")]
pub(crate) use drive_load::coast_equilibrium;
