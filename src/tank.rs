//! The tank: its rig (structural markers bound by node name), the kinematic `Servo` motor
//! for the turret/gun, and the asset-load binding. The tank declares *structure*; features
//! (aim, shooting) attach their own behavior to these markers reactively.

use avian3d::prelude::{
    Collider, ColliderDensity, CollisionLayers, LayerMask, RayCaster, RigidBody, SpatialQueryFilter,
};
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use crate::Layer;
use crate::state::GameplaySet;

// Hull collision box, metres. PLACEHOLDER Tiger-ish estimates — TUNE to the model's hull
// bounding box (Bevy-local: X = width over tracks, Y = height ground→roof, Z = length).
const HULL_WIDTH: f32 = 3.5;
const HULL_HEIGHT: f32 = 1.4;
const HULL_LENGTH: f32 = 6.3;
/// Ground clearance of the hull belly (m): the box is raised by this so the roadwheels carry the
/// tank on flat ground and the box only handles obstacle collisions (and bottoming-out).
const HULL_BELLY: f32 = 0.4;
/// Uniform density giving ~57 t (≈ Tiger I) at the current hull box. TUNE alongside the dims.
const HULL_DENSITY: f32 = 1850.0;

/// How far a suspension ray reaches from the hub (metres). Must exceed the effective radius
/// (~0.5166) so it finds the ground at rest, with margin for droop.
const SUSPENSION_RAY_LENGTH: f32 = 0.85;

// --- Rig markers. Name = the structural contract between the model and the code. ---

#[derive(Component)]
pub struct Turret;

#[derive(Component)]
pub struct Gun;

#[derive(Component)]
pub struct Hull;

/// Marks the vehicle's root entity — the dynamic rigid body (chassis). Suspension/drive forces
/// are applied here; debug x-ray walks its descendants.
#[derive(Component)]
pub struct Tank;

#[derive(Component)]
pub struct Muzzle;

/// The recoiling barrel node (child of `Gun`, parent of `Muzzle`).
#[derive(Component)]
pub struct GunBarrel;

/// Which track a roadwheel drives (for differential thrust). Left wheels sit at −X, right at +X.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TrackSide {
    Left,
    Right,
}

/// A load-bearing roadwheel — a suspension/drive contact station, tagged with its track side.
/// Carries a downward [`RayCaster`] (the suspension ray); the sprocket and idler are excluded.
#[derive(Component)]
pub struct Roadwheel {
    pub side: TrackSide,
}

/// The authored centre-of-mass: an Empty (`Center_Of_Mass`) placed in the model. `driving` reads
/// its position and sets the body's centre of mass from it — the model owns the COM.
#[derive(Component)]
pub struct CenterOfMassAnchor;

/// Travel limits for a [`Servo`].
#[derive(Clone, Copy)]
enum Travel {
    Limited { min: f32, max: f32 },
    Continuous,
}

/// A 1-DOF kinematic rotational motor with a trapezoidal motion profile. Aiming writes
/// [`Servo::target`]; `drive_servos` slews `current` toward it and applies the rotation.
#[derive(Component)]
pub struct Servo {
    axis: Vec3,
    current: f32,
    velocity: f32,
    /// Desired angle, in the parent-local frame. Written by the aim feature.
    pub target: f32,
    max_speed: f32,
    accel: f32,
    travel: Travel,
}

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn_tank)
        .add_systems(FixedUpdate, drive_servos.in_set(GameplaySet));
}

fn spawn_tank(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands
        .spawn((
            WorldAssetRoot(
                asset_server.load(GltfAssetLabel::Scene(0).from_asset("tiger_1/tiger_1.glb")),
            ),
            Transform::from_xyz(10.0, 2.0, 5.0).with_rotation(Quat::from_rotation_z(0.7)),
            // The hull is a dynamic rigid body — Avian now owns its Transform (ADR-0005).
            Tank,
            RigidBody::Dynamic,
        ))
        // Hull collider: an offset child box for the upper hull, raised by the belly clearance so
        // the roadwheels (not this box) carry the tank on flat ground. Density sets the mass.
        .with_children(|children| {
            children.spawn((
                Collider::cuboid(HULL_WIDTH, HULL_HEIGHT, HULL_LENGTH),
                Transform::from_xyz(0.0, HULL_BELLY + HULL_HEIGHT / 2.0, 0.0),
                // Vehicle layer — the wheel rays are filtered to skip this.
                CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
                ColliderDensity(HULL_DENSITY),
            ));
        })
        .observe(on_tank_ready);
}

