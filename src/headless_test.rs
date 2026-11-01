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

/// Backstop only — NOT a performance budget. With the boots serialized (see [`BOOT_LEASE`]) a boot
/// has the whole box to itself, so it finishes in seconds; this bound exists purely so a genuine
/// hang (a wiring bug that never reaches `Playing`) fails with the diagnosis below instead of
/// sitting until the CI job timeout. It is generous on purpose: the loop exits the instant the sim
/// is up, so a wide bound costs a healthy run exactly nothing.
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

// --- Tiger transmission gates -------------------------------------------------------------------
//
// The process fix behind the phase-2.5 postmortem: every physics gate ran on the sandbox's T-34
// lab vehicle, and vehicle-scaling defects (steering capacity vs footprint scrub) sailed through
// on the smaller tank. These gates drive the REAL Tiger blueprint through the offline
// composition — the same boot, spawn path, spec, and terrain the `--offline` feel session runs.
// They are permanent `cargo test` members: the sandbox gates remain, but can never again be the
// only physics evidence.

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

/// The fix-1 smoking gun: a standstill full-throttle climb must walk the Tiger ladder
/// MONOTONICALLY. Pre-fix, every shift's own torque-cut window bled belt speed
/// (I·v̇ = Q − R keeps subtracting the ground reaction while Q is cut) and the low gears'
/// steep rpm-per-speed slope turned that into hundreds of rpm — the down band fired the
/// tick the freeze lifted (measured trace [1,2,1,2,1,2,3,2,3,4,3,4,5,6,7,8]). With the
/// predicted-landing gate + reversal dwell the gear sequence never decreases.
#[test]
fn gear_climb_monotone_tiger() {
    let (mut app, tank) = booted_offline_sim();
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

/// No authored speed cap bounds the game's tanks: the top gear at the fuel governor does. Full
/// throttle on flat ground must reach the ladder's top gear, come within 3 % (ASSUMED acceptance
/// band for rolling losses) of its DERIVED governed speed, and hold there: steady over the last
/// second, and never past the rpm where the governor has cut the engine's torque to zero. A
/// governor or ladder mutant that lets the hull run away, or stalls it short of top gear, fails.
#[test]
fn full_throttle_reaches_and_holds_the_governed_top_speed() {
    let (mut app, tank) = booted_offline_sim();
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
            speed <= ceiling,
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
        mean >= 0.97 * governed,
        "top speed {mean:.3} m/s short of the governed {governed:.3} m/s"
    );
    assert!(
        spread <= 0.01 * governed,
        "top speed must hold steady (spread {spread:.4} m/s over the last second)"
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
                point.body == tank && world.get::<BumpStop>(point.body_collider).is_some()
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

/// THE GAME'S BUOYANCY CARRIES A DRIVING TANK ONTO LOOSE PIECES. A sandbox-only adapter, sparse
/// rubble probes or a missing game-side reaction fails this gate or the footprint/clamp gates in
/// `track::rubble`. The pieces lie between the rigid side slices: neither the slices nor the
/// compression stops may supply their lift. FIXTURE DATA: two long pieces, height 0.1 m, each
/// one eighth of the hull's mass; dimensions and positions otherwise come from its running gear.
/// ASSUMED acceptance: the hull rises by half to 1.25 times the piece height, and a piece's centre
/// never rises a whole piece height above its starting centre.
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
                crate::buildings::breakage::piece_layers(),
            ))
            .id()
    });
    let mut carried = false;
    let mut max_piece_y = height / 2.0;
    let mut max_piece_speed = 0.0_f32;
    let mut arrived = false;
    for _ in 0..(20 * FIXED_TICKS_PER_SECOND) {
        drive_tick(&mut app, tank, 0.35, 0.0);
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
        for piece in pieces {
            max_piece_y = max_piece_y.max(world.get::<Position>(piece).unwrap().0.y);
            max_piece_speed =
                max_piece_speed.max(world.get::<LinearVelocity>(piece).unwrap().0.length());
        }
        if world.get::<Position>(tank).unwrap().0.z > front + span {
            arrived = true;
            break;
        }
    }
    assert!(arrived, "the tank never drove onto the fixture pieces");
    for _ in 0..(4 * FIXED_TICKS_PER_SECOND) {
        drive_tick(&mut app, tank, 0.0, 0.0);
        for piece in pieces {
            max_piece_y = max_piece_y.max(app.world().get::<Position>(piece).unwrap().0.y);
            max_piece_speed =
                max_piece_speed.max(app.world().get::<LinearVelocity>(piece).unwrap().0.length());
        }
    }
    let rise = app.world().get::<Position>(tank).unwrap().0.y - at.y;
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
        "MEASURED driven hull rise {rise:.6} m over {height} m pieces; peak piece centre {max_piece_y:.6} m, peak speed {max_piece_speed:.6} m/s; DERIVED coverage share {share:.4}, share-law rise {:.6} m",
        share * height
    );
    assert!(
        carried,
        "the game's track reports never carried the tank on a loose piece"
    );
    assert!(
        (height * 0.5..=height * 1.25).contains(&rise),
        "the hull rose {rise} over a {height} m piece"
    );
    assert!(
        max_piece_y < height * 1.5,
        "a piece was launched above its own height"
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
    /// Slope-segment witnesses of the transmission's grade memory.
    saw_grade_demand: bool,
    saw_grade_confirm: bool,
    saw_grade_target: bool,
    saw_hill_hold: bool,
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
    // near-rest band while steer is commanded.
    let _ = (position, rotation, linear, angular, local_com);
    let loaded_contact_is_slipping = contacts
        .0
        .iter()
        .flatten()
        .any(|c| c.load > 0.0 && c.slip.abs() > 0.3);
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
    let transmission_advanced = *app
        .world()
        .get::<crate::track::sim::TankTransmission>(tank)
        .expect("the controlled tank carries transmission state")
        != fresh_tank_transmission(&app);

    // The slope segment, on the declared transmission: the tank is placed rolling uphill on the
    // course's 20° face in F6 under Sequential addressing, belts and hull matched at a fixture
    // 4.0 m/s, W held. Paying adjacent shift windows accumulates the demand EMA and
    // grade-confirmation evidence, retains a grade target, and latches hill hold before it crests.
    app.world_mut()
        .resource_mut::<crate::track::sim::TrackGear>()
        .trans_mut()
        .shift_addressing = crate::track::transmission::ShiftAddressing::Sequential;
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
        *e.get_mut::<crate::track::sim::DriveSignal>().unwrap() = crate::track::sim::DriveSignal {
            throttle: 1.0,
            steer: 0.0,
        };
        let mut drive = e.get_mut::<crate::track::sim::TrackDrive>().unwrap();
        drive.sides[0].speed = approach_speed;
        drive.sides[1].speed = approach_speed;
        *e.get_mut::<crate::track::sim::TankTransmission>().unwrap() = transmission;
    }
    let (mut demand, mut confirm, mut target, mut hold) = (false, false, false, false);
    for _ in 0..SLOPE_TICKS {
        drive_tick(&mut app, tank, 1.0, 0.0);
        let state = app
            .world()
            .get::<crate::track::sim::TankTransmission>(tank)
            .expect("the controlled tank carries transmission state")
            .0;
        demand |= state.demand_n > 0.0;
        confirm |= state.grade_confirm_ticks > 0;
        target |= state.grade_target > 0;
        hold |= state.hill_hold;
    }

    let mut run = app
        .world_mut()
        .remove_resource::<ScriptedDeterminismRun>()
        .expect("the scripted digest collector remains installed");
    run.transmission_advanced = transmission_advanced;
    run.saw_grade_demand = demand;
    run.saw_grade_confirm = confirm;
    run.saw_grade_target = target;
    run.saw_hill_hold = hold;
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
        run.saw_grade_demand,
        "{label} slope segment exercised the demand EMA",
    );
    assert!(
        run.saw_grade_confirm,
        "{label} slope segment accumulated grade-deficit evidence",
    );
    assert!(
        run.saw_grade_target,
        "{label} slope segment retained a sequential grade target",
    );
    assert!(run.saw_hill_hold, "{label} slope segment latched hill hold");
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
/// transmission's grade memory (demand EMA, confirmation counter, held target, hill-hold latch)
/// replays exactly too. The witness assertions
/// keep this from passing because the scenario never reached contact, element strain, slip
/// traction, steering slip, fire, a transmission off its spawn state, or that grade memory.
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

