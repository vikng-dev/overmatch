//! The suspension editor (`bin/suspension_editor`) — a dev tool that closes the gap between the
//! Blender-authored rest pose and the in-game suspension result.
//!
//! It loads the real Tiger (its `.glb` for the visual, its baked `TankBlueprint` for geometry +
//! spec) and overlays, as toggleable gizmo layers, the ENTIRE authored→derived suspension pipeline
//! that the design converged on (`track-model/`):
//!
//!   * the running-gear **circles** (pin-line: sprocket pitch circle, road-wheel + idler circles);
//!   * the **route** — the taut wrap at the loaded rest pose (orange);
//!   * the **max-droop cast shape** (green) — rest circles lowered by the static deflection; the
//!     soft penalty datum ("if the ground is inside this, push");
//!   * the **max-compression cast shape** (red) — rest circles raised by the bump-stop; the hard
//!     bottoming collider;
//!   * the **buoyant box** — the convex approximation of the droop shape's ground run;
//!   * the **belt** — the discrete links resampled at pitch, with **pin markers**;
//!   * the **sprocket teeth** ring with `tooth-0` and the between-pins mesh point;
//!   * the **contact** penetration of the droop shape against the ground.
//!
//! Everything is DERIVED live from the sources of truth via [`derive`] (the "universal laws"), so
//! tweaking a knob (`[` `]` ride frequency, `-` `=` bump-stop) moves the cast shapes in real time —
//! that live re-derivation is the point of an *editor* rather than a viewer. Per Yan's plan the
//! model is prototyped here first, then graduated into the sim/view tiers.
//!
//! Structure mirrors `sandbox.rs`: a `pub fn plugin` mounted by the 13-line `bin/suspension_editor`
//! (the bin is a separate crate and can't see the crate-private `bake`/`spec`; the logic lives here
//! in the library). Read-only: it never spawns a sim body or steps physics — it reads the blueprint
//! and draws.

mod derive;

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::time::Real;
use bevy::ui::IsDefaultUiCamera;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use bevy::world_serialization::WorldAssetRoot;

use crate::bake::{self, TankBlueprint};
use crate::spec::{self, TrackSpec};
use crate::tank::TrackSide;
use derive::SuspensionParams;

/// Which vertical offset the running-gear circles get before the route is built — the three cast
/// poses the editor overlays.
#[derive(Clone, Copy)]
enum Pose {
    /// The loaded rest pose Blender models (wheels where the tank's weight settles them).
    Rest,
    /// Fully-extended: rest circles lowered by the static deflection. The green soft datum.
    Droop,
    /// Fully-compressed: rest circles raised by the bump-stop. The red hard datum.
    Compression,
}

pub fn plugin(app: &mut App) {
    app
        // `spec` registers the `.tank.ron` loader; `bake` extracts the Tiger glb into `TankBlueprint`
        // at Startup (spec + road-wheel hull-local geometry) — the editor's only data source. No
        // sim/physics/tank-spawn: this is a pure read-and-draw dev tool.
        .add_plugins((spec::plugin, bake::plugin))
        .init_resource::<EditorParams>()
        .init_resource::<VizToggles>()
        .add_systems(
            Startup,
            (
                spawn_camera,
                spawn_light,
                spawn_model,
                spawn_panel,
                grab_cursor,
            ),
        )
        .add_systems(
            Update,
            (
                setup_scene.run_if(scene_unbuilt),
                fly_camera.run_if(cursor_locked),
                toggle_pause,
                toggle_layers,
                tweak_params,
                toggle_model,
                draw_suspension.run_if(resource_exists::<SceneGeom>),
                update_panel,
                dump_and_exit.run_if(resource_exists::<SceneGeom>),
            ),
        );
}

// ---------------------------------------------------------------------------------------------
// Resources & components
// ---------------------------------------------------------------------------------------------

/// The live-tweakable suspension knobs (the NEW authoring params not yet in the RON).
#[derive(Resource, Default)]
struct EditorParams(SuspensionParams);

/// Which overlay layers are drawn. All on by default; number keys toggle them.
#[derive(Resource)]
struct VizToggles {
    circles: bool,
    route: bool,
    droop: bool,
    compression: bool,
    buoyant_box: bool,
    belt: bool,
    sprocket: bool,
    contact: bool,
}

