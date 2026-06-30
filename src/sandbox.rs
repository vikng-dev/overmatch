//! The armor ballistics sandbox — an isolated tool to develop and tune the penetration march
//! deterministically, decoupled from driving/aiming. See
//! `.agents/docs/design/armor-penetration-and-damage.md` §11. Mounted by `bin/armor_sandbox`, not
//! by `GamePlugin`: it composes a *subset* of the game's feature plugins plus the sandbox controls.
//!
//! v1 increment: a free-fly camera that *is* the gun (WASD to float, Shift/Ctrl up/down, mouse to
//! look, left-click to fire a shell straight down the view axis), basic time controls (pause +
//! slow-mo, on real time so you can still fly while the sim is frozen), and placeholder target
//! slabs. The penetration march, ballistic volumes, and spall grow on top of `ballistics::Impact`
//! in later increments.

use avian3d::prelude::{
    Collider, CollisionLayers, LayerMask, PhysicsInterpolationPlugin, PhysicsPlugins, RigidBody,
};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::time::{Real, Virtual};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use bevy::asset::LoadState;
use bevy::camera::ClearColorConfig;
use bevy::camera::visibility::RenderLayers;
use bevy::ui::IsDefaultUiCamera;

use crate::Layer;
use crate::ballistics::{
    self, ArmorVolume, BallisticVolume, ComponentHealth, ComponentVolume, FireShell, ImpactMarker,
    PenetrationMarks, ShellPath, ShellReadout, SpallMarks,
};
use crate::damage::{
    self, Ammo, CookedOff, CrewStation, Crewman, Dead, LaunchedTurret, PendingSwap, SWAP_SECONDS,
    TankKnockedOut, VolumeOf,
};
use crate::hud::{self, HudCamera};
use crate::spec::{self, TankSpec, TankSpecHandle};
use crate::tank::{Tank, on_tank_ready};
use crate::world;

/// Muzzle speed for sandbox shots (m/s) — the 88 mm, matching the game's gun for now. Becomes a
/// live knob (with shell type) once the march needs tuning.
const MUZZLE_SPEED: f32 = 773.0;
/// Shell calibre (m) — the 88. Drives overmatch against the thin plates.
const CALIBER: f32 = 0.088;
/// Projectile mass (kg) — the 88's PzGr 39 (~10.2 kg). Primary driver of penetration capability.
const SHELL_MASS: f32 = 10.2;

/// The free-fly camera that doubles as the gun: shells spawn at its centre, firing down its view
/// axis. Inspection viewpoint and firing solution are one object.
#[derive(Component)]
struct FreeFlyCam;

/// The status line (time scale + live shell count).
#[derive(Component)]
struct StatusText;

/// A pooled floating label, reassigned to a live shell each frame (no per-shell UI churn).
#[derive(Component)]
struct ShellLabel;

/// The top-right readout of each layer's current tap-loop state.
#[derive(Component)]
struct LayerStatusText;

/// The bottom crew bar: one cell per seat, driven by the `1`–`5` swap input.
#[derive(Component)]
struct CrewBarText;

/// Crew-bar selection: the seat tapped as the swap *source*, awaiting a target.
#[derive(Resource, Default)]
struct CrewSelect {
    source: Option<CrewStation>,
}

/// The slow-motion ladder the Up/Down arrows step through (a shell flies ~773 m/s).
const SPEEDS: [f32; 6] = [1.0, 0.25, 0.06, 0.015, 0.004, 0.001];

/// Index into [`SPEEDS`]; Up moves toward 0 (faster), Down toward the end (slower).
#[derive(Resource, Default)]
struct SpeedIndex(usize);

/// The hull's tap-loop state: solid → x-ray (translucent) → hidden.
#[derive(Default, Clone, Copy, PartialEq)]
enum MeshState {
    #[default]
    Solid,
    Xray,
    Hidden,
}

impl MeshState {
    fn next(self) -> Self {
        match self {
            MeshState::Solid => MeshState::Xray,
            MeshState::Xray => MeshState::Hidden,
            MeshState::Hidden => MeshState::Solid,
        }
    }

