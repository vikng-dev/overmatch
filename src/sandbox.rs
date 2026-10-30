//! The armour sandbox, mounted only by `bin/armor_sandbox`: a shooting range for the game's damage
//! model.
//!
//! The free-fly camera holds the seed tank's primary weapon as a [`FreeShooter`] ([`RangeGun`]) and
//! fires its selected round down the view axis through the game's round-to-shell mapping
//! ([`FireShell::of_round`]). Downrange stand four static RHA plates and the seed tank.
//!
//! * **The look explains the tank.** A shot on the tank is recorded and delivered as in
//!   single-player (`damage_cam::recorder`, `delivery::offline_plugin`). The game's x-ray draws it:
//!   its subject here is the target ([`XRaySubject::Struck`]), its records the free shooter's of
//!   hits on the target's current life, its tint the target's live HP, its masks the free-fly
//!   camera's (the one [`PlayerView`]). Each record replays once on the look's clock to its hold
//!   beat and holds until the next shot; X toggles the look, on from the start. The pen cam corner
//!   stays closed by its own setting ([`PenCamCorner::Off`]).
//! * **The rebuild.** A knocked-out target stands until R (or the panel's Rebuild) rebuilds it. The
//!   look lets go of the target before it goes ([`TargetRebuild::rebuild`]).
//! * **The rest is gizmos.** A shot that met no tank — a plate, the ground — draws its path, its
//!   crossings and its end; a shot that met a tank draws none of that, and its spent shell goes
//!   ([`sweep_tank_shots`]). The armour overlay (F2, off by default) draws the target's armour
//!   plates over everything, which the look draws as one body with the rest of the hull.
//!
//! * **Text only at a glance.** The view carries the selected round's name (its speed beside it only
//!   when overridden, [`round_line`]), the aim dot and the key legend, hidden until H. The look shows
//!   the target's crew and module state, so no crew bar or status card is mounted; the egui panel
//!   holds the controls and what the picture cannot show (time rate, march cadence).
//!
//! Sandbox controls use real time so inspection remains available while simulation time is paused.
//!
//! For in-engine captures (`dev_capture`) the range runs on an autopilot instead of the mouse:
//! `OVERMATCH_RANGE_POSE=x,y,z,lx,ly,lz` stands the camera-gun at `x,y,z` looking at `lx,ly,lz`,
//! and `OVERMATCH_RANGE_FIRE=<round>@<s>[,…]` fires the gun's round `<round>` (an index into its
//! rounds) down the view axis at app second `<s>`. `OVERMATCH_RANGE_FLY=x,y,z,lx,ly,lz@<a>-<b>`
//! moves the eye and the point looked at in a straight line from the pose's to these between app
//! seconds `<a>` and `<b>`, then holds there; while it is set the app clock steps a fixed 1/30 s a
//! frame, so a frame's pose and the shot's tick are the same in every run. Any one leaves the
//! cursor free. `OVERMATCH_RANGE_XRAY=0` starts the range with the x-ray off, the target in its own
//! surface.

use std::collections::HashSet;

use avian3d::prelude::{Collider, CollisionLayers, LayerMask, RigidBody};
use bevy::ecs::system::SystemParam;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::time::{Real, Virtual};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use bevy::camera::ClearColorConfig;
use bevy::camera::visibility::RenderLayers;
use bevy::gltf::GltfMeshName;
use bevy::ui::IsDefaultUiCamera;

use crate::ballistics::{
    self, ArmorVolume, ArmourMark, BallisticVolume, ComponentHealth, FireShell, PenetrationMarks,
    Projectile, ShellPath, ShotSource,
};
use crate::command::{Bindings, RoundStep};
use crate::damage::{self, CookedOff, Dead, LaunchedTurret, TankKnockedOut, VolumeOf};
use crate::damage_cam::pen_cam::game::{PenCamSet, XRay, XRayRelease, XRaySubject};
use crate::settings::{PenCamCorner, Settings};
use crate::tank::{
    Tank, TankPresentation, TankRoot, TankSimSource, ViewOf, Weapon, WeaponIndex,
    spawn_complete_tank,
};
use crate::view::PlayerView;
use crate::{CombatantId, CombatantLife, FreeShooter, Layer, ShotClock, ShotId, bake, spec, world};

// The clickable egui control panel — the sandbox's View / Shot / Time / Telemetry / Scene surface.
// Behind `dev_ui` so `bevy_egui` compiles ONLY into the sandbox build, never the shipping client
// (`bin/overmatch`); mounted below under the same gate. (`cargo armor` = `--features dev_ui`.)
#[cfg(feature = "dev_ui")]
mod panel;

#[cfg(test)]
mod tests;

// The wall prototype: a brick wall the range's rounds, a point deposit and a blast break up under
// candidate crack laws (the game's environment destruction design, 2026-10-30).
mod wall;

/// The spec sheet of the tank the sandbox spawns; the range's gun is its primary weapon as
/// authored there ([`seed_gun`]).
const SEED_SPEC: &str = include_str!("../assets/tiger_1/tiger_1.tank.ron");

/// The free-fly camera's combatant: the shooter every range round names.
const RANGE_SHOOTER: CombatantId = CombatantId(1);

/// The target tank's combatant, the same across its rebuilds; [`TargetLife`] counts them.
const RANGE_TARGET: CombatantId = CombatantId(2);

/// The egui control panel's width, logical pixels (egui points at bevy_egui's default zoom): the
/// panel is fixed-width on the window's left edge. No panel is compiled without `dev_ui`.
#[cfg(feature = "dev_ui")]
const PANEL_WIDTH: f32 = 300.0;
#[cfg(not(feature = "dev_ui"))]
const PANEL_WIDTH: f32 = 0.0;

/// Where the key legend starts, logical pixels: clear of the panel.
const UI_LEFT: f32 = PANEL_WIDTH + 10.0;

/// The round the range fires: an index into the [`RangeGun`]'s rounds (E/Q, or the panel), and an
/// optional muzzle-speed override, m/s, that stands in for range; `None` fires the round at its own
/// speed. Every other property of the shot is the round's data. `Copy + PartialEq` for the panel's
/// local-copy, write-on-change pattern.
#[derive(Resource, Clone, Copy, PartialEq, Default)]
struct ShotParams {
    round: u8,
    muzzle_speed: Option<f32>,
}