impl Default for VizToggles {
    fn default() -> Self {
        Self {
            circles: true,
            route: true,
            droop: true,
            compression: true,
            buoyant_box: false,
            belt: true,
            sprocket: true,
            contact: true,
        }
    }
}

/// Scene geometry derived once the blueprint is available: where the ground sits (so the rest track
/// just touches it), the bump that varies the footprint penetration, and the material loop length.
#[derive(Resource)]
struct SceneGeom {
    /// World y of the flat ground — the rest track's lowest point, so droop penetrates and
    /// compression floats.
    ground_y: f32,
    /// A raised bump under the belly: z-span and height, so contact penetration varies.
    bump_z: (f32, f32),
    bump_h: f32,
    /// Belt loop length = pitch × link_count (the exact material loop).
    belt_len: f32,
}

impl SceneGeom {
    /// Ground surface height at side-plane z (flat, with the belly bump).
    fn height(&self, z: f32) -> f32 {
        if z > self.bump_z.0 && z < self.bump_z.1 {
            self.ground_y + self.bump_h
        } else {
            self.ground_y
        }
    }
}

/// The free-fly inspection camera.
#[derive(Component)]
struct FlyCam;

/// The raw glb visual model root (toggled with `M`).
#[derive(Component)]
struct ModelRoot;

/// The parameter/keybind panel text.
#[derive(Component)]
struct PanelText;

// ---------------------------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------------------------

fn spawn_camera(mut commands: Commands) {
    // A single camera renders the scene, the gizmos (layer 0), and the UI (marked default).
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(7.5, 2.2, 0.0).looking_at(Vec3::new(0.0, 0.55, 0.0), Vec3::Y),
        FlyCam,
        IsDefaultUiCamera,
        // Per-view ambient fill so the model reads without a full lighting rig (AmbientLight is a
        // camera component in bevy 0.19, not a resource).
        AmbientLight {
            brightness: 220.0,
            ..default()
        },
    ));
}

fn spawn_light(mut commands: Commands) {
    commands.spawn((
        DirectionalLight {
            illuminance: 9_000.0,
            ..default()
        },
        Transform::from_xyz(6.0, 10.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Spawn the raw Tiger glb as a plain scene (no sim body): the model to overlay the derived geometry
/// on. Its mesh vertices carry their authored hull-local position, so at the identity transform it
/// registers with the side-plane spec coordinates the gizmos use.
fn spawn_model(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn((
        ModelRoot,
        WorldAssetRoot(
            asset_server.load(GltfAssetLabel::Scene(0).from_asset("tiger_1/tiger_1.glb")),
        ),
        Transform::IDENTITY,
    ));
}

/// Lock + hide the cursor for mouse-look (a query, so a not-yet-present cursor is a no-op).
fn grab_cursor(mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>) {
    for (mut window, mut cursor) in &mut windows {
        let center = window.size() / 2.0;
        window.set_cursor_position(Some(center));
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
}

/// Build the ground + bump once the blueprint has landed: place the ground at the rest track's
/// lowest point so the three cast shapes read against it at a glance.
fn scene_unbuilt(scene: Option<Res<SceneGeom>>, blueprint: Option<Res<TankBlueprint>>) -> bool {
    scene.is_none() && blueprint.is_some()
}

fn setup_scene(
    mut commands: Commands,
    blueprint: Res<TankBlueprint>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let track = &blueprint.spec.track;
    let belt_len = track.pitch * track.link_count as f32;

    // Ground at the rest track's lowest point: build the rest route and take its min y.
    let circles = side_circles(
        track,
        &blueprint,
        TrackSide::Right,
        Pose::Rest,
        &Default::default(),
    );
    let route = crate::track::route::build_route(&circles, belt_len);
    let ground_y = route.pts.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);

    let geom = SceneGeom {
        ground_y,
        bump_z: (-0.6, 0.9),
        bump_h: 0.18,
        belt_len,
    };

    // Flat ground slab (top face at ground_y).
    let ground_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.17, 0.2),
        perceptual_roughness: 0.95,
        ..default()
    });
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(40.0, 0.4, 40.0))),
        MeshMaterial3d(ground_mat.clone()),
        Transform::from_xyz(0.0, ground_y - 0.2, 0.0),
    ));
    // The belly bump — a raised block so the footprint penetration varies across the track.
    let span = geom.bump_z.1 - geom.bump_z.0;
    let width = track.plane_x * 2.0 + track.width;
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(width, geom.bump_h, span))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.22, 0.23, 0.27),
            perceptual_roughness: 0.95,
            ..default()
        })),
        Transform::from_xyz(
            0.0,
            ground_y + geom.bump_h * 0.5,
            (geom.bump_z.0 + geom.bump_z.1) * 0.5,
        ),
    ));

    commands.insert_resource(geom);
}