    fn label(self) -> &'static str {
        match self {
            MeshState::Solid => "solid",
            MeshState::Xray => "xray",
            MeshState::Hidden => "off",
        }
    }
}

/// A volume layer's tap-loop state: off → drawn-on-top → solid (depth-tested) → x-ray (translucent).
#[derive(Default, Clone, Copy, PartialEq)]
enum VolumeState {
    #[default]
    Hidden,
    OnTop,
    Solid,
    Xray,
}

impl VolumeState {
    fn next(self) -> Self {
        match self {
            VolumeState::Hidden => VolumeState::OnTop,
            VolumeState::OnTop => VolumeState::Solid,
            VolumeState::Solid => VolumeState::Xray,
            VolumeState::Xray => VolumeState::Hidden,
        }
    }

    fn label(self) -> &'static str {
        match self {
            VolumeState::Hidden => "off",
            VolumeState::OnTop => "on top",
            VolumeState::Solid => "solid",
            VolumeState::Xray => "xray",
        }
    }
}

/// The target's per-layer view state, advanced by `F1/F2/F3`. Opens on a useful default: hull
/// translucent (xray), armour translucent (xray), components solid — so the inner volumes read at
/// a glance without first cycling the layers.
#[derive(Resource)]
struct LayerView {
    mesh: MeshState,
    armor: VolumeState,
    components: VolumeState,
}

impl Default for LayerView {
    fn default() -> Self {
        Self {
            mesh: MeshState::Xray,
            armor: VolumeState::Xray,
            components: VolumeState::Solid,
        }
    }
}

/// Opaque unlit materials for the volumes (so they read the same in the main and overlay passes,
/// with no light on the overlay layer), plus a translucent material the hull swaps to in its middle
/// state. "On top" is done by render layer, not by a material trick.
#[derive(Resource)]
struct VolumeMaterials {
    armor: Handle<StandardMaterial>,
    armor_xray: Handle<StandardMaterial>,
    component: Handle<StandardMaterial>,
    component_xray: Handle<StandardMaterial>,
    hull_translucent: Handle<StandardMaterial>,
}

/// Render layer for volumes drawn "on top" — the overlay camera renders only this, with its own
/// depth buffer and no clear, so it composites over the main scene regardless of containment.
const OVERLAY_LAYER: usize = 1;
/// Isolated render layer for the UI camera: no geometry is placed on it, so that camera renders only
/// the HUD. Highest camera `order` = drawn last = HUD sits above the scene *and* the on-top volumes.
const UI_LAYER: usize = 2;

/// Marks the overlay camera (renders [`OVERLAY_LAYER`] on top of the main view).
#[derive(Component)]
struct OverlayCamera;

/// Tags a painted volume mesh (so the apply step can swap its normal/x-ray material).
#[derive(Component)]
struct VolumePaint {
    armor: bool,
}

/// Tags a hull visual mesh and remembers its original material, so x-ray can swap it translucent and
/// back.
#[derive(Component)]
struct HullMesh {
    original: Handle<StandardMaterial>,
}

pub fn plugin(app: &mut App) {
    // The sandbox's own App composition — physics + the shared shell mechanic + a battlefield to
    // hit. Deliberately omits driving, aim, the game cameras, sight, and shooting.
    app.add_plugins((
        PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all()),
        world::plugin,
        ballistics::plugin,
        damage::plugin,
        // `spec` registers the `.tank.ron` loader so the target tank's volumes bind with their data.
        spec::plugin,
        // Shared tank-state HUD (component HP + aggregate status labels), reprojected through the
        // `HudCamera` tag on the free-fly camera below.
        hud::plugin,
    ))
    // Keep spent shells frozen in place (with their tracer + marks) for inspection.
    .insert_resource(ballistics::RetainSpentShells(true))
    // Default to smooth per-frame motion; `T` toggles to the true fixed-rate cadence.
    .insert_resource(ballistics::MarchMode::Demo)
    .init_resource::<LayerView>()
    .init_resource::<SpeedIndex>()
    .init_resource::<CrewSelect>()
    // Paint translucent materials onto the volume meshes as they bind.
    .add_observer(paint_armor)
    .add_observer(paint_component)
    .add_systems(
        Startup,
        (
            spawn_camera,
            grab_cursor,
            spawn_targets,
            spawn_hud,
            load_target,
            setup_volume_materials,
            spawn_overlay_light,
        ),
    )
    .add_systems(
        Update,
        (
            fly_camera.run_if(cursor_locked),
            fire.run_if(cursor_locked),
            time_controls,
            toggle_full_pause,
            clear_shots,
            reset_world,
            toggle_march_mode,
            spawn_target_when_ready,
            tag_hull_meshes,
            toggle_layers,
            apply_layer_visibility,
            update_layer_status,
            draw_shell_paths,
            draw_penetrations,
            draw_spall,
            draw_consequence_gizmos,
            update_status,
            update_shell_labels,
        ),
    )
    // Crew bar (`1`–`5` swap input + render) — split out so the Update tuple stays within Bevy's
    // 20-system limit.
    .add_systems(Update, (crew_swap_input, update_crew_bar));
}