/// Every string a `Text` node shows this frame — its own `Visibility` and every ancestor's other
/// than `Hidden`: the sandboxes' HUD surface, read without reaching into another module's private
/// marker components.
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
        app.world().resource::<crate::WeaponClock>().0 > 0,
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

    /// Whether each armour impact the round produced entered the material.
    #[derive(Resource, Default)]
    struct ArmorImpacts(Vec<bool>);

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
            log.0.push(impact.outcome.verdict().entered());
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

    let total_hp = |app: &mut App| -> u32 {
        let world = app.world_mut();
        world
            .query::<&ComponentHealth>()
            .iter(world)
            .map(|health| health.held())
            .sum()
    };
    let before = total_hp(&mut app);
    assert!(before > 0, "the target carries no HP pools to lower");

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
        origin,
        direction,
        speed: 755.0,
        caliber: 0.088,
        mass: 10.2,
        drag_k: crate::ballistics::fixture_drag_k(0.088, 10.2),
        nose_factor: 0.3,
        penetrator_caliber: 0.088,
        penetrator_mass: 10.2,
        wound: 30.0,
        burst: None,
        fuze_mode: 0,
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

/// THE FOREST'S TICK COST — a measurement bed, never a gate (`-- --ignored forest_tick_cost_bed
/// --nocapture`): the shipped map's trees as one static body with a convex hull per segment,
/// against the graybox cones they replaced and against no trees, on the shipped terrain with the
/// duel's two tanks idle and no tree touched. Prints per-tick wall-clock percentiles of whole
/// fixed ticks under the test profile, so only the differences between the rows carry meaning.
#[test]
#[ignore = "a measurement bed, run on demand"]
fn forest_tick_cost_bed() {
    use crate::map::Prototype;
    const SETTLE: usize = 300;
    const TICKS: usize = 600;
    let shipped = crate::map::tests::shipped_manifest();
    let cones = {
        let mut manifest = shipped.clone();
        // The graybox the trees replaced, in the library's id order (prototypes.ron records them).
        let sizes = [(18.96, 6.5), (14.08, 5.8), (14.55, 6.2)];
        let mut k = 0;
        for prototype in manifest.prototypes.values_mut() {
            if matches!(prototype, Prototype::Tree { .. }) {
                let (height_m, canopy_diameter_m) = sizes[k % sizes.len()];
                *prototype = Prototype::Fir {
                    height_m,
                    canopy_diameter_m,
                    substance: "Wood".to_owned(),
                };
                k += 1;
            }
        }
        manifest
    };
    let bare = {
        let mut manifest = shipped.clone();
        let prototypes = manifest.prototypes.clone();
        manifest
            .instances
            .retain(|instance| !matches!(prototypes[&instance.prototype], Prototype::Tree { .. }));
        manifest
    };
    let mut rows = Vec::new();
    for (label, manifest) in [
        ("trees, a hull per segment", shipped),
        ("cones, the graybox", cones),
        ("no trees", bare),
    ] {
        let grid = crate::terrain_grid::tests::shipped_grid();
        let mut app = booted_sim_on_map(Some(grid), Some(manifest));
        start_fixed_clock(&mut app);
        for _ in 0..SETTLE {
            app.update();
        }
        let mut ticks_us: Vec<u128> = (0..TICKS)
            .map(|_| {
                let started = Instant::now();
                app.update();
                started.elapsed().as_micros()
            })
            .collect();
        ticks_us.sort_unstable();
        let world = app.world_mut();
        let colliders = world
            .query::<&avian3d::prelude::Collider>()
            .iter(world)
            .count();
        let trees = world.query::<&crate::trees::TreeBody>().iter(world).count();
        rows.push(format!(
            "{label}: {trees} tree bodies, {colliders} colliders; per tick p50 {} us, p95 {} us, \
             max {} us",
            ticks_us[TICKS / 2],
            ticks_us[TICKS * 95 / 100],
            ticks_us[TICKS - 1],
        ));
    }
    for row in &rows {
        eprintln!("forest tick cost: {row}");
    }
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

/// A freed trunk's segment bounds follow its finite-mass physics body, while the stump stays
/// where it stood. The game's damage-model casts and hull contacts rely on these live bounds;
/// moving only the view or leaving child colliders under the map body fails this fixture.
#[test]
fn a_felled_trees_colliders_follow_its_dynamic_body() {
    use crate::map::Prototype;
    use crate::trees::{TreeBody, TreePiece, TreeState};
    use avian3d::prelude::ColliderAabb;
    use avian3d::prelude::{Position, RigidBody, Rotation};
    let shipped = crate::map::tests::shipped_manifest();
    // Fixture selection: one tree kind the shipped map places. Every bound below is read from the
    // spawned body's own data and its resting record, never from this kind's numbers.
    let kind = shipped
        .trees
        .get("fir_1")
        .expect("the shipped map's tree")
        .clone();
    let bare = {
        let mut manifest = shipped;
        let prototypes = manifest.prototypes.clone();
        manifest
            .instances
            .retain(|instance| !matches!(prototypes[&instance.prototype], Prototype::Tree { .. }));
        manifest
    };
    let grid = crate::terrain_grid::tests::shipped_grid();
    let base = Vec3::new(40.0, grid.height_at(40.0, 40.0), 40.0);
    let mut app = booted_sim_on_map(Some(grid), Some(bare));
    start_fixed_clock(&mut app);
    let root = {
        let world = app.world_mut();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, world);
        crate::trees::spawn(
            &mut commands,
            &kind,
            crate::scatter::MapInstance {
                id: "fir_bed".to_owned(),
                ordinal: 0,
            },
            Transform::from_translation(base),
        );
        queue.apply(world);
        world
            .query_filtered::<Entity, With<TreeBody>>()
            .single(world)
            .expect("one tree")
    };
    for _ in 0..3 {
        app.update();
    }
    // The segments in spawn order: bottom first.
    let bounds = |app: &App| -> Vec<ColliderAabb> {
        let world = app.world();
        world
            .get::<Children>(root)
            .expect("segments")
            .iter()
            .filter_map(|child| world.get::<ColliderAabb>(child).copied())
            .collect()
    };
    let prefix = app
        .world()
        .get::<TreeBody>(root)
        .expect("the body")
        .prefix
        .clone();
    let n = kind.segments.len();
    // DERIVED tolerance: every coordinate read is under 256 m (asserted), where one f32 rounding
    // is at most 2⁻¹⁷ m ≈ 7.6 µm; avian's vertex route and this one differ by a few roundings,
    // far under the millimetre.
    const TOLERANCE: f32 = 1.0e-3;
    assert!(base.abs().max_element() + prefix[n] < 256.0);
    let holds = |aabb: &ColliderAabb, point: Vec3| {
        (aabb.min - Vec3::splat(TOLERANCE)).cmple(point).all()
            && point.cmple(aabb.max + Vec3::splat(TOLERANCE)).all()
    };
    let standing = bounds(&app);
    assert_eq!(standing.len(), n);
    let tip_standing = standing[n - 1];
    let standing_top = base + Vec3::Y * prefix[n];
    for point in [base + Vec3::Y * prefix[n - 1], standing_top] {
        assert!(
            holds(&tip_standing, point),
            "standing: the tip segment's bounds {tip_standing:?} hold its centre {point}"
        );
    }
    let cut = 1;
    let parts = app
        .world()
        .get::<crate::trees::TreeParts>(root)
        .unwrap()
        .0
        .clone();
    let body = app.world().get::<TreeBody>(root).unwrap();
    let mass: f32 = body.structure.chunks[usize::from(cut)..]
        .iter()
        .map(|chunk| chunk.mass)
        .sum();
    let at = body.structure.chunks[n - 1].centroid;
    app.world_mut().entity_mut(root).insert((
        TreeState::Down { cut, dir: 0 },
        crate::trees::TrunkLoads(vec![crate::buildings::fracture::Load {
            chunk: n - 1,
            at,
            blow: Vec3::X * mass * 2.0,
            press: Vec3::ZERO,
            energy: 0.0,
            crushed: 0.0,
            peak: None,
            origin: crate::buildings::fracture::Origin::Field,
        }]),
    ));
    for _ in 0..128 {
        app.update();
    }
    let piece = app
        .world_mut()
        .query_filtered::<Entity, With<TreePiece>>()
        .single(app.world())
        .unwrap();
    assert_eq!(
        app.world().get::<RigidBody>(piece),
        Some(&RigidBody::Dynamic)
    );
    assert_eq!(
        *app.world().get::<ColliderAabb>(parts[0]).unwrap(),
        standing[0]
    );
    let tip = parts[n - 1];
    let bound = app.world().get::<ColliderAabb>(tip).unwrap();
    let position = app.world().get::<Position>(tip).unwrap().0;
    let rotation = app.world().get::<Rotation>(tip).unwrap().0;
    for point in [
        position,
        position + rotation * Vec3::Y * (prefix[n] - prefix[n - 1]),
    ] {
        assert!(
            holds(bound, point),
            "the live tip bounds {bound:?} hold {point}"
        );
    }
    assert!(
        !holds(bound, standing_top),
        "the tip left its standing bound"
    );
    assert_eq!(
        app.world()
            .get::<avian3d::prelude::ColliderOf>(tip)
            .unwrap()
            .body,
        piece
    );
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
                Prototype::Building { substance, .. } | Prototype::Fir { substance, .. } => {
                    registry.contact_friction(substance).unwrap()
                }
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
