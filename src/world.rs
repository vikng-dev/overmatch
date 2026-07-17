//! The battlefield: environment lighting and a suspension test course now, real terrain later.
//! Also home to the ground-plane query that aiming and the camera both use — the seam to swap
//! for an Avian raycast once terrain has colliders.

use avian3d::prelude::{
    Collider, CollisionLayers, LayerMask, RigidBody, SpatialQuery, SpatialQueryFilter,
};
use bevy::prelude::*;

use crate::Layer;

/// The world's static terrain as DATA (track architecture §5): every terrain block in authoring
/// form — a unit cube posed/scaled by its Transform (the Avian collider idiom). Colliders are
/// spawned FROM this list, and the track module's analytic `BlockField` is built from the SAME
/// list, so the two representations cannot drift. `revision` bumps whenever the set changes
/// (map load; future streaming/destruction) so consumers know to rebuild and reseed.
#[derive(Resource)]
pub struct TerrainMap {
    pub revision: u64,
    pub blocks: Vec<Transform>,
}

/// Side length of the (square) ground plane, in metres.
const GROUND_SIZE: f32 = 1000.0;
/// Thickness of the ground slab. Only the top face (at y=0) matters; the rest is buried.
const GROUND_THICKNESS: f32 = 1.0;

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn_environment);
}

fn spawn_environment(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut blocks: Vec<Transform> = Vec::new();
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // The ground: a static slab whose top face sits at y=0 — the same plane the analytic
    // `ground_distance` assumes, so aim/camera are unaffected.
    spawn_block(
        &mut commands,
        &mut blocks,
        meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        materials.add(Color::srgb(0.32, 0.42, 0.28)),
        Transform::from_xyz(0.0, -GROUND_THICKNESS / 2.0, 0.0).with_scale(Vec3::new(
            GROUND_SIZE,
            GROUND_THICKNESS,
            GROUND_SIZE,
        )),
    );

    // The suspension test course — deliberate, known geometry (not a scenic map) laid out down
    // the −Z lane in front of spawn, so each obstacle isolates one suspension behaviour and you
    // can tell the *sim* from the *terrain*. All on the Terrain layer, so the wheel rays read it
    // identically to the ground.
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let ramp_mat = materials.add(Color::srgb(0.45, 0.38, 0.28));
    let bump_mat = materials.add(Color::srgb(0.40, 0.33, 0.24));
    spawn_test_course(&mut commands, &mut blocks, &cube, &ramp_mat, &bump_mat);
    commands.insert_resource(TerrainMap {
        revision: 0,
        blocks,
    });
}

/// Spawn a static, unit-cube collision block scaled/posed by `transform` (the Avian idiom: a
/// `Collider::cuboid(1,1,1)` that the Transform's scale stretches), on the Terrain layer — and
/// record it in the [`TerrainMap`] block list (the single terrain data source).
fn spawn_block(
    commands: &mut Commands,
    blocks: &mut Vec<Transform>,
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    transform: Transform,
) {
    blocks.push(transform);
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material),
        transform,
        RigidBody::Static,
        Collider::cuboid(1.0, 1.0, 1.0),
        CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
    ));
}

/// The four-obstacle suspension course. Each obstacle is a static cuboid (or row of them) sized
/// to isolate one thing the per-wheel suspension does. Reuses one unit-cube mesh and two
/// materials, cloned per block.
fn spawn_test_course(
    commands: &mut Commands,
    blocks: &mut Vec<Transform>,
    cube: &Handle<Mesh>,
    ramp_mat: &Handle<StandardMaterial>,
    bump_mat: &Handle<StandardMaterial>,
) {
    // 1. Graduated climbs — ramps at 10°/20°/30°, side by side, to compare pitch and find the
    //    climb limit. (With ~200 kN total thrust vs ~456 kN weight, 20° climbs but 30° stalls —
    //    gravity-along-slope exceeds thrust — so this also shows where it gives out.) Each is a
    //    slab tilted about X and sunk so its low edge's top sits ~1 m under the ground slab: the
    //    upslope crosses y=0 flush (step-free entry), the high edge a crest with a drop beyond.
    //    Low-edge top y = center_y + (thickness/2)·cosθ − (run/2)·sinθ; solve for center_y at −1 m.
    let (run, width, thick) = (10.0_f32, 10.0_f32, 2.0_f32);
    for (i, deg) in [10.0_f32, 20.0, 30.0].into_iter().enumerate() {
        let (sin, cos) = deg.to_radians().sin_cos();
        let center_y = -1.0 - (thick / 2.0) * cos + (run / 2.0) * sin;
        let x = (i as f32 - 1.0) * 14.0; // −14, 0, +14
        spawn_block(
            commands,
            blocks,
            cube.clone(),
            ramp_mat.clone(),
            Transform::from_xyz(x, center_y, -40.0)
                .with_rotation(Quat::from_rotation_x(deg.to_radians()))
                .with_scale(Vec3::new(width, thick, run)),
        );
    }

    // 2. Side-slope — a banked lane tilted about Z, driven ALONG Z so the tank is canted sideways:
    //    shows roll, lateral weight transfer, and whether it holds the face or slides off. Centred
    //    at y=0 so the banked top crosses ground near the lane centre (a roughly flush approach).
    spawn_block(
        commands,
        blocks,
        cube.clone(),
        ramp_mat.clone(),
        Transform::from_xyz(38.0, 0.0, -45.0)
            .with_rotation(Quat::from_rotation_z(18.0_f32.to_radians()))
            .with_scale(Vec3::new(16.0, 2.0, 26.0)),
    );

    // 3. Step / curb — a low box driven over: front wheels lift over the hard edge, then the rear.
    //    Single-wheel articulation against a vertical edge (top at y=0.4).
    spawn_block(
        commands,
        blocks,
        cube.clone(),
        bump_mat.clone(),
        Transform::from_xyz(0.0, 0.2, -70.0).with_scale(Vec3::new(14.0, 0.4, 4.0)),
    );

    // 4. Washboard — a row of low bumps; wheels rise and fall independently while the hull stays
    //    composed (the most legible "suspension is working" demo). Boxes approximate rounded bumps
    //    — a round profile is a later refinement.
    for i in 0..6 {
        let z = -82.0 - i as f32 * 1.6;
        spawn_block(
            commands,
            blocks,
            cube.clone(),
            bump_mat.clone(),
            Transform::from_xyz(0.0, 0.12, z).with_scale(Vec3::new(14.0, 0.25, 0.6)),
        );
    }
}

/// Distance along `ray` to the terrain, capped at `max`, falling back to `max` when the ray
/// misses (sky / above the horizon). A world raycast against the `Terrain` layer ONLY — the orbit
/// camera's ground pull-in, which must ignore tanks (a tank crossing behind the player must not
/// yank the camera in). Aim rays use `aim::aim_distance` instead, which adds the `Armor` layer so
/// the aim dots predict what a shell would actually meet, tanks included.
pub fn ground_distance(spatial: &SpatialQuery, ray: Ray3d, max: f32) -> f32 {
    spatial
        .cast_ray(
            ray.origin,
            ray.direction,
            max,
            true,
            &SpatialQueryFilter::from_mask(Layer::Terrain),
        )
        .map(|hit| hit.distance)
        .unwrap_or(max)
}