fn spawn_camera(mut commands: Commands) {
    commands
        .spawn((
            Camera3d::default(),
            Transform::from_xyz(0.0, 3.0, 18.0).looking_at(Vec3::new(0.0, 2.0, 0.0), Vec3::Y),
            FreeFlyCam,
            // The shared HUD reprojects its world-anchored labels through this camera.
            HudCamera,
            // Main 3D pass (order 0, render layer 0): the scene + gizmos.
        ))
        .with_children(|parent| {
            // Overlay camera: a child (so it shares the fly camera's pose), drawn after the main
            // camera with no clear, rendering only the overlay layer — its own depth buffer makes
            // those volumes draw on top of the scene even when geometrically inside the hull.
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
        });

    // Dedicated UI camera at the highest order, so the HUD (HP labels, reticle, legend, status) draws
    // *above* both the main pass and the on-top overlay volumes — otherwise opaque "on top" component
    // meshes (overlay, order 1) would paint over UI carried by the order-0 main camera. It renders no
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
    // Steel: reference-mm of armor per metre of material, so a plate's cost ≈ its thickness in mm.
    const STEEL: f32 = 1000.0;
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
                material_factor: STEEL,
            },
        ));
    }
}

/// The target tank's spec, loading. The tank is spawned only once it's ready (a load dependency,
/// ADR-0011), so `on_tank_ready` binds its volumes with their data in one pass.
#[derive(Resource)]
struct PendingTarget(Handle<TankSpec>);

fn load_target(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(PendingTarget(asset_server.load("tiger_1/tiger_1.tank.ron")));
}

/// Once the spec is loaded, spawn the real Tiger as a **static** target (no driving/suspension — it
/// just stands there to be shot) and bind it with the shared `on_tank_ready`, which now attaches the
/// ballistic-volume colliders the march raycasts.
fn spawn_target_when_ready(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pending: Option<Res<PendingTarget>>,
) {
    let Some(pending) = pending else {
        return;
    };
    if !matches!(asset_server.load_state(&pending.0), LoadState::Loaded) {
        return;
    }
    commands
        .spawn((
            WorldAssetRoot(
                asset_server.load(GltfAssetLabel::Scene(0).from_asset("tiger_1/tiger_1.glb")),
            ),
            TankSpecHandle(pending.0.clone()),
            Transform::from_xyz(0.0, 2.0, -12.0),
            Name::new("Tiger I target"),
            Tank,
            RigidBody::Static,
        ))
        .observe(on_tank_ready);
    commands.remove_resource::<PendingTarget>();
}

fn setup_volume_materials(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    // Lit + matte, so adjacent/overlapping volumes shade differently and read as separate forms.
    // (The overlay layer gets its own light in `spawn_overlay_light`, else these render dark there.)
    let solid = |color: Color| StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.75,
        ..default()
    };
    // X-ray = the same colour, translucent + depth-tested in the main pass (parallel to the hull's).
    let xray = |color: Srgba| StandardMaterial {
        base_color: color.with_alpha(0.3).into(),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.75,
        ..default()
    };
    commands.insert_resource(VolumeMaterials {
        armor: materials.add(solid(Color::srgb(0.35, 0.55, 0.95))),
        armor_xray: materials.add(xray(Srgba::new(0.35, 0.55, 0.95, 1.0))),
        component: materials.add(solid(Color::srgb(0.95, 0.55, 0.18))),
        component_xray: materials.add(xray(Srgba::new(0.95, 0.55, 0.18, 1.0))),
        hull_translucent: materials.add(StandardMaterial {
            base_color: Color::srgba(0.62, 0.64, 0.68, 0.16),
            alpha_mode: AlphaMode::Blend,
            ..default()
        }),
    });
}