/// The range's gun: the seed tank's primary weapon, a child of the free-fly camera, which fires it
/// as a [`FreeShooter`]. Its [`TankRoot`] names the camera and its [`WeaponIndex`] is 0, so the
/// recorder finds it from a round's [`ShotSource`] as it finds a tank's weapon.
#[derive(Component)]
struct RangeGun;

/// The seed tank's primary weapon, every round it stows fired from its gun.
fn seed_gun() -> Weapon {
    let sheet: spec::TankSpec =
        ron::de::from_str(SEED_SPEC).expect("the shipped spec sheet parses");
    let (name, weapon) = sheet
        .weapons
        .iter()
        .find(|(_, weapon)| weapon.trigger == spec::Trigger::Primary)
        .expect("the seed tank authors a primary weapon");
    Weapon::of_spec(name, weapon, None)
        .unwrap_or_else(|err| panic!("the seed tank's primary weapon: {err}"))
}

/// Which life of [`RANGE_TARGET`] the standing target is: one more each rebuild, so a record of an
/// earlier target is never read as the live one's.
#[derive(Resource, Default)]
struct TargetLife(u32);

/// Whether the egui panel currently holds keyboard or pointer focus — written each frame by the
/// `dev_ui` panel, read by [`panel_capturing`] to gate the firing/fly/time input off so a slider
/// drag or an arrow-key nudge on a focused widget never leaks into the sim. Always present (default
/// false) so those gates behave identically when the panel is not compiled in.
#[derive(Resource, Default)]
struct PanelWantsInput {
    keyboard: bool,
    pointer: bool,
}

/// Run condition: an egui widget has focus, so player input must be suppressed this frame.
fn panel_capturing(want: Res<PanelWantsInput>) -> bool {
    want.keyboard || want.pointer
}

/// Panel→system intent seam for the board wipe (`C` or the Scene button): the panel raises it and
/// [`clear_shots`] consumes it, so keyboard and panel share one commit path. Always present.
#[derive(Resource, Default)]
struct ClearRequested(bool);

/// Panel→system intent seam for the world rebuild (`R` or the Scene button); consumed by
/// [`reset_world`]. Always present.
#[derive(Resource, Default)]
struct ResetRequested(bool);

/// The free-fly camera that doubles as the gun: shells spawn at its centre, firing down its view
/// axis. Inspection viewpoint and firing solution are one object.
#[derive(Component)]
struct FreeFlyCam;

/// The slow-motion ladder the Up/Down arrows step through (a shell flies ~773 m/s).
const SPEEDS: [f32; 6] = [1.0, 0.25, 0.06, 0.015, 0.004, 0.001];

/// Index into [`SPEEDS`]; Up moves toward 0 (faster), Down toward the end (slower).
#[derive(Resource, Default)]
struct SpeedIndex(usize);

/// The armour overlay, F2 (or the panel): the target's armour plates — the union the march walks,
/// which the look draws as one body with the rest of the hull — drawn over the whole view, opaque
/// or translucent ([`draw_armour_overlay`]). Off by default. `Copy + PartialEq` for the panel's
/// write-on-change discipline.
#[derive(Resource, Default, Clone, Copy, PartialEq)]
enum ArmourOverlay {
    #[default]
    Off,
    Opaque,
    Translucent,
}

impl ArmourOverlay {
    /// The states in tap order, for the panel's segmented control.
    #[cfg_attr(
        not(feature = "dev_ui"),
        allow(dead_code, reason = "read only by the dev_ui panel")
    )]
    const ALL: [Self; 3] = [Self::Off, Self::Opaque, Self::Translucent];

    fn next(self) -> Self {
        match self {
            Self::Off => Self::Opaque,
            Self::Opaque => Self::Translucent,
            Self::Translucent => Self::Off,
        }
    }

    #[cfg_attr(
        not(feature = "dev_ui"),
        allow(dead_code, reason = "read only by the dev_ui panel")
    )]
    fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Opaque => "opaque",
            Self::Translucent => "translucent",
        }
    }
}

/// The armour overlay's materials: armour blue, opaque and translucent, lit by the overlay's own
/// sun ([`spawn_overlay_light`]).
#[derive(Resource)]
struct OverlayMaterials {
    opaque: Handle<StandardMaterial>,
    translucent: Handle<StandardMaterial>,
}

/// Render layer the overlay camera alone draws, with its own depth buffer and no clear, so what is
/// on it composites over the main scene regardless of containment. Clear of every `render_policy`
/// channel, which this root resolves for the pen cam's cameras.
const OVERLAY_LAYER: usize = 6;
/// Isolated render layer for the UI camera: no geometry is placed on it, so that camera renders only
/// the HUD. Highest camera `order` = drawn last = HUD sits above the scene *and* the overlay.
/// Clear of every `render_policy` channel, like [`OVERLAY_LAYER`].
const UI_LAYER: usize = 7;

/// Marks the overlay camera (renders [`OVERLAY_LAYER`] on top of the main view).
#[derive(Component)]
struct OverlayCamera;

/// An overlay copy of one of the target's armour primitives: a root entity, so the look, which
/// dresses what hangs under the target, never dresses it; posed each frame at its source.
#[derive(Component)]
struct ArmourCopy {
    of: Entity,
}

/// A shot that met a tank: the look explains it, so the range draws none of its march, and its
/// shell goes once spent ([`sweep_tank_shots`]).
#[derive(Component)]
struct MetTank;

