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

fn start_fixed_clock(app: &mut App) {
    // Verified against Bevy 0.19: one `App::update` runs exactly one fixed loop.
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
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
    // DELETE THESE LINES when `src/tests/bevy_ktx2_uastc_fallback.rs` fails — that failure IS the
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
/// a host that mounts the view half alone:
///   * a round at or above `TRACER_MAX_CALIBER` is dressed with `ShellVisual` + its `shell.glb`
///     scene root (`WorldAssetRoot`);
///   * a sub-calibre tracer round streams a `TracerStreak` child and gets NO scene root;
///   * a sub-calibre round without a tracer gets no scene root either.
///
/// The main-gun arm is load-bearing: without it the "no scene root" assertions could be satisfied
/// by a view plugin that dresses nothing. [`a_server_composition_dresses_no_round_at_all`] is the
/// gate for the server side, which mounts no view at all.
#[test]
fn mg_rounds_stream_tracers_and_spawn_no_shell_scene() {
    use crate::ballistics::{
        Projectile, ShellPath, ShellVisual, TRACER_MAX_CALIBER, TracerRound, TracerStreak,
    };
    use bevy::world_serialization::WorldAssetRoot;

    let mut app = headless_shell();
    app.add_plugins(crate::ballistics::view_plugin);
    finish_plugins(&mut app);
    // `Startup` builds the shell scene handle and the streak mesh/material the observer clones.
    app.update();

    let round = |caliber: f32| {
        (
            Projectile::view_test_round(caliber),
            Transform::default(),
            ShellPath::default(),
        )
    };
    let world = app.world_mut();
    let main_gun = world.spawn(round(2.0 * TRACER_MAX_CALIBER)).id();
    let tracer = world
        .spawn((round(0.5 * TRACER_MAX_CALIBER), TracerRound))
        .id();
    let plain = world.spawn(round(0.5 * TRACER_MAX_CALIBER)).id();
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
        "the sub-calibre tracer round, and only it, must stream a TracerStreak",
    );
    assert!(
        !scene(tracer) && !scene(plain),
        "a sub-calibre round spawned a shell.glb scene root (WorldAssetRoot) — MG bullets must NOT \
         render as main-gun shell scenes",
    );
    assert!(
        world.get::<ShellVisual>(main_gun).is_some() && scene(main_gun),
        "a main-gun round was never dressed — it must get ShellVisual + its shell.glb scene root \
         from ballistics::view_plugin",
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

    // The dedicated server's ballistics composition, on the real spawn path.
    let mut app = booted_sp_app();

    let mut tank_q = app
        .world_mut()
        .query_filtered::<Entity, (With<Tank>, With<Controlled>)>();
    let tank = tank_q.single(app.world()).expect("one controlled tank");

    // Clear the bore: the map parks a second tank ~17 m down the barrel, and an 88 covers that
    // inside ONE fixed tick — the round strikes armour and is gone before any sample below can see
    // it fly. Park the other tank far off the firing line. What a server-composed round CARRIES is
    // this test's subject; what it hits is not.
    let world = app.world_mut();
    let mut others = world
        .query_filtered::<&mut avian3d::prelude::Position, (With<Tank>, Without<Controlled>)>();
    for mut position in others.iter_mut(world) {
        position.0 += Vec3::X * 500.0;
    }

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
        drag_k: crate::ballistics::fixture_drag_k(0.0079, 0.0118),
        nose_factor: 0.5,
        penetrator_caliber: 0.0079,
        penetrator_mass: 0.0118,
        wound: 10.0,
        burst: None,
        fuze_mode: 0,
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
        round: 0,
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

/// The pivot gate body: zero throttle, full steer, ≥ 4 s; the mean yaw rate over the last second
/// must clear the adapter's floor and the belts must actually counter-rotate. ZERO is the original
/// bug this pins: the Tiger's steering capacity read on the wrong axis could not break its own
/// footprint scrub.
fn tiger_pivot_gate(
    app: &mut App,
    tank: Entity,
    mode: crate::track::transmission::TransmissionMode,
    min_yaw: f32,
) {
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

/// Tiger pivot under each steering adapter, on one boot; each adapter's pivot scale is a different
/// LAW, so each has its own floor:
///
/// * L600 fixed-radius adapter (the vehicle's authored architecture): the MARGINAL brake-gated
///   neutral turn toward the DERIVED `neutral_d_full` = 0.2885 m/s (0.2808 before half_tread went
///   1.4904 → 1.5312; no unprovenanced 0.75 fraction shrinks the target any more). MEASURED on the
///   declared data: 0.131 rad/s mean ground yaw, belts exactly ±neutral_d_full (the
///   belt-kinematic ceiling d/half-tread ≈ 0.188 rad/s, less scrub slip); gated at ≥ 0.10 rad/s
///   (margin for platform float drift — the restoration literature's "technically yes, advisable
///   no" crawl is exactly this regime).
/// * Hybrid continuous adapter: POWER-limited (the standstill pivot commands steer force up to
///   capacity and the power-conservation scale is the binding limiter, so the rate settles where
///   engine power balances scrub dissipation; the old neutral_d_full speed FLOOR used ~68 kW of the
///   ~407 kW budget and pivoted at 0.131 rad/s). MEASURED on the declared data: 0.654 rad/s mean
///   ground yaw pre-stage-B; 0.646 rad/s with the stage-B crank (the declutched steer demand parks
///   the crank at the same peak-torque operating point the old rev floor used, minus the
///   rev-governor's ~30 rpm taper droop — steady rate preserved by design); gated at ≥ 0.35 rad/s
///   (margin, same policy as the L600 gate).
///
/// The adapter switches as the offline `T` key does: the dial moves and every tank's transmission
/// restarts from its constructed state. The hybrid run starts from the L600 pivot; the floor is
/// read on its last second, past the spin-up.
#[test]
fn pivot_tiger_clears_each_steering_adapters_yaw_floor() {
    use crate::track::sim::{TankTransmission, TransmissionFeelTest};
    use crate::track::transmission::TransmissionMode;

    let (mut app, tank) = booted_offline_sim(TransmissionMode::FixedRadii);
    tiger_pivot_gate(&mut app, tank, TransmissionMode::FixedRadii, 0.10);

    app.insert_resource(TransmissionFeelTest(TransmissionMode::Hybrid));
    let fresh = fresh_tank_transmission(&app);
    let world = app.world_mut();
    for mut state in world.query::<&mut TankTransmission>().iter_mut(world) {
        *state = fresh;
    }
    tiger_pivot_gate(&mut app, tank, TransmissionMode::Hybrid, 0.35);
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
        .expect("the tank declares a transmission")
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
        if impact.surface == crate::ballistics::ImpactSurface::Armor {
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

/// THE LYING PIECE'S COLLIDERS LIE DOWN IN THE PHYSICS WORLD (A2, ADR-0016): a tree felled on the
/// shipped terrain has, within a few ticks, its tip segment carried in avian's own bounds — the
/// `ColliderAabb` its broad phase reads — where the body's own data puts it, and its stump's
/// bounds where they stood; the segments between derive by the same law and are not read here.
/// Standing, the tip's bounds hold its bottom and top centres, `prefix[n-1]` and `prefix[n]` up
/// the root's axis; down, they hold the same centres laid along the root's resting record — the
/// record's seat, then the height above the cut along `(dx sin t, cos t, dz sin t)` — and no
/// longer the standing top. A static body's child collider whose bounds avian refreshed
/// only at insert leaves the fallen trunk standing for every hull and round; a lying piece
/// placed off its pivot lies outside its own record. Both red this.
#[test]
fn a_felled_trees_colliders_lie_down_in_the_physics_world() {
    use crate::map::Prototype;
    use crate::trees::{Resting, TreeBody, TreeState};
    use avian3d::prelude::ColliderAabb;

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
    *app.world_mut().get_mut::<TreeState>(root).unwrap() = TreeState::Down { cut, dir: 0 };
    for _ in 0..3 {
        app.update();
    }
    let down = bounds(&app);
    assert_eq!(down[0], standing[0], "the stump's bounds stay");
    let tip_down = down[n - 1];
    let resting = *app
        .world()
        .get::<Resting>(root)
        .expect("the resting record");
    let (sin, cos) = (libm::sinf(resting.tilt), libm::cosf(resting.tilt));
    let laid = Vec3::new(resting.dx * sin, cos, resting.dz * sin);
    for k in [n - 1, n] {
        let point = base + resting.seat + laid * (prefix[k] - prefix[usize::from(cut)]);
        assert!(
            holds(&tip_down, point),
            "down: the tip segment's bounds {tip_down:?} hold its laid centre {point}"
        );
    }
    assert!(
        !holds(&tip_down, standing_top),
        "down: the tip segment's bounds {tip_down:?} left the standing top {standing_top}"
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
