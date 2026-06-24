//! The battlefield: environment lighting now, terrain later. Also home to the ground-plane
//! query that aiming and the camera both use — the seam to swap for an Avian raycast once
//! terrain has colliders.

use avian3d::prelude::{Collider, CollisionLayers, LayerMask, RigidBody};
use bevy::prelude::*;

use crate::Layer;

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
    commands.spawn((
        DirectionalLight { illuminance: 10_000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // The ground: a static slab whose top face sits at y=0 — the same plane the analytic
    // `ground_distance` assumes, so aim/camera are unaffected. A unit cuboid collider scaled
    // by the Transform (the Avian idiom), buried so only the top surface is in play.
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.32, 0.42, 0.28))),
        Transform::from_xyz(0.0, -GROUND_THICKNESS / 2.0, 0.0)
            .with_scale(Vec3::new(GROUND_SIZE, GROUND_THICKNESS, GROUND_SIZE)),
        RigidBody::Static,
        Collider::cuboid(1.0, 1.0, 1.0),
        // Terrain layer: what the wheel suspension rays are allowed to hit.
        CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
    ));
}

/// Distance along `ray` to the ground (y=0 plane), capped at `max`, falling back to `max`
/// when the ray misses. The single seam to swap for a world raycast once terrain exists.
pub fn ground_distance(ray: Ray3d, max: f32) -> f32 {
    ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
        .filter(|&d| d > 0.0)
        .map(|d| d.min(max))
        .unwrap_or(max)
}