pub fn plugin(app: &mut App) {
    // The armor sandbox stays on the flat slab + authored blocks: its target placement and shot
    // scripting assume y=0 ground, and a dev tool should not shift when the game's heightmap does.
    app.insert_resource(crate::terrain_grid::ForceFlatWorld);
    // THIS INSTRUMENT DRAWS THE AUTHORED SURFACE, at every distance. The penetration march itself is
    // never at risk — the armour and component volumes are painted on the sim glb's parts, which
    // carry no chain — but the visual hull IS what a person aims the camera-gun at and reads an
    // entry point against, and at the 30 m the target stands from the default vantage half its
    // chains are already off their source rung (up to ~16 mm of surface). A tool whose whole use is
    // "look at where this shell went into this plate" shows the authored plate.
    app.insert_resource(crate::geometry_lod::SourceDetailOnly);
    // The sandbox's own App composition — physics + the shared shell mechanic + a battlefield to
    // hit. Deliberately omits driving, aim, the game cameras, sight, and shooting.
    app.add_plugins((
        crate::offline_physics(),
        // This is a WINDOWED root on `DefaultPlugins`, so it is on bevy's deadlocking exit path
        // exactly like the game is: without this, quitting the sandbox wedges the process instead
        // of ending it (see `crate::quit`).
        crate::quit::plugin,
        world::plugin,
        ballistics::sim_plugin,
        damage::plugin,
        // `spec` registers the `.tank.ron` loader so the target tank's volumes bind with their data.
        spec::plugin,
        // The certificate: the sim artifact `bake` opens is fingerprinted before it is read, and
        // the tank's scene primitives get their certified rungs.
        crate::geometry_lod::sim_plugin,
        // The live view both LOD ladders select through — this root mounts `world::plugin`'s
        // terrain layer as well as the tank's chains.
        crate::view::plugin,
        crate::geometry_lod::view_plugin,
        // The target's surface: its materials' visuals under its livery, as the game draws it.
        crate::tank_surface::plugin,
        // The tank-geometry extractor: the target's sim body (armor volumes included) spawns from
        // `TankBlueprint`, exactly like the game's tanks; the shadow harness rides along.
        bake::plugin,
        // The render-side view attach: the target is static, but cook-off detaches the sim
        // turret and the rendered glb turret must follow the free body.
        crate::tank::view_attach_plugin,
        // The render half of ballistics — the shell scene, the tracer streak, and the visibility a
        // hold draws with. This root mounts `ballistics` but NOT `vfx`, which is exactly why that
        // half is a ballistics-owned view plugin rather than a vfx concern: without this line the
        // sandbox's shells would fly invisibly.
        ballistics::view_plugin,
        // The shipped font (`UiFonts`) the round label draws a round's name in; the sandbox's other
        // labels keep the default font.
        crate::ui_font::plugin,
    ))
    // The pen cam as single-player runs it: the recorder of the authority's main-gun rounds, the
    // vocabulary its explanations fold, the offline delivery to the player's combatant (the free
    // shooter), the render channels its cameras resolve through, and the x-ray and the kill cam
    // (which stands aside for a free shooter).
    .add_plugins((
        crate::damage_cam::recorder::plugin,
        crate::damage_cam::delivery::plugin,
        crate::damage_cam::delivery::offline_plugin,
        crate::render_policy::plugin,
        crate::damage_cam::pen_cam::game::plugin,
    ))
    // The look's subject is the target, on from the start unless a capture asks otherwise; the
    // corner never opens (module doc).
    .insert_resource(XRaySubject::Struck(RANGE_TARGET))
    .insert_resource(XRay {
        on: std::env::var("OVERMATCH_RANGE_XRAY").as_deref() != Ok("0"),
    })
    .insert_resource(Settings {
        pen_cam_corner: PenCamCorner::Off,
        ..default()
    })
    // The kill cam's pose reads the orbit camera's turret pivot. This root mounts no orbit camera and
    // the kill cam stands aside for a free shooter, so the pivot stays unset.
    .init_resource::<crate::camera::TurretPivot>()
    // The weapon clock, one tick per fixed step as the game advances it: the offline delivery
    // publishes it as the shot clock, so each range round's `ShotId` names its tick. This root
    // mounts no `shooting::plugin`: the target's own gun is a passive test article, never armed.
    .init_resource::<crate::WeaponClock>()
    .add_systems(FixedLast, crate::shooting::advance_weapon_clock)
    // The game's keys for the round selector and the x-ray.
    .init_resource::<Bindings>()
    .init_resource::<TargetLife>()
    // Keep spent shells frozen in place (with their tracer + marks) for inspection.
    .insert_resource(ballistics::RetainSpentShells(true))
    // Default to smooth per-frame motion; `T` toggles to the true fixed-rate cadence.
    .insert_resource(ballistics::MarchMode::Demo)
    .init_resource::<ArmourOverlay>()
    .init_resource::<SpeedIndex>()
    .init_resource::<ShotParams>()
    // Panel↔keyboard shared seams + the egui input-capture flags. Always present (default) so the
    // firing/fly/time run-conditions compile and behave without `dev_ui`; the panel writes them only
    // when it is compiled in.
    .init_resource::<PanelWantsInput>()
    .init_resource::<ClearRequested>()
    .init_resource::<ResetRequested>()
    .add_systems(
        Startup,
        (
            spawn_camera,
            grab_cursor.run_if(not(resource_exists::<Autopilot>)),
            spawn_targets,
            spawn_hud,
            load_target,
            setup_overlay_materials,
            spawn_overlay_light,
        ),
    )
    .add_systems(
        Update,
        (
            // The direct-manipulation input gates OFF while an egui widget has focus, so a slider
            // drag or an arrow-key nudge on a focused widget never leaks into the gun/camera/clock
            // (`panel_capturing`). `fly_camera`/`fire` stay cursor-locked as well — freeing the
            // cursor to reach the panel already stops them; the focus gate is belt-and-suspenders and
            // the ONLY thing standing between a panel click and a fired shell were the cursor ever
            // free while a widget is live.
            fly_camera
                .run_if(cursor_locked)
                .run_if(not(panel_capturing)),
            (select_round, fire)
                .chain()
                .run_if(cursor_locked)
                .run_if(not(panel_capturing)),
            // Up/Down step the slow-mo ladder; a focused egui slider also reads the arrows, so gate
            // this the same way the fly does (the armour-sandbox analogue of the track sandbox's
            // `read_drive_input` gate).
            time_controls.run_if(not(panel_capturing)),
            toggle_full_pause,
            clear_shots,
            toggle_march_mode,
            spawn_target_from_blueprint,
            (toggle_armour_overlay, draw_armour_overlay).chain(),
            (
                sweep_tank_shots,
                draw_shell_paths,
                draw_penetrations,
                draw_shell_ends,
            )
                .chain(),
            // Despawns the target the look may be drawing (`TargetRebuild::rebuild`).
            reset_world.after(PenCamSet),
            update_round_label,
            toggle_legend,
        ),
    );

    // The egui control panel — the sandbox's whole non-camera control surface (View / Shot / Time /
    // Telemetry / Scene). Behind `dev_ui` so `bevy_egui` is compiled ONLY here (the `armor_sandbox`
    // bin declares `required-features = ["dev_ui"]`), never into `bin/overmatch`. It brings its own
    // `EguiPlugin` and runs in `EguiPrimaryContextPass`. Launch it with the `cargo armor` alias.
    #[cfg(feature = "dev_ui")]
    app.add_plugins(panel::plugin);
    autopilot_plugin(app);
    wall::plugin(app);
    // The per-frame wall-clock recorder (idle unless `SPIKE_FRAME_COST` is set): the frame cost of
    // an autopilot capture, wall-clock under the flight's fixed step too.
    app.add_plugins(crate::frame_cost::client_plugin);
}