// ---------------------------------------------------------------------------------------------
// Geometry — the derived running-gear circles for a side at a given cast pose
// ---------------------------------------------------------------------------------------------

/// The running-gear circles for one side (front→rear, sprocket at index 0), pin-line radii, in the
/// hull-local side plane `(z, y)` — the input the route builder expects. `Pose` sets how far the
/// SPRUNG road wheels move from their loaded rest (sprocket + idler are hull-fixed, so the cast
/// shapes pin at the ends and swing in the middle — the trapezoid the design sketched).
fn side_circles(
    track: &TrackSpec,
    blueprint: &TankBlueprint,
    side: TrackSide,
    pose: Pose,
    params: &SuspensionParams,
) -> Vec<(Vec2, f32)> {
    let sprocket = (
        Vec2::new(track.sprocket.center.0, track.sprocket.center.1),
        derive::sprocket_pitch_radius(track.pitch, track.sprocket.teeth),
    );
    // Idler: its measured radius (pin-line handling for the idler is a convention the editor is
    // here to settle — the shipped view feeds the raw radius, so match that for now).
    let idler = (
        Vec2::new(track.idler.center.0, track.idler.center.1),
        track.idler.radius,
    );
    let wheels: Vec<Vec2> = blueprint
        .geometry
        .roadwheels
        .iter()
        .filter(|(_, s)| *s == side)
        .map(|&(i, _)| {
            let p = blueprint.geometry.nodes[i].root_position;
            Vec2::new(p.z, p.y)
        })
        .collect();
    let pin_r = derive::pin_line_radius(track.wheel_radius, track.thickness);
    assemble_circles(sprocket, idler, &wheels, pin_r, pose, params)
}

/// Assemble the front→rear pin-line circle list from raw running-gear data, applying the cast-pose
/// vertical offset to the SPRUNG road wheels only (sprocket + idler are hull-fixed, so the cast
/// shapes pin at the ends and swing at the belly). Pure — the ECS-facing `side_circles` feeds it
/// blueprint data; the tests feed it synthetic gear.
fn assemble_circles(
    sprocket: (Vec2, f32),
    idler: (Vec2, f32),
    wheels: &[Vec2],
    pin_r: f32,
    pose: Pose,
    params: &SuspensionParams,
) -> Vec<(Vec2, f32)> {
    let dy = match pose {
        Pose::Rest => 0.0,
        Pose::Droop => -derive::static_deflection(params.ride_frequency),
        Pose::Compression => params.bump_stop,
    };
    let mut circles = vec![sprocket];
    let mut moved: Vec<Vec2> = wheels.iter().map(|w| Vec2::new(w.x, w.y + dy)).collect();
    moved.sort_by(|a, b| a.x.total_cmp(&b.x));
    circles.extend(moved.into_iter().map(|c| (c, pin_r)));
    circles.push(idler);
    circles
}

/// Side sign for a track side: Left → −plane_x, Right → +plane_x.
fn side_sign(side: TrackSide) -> f32 {
    match side {
        TrackSide::Left => -1.0,
        TrackSide::Right => 1.0,
    }
}

/// Lift a side-plane `(z, y)` point to a world position on the given side.
fn world(side: TrackSide, p: Vec2, plane_x: f32) -> Vec3 {
    Vec3::new(side_sign(side) * plane_x, p.y, p.x)
}

// ---------------------------------------------------------------------------------------------
// The draw — every layer, both sides
// ---------------------------------------------------------------------------------------------