/// Walk the loaded scene and bind structural markers + the turret/gun servos by node name.
fn on_tank_ready(
    ready: On<WorldInstanceReady>,
    mut commands: Commands,
    children: Query<&Children>,
    names: Query<&Name>,
) {
    for entity in children.iter_descendants(ready.entity) {
        // Most descendants are unnamed mesh nodes — skip them quietly.
        let Ok(name) = names.get(entity) else { continue };
        let mut entity = commands.entity(entity);
        match name.as_str() {
            "Turret" => {
                entity.insert((
                    Turret,
                    Servo {
                        axis: Vec3::Y,
                        current: 0.0,
                        velocity: 0.0,
                        target: 0.0,
                        max_speed: 0.6,
                        accel: 0.3,
                        travel: Travel::Continuous,
                    },
                ));
            }
            "Gun" => {
                entity.insert((
                    Gun,
                    Servo {
                        axis: Vec3::X,
                        current: 0.0,
                        velocity: 0.0,
                        target: 0.0,
                        max_speed: 0.4,
                        accel: 2.0,
                        travel: Travel::Limited {
                            min: (-8.0_f32).to_radians(),
                            max: 15.0_f32.to_radians(),
                        },
                    },
                ));
            }
            "Hull" => {
                entity.insert(Hull);
            }
            "Muzzle" => {
                entity.insert(Muzzle);
            }
            "Gun_Barrel" => {
                entity.insert(GunBarrel);
            }
            "Center_Of_Mass" => {
                entity.insert(CenterOfMassAnchor);
            }
            // Roadwheels (Wheel_L_0.., Wheel_R_0..): each gets a downward suspension ray,
            // filtered to Terrain so it skips the hull's own collider. The wheel node has
            // identity rotation, so local -Y is the hull-down suspension axis.
            s if s.starts_with("Wheel_") => {
                let side = if s.starts_with("Wheel_L") {
                    TrackSide::Left
                } else {
                    TrackSide::Right
                };
                entity.insert((
                    Roadwheel { side },
                    RayCaster::new(Vec3::ZERO, Dir3::NEG_Y)
                        .with_max_distance(SUSPENSION_RAY_LENGTH)
                        .with_query_filter(SpatialQueryFilter::from_mask(Layer::Terrain)),
                ));
            }
            _ => {}
        }
    }
}

fn drive_servos(mut q: Query<(&mut Transform, &mut Servo)>, time: Res<Time>) {
    let dt = time.delta_secs();
    for (mut transform, mut servo) in &mut q {
        let prev = servo.current;
        let error = match servo.travel {
            Travel::Limited { .. } => servo.target - servo.current,
            Travel::Continuous => shortest_angle(servo.target - servo.current),
        };
        let braking_dist = (servo.velocity * servo.velocity) / (2.0 * servo.accel);

        if error.abs() <= braking_dist {
            let dv = servo.accel * dt;
            servo.velocity = if servo.velocity > 0.0 {
                (servo.velocity - dv).max(0.0)
            } else {
                (servo.velocity + dv).min(0.0)
            };
        } else {
            servo.velocity += error.signum() * servo.accel * dt;
            servo.velocity = servo.velocity.clamp(-servo.max_speed, servo.max_speed);
        }

        servo.current += servo.velocity * dt;
        if let Travel::Limited { min, max } = servo.travel {
            servo.current = servo.current.clamp(min, max);
        }

        if error.abs() < 0.001 && servo.velocity.abs() < 0.01 {
            servo.velocity = 0.0;
            if let Travel::Limited { min, max } = servo.travel {
                servo.current = servo.target.clamp(min, max);
            }
        }

        let delta = servo.current - prev;
        transform.rotate_local(Quat::from_axis_angle(servo.axis, delta));
    }
}

/// Wrap an angle difference into [-PI, PI] for shortest-path rotation.
fn shortest_angle(diff: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (diff + PI).rem_euclid(TAU) - PI
}