fn spawn_camera(mut commands: Commands) {
    let camera = commands
        .spawn((
            Camera3d::default(),
            Transform::from_xyz(0.0, 3.0, 18.0).looking_at(Vec3::new(0.0, 2.0, 0.0), Vec3::Y),
            FreeFlyCam,
            // THE SANDBOX'S PLAYER VIEW: this is the camera its player looks through, so it carries
            // the declaration the game's camera does and every shared reader of the live view — the
            // LOD ladders, the HUD reprojection, the billboards — reads it here too. The overlay and
            // UI cameras below carry none, which is exactly what keeps them out of that answer.
            PlayerView,
            // The player's combatant on the range: it fires the gun below, one life throughout.
            FreeShooter,
            RANGE_SHOOTER,
            CombatantLife(0),
            // Main 3D pass (order 0, render layer 0): the scene + gizmos.
        ))
        .with_children(|parent| {
            // Overlay camera: a child (so it shares the fly camera's pose), drawn after the main
            // camera with no clear, rendering only the overlay layer — its own depth buffer makes
            // the armour overlay draw on top of the scene.
            parent.spawn((
                Camera3d::default(),
                Camera {
                    order: 1,
                    clear_color: ClearColorConfig::None,
                    ..default()
                },
                RenderLayers::layer(OVERLAY_LAYER),
                OverlayCamera,
            ));
        })
        .id();
    commands.spawn((
        Name::new("range gun"),
        RangeGun,
        seed_gun(),
        WeaponIndex(0),
        TankRoot(camera),
        ChildOf(camera),
    ));

    // Dedicated UI camera at the highest order, so the HUD draws *above* both the main pass and the
    // armour overlay — otherwise the overlay's opaque plates (order 1) would paint over UI carried
    // by the order-0 main camera. It renders no
    // 3D itself (its layer holds no geometry; gizmos default to layer 0) and doesn't clear the frame,
    // so it only composites the UI. (bevy_camera 0.19: higher `order` renders later/on top; bevy_ui
    // 0.19: UI without an explicit target goes to the `IsDefaultUiCamera`.)
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: 2,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        RenderLayers::layer(UI_LAYER),
        IsDefaultUiCamera,
    ));
}

/// Lock + hide the cursor for mouse-look. A query (not `Single`) so a not-yet-present cursor at
/// startup is a no-op rather than a panic.
fn grab_cursor(mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>) {
    for (mut window, mut cursor) in &mut windows {
        let center = window.size() / 2.0;
        window.set_cursor_position(Some(center));
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
}

/// Placeholder ballistic volumes — translucent steel slabs on the `Armor` layer of increasing
/// thickness, so the penetrator marches *through* them (recording entry/exit) and only the ground
/// stops it. Same material (steel), so **thickness is the variable**: the thin plates are overmatched
/// by the 88 (no ricochet even at steep angles); the thick ones ricochet and can defeat the round.
/// Real model volumes replace these when they land (design doc §12 contract).
fn spawn_targets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // RHA, the reference steel: a plate's cost in reference-mm is its thickness in mm.
    // (x, thickness_m, tint) — 15 mm (overmatched), 50 mm, 100 mm, 300 mm (defeats it head-on).
    let plates = [
        (-6.0_f32, 0.015_f32, Color::srgba(0.72, 0.74, 0.82, 0.40)),
        (-2.0, 0.05, Color::srgba(0.60, 0.62, 0.72, 0.45)),
        (2.0, 0.10, Color::srgba(0.50, 0.52, 0.62, 0.50)),
        (6.0, 0.30, Color::srgba(0.40, 0.42, 0.52, 0.60)),
    ];
    for (x, thickness, tint) in plates {
        let material = materials.add(StandardMaterial {
            base_color: tint,
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(3.0, 3.0, thickness))),
            MeshMaterial3d(material),
            Transform::from_xyz(x, 2.0, 0.0),
            RigidBody::Static,
            Collider::cuboid(3.0, 3.0, thickness),
            CollisionLayers::new([Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: crate::ballistics::VolumeMaterial::of(
                    crate::substances::SubstanceRegistry::shipped_static(),
                    "RHA",
                )
                .expect("the shipped registry names RHA"),
                substance: "RHA".to_string(),
            },
        ));
    }
}

/// Presentation handles for a pending target. Simulation data comes from `TankBlueprint` and does
/// not wait for either asset to resolve.
#[derive(Resource)]
struct PendingTarget(TankPresentation);

/// The seed tank's presentation, the target the next [`spawn_target_from_blueprint`] builds.
fn pending_target(asset_server: &AssetServer) -> PendingTarget {
    PendingTarget(TankPresentation::new(
        Some(asset_server.load(GltfAssetLabel::Scene(0).from_asset(crate::tank::TANK_GLB_PATH))),
        asset_server.load(crate::tank::TANK_SPEC_PATH),
    ))
}

fn load_target(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(pending_target(&asset_server));
}

/// Rebuilding the target: the look lets go of it, every tank and launched turret goes, and the
/// next life of [`RANGE_TARGET`] is pending.
#[derive(SystemParam)]
struct TargetRebuild<'w, 's> {
    asset_server: Res<'w, AssetServer>,
    targets: Query<'w, 's, Entity, Or<(With<Tank>, With<LaunchedTurret>)>>,
    life: ResMut<'w, TargetLife>,
    xray: XRayRelease<'w, 's>,
    commands: Commands<'w, 's>,
}

impl TargetRebuild<'_, '_> {
    fn rebuild(&mut self) {
        self.xray.release(&mut self.commands);
        for entity in &self.targets {
            self.commands.entity(entity).despawn();
        }
        self.life.0 = self.life.0.saturating_add(1);
        self.commands
            .insert_resource(pending_target(&self.asset_server));
    }
}