/// When an armor volume binds, paint its mesh primitives translucent blue + tag them.
fn paint_armor(
    add: On<Add, ArmorVolume>,
    children: Query<&Children>,
    meshes: Query<(), With<Mesh3d>>,
    materials: Res<VolumeMaterials>,
    mut commands: Commands,
) {
    paint_volume(
        add.entity,
        true,
        &children,
        &meshes,
        &materials.armor,
        &mut commands,
    );
}

/// When a component volume binds, paint its mesh primitives translucent amber + tag them.
fn paint_component(
    add: On<Add, ComponentVolume>,
    children: Query<&Children>,
    meshes: Query<(), With<Mesh3d>>,
    materials: Res<VolumeMaterials>,
    mut commands: Commands,
) {
    paint_volume(
        add.entity,
        false,
        &children,
        &meshes,
        &materials.component,
        &mut commands,
    );
}

/// Set `material` + a [`VolumePaint`] tag on every mesh in the volume node's subtree (the glTF
/// loader puts the mesh on a child primitive, so walk descendants).
fn paint_volume(
    node: Entity,
    armor: bool,
    children: &Query<&Children>,
    meshes: &Query<(), With<Mesh3d>>,
    material: &Handle<StandardMaterial>,
    commands: &mut Commands,
) {
    for entity in std::iter::once(node).chain(children.iter_descendants(node)) {
        if meshes.contains(entity) {
            commands.entity(entity).insert((
                MeshMaterial3d(material.clone()),
                VolumePaint { armor },
                // Start in the main pass; the apply step moves it to the overlay layer when "on top".
                RenderLayers::layer(0),
            ));
        }
    }
}

/// Tag the hull's *visual* meshes (and remember their material), so x-ray can swap them translucent.
/// A hull mesh is any mesh that is neither a ballistic volume nor a collider proxy (checked up the
/// hierarchy). Runs each frame; `Without<HullMesh>` makes it tag each mesh just once.
fn tag_hull_meshes(
    candidates: Query<
        (Entity, &MeshMaterial3d<StandardMaterial>),
        (With<Mesh3d>, Without<HullMesh>, Without<VolumePaint>),
    >,
    parents: Query<&ChildOf>,
    volumes: Query<(), Or<(With<ArmorVolume>, With<ComponentVolume>)>>,
    colliders: Query<(), With<Collider>>,
    mut commands: Commands,
) {
    for (entity, material) in &candidates {
        let mut probe = entity;
        let mut is_hull = true;
        loop {
            if volumes.contains(probe) || colliders.contains(probe) {
                is_hull = false;
                break;
            }
            match parents.get(probe) {
                Ok(parent) => probe = parent.parent(),
                Err(_) => break,
            }
        }
        if is_hull {
            commands.entity(entity).insert(HullMesh {
                original: material.0.clone(),
            });
        }
    }
}

/// A directional light on the overlay layer, matching the world light's direction — without it the
/// "on top" volumes (rendered by the overlay camera) get no scene light and read flat/dark.
fn spawn_overlay_light(mut commands: Commands) {
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
        RenderLayers::layer(OVERLAY_LAYER),
    ));
}

/// Refresh the top-right readout of each layer's tap-loop state.
fn update_layer_status(view: Res<LayerView>, mut text: Query<&mut Text, With<LayerStatusText>>) {
    let Ok(mut text) = text.single_mut() else {
        return;
    };
    *text = Text::new(format!(
        "F1 mesh: {}\nF2 armor: {}\nF3 components: {}",
        view.mesh.label(),
        view.armor.label(),
        view.components.label(),
    ));
}