const ORANGE: Color = Color::srgb(1.0, 0.6, 0.1);
const GREEN: Color = Color::srgb(0.25, 0.9, 0.4);
const RED: Color = Color::srgb(0.95, 0.28, 0.22);
const CYAN: Color = Color::srgb(0.35, 0.8, 1.0);
const YELLOW: Color = Color::srgb(1.0, 0.9, 0.35);
const MAGENTA: Color = Color::srgb(1.0, 0.3, 0.85);
const WHITE: Color = Color::srgb(0.95, 0.96, 1.0);

fn draw_suspension(
    mut gizmos: Gizmos,
    blueprint: Res<TankBlueprint>,
    params: Res<EditorParams>,
    toggles: Res<VizToggles>,
    geom: Res<SceneGeom>,
) {
    let track = &blueprint.spec.track;
    let plane_x = track.plane_x;
    let p = &params.0;

    for side in [TrackSide::Left, TrackSide::Right] {
        // --- Circles (pin-line running gear) ---
        if toggles.circles {
            for (c, r) in side_circles(track, &blueprint, side, Pose::Rest, p) {
                draw_circle(&mut gizmos, world(side, c, plane_x), r, CYAN);
            }
        }

        // --- Cast poses: rest route, droop (green), compression (red) ---
        let rest = crate::track::route::build_route(
            &side_circles(track, &blueprint, side, Pose::Rest, p),
            geom.belt_len,
        );
        if toggles.route {
            draw_loop(&mut gizmos, side, &rest.pts, plane_x, ORANGE);
        }
        let droop = crate::track::route::build_route(
            &side_circles(track, &blueprint, side, Pose::Droop, p),
            geom.belt_len,
        );
        if toggles.droop {
            draw_loop(&mut gizmos, side, &droop.pts, plane_x, GREEN);
        }
        if toggles.compression {
            let comp = crate::track::route::build_route(
                &side_circles(track, &blueprint, side, Pose::Compression, p),
                geom.belt_len,
            );
            draw_loop(&mut gizmos, side, &comp.pts, plane_x, RED);
        }

        // --- Buoyant box: convex bound of the droop shape's lower (ground) run ---
        if toggles.buoyant_box {
            draw_buoyant_box(&mut gizmos, side, &droop.pts, plane_x, track.width, GREEN);
        }

        // --- Belt: discrete links resampled at pitch, with pin markers ---
        if toggles.belt {
            let pins = crate::track::route::resample(&rest.pts, track.pitch, 0.0);
            for (i, w) in pins.windows(2).enumerate() {
                let a = world(side, w[0], plane_x);
                let b = world(side, w[1], plane_x);
                gizmos.line(a, b, YELLOW);
                // Pin markers; pin-0 highlighted so the loop origin reads.
                let color = if i == 0 { MAGENTA } else { YELLOW };
                gizmos.sphere(Isometry3d::from_translation(a), 0.03, color);
            }
        }

        // --- Sprocket teeth ring + tooth-0 ---
        if toggles.sprocket {
            draw_sprocket(&mut gizmos, side, track, plane_x);
        }

        // --- Contact: droop-shape penetration against the ground ---
        if toggles.contact {
            draw_contact(&mut gizmos, side, &droop.pts, plane_x, &geom);
        }
    }
}

/// A circle in the hull side plane (normal along world X).
fn draw_circle(gizmos: &mut Gizmos, center: Vec3, radius: f32, color: Color) {
    let facing = Quat::from_rotation_arc(Vec3::Z, Vec3::X);
    gizmos.circle(Isometry3d::new(center, facing), radius, color);
}

/// A closed side-plane polyline lifted to a side.
fn draw_loop(gizmos: &mut Gizmos, side: TrackSide, pts: &[Vec2], plane_x: f32, color: Color) {
    gizmos.linestrip(pts.iter().map(|&p| world(side, p, plane_x)), color);
}