/// Spawn the static target synchronously from the blueprint; its glb view may attach later.
fn spawn_target_from_blueprint(
    mut commands: Commands,
    pending: Option<Res<PendingTarget>>,
    life: Res<TargetLife>,
    source: TankSimSource,
) {
    let Some(pending) = pending else {
        return;
    };
    let Some(content) = source.get() else {
        return;
    };
    spawn_complete_tank(
        &mut commands,
        content,
        pending.0.clone(),
        (
            Transform::from_xyz(0.0, 2.0, -12.0),
            Name::new("range target"),
            RigidBody::Static,
            RANGE_TARGET,
            CombatantLife(life.0),
        ),
    );
    commands.remove_resource::<PendingTarget>();
}

/// The [`ArmourOverlay`]'s two materials, one armour blue: lit + matte, so adjacent plates shade
/// apart, its own sun lighting the overlay layer ([`spawn_overlay_light`]).
fn setup_overlay_materials(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let blue = Srgba::new(0.35, 0.55, 0.95, 1.0);
    commands.insert_resource(OverlayMaterials {
        opaque: materials.add(StandardMaterial {
            base_color: blue.into(),
            perceptual_roughness: 0.75,
            ..default()
        }),
        translucent: materials.add(StandardMaterial {
            base_color: blue.with_alpha(0.3).into(),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.75,
            ..default()
        }),
    });
}

/// A directional light on the overlay layer, matching the world light's direction — without it the
/// overlay's plates (rendered by the overlay camera) get no scene light and read flat/dark.
///
/// Both the direction and the intensity come from `world`'s single sun definition rather than a
/// second copy of the numbers: the overlay is drawn OVER the same scene, so any drift between the
/// two lights shows up as plates lit from a different sun than the tank under them. No shadows —
/// this light exists only to shade the overlay layer.
fn spawn_overlay_light(mut commands: Commands) {
    commands.spawn((
        crate::world::sun_light(),
        crate::world::sun_transform(),
        RenderLayers::layer(OVERLAY_LAYER),
    ));
}

/// F2 steps the [`ArmourOverlay`] (the panel writes the same resource).
fn toggle_armour_overlay(keys: Res<ButtonInput<KeyCode>>, mut overlay: ResMut<ArmourOverlay>) {
    if keys.just_pressed(KeyCode::F2) {
        *overlay = overlay.next();
    }
}

/// Which render primitives are armour's: a primitive whose nearest [`ViewOf`], itself or an
/// ancestor, names an [`ArmorVolume`].
#[derive(SystemParam)]
struct ArmourPrimitives<'w, 's> {
    views: Query<'w, 's, &'static ViewOf>,
    parents: Query<'w, 's, &'static ChildOf>,
    armour: Query<'w, 's, (), With<ArmorVolume>>,
}

impl ArmourPrimitives<'_, '_> {
    fn contains(&self, primitive: Entity) -> bool {
        std::iter::once(primitive)
            .chain(self.parents.iter_ancestors(primitive))
            .find_map(|node| self.views.get(node).ok())
            .is_some_and(|view| self.armour.contains(view.0))
    }
}

/// Draw the [`ArmourOverlay`]: an [`ArmourCopy`] of each of the [`ArmourPrimitives`], on
/// [`OVERLAY_LAYER`], posed at its source each frame. A copy goes with its source, and every copy
/// with the overlay off. `render_policy` resolves a new mesh onto the battlefield channel in the
/// frame it lands, so a copy is shown only once its layer reads the overlay's.
fn draw_armour_overlay(
    overlay: Res<ArmourOverlay>,
    materials: Option<Res<OverlayMaterials>>,
    primitives: Query<
        (Entity, &Mesh3d, &GlobalTransform),
        (With<GltfMeshName>, Without<ArmourCopy>),
    >,
    armour: ArmourPrimitives,
    mut copies: Query<(
        Entity,
        &ArmourCopy,
        &mut Mesh3d,
        &mut MeshMaterial3d<StandardMaterial>,
        &mut Transform,
        &mut RenderLayers,
        &mut Visibility,
    )>,
    mut commands: Commands,
) {
    let material = match (*overlay, materials.as_deref()) {
        (ArmourOverlay::Opaque, Some(materials)) => Some(&materials.opaque),
        (ArmourOverlay::Translucent, Some(materials)) => Some(&materials.translucent),
        (ArmourOverlay::Off, _) | (_, None) => None,
    };
    let Some(material) = material else {
        for (copy, ..) in &copies {
            commands.entity(copy).despawn();
        }
        return;
    };
    let layer = RenderLayers::layer(OVERLAY_LAYER);
    let mut copied = HashSet::new();
    for (copy, of, mut mesh, mut worn, mut pose, mut layers, mut visibility) in &mut copies {
        let Ok((_, source, at)) = primitives.get(of.of) else {
            commands.entity(copy).despawn();
            continue;
        };
        copied.insert(of.of);
        mesh.set_if_neq(source.clone());
        if worn.0 != *material {
            worn.0 = material.clone();
        }
        pose.set_if_neq(at.compute_transform());
        let on_layer = *layers == layer;
        if !on_layer {
            *layers = layer.clone();
        }
        visibility.set_if_neq(if on_layer {
            Visibility::Visible
        } else {
            Visibility::Hidden
        });
    }
    for (primitive, mesh, at) in &primitives {
        if copied.contains(&primitive) || !armour.contains(primitive) {
            continue;
        }
        commands.spawn((
            Name::new("armour overlay"),
            ArmourCopy { of: primitive },
            mesh.clone(),
            MeshMaterial3d(material.clone()),
            at.compute_transform(),
            layer.clone(),
            Visibility::Hidden,
        ));
    }
}

/// Free-fly the camera-gun. Look from mouse delta (yaw/pitch read back from the current rotation,
/// no stored euler state, as in the orbit camera). Move on **real** time, so you can still reposition
/// while the sim is paused. WASD = planar relative to look, Shift = up, Ctrl = down.
fn fly_camera(
    camera: Single<&mut Transform, With<FreeFlyCam>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    time: Res<Time<Real>>,
) {
    let mut transform = camera.into_inner();
    crate::camera::free_fly_transform(&mut transform, &keys, motion.delta, time.delta_secs());
}