/// `1/2/3` advance the mesh / armor / component tap-loops.
fn toggle_layers(keys: Res<ButtonInput<KeyCode>>, mut view: ResMut<LayerView>) {
    // Moved off the number row (now the crew bar, `1`–`5`) onto the function keys.
    if keys.just_pressed(KeyCode::F1) {
        view.mesh = view.mesh.next();
    }
    if keys.just_pressed(KeyCode::F2) {
        view.armor = view.armor.next();
    }
    if keys.just_pressed(KeyCode::F3) {
        view.components = view.components.next();
    }
}

/// `1`–`5` crew-bar input: tap a seat to select it as the swap source, tap a second to start a
/// [`PendingSwap`]; re-tapping the source (or any seat while a swap is mid-flight) cancels.
fn crew_swap_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut select: ResMut<CrewSelect>,
    tank: Query<(Entity, Option<&PendingSwap>), With<Tank>>,
    seats: Query<(Entity, &CrewStation, &VolumeOf)>,
    mut commands: Commands,
) {
    const DIGITS: [KeyCode; 5] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
    ];
    let Some(slot) = DIGITS.iter().position(|k| keys.just_pressed(*k)) else {
        return;
    };
    let Ok((tank, pending)) = tank.single() else {
        return;
    };

    // Seats for this tank in enum (slot) order.
    let mut ordered: Vec<(Entity, CrewStation)> = seats
        .iter()
        .filter(|(_, _, owner)| owner.tank() == tank)
        .map(|(e, s, _)| (e, *s))
        .collect();
    ordered.sort_by_key(|(_, s)| *s);
    let Some(&(seat_entity, seat_station)) = ordered.get(slot) else {
        return;
    };

    // Any tap while a swap is in flight cancels it.
    if pending.is_some() {
        commands.entity(tank).remove::<PendingSwap>();
        select.source = None;
        return;
    }

    match select.source {
        None => select.source = Some(seat_station),
        Some(src) if src == seat_station => select.source = None, // re-tap = deselect
        Some(src) => {
            if let Some(&(src_entity, _)) = ordered.iter().find(|(_, s)| *s == src) {
                commands.entity(tank).insert(PendingSwap {
                    a: src_entity,
                    b: seat_entity,
                    remaining: SWAP_SECONDS,
                });
            }
            select.source = None;
        }
    }
}

/// Render the crew bar: `N:Seat` per seat in enum order, plus the occupant (when foreign) and a
/// `+`/`-` alive/dead mark. The selected source is bracketed `[..]`; a pending swap marks both seats
/// `~..~` and shows its countdown.
fn update_crew_bar(
    select: Res<CrewSelect>,
    tank: Query<(Entity, Option<&PendingSwap>), With<Tank>>,
    seats: Query<(Entity, &CrewStation, &Crewman, Option<&Dead>, &VolumeOf)>,
    mut bar: Query<&mut Text, With<CrewBarText>>,
) {
    let Ok(mut text) = bar.single_mut() else {
        return;
    };
    let Ok((tank, pending)) = tank.single() else {
        *text = Text::new("");
        return;
    };

    let mut ordered: Vec<(Entity, CrewStation, Crewman, bool)> = seats
        .iter()
        .filter(|(_, _, _, _, owner)| owner.tank() == tank)
        .map(|(e, s, c, dead, _)| (e, *s, *c, dead.is_some()))
        .collect();
    ordered.sort_by_key(|(_, s, _, _)| *s);

    let cells: Vec<String> = ordered
        .iter()
        .enumerate()
        .map(|(i, (entity, seat, crewman, dead))| {
            // Notes after the seat name: who is actually manning it (when foreign), and whether the
            // occupant is dead — e.g. "Loader (Commander, dead)".
            let mut notes: Vec<&str> = Vec::new();
            if crewman.home != *seat {
                notes.push(crewman.home.label());
            }
            if *dead {
                notes.push("dead");
            }
            let detail = if notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", notes.join(", "))
            };
            let mut cell = format!("{}: {}{}", i + 1, seat.label(), detail);
            if select.source == Some(*seat) {
                cell = format!("[{cell}]");
            }
            if let Some(ps) = pending
                && (*entity == ps.a || *entity == ps.b)
            {
                cell = format!("~{cell}~");
            }
            cell
        })
        .collect();

    let prefix = match pending {
        Some(ps) => format!("SWAP {:.1}s   ", ps.remaining.max(0.0)),
        None => String::new(),
    };
    *text = Text::new(format!("{prefix}{}", cells.join("    ")));
}