/// The convex box approximating the droop shape's ground run — Yan's "buoyant box sailing through
/// terrain". Take the lower half of the loop (below the mid-height), bound it in (z, y), and draw
/// the box extruded across the track width.
fn draw_buoyant_box(
    gizmos: &mut Gizmos,
    side: TrackSide,
    pts: &[Vec2],
    plane_x: f32,
    track_width: f32,
    color: Color,
) {
    let (mut ylo, mut yhi) = (f32::INFINITY, f32::NEG_INFINITY);
    for p in pts {
        ylo = ylo.min(p.y);
        yhi = yhi.max(p.y);
    }
    let mid = (ylo + yhi) * 0.5;
    let lower: Vec<Vec2> = pts.iter().copied().filter(|p| p.y < mid).collect();
    if lower.len() < 2 {
        return;
    }
    let (mut zlo, mut zhi, mut blo, mut bhi) = (
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    );
    for p in &lower {
        zlo = zlo.min(p.x);
        zhi = zhi.max(p.x);
        blo = blo.min(p.y);
        bhi = bhi.max(p.y);
    }
    let s = side_sign(side) * plane_x;
    let (x0, x1) = (s - track_width * 0.5, s + track_width * 0.5);
    // 8 corners of the box, drawn as 12 edges.
    let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let corners = [
        c(x0, blo, zlo),
        c(x1, blo, zlo),
        c(x1, blo, zhi),
        c(x0, blo, zhi),
        c(x0, bhi, zlo),
        c(x1, bhi, zlo),
        c(x1, bhi, zhi),
        c(x0, bhi, zhi),
    ];
    for (a, b) in [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        gizmos.line(corners[a], corners[b], color);
    }
}

/// The sprocket tooth ring: `teeth` radial ticks at the pitch radius, `tooth-0` highlighted. Teeth
/// mesh BETWEEN pins (the settled convention), so the ticks sit at the pitch circle where the pin
/// line rides.
fn draw_sprocket(gizmos: &mut Gizmos, side: TrackSide, track: &TrackSpec, plane_x: f32) {
    let center = Vec2::new(track.sprocket.center.0, track.sprocket.center.1);
    let r = derive::sprocket_pitch_radius(track.pitch, track.sprocket.teeth);
    let center_w = world(side, center, plane_x);
    draw_circle(gizmos, center_w, r, WHITE);
    let teeth = track.sprocket.teeth;
    for t in 0..teeth {
        let ang = std::f32::consts::TAU * t as f32 / teeth as f32;
        let dir = Vec2::from_angle(ang);
        let tip = center + dir * (r + 0.05);
        let base = center + dir * (r - 0.05);
        let color = if t == 0 { RED } else { WHITE };
        gizmos.line(world(side, base, plane_x), world(side, tip, plane_x), color);
    }
}

/// Draw the droop shape's ground penetration: for each lower-run point below the ground surface,
/// a vertical stub up to the surface, colored blue (grazing) → red (deep) — the support the buoyant
/// box would feel.
fn draw_contact(
    gizmos: &mut Gizmos,
    side: TrackSide,
    pts: &[Vec2],
    plane_x: f32,
    geom: &SceneGeom,
) {
    let (mut ylo, mut yhi) = (f32::INFINITY, f32::NEG_INFINITY);
    for p in pts {
        ylo = ylo.min(p.y);
        yhi = yhi.max(p.y);
    }
    let mid = (ylo + yhi) * 0.5;
    for p in pts.iter().filter(|p| p.y < mid) {
        let surface = geom.height(p.x);
        let depth = surface - p.y;
        if depth <= 0.0 {
            continue;
        }
        let t = (depth / 0.15).clamp(0.0, 1.0);
        let color = Color::srgb(0.2 + 0.75 * t, 0.5 * (1.0 - t), 1.0 - 0.8 * t);
        let low = world(side, *p, plane_x);
        let high = Vec3::new(low.x, surface, low.z);
        gizmos.line(low, high, color);
        gizmos.sphere(Isometry3d::from_translation(low), 0.02, color);
    }
}

// ---------------------------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------------------------

fn cursor_locked(cursors: Query<&CursorOptions>) -> bool {
    cursors
        .single()
        .map(|c| c.grab_mode == CursorGrabMode::Locked)
        .unwrap_or(false)
}