/// The fire binding fires the [`RangeGun`]'s selected round down the view axis, as the game's
/// round-to-shell mapping makes it, at the override speed when one is set. The camera has no
/// parent, so its `Transform` is its world pose (no one-frame `GlobalTransform` lag).
///
/// One round per shot-clock tick: a round's [`ShotId`] is its tick, and two rounds sharing one would
/// be one shot to the recorder and the delivery. The gun is never armed, so it never reloads.
#[expect(
    clippy::too_many_arguments,
    reason = "a trigger reading its binding, both devices, the shooter, its gun, the selection, the clock and its own last tick"
)]
fn fire(
    bindings: Res<Bindings>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    camera: Single<(Entity, &Transform, &CombatantId), With<FreeFlyCam>>,
    gun: Single<&Weapon, With<RangeGun>>,
    shot: Res<ShotParams>,
    clock: Res<ShotClock>,
    mut last_tick: Local<Option<u32>>,
    mut commands: Commands,
) {
    if !bindings.fire_primary.just_pressed(&keys, &mouse) || *last_tick == Some(clock.0) {
        return;
    }
    let (camera, transform, &combatant) = *camera;
    if fire_round(
        &mut commands,
        (camera, transform, combatant),
        &gun,
        *shot,
        clock.0,
    ) {
        *last_tick = Some(clock.0);
    }
}

/// Fire `gun`'s round `shot` from the camera-gun `shooter` down its view axis on shot-clock tick
/// `tick`: whether the gun lists that round.
fn fire_round(
    commands: &mut Commands,
    (camera, transform, combatant): (Entity, &Transform, CombatantId),
    gun: &Weapon,
    shot: ShotParams,
    tick: u32,
) -> bool {
    let Some(loaded) = gun.rounds.get(usize::from(shot.round)) else {
        return false;
    };
    let round = FireShell::of_round(
        &loaded.round,
        transform.translation,
        transform.forward(),
        gun.fire_cycle.mechanism(),
        true,
        Some(ShotSource {
            tank: camera,
            weapon: 0,
            round: shot.round,
        }),
        Some(ShotId {
            combatant,
            weapon: 0,
            fire_tick: tick,
        }),
    );
    commands.trigger(FireShell {
        speed: shot.muzzle_speed.unwrap_or(round.speed),
        ..round
    });
    true
}

/// The capture autopilot (module doc): the camera-gun's pose (eye and the point looked at), taken
/// on the first frame, its flight, and the rounds still to fire with their app seconds, earliest
/// first.
#[derive(Resource)]
struct Autopilot {
    pose: Option<(Vec3, Vec3)>,
    fly: Option<Flight>,
    fire: std::collections::VecDeque<(u8, f32)>,
}

/// The autopilot's flight: from the pose's eye and point looked at to `to`'s, between app seconds
/// `from_s` and `to_s`.
struct Flight {
    from: (Vec3, Vec3),
    to: (Vec3, Vec3),
    from_s: f32,
    to_s: f32,
}

impl Flight {
    /// The camera's pose at app second `t`: held at either end outside the flight.
    fn at(&self, t: f32) -> Transform {
        let s = ((t - self.from_s) / (self.to_s - self.from_s).max(f32::EPSILON)).clamp(0.0, 1.0);
        let (eye, look) = (
            self.from.0.lerp(self.to.0, s),
            self.from.1.lerp(self.to.1, s),
        );
        Transform::from_translation(eye).looking_at(look, Vec3::Y)
    }
}

/// An eye and the point it looks at, `x,y,z,lx,ly,lz`.
fn parse_pose(name: &str, pose: &str) -> Option<(Vec3, Vec3)> {
    let v: Vec<f32> = pose
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    let [x, y, z, lx, ly, lz] = v[..] else {
        warn!("{name}={pose}: expected x,y,z,lx,ly,lz");
        return None;
    };
    Some((Vec3::new(x, y, z), Vec3::new(lx, ly, lz)))
}

/// Mount the [`Autopilot`] when any of its variables is set.
fn autopilot_plugin(app: &mut App) {
    let pose = std::env::var("OVERMATCH_RANGE_POSE").ok();
    let fire = std::env::var("OVERMATCH_RANGE_FIRE").ok();
    let fly = std::env::var("OVERMATCH_RANGE_FLY").ok();
    if pose.is_none() && fire.is_none() && fly.is_none() {
        return;
    }
    let pose = pose.and_then(|pose| parse_pose("OVERMATCH_RANGE_POSE", &pose));
    let fly = fly.and_then(|fly| {
        let (to, span) = fly.split_once('@')?;
        let (from_s, to_s) = span.split_once('-')?;
        Some(Flight {
            from: pose?,
            to: parse_pose("OVERMATCH_RANGE_FLY", to)?,
            from_s: from_s.trim().parse().ok()?,
            to_s: to_s.trim().parse().ok()?,
        })
    });
    if fly.is_some() {
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_nanos(33_333_333),
        ));
    }
    let mut shots: Vec<(u8, f32)> = fire
        .iter()
        .flat_map(|fire| fire.split(','))
        .filter_map(|shot| {
            let (round, at) = shot.split_once('@')?;
            Some((round.trim().parse().ok()?, at.trim().parse().ok()?))
        })
        .collect();
    shots.sort_by(|a, b| a.1.total_cmp(&b.1));
    info!(
        "armour sandbox: autopilot pose {pose:?}, fly {:?}, fire {shots:?}",
        fly.as_ref().map(|fly| (fly.to, fly.from_s, fly.to_s))
    );
    app.insert_resource(Autopilot {
        pose,
        fly,
        fire: shots.into(),
    })
    .add_systems(Update, drive_autopilot);
}

/// Stand the camera-gun at the autopilot's pose, fly it, then fire each round at its second, one
/// per shot-clock tick.
#[expect(
    clippy::too_many_arguments,
    reason = "the trigger's own inputs, as `fire` reads them, plus the clock and the autopilot"
)]
fn drive_autopilot(
    time: Res<Time>,
    mut pilot: ResMut<Autopilot>,
    camera: Single<(Entity, &mut Transform, &CombatantId), With<FreeFlyCam>>,
    gun: Single<&Weapon, With<RangeGun>>,
    mut shot: ResMut<ShotParams>,
    clock: Res<ShotClock>,
    mut last_tick: Local<Option<u32>>,
    mut commands: Commands,
) {
    let (camera, mut transform, &combatant) = camera.into_inner();
    if let Some((eye, look)) = pilot.pose.take() {
        *transform = Transform::from_translation(eye).looking_at(look, Vec3::Y);
    }
    if let Some(fly) = &pilot.fly {
        *transform = fly.at(time.elapsed_secs());
    }
    let Some(&(round, at)) = pilot.fire.front() else {
        return;
    };
    if time.elapsed_secs() < at || *last_tick == Some(clock.0) {
        return;
    }
    pilot.fire.pop_front();
    *shot = ShotParams {
        round,
        muzzle_speed: None,
    };
    *last_tick = Some(clock.0);
    fire_round(
        &mut commands,
        (camera, &*transform, combatant),
        &gun,
        *shot,
        clock.0,
    );
}