/// Apply the layer states to the target's meshes. The hull swaps material/visibility for its loop;
/// each volume mesh sets its visibility and **render layer** — moving to the overlay layer draws it
/// on top (via the overlay camera), staying on layer 0 keeps it depth-tested in the main pass.
/// `Visibility::Visible` shows a volume even through a hidden hull. Writes only on change, re-checked
/// each frame so late-binding meshes pick up the current state.
fn apply_layer_visibility(
    view: Res<LayerView>,
    materials: Option<Res<VolumeMaterials>>,
    mut hull_meshes: Query<
        (
            &HullMesh,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        Without<VolumePaint>,
    >,
    mut volume_meshes: Query<
        (
            &VolumePaint,
            &mut Visibility,
            &mut RenderLayers,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        Without<HullMesh>,
    >,
) {
    let Some(materials) = materials else {
        return;
    };

    // Hull: opaque (original) → x-ray (translucent) → hidden.
    for (hull, mut visibility, mut material) in &mut hull_meshes {
        let (want_vis, want_mat) = match view.mesh {
            MeshState::Solid => (Visibility::Inherited, &hull.original),
            MeshState::Xray => (Visibility::Inherited, &materials.hull_translucent),
            MeshState::Hidden => (Visibility::Hidden, &hull.original),
        };
        if *visibility != want_vis {
            *visibility = want_vis;
        }
        if material.0 != *want_mat {
            material.0 = want_mat.clone();
        }
    }

    // Volumes: off → on-top (overlay layer, opaque) → solid (main pass, opaque) → x-ray (main pass,
    // translucent).
    for (paint, mut visibility, mut layers, mut material) in &mut volume_meshes {
        let state = if paint.armor {
            view.armor
        } else {
            view.components
        };
        let opaque = if paint.armor {
            &materials.armor
        } else {
            &materials.component
        };
        let ghost = if paint.armor {
            &materials.armor_xray
        } else {
            &materials.component_xray
        };
        let (want_vis, want_layer, want_mat) = match state {
            VolumeState::Hidden => (Visibility::Hidden, 0, opaque),
            VolumeState::OnTop => (Visibility::Visible, OVERLAY_LAYER, opaque),
            VolumeState::Solid => (Visibility::Visible, 0, opaque),
            VolumeState::Xray => (Visibility::Visible, 0, ghost),
        };
        if *visibility != want_vis {
            *visibility = want_vis;
        }
        let want_layers = RenderLayers::layer(want_layer);
        if *layers != want_layers {
            *layers = want_layers;
        }
        if material.0 != *want_mat {
            material.0 = want_mat.clone();
        }
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

    const SENS: f32 = 0.003;
    const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.001;
    let (mut yaw, mut pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    yaw -= motion.delta.x * SENS;
    pitch = (pitch - motion.delta.y * SENS).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);

    // WASD on the horizontal plane in the camera's heading — looking down and pressing W keeps you
    // moving forward over the ground, not diving into it. Shift/Ctrl change altitude. Near-vertical
    // look leaves no horizontal heading, so `normalize_or_zero` just no-ops that frame.
    const SPEED: f32 = 12.0;
    let forward = Vec3::from(transform.forward())
        .with_y(0.0)
        .normalize_or_zero();
    let right = Vec3::from(transform.right())
        .with_y(0.0)
        .normalize_or_zero();
    let mut dir = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        dir += forward;
    }
    if keys.pressed(KeyCode::KeyS) {
        dir -= forward;
    }
    if keys.pressed(KeyCode::KeyD) {
        dir += right;
    }
    if keys.pressed(KeyCode::KeyA) {
        dir -= right;
    }
    if keys.pressed(KeyCode::ShiftLeft) {
        dir += Vec3::Y;
    }
    if keys.pressed(KeyCode::ControlLeft) {
        dir -= Vec3::Y;
    }
    if dir != Vec3::ZERO {
        transform.translation += dir.normalize() * SPEED * time.delta_secs();
    }
}

/// Left-click fires a shell straight down the view axis. The camera has no parent, so its
/// `Transform` is its world pose — read it directly (no one-frame `GlobalTransform` lag).
fn fire(
    camera: Single<&Transform, With<FreeFlyCam>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    commands.trigger(FireShell {
        origin: camera.translation,
        direction: camera.forward(),
        speed: MUZZLE_SPEED,
        caliber: CALIBER,
        mass: SHELL_MASS,
    });
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
/// every impact marker — so you can start a clean shot.
fn clear_shots(
    keys: Res<ButtonInput<KeyCode>>,
    shots: Query<Entity, Or<(With<ShellPath>, With<ImpactMarker>)>>,
    mut health: Query<&mut ComponentHealth>,
    dead: Query<Entity, With<Dead>>,
    cooked_off: Query<Entity, With<CookedOff>>,
    knocked_out: Query<Entity, With<TankKnockedOut>>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::KeyC) {
        return;
    }
    for entity in &shots {
        commands.entity(entity).despawn();
    }
    // Reset accumulated component damage so the next shot reads against a fresh target.
    for mut hp in &mut health {
        hp.current = hp.max;
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

/// `r` rebuilds the sandbox target from its source scene/spec. This is heavier than `c`, but it
/// restores hierarchy after cookoff has detached/launched the turret.
fn reset_world(
    keys: Res<ButtonInput<KeyCode>>,
    asset_server: Res<AssetServer>,
    targets: Query<Entity, Or<(With<Tank>, With<LaunchedTurret>)>>,
    shots: Query<Entity, Or<(With<ShellPath>, With<ImpactMarker>)>>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::KeyR) {
        return;
    }
    for entity in &shots {
        commands.entity(entity).despawn();
    }
    for entity in &targets {
        commands.entity(entity).despawn();
    }
    commands.insert_resource(PendingTarget(asset_server.load("tiger_1/tiger_1.tank.ron")));
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

/// Tracer: draw each in-flight shell's accumulated path as a gizmo polyline. The first piece of the
/// inspection draw the penetration march will build on (path segments, entry/exit, spall cones).
fn draw_shell_paths(mut gizmos: Gizmos, paths: Query<&ShellPath>) {
    for path in &paths {
        gizmos.linestrip(path.points.iter().copied(), Color::srgb(1.0, 0.85, 0.2));
    }
}

/// Inspection draw for the march: each volume crossing as a green entry marker, a red exit marker,
/// and an orange through-span (its length is the geometric line-of-sight thickness).
fn draw_penetrations(mut gizmos: Gizmos, marks: Query<&PenetrationMarks>) {
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

/// Spall draw: each fragment ray from a perforation exit — hot orange where it deposited HP into a
/// component, dim grey where it merely shadowed (armor) or flew into air. The spray *is* the cone;
/// its density reads the material × residual-energy budget (design §5).
fn draw_spall(mut gizmos: Gizmos, marks: Query<&SpallMarks>) {
    // A short representative length for the cone outline (fragments stop where they hit).
    const OUTLINE: f32 = 1.2;
    for mark in &marks {
        for burst in &mark.bursts {
            // Faint cone outline: the axis and a rim circle, so the cone's aim + spread read even
            // when only a few fragments are thrown.
            let axis = Vec3::from(burst.axis);
            let tip = burst.origin + axis * OUTLINE;
            let rim = OUTLINE * burst.half_angle.tan();
            let facing = Quat::from_rotation_arc(Vec3::Z, axis);
            gizmos.line(burst.origin, tip, Color::srgb(0.35, 0.37, 0.42));
            gizmos.circle(
                Isometry3d::new(tip, facing),
                rim,
                Color::srgb(0.35, 0.37, 0.42),
            );
            for frag in &burst.fragments {
                let color = if frag.deposited {
                    Color::srgb(1.0, 0.4, 0.1)
                } else {
                    Color::srgb(0.45, 0.47, 0.52)
                };
                gizmos.line(burst.origin, frag.end, color);
                if frag.deposited {
                    gizmos.sphere(
                        Isometry3d::from_translation(frag.end),
                        0.05,
                        Color::srgb(1.0, 0.2, 0.1),
                    );
                }
            }
        }
    }
}

fn draw_consequence_gizmos(
    mut gizmos: Gizmos,
    cooked_ammo: Query<&GlobalTransform, (With<Ammo>, With<CookedOff>)>,
) {
    for transform in &cooked_ammo {
        gizmos.sphere(
            Isometry3d::from_translation(transform.translation()),
            0.45,
            Color::srgb(1.0, 0.35, 0.02),
        );
    }
}

/// The static keybindings legend, the status line, and a small pool of floating shell labels.
fn spawn_hud(mut commands: Commands) {
    commands.spawn((
        Text::new(
            "WASD / Shift / Ctrl  fly\n\
             LMB  fire    C  clear shots    R  reset world\n\
             Space  freeze    Esc  pause + free cursor    Up/Down  slow-mo    T  real/demo time\n\
             1-5  crew seats: tap source then target to swap occupants (re-tap cancels)",
        ),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.85, 0.87, 0.95)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(10.0),
            ..default()
        },
    ));
    // Crew bar, bottom-left — one cell per seat, driven by `update_crew_bar`.
    commands.spawn((
        CrewBarText,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::srgb(0.92, 0.94, 0.78)),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(10.0),
            left: Val::Px(10.0),
            ..default()
        },
    ));
    commands.spawn((
        StatusText,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.6, 1.0, 0.7)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(86.0),
            left: Val::Px(10.0),
            ..default()
        },
    ));
    // Layer-state readout, top-right.
    commands.spawn((
        LayerStatusText,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.85, 0.88, 0.95)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            right: Val::Px(12.0),
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
    // Pool of labels positioned over live shells each frame; hidden while unused.
    for _ in 0..8 {
        commands.spawn((
            ShellLabel,
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(13.0),
                ..default()
            },
            TextColor(Color::srgb(1.0, 0.9, 0.5)),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Visibility::Hidden,
        ));
    }
    // The component-HP and aggregate tank-status label pools live in the shared `hud` module.
}