/// Free-fly: mouse look, WASD planar, Shift/Ctrl altitude, on real time so inspection survives.
fn fly_camera(
    camera: Single<&mut Transform, With<FlyCam>>,
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

    const SPEED: f32 = 6.0;
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

/// Esc releases/recaptures the cursor (so you can leave the window or resume mouse-look).
fn toggle_pause(
    keys: Res<ButtonInput<KeyCode>>,
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
    } else {
        let center = window.size() / 2.0;
        window.set_cursor_position(Some(center));
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
}

/// Number keys toggle overlay layers.
fn toggle_layers(keys: Res<ButtonInput<KeyCode>>, mut t: ResMut<VizToggles>) {
    if keys.just_pressed(KeyCode::Digit1) {
        t.circles = !t.circles;
    }
    if keys.just_pressed(KeyCode::Digit2) {
        t.route = !t.route;
    }
    if keys.just_pressed(KeyCode::Digit3) {
        t.droop = !t.droop;
    }
    if keys.just_pressed(KeyCode::Digit4) {
        t.compression = !t.compression;
    }
    if keys.just_pressed(KeyCode::Digit5) {
        t.buoyant_box = !t.buoyant_box;
    }
    if keys.just_pressed(KeyCode::Digit6) {
        t.belt = !t.belt;
    }
    if keys.just_pressed(KeyCode::Digit7) {
        t.sprocket = !t.sprocket;
    }
    if keys.just_pressed(KeyCode::Digit8) {
        t.contact = !t.contact;
    }
}

/// `[` `]` nudge ride frequency (softer/stiffer → droop shape moves); `-` `=` nudge the bump-stop
/// (compression shape moves). This live re-derivation is what makes it an editor.
fn tweak_params(keys: Res<ButtonInput<KeyCode>>, mut params: ResMut<EditorParams>) {
    let p = &mut params.0;
    if keys.just_pressed(KeyCode::BracketLeft) {
        p.ride_frequency = (p.ride_frequency - 0.1).max(0.4);
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        p.ride_frequency = (p.ride_frequency + 0.1).min(3.0);
    }
    if keys.just_pressed(KeyCode::Minus) {
        p.bump_stop = (p.bump_stop - 0.02).max(0.0);
    }
    if keys.just_pressed(KeyCode::Equal) {
        p.bump_stop = (p.bump_stop + 0.02).min(0.5);
    }
}

/// `M` toggles the glb visual model.
fn toggle_model(
    keys: Res<ButtonInput<KeyCode>>,
    mut model: Query<&mut Visibility, With<ModelRoot>>,
) {
    if !keys.just_pressed(KeyCode::KeyM) {
        return;
    }
    for mut vis in &mut model {
        *vis = match *vis {
            Visibility::Hidden => Visibility::Inherited,
            _ => Visibility::Hidden,
        };
    }
}

// ---------------------------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------------------------

fn spawn_panel(mut commands: Commands) {
    commands.spawn((
        PanelText,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(13.0),
            ..default()
        },
        TextColor(Color::srgb(0.86, 0.9, 0.98)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(10.0),
            ..default()
        },
    ));
}