/// The round bindings step the [`RangeGun`]'s selection through its rounds, wrapping, and drop a
/// speed override: an override is set against one round's own speed.
fn select_round(
    bindings: Res<Bindings>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gun: Single<&Weapon, With<RangeGun>>,
    mut shot: ResMut<ShotParams>,
) {
    let step = if bindings.round_next.just_pressed(&keys, &mouse) {
        RoundStep::Next
    } else if bindings.round_previous.just_pressed(&keys, &mouse) {
        RoundStep::Previous
    } else {
        return;
    };
    if let Some(round) = step.apply(shot.round, gun.rounds.len()) {
        *shot = ShotParams {
            round,
            muzzle_speed: None,
        };
    }
}

/// Run condition: the cursor is captured (mouse-look active). Esc releases it, which disables flying
/// and firing so a freed cursor doesn't spin the view.
fn cursor_locked(cursors: Query<&CursorOptions>) -> bool {
    cursors
        .single()
        .map(|cursor| cursor.grab_mode == CursorGrabMode::Locked)
        .unwrap_or(false)
}

/// Esc = a real pause: release the cursor (so you can leave the window) and stop time; press again to
/// recapture and resume. Distinct from Space, which freezes time but keeps the cursor captured so you
/// can keep flying around a frozen shot.
fn toggle_full_pause(
    keys: Res<ButtonInput<KeyCode>>,
    mut time: ResMut<Time<Virtual>>,
    mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    let Ok((mut window, mut cursor)) = windows.single_mut() else {
        return;
    };
    if cursor.grab_mode == CursorGrabMode::Locked {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
        time.pause();
    } else {
        let center = window.size() / 2.0;
        window.set_cursor_position(Some(center));
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
        time.unpause();
    }
}

/// `c` wipes the board: every shell (in-flight or frozen) with its tracer + penetration marks, and
/// the target's HP pools refilled and knockouts cleared — so you can start a clean shot.
#[expect(
    clippy::too_many_arguments,
    reason = "a sandbox command sweeping shells, health and three damage-state markers"
)]
fn clear_shots(
    keys: Res<ButtonInput<KeyCode>>,
    mut requested: ResMut<ClearRequested>,
    shots: Query<Entity, With<ShellPath>>,
    mut health: Query<&mut ComponentHealth>,
    dead: Query<Entity, With<Dead>>,
    cooked_off: Query<Entity, With<CookedOff>>,
    knocked_out: Query<Entity, With<TankKnockedOut>>,
    mut commands: Commands,
) {
    // `C` or the panel's Scene button. Consume the flag on the frame it fires so the deref (and its
    // change-tick) lands only on a real clear.
    let by_panel = requested.0;
    if by_panel {
        requested.0 = false;
    }
    if !(keys.just_pressed(KeyCode::KeyC) || by_panel) {
        return;
    }
    for entity in &shots {
        commands.entity(entity).despawn();
    }
    // Reset accumulated component damage so the next shot reads against a fresh target.
    for mut hp in &mut health {
        hp.refill();
    }
    for entity in &dead {
        commands.entity(entity).remove::<Dead>();
    }
    for entity in &cooked_off {
        commands.entity(entity).remove::<CookedOff>();
    }
    for entity in &knocked_out {
        commands.entity(entity).remove::<TankKnockedOut>();
    }
}

/// `r` rebuilds the sandbox target from its source scene/spec and wipes the shots. This is heavier
/// than `c`, but it restores hierarchy after cookoff has detached/launched the turret.
fn reset_world(
    keys: Res<ButtonInput<KeyCode>>,
    mut requested: ResMut<ResetRequested>,
    shots: Query<Entity, With<ShellPath>>,
    mut target: TargetRebuild,
) {
    // `R` or the panel's Scene button; consume the flag on the frame it fires (see `clear_shots`).
    let by_panel = requested.0;
    if by_panel {
        requested.0 = false;
    }
    if !(keys.just_pressed(KeyCode::KeyR) || by_panel) {
        return;
    }
    for entity in &shots {
        target.commands.entity(entity).despawn();
    }
    target.rebuild();
}

/// Time controls on the **virtual** clock (which drives the fixed timestep the march/physics run
/// on): `P` toggles pause; `1`/`2`/`3` set 1×/0.25×/0.1× for slow-motion study. Single-step lands
/// in a later increment.
fn time_controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut time: ResMut<Time<Virtual>>,
    mut index: ResMut<SpeedIndex>,
) {
    if keys.just_pressed(KeyCode::Space) {
        if time.is_paused() {
            time.unpause();
        } else {
            time.pause();
        }
    }
    // Up = faster (toward 1×), Down = slower (toward bullet-time); changing speed resumes.
    let mut changed = false;
    if keys.just_pressed(KeyCode::ArrowUp) && index.0 > 0 {
        index.0 -= 1;
        changed = true;
    }
    if keys.just_pressed(KeyCode::ArrowDown) && index.0 + 1 < SPEEDS.len() {
        index.0 += 1;
        changed = true;
    }
    if changed {
        time.set_relative_speed(SPEEDS[index.0]);
        time.unpause();
    }
}

/// `T` toggles the shell march between real (true fixed server cadence) and demo (smooth per-frame).
fn toggle_march_mode(keys: Res<ButtonInput<KeyCode>>, mut mode: ResMut<ballistics::MarchMode>) {
    if keys.just_pressed(KeyCode::KeyT) {
        *mode = match *mode {
            ballistics::MarchMode::Real => ballistics::MarchMode::Demo,
            ballistics::MarchMode::Demo => ballistics::MarchMode::Real,
        };
    }
}

/// Tracer: each shell's accumulated path as a gizmo polyline, of the shells that met no tank.
fn draw_shell_paths(mut gizmos: Gizmos, paths: Query<&ShellPath, Without<MetTank>>) {
    for path in &paths {
        let mut start = 0;
        for end in path
            .segment_starts
            .iter()
            .copied()
            .chain(std::iter::once(path.points.len()))
        {
            if end.saturating_sub(start) >= 2 {
                gizmos.linestrip(
                    path.points[start..end].iter().copied(),
                    Color::srgb(1.0, 0.85, 0.2),
                );
            }
            start = end;
        }
    }
}