/// Refresh the status line: current time scale (or "paused") and the live shell count.
fn update_status(
    time: Res<Time<Virtual>>,
    mode: Res<ballistics::MarchMode>,
    shells: Query<(), With<ShellReadout>>,
    mut status: Query<&mut Text, With<StatusText>>,
) {
    let Ok(mut text) = status.single_mut() else {
        return;
    };
    let rate = if time.is_paused() {
        "paused".to_string()
    } else {
        format!("{:.3}x", time.relative_speed())
    };
    let mode = match *mode {
        ballistics::MarchMode::Real => "real",
        ballistics::MarchMode::Demo => "demo",
    };
    *text = Text::new(format!(
        "time {} [{}]   shells {}",
        rate,
        mode,
        shells.iter().count()
    ));
}

/// Position each pooled label beside a live shell (reprojected to screen) and write its speed,
/// remaining capability, and plate count; hide the leftover labels.
fn update_shell_labels(
    camera: Single<(&Camera, &GlobalTransform), With<FreeFlyCam>>,
    shells: Query<(&Transform, &ShellReadout, &PenetrationMarks)>,
    mut labels: Query<(&mut Node, &mut Text, &mut Visibility), With<ShellLabel>>,
) {
    let (camera, cam_transform) = *camera;
    let mut shells = shells.iter();
    for (mut node, mut text, mut visibility) in &mut labels {
        let Some((transform, readout, marks)) = shells.next() else {
            *visibility = Visibility::Hidden;
            continue;
        };
        match camera.world_to_viewport(cam_transform, transform.translation) {
            Ok(screen) => {
                node.left = Val::Px(screen.x + 12.0);
                node.top = Val::Px(screen.y - 8.0);
                *text = Text::new(format!(
                    "{:.0} m/s\n{:.0} mm\n{} crossed",
                    readout.speed,
                    readout.capability,
                    marks.events.len()
                ));
                *visibility = Visibility::Visible;
            }
            Err(_) => *visibility = Visibility::Hidden,
        }
    }
}