fn update_panel(
    blueprint: Option<Res<TankBlueprint>>,
    params: Res<EditorParams>,
    toggles: Res<VizToggles>,
    geom: Option<Res<SceneGeom>>,
    mut text: Query<&mut Text, With<PanelText>>,
) {
    let Ok(mut text) = text.single_mut() else {
        return;
    };
    let Some(blueprint) = blueprint else {
        *text = Text::new("loading Tiger blueprint...");
        return;
    };
    let spec = &blueprint.spec;
    let track = &spec.track;
    let p = &params.0;

    let sprocket_r = derive::sprocket_pitch_radius(track.pitch, track.sprocket.teeth);
    let droop = derive::static_deflection(p.ride_frequency);
    let k = derive::spring_rate(spec.mass, p.ride_frequency);
    let c = derive::damping_coefficient(spec.mass, p.ride_frequency, p.damping_ratio);
    let pin_r = derive::pin_line_radius(track.wheel_radius, track.thickness);
    let belt_len = geom.as_ref().map(|g| g.belt_len).unwrap_or(0.0);
    let n_wheels = track_side_count(&blueprint, TrackSide::Right);

    // Self-check: rebuild the rest route and confirm the derived link count matches the authored
    // one, and that the belt resampled at pitch really does step by `pitch` (the loop closes on the
    // material length). This is the editor validating the universal laws against the RON.
    let rest_circles = side_circles(track, &blueprint, TrackSide::Right, Pose::Rest, p);
    let taut = crate::track::route::build_route(&rest_circles, 0.0);
    let derived_count = derive::link_count(taut.total(), track.pitch);
    let draped = crate::track::route::build_route(&rest_circles, belt_len);
    let pins = crate::track::route::resample(&draped.pts, track.pitch, 0.0);
    let pin_pitch = if pins.len() >= 2 {
        derive::pitch_from_pins(pins[0], pins[1])
    } else {
        0.0
    };

    let on = |b: bool| if b { "on " } else { "off" };
    *text = Text::new(format!(
        "SUSPENSION EDITOR - Tiger I\n\
         \n\
         SOURCES (glb + RON)\n\
         mass          {:.0} kg\n\
         pitch         {:.3} m   link_count {}\n\
         wheel_radius  {:.3} m   thickness  {:.3} m\n\
         sprocket      {} teeth\n\
         idler_radius  {:.3} m   plane_x    {:.3} m\n\
         road wheels   {} / side\n\
         \n\
         DERIVED (universal laws)\n\
         sprocket pitch r  {:.4} m  (pitch*teeth/tau)\n\
         pin-line r        {:.4} m  (wheel_r + thick/2)\n\
         belt loop         {:.3} m  (pitch*count)\n\
         taut wrap         {:.3} m  -> derived links {} (authored {})\n\
         pin pitch check   {:.4} m  (authored {:.3})\n\
         \n\
         SUSPENSION (tweakable)\n\
         ride freq [ ]     {:.2} Hz\n\
         bump stop  - =    {:.3} m\n\
         damping ratio     {:.2}\n\
         -> spring rate    {:.0} N/m\n\
         -> damper c       {:.0} N.s/m\n\
         -> max droop      {:.3} m  (static defl)\n\
         \n\
         LAYERS  1 circles:{}  2 route:{}  3 droop:{}  4 comp:{}\n\
         5 box:{}  6 belt:{}  7 sprocket:{}  8 contact:{}\n\
         M model   WASD/mouse fly   Esc cursor\n\
         green=max droop (soft)  red=max compression (hard)  orange=rest route",
        spec.mass,
        track.pitch,
        track.link_count,
        track.wheel_radius,
        track.thickness,
        track.sprocket.teeth,
        track.idler.radius,
        track.plane_x,
        n_wheels,
        sprocket_r,
        pin_r,
        belt_len,
        taut.total(),
        derived_count,
        track.link_count,
        pin_pitch,
        track.pitch,
        p.ride_frequency,
        p.bump_stop,
        p.damping_ratio,
        k,
        c,
        droop,
        on(toggles.circles),
        on(toggles.route),
        on(toggles.droop),
        on(toggles.compression),
        on(toggles.buoyant_box),
        on(toggles.belt),
        on(toggles.sprocket),
        on(toggles.contact),
    ));
}

/// Headless inspection: when `SUSPENSION_EDITOR_DUMP=1`, log the real Tiger's derived suspension
/// geometry once the scene is built, then exit. A no-GUI way to sanity-check the model (and a CI /
/// scripting hook) — zero cost otherwise.
fn dump_and_exit(
    blueprint: Res<TankBlueprint>,
    params: Res<EditorParams>,
    geom: Res<SceneGeom>,
    mut exit: MessageWriter<AppExit>,
) {
    if std::env::var("SUSPENSION_EDITOR_DUMP").is_err() {
        return;
    }
    let track = &blueprint.spec.track;
    let p = &params.0;
    let bounds = |pose: Pose| {
        let c = side_circles(track, &blueprint, TrackSide::Right, pose, p);
        let r = crate::track::route::build_route(&c, geom.belt_len);
        let (mut ylo, mut yhi, mut zlo, mut zhi) = (
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        );
        for q in &r.pts {
            ylo = ylo.min(q.y);
            yhi = yhi.max(q.y);
            zlo = zlo.min(q.x);
            zhi = zhi.max(q.x);
        }
        (ylo, yhi, zlo, zhi, r.total())
    };
    let (r_ylo, r_yhi, r_zlo, r_zhi, r_len) = bounds(Pose::Rest);
    let (d_ylo, ..) = bounds(Pose::Droop);
    let (c_ylo, ..) = bounds(Pose::Compression);
    let taut = crate::track::route::build_route(
        &side_circles(track, &blueprint, TrackSide::Right, Pose::Rest, p),
        0.0,
    );
    info!("=== SUSPENSION EDITOR DUMP (Tiger, right side) ===");
    info!(
        "wheels/side {}  pitch {:.3}  authored links {}  derived(taut {:.3}) {}",
        track_side_count(&blueprint, TrackSide::Right),
        track.pitch,
        track.link_count,
        taut.total(),
        derive::link_count(taut.total(), track.pitch),
    );
    info!(
        "sprocket pitch_r {:.4}  pin-line_r {:.4}  belt_len {:.3}  rest route_len {:.3}",
        derive::sprocket_pitch_radius(track.pitch, track.sprocket.teeth),
        derive::pin_line_radius(track.wheel_radius, track.thickness),
        geom.belt_len,
        r_len,
    );
    info!(
        "rest route: y [{:.3}, {:.3}]  z [{:.3}, {:.3}]  ground_y {:.3}",
        r_ylo, r_yhi, r_zlo, r_zhi, geom.ground_y
    );
    info!(
        "belly min-y  rest {:.3}  droop {:.3} (Δ{:.3})  compression {:.3} (Δ{:.3})  [ride {:.2}Hz bump {:.3}]",
        r_ylo,
        d_ylo,
        r_ylo - d_ylo,
        c_ylo,
        c_ylo - r_ylo,
        p.ride_frequency,
        p.bump_stop,
    );
    info!(
        "static deflection {:.3}  spring_rate {:.0} N/m  damper {:.0} N.s/m",
        derive::static_deflection(p.ride_frequency),
        derive::spring_rate(blueprint.spec.mass, p.ride_frequency),
        derive::damping_coefficient(blueprint.spec.mass, p.ride_frequency, p.damping_ratio),
    );
    exit.write(AppExit::Success);
}