/// Inspection draw for the march of the shells that met no tank: each volume crossing as a green
/// entry marker, a red exit marker, and an orange through-span (its length is the geometric
/// line-of-sight thickness).
fn draw_penetrations(mut gizmos: Gizmos, marks: Query<&PenetrationMarks, Without<MetTank>>) {
    for mark in &marks {
        for event in &mark.events {
            // Entry green normally, magenta when this crossing was an overmatch.
            let entry_color = if event.overmatched {
                Color::srgb(1.0, 0.2, 1.0)
            } else {
                Color::srgb(0.2, 1.0, 0.3)
            };
            gizmos.sphere(Isometry3d::from_translation(event.entry), 0.06, entry_color);
            gizmos.sphere(
                Isometry3d::from_translation(event.exit),
                0.06,
                Color::srgb(1.0, 0.2, 0.2),
            );
            gizmos.line(event.entry, event.exit, Color::srgb(1.0, 0.45, 0.1));
        }
        // Ricochets — a distinct cyan marker where the round skipped off without entering.
        for &point in &mark.ricochets {
            gizmos.sphere(
                Isometry3d::from_translation(point),
                0.1,
                Color::srgb(0.3, 0.8, 1.0),
            );
        }
    }
}

/// Whether a shell's armour log met a tank: bounced off, crossed or ran a line through a volume
/// `of_tank` names, or burst with a field that walks a tank.
fn met_a_tank(log: &[ArmourMark], of_tank: impl Fn(Entity) -> bool) -> bool {
    log.iter().any(|mark| match mark {
        ArmourMark::Bounce { struck, .. } => of_tank(*struck),
        ArmourMark::Crossing { volumes, .. } | ArmourMark::Wound { volumes, .. } => {
            volumes.iter().any(|crossing| of_tank(crossing.volume))
        }
        ArmourMark::Burst { tank, .. } => tank.is_some(),
        ArmourMark::Failed => false,
    })
}

/// A shell that met a tank ([`met_a_tank`], a volume with a [`VolumeOf`]) is [`MetTank`] while it
/// flies and goes once spent: the look explains it.
fn sweep_tank_shots(
    shells: Query<(Entity, &PenetrationMarks, Has<Projectile>, Has<MetTank>), With<ShellPath>>,
    tank_volumes: Query<(), With<VolumeOf>>,
    mut commands: Commands,
) {
    for (shell, marks, flying, marked) in &shells {
        if !marked && !met_a_tank(&marks.log, |volume| tank_volumes.contains(volume)) {
            continue;
        }
        if !flying {
            commands.entity(shell).despawn();
        } else if !marked {
            commands.entity(shell).insert(MetTank);
        }
    }
}

/// A red sphere where each spent shell that met no tank came to rest, the end of its path.
fn draw_shell_ends(
    mut gizmos: Gizmos,
    spent: Query<&ShellPath, (Without<Projectile>, Without<MetTank>)>,
) {
    for path in &spent {
        if let Some(&end) = path.points.last() {
            gizmos.sphere(
                Isometry3d::from_translation(end),
                0.2,
                Color::srgb(1.0, 0.3, 0.1),
            );
        }
    }
}

/// The view's line naming the [`RangeGun`]'s selected round.
#[derive(Component)]
struct RoundLabel;

/// The key legend, hidden until H ([`toggle_legend`]).
#[derive(Component)]
struct Legend;

/// The selected round's line: its display name, and the speed it fires at only when overridden.
fn round_line(gun: &Weapon, shot: ShotParams) -> String {
    let Some(loaded) = gun.rounds.get(usize::from(shot.round)) else {
        return String::new();
    };
    match shot.muzzle_speed {
        Some(speed) => format!("{}  {speed:.0} m/s", loaded.name),
        None => loaded.name.clone(),
    }
}

/// Keep the [`RoundLabel`] on the selected round, written only on a change.
fn update_round_label(
    shot: Res<ShotParams>,
    gun: Single<&Weapon, With<RangeGun>>,
    mut label: Single<&mut Text, With<RoundLabel>>,
) {
    let line = round_line(&gun, *shot);
    if label.0 != line {
        label.0 = line;
    }
}

/// The view's text: the [`Legend`] bottom left, hidden, the [`RoundLabel`] bottom right, and the
/// aim dot at the centre.
fn spawn_hud(mut commands: Commands, fonts: Res<crate::ui_font::UiFonts>) {
    commands.spawn((
        Legend,
        Text::new(
            "WASD / Shift / Ctrl  fly\n\
             LMB  fire\n\
             E / Q  round\n\
             X  x-ray\n\
             F2  armour overlay\n\
             C  clear\n\
             R  rebuild\n\
             Space  freeze\n\
             Up / Down  slow-mo\n\
             T  march cadence\n\
             Esc  free cursor\n\
             H  keys\n\
             wall: 1-5 law  K cut  N new  B round  F fire\n\
             wall: P point  [ ] energy  O blast  , . mass  ; ' standoff\n\
             wall: V casing  U structural  J spall  G camera",
        ),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.85, 0.87, 0.95)),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(12.0),
            left: Val::Px(UI_LEFT),
            ..default()
        },
        Visibility::Hidden,
    ));
    // The shipped font: it draws a round's display name, which is data, whole.
    commands.spawn((
        RoundLabel,
        Text::new(""),
        TextFont {
            font: fonts.body.clone().into(),
            font_size: FontSize::Px(20.0),
            ..default()
        },
        TextColor(Color::srgb(1.0, 0.9, 0.5)),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(12.0),
            right: Val::Px(16.0),
            ..default()
        },
    ));
    // Fixed white aim dot at screen centre — the Sight, as in the game.
    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|parent| {
            parent.spawn((
                Node {
                    width: Val::Px(6.0),
                    height: Val::Px(6.0),
                    border_radius: BorderRadius::MAX,
                    ..default()
                },
                BackgroundColor(Color::WHITE),
            ));
        });
}

/// H shows or hides the [`Legend`].
fn toggle_legend(
    keys: Res<ButtonInput<KeyCode>>,
    mut legend: Single<&mut Visibility, With<Legend>>,
) {
    if keys.just_pressed(KeyCode::KeyH) {
        legend.toggle_inherited_hidden();
    }
}