fn track_side_count(blueprint: &TankBlueprint, side: TrackSide) -> usize {
    blueprint
        .geometry
        .roadwheels
        .iter()
        .filter(|(_, s)| *s == side)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic running gear with the road wheels clearly the lowest circles, so the belly (route
    /// min y) tracks the sprung wheels.
    fn gear() -> ((Vec2, f32), (Vec2, f32), Vec<Vec2>) {
        let sprocket = (Vec2::new(-2.0, 0.6), 0.39);
        let idler = (Vec2::new(2.0, 0.6), 0.40);
        let wheels = vec![
            Vec2::new(-1.0, 0.4),
            Vec2::new(0.0, 0.4),
            Vec2::new(1.0, 0.4),
        ];
        (sprocket, idler, wheels)
    }

    fn belly_y(pose: Pose, params: &SuspensionParams) -> f32 {
        let (sprocket, idler, wheels) = gear();
        let circles = assemble_circles(sprocket, idler, &wheels, 0.5, pose, params);
        let route = crate::track::route::build_route(&circles, 12.6);
        assert!(
            route.pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()),
            "route must be finite"
        );
        route.pts.iter().map(|p| p.y).fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn droop_drops_and_compression_lifts_the_belly() {
        let p = SuspensionParams::default();
        let rest = belly_y(Pose::Rest, &p);
        let droop = belly_y(Pose::Droop, &p);
        let comp = belly_y(Pose::Compression, &p);
        // Max droop sits BELOW the loaded rest (green low), max compression ABOVE (red high) —
        // exactly the design's cast-shape ordering.
        assert!(droop < rest, "droop {droop} should sit below rest {rest}");
        assert!(
            comp > rest,
            "compression {comp} should sit above rest {rest}"
        );
        // The droop stroke equals the static deflection the wheels dropped by.
        let expected = derive::static_deflection(p.ride_frequency);
        assert!(
            (rest - droop - expected).abs() < 1e-4,
            "droop stroke {} should equal static deflection {expected}",
            rest - droop
        );
    }

    #[test]
    fn sprocket_stays_index_zero_and_loop_closes() {
        let (sprocket, idler, wheels) = gear();
        let circles = assemble_circles(
            sprocket,
            idler,
            &wheels,
            0.5,
            Pose::Rest,
            &SuspensionParams::default(),
        );
        assert_eq!(
            circles[0], sprocket,
            "route builder needs sprocket at index 0"
        );
        assert_eq!(*circles.last().unwrap(), idler);
        let route = crate::track::route::build_route(&circles, 12.6);
        assert_eq!(route.pts.first(), route.pts.last(), "the loop must close");
    }
}
