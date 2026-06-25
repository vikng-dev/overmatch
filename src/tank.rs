//! The tank: its rig (structural markers bound by node name), the kinematic `Servo` motor
//! for the turret/gun, and the asset-load binding. The tank declares *structure*; features
//! (aim, shooting) attach their own behavior to these markers reactively.

use avian3d::prelude::{
    ColliderConstructor, ColliderConstructorHierarchy, ColliderDensity, CollisionLayers, LayerMask,
    RayCaster, RigidBody, SpatialQueryFilter,
};
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;
use serde::Deserialize;

use crate::Layer;
use crate::spec::TankSpecHandle;
use crate::state::GameplaySet;

/// Uniform density of the hull collider (kg/m³): roughly Tiger-I mass at the authored collision
/// proxy's volume. PLACEHOLDER — mass is per-variant data (bucket 2), a later migration.
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

/// The local axis a servo rotates about. Cardinal-only — tank servos yaw/pitch about a hull axis;
/// a canted mount would add a `Custom(Dir3)` variant. Resolved to a vector in `drive_servos`.
#[derive(Clone, Copy, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    fn to_vec3(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }
}

/// Travel limits for a [`ServoSpec`], in **degrees** (the authoring unit).
#[derive(Clone, Copy, Deserialize)]
pub enum Travel {
    Limited { min: f32, max: f32 },
    Continuous,
}

// A 1-DOF kinematic rotational motor (trapezoidal motion profile), split three ways so each
// concern has one owner: per-variant config, the commanded intent, and the live mechanism state.
// `drive_servos` is the behaviour; it reads spec + command and drives state + the transform.

/// Servo config: rotation axis, speed/accel limits, travel range. Per-variant data authored in the
/// tank's `.tank.ron` spec sheet (ADR-0010) and applied to the bound servo node. Angles are in
/// **degrees** — the human-facing authoring unit; `drive_servos` converts to radians (the
/// computed/runtime unit shared with `ServoCommand` and `ServoState`).
#[derive(Component, Clone, Deserialize)]
pub struct ServoSpec {
    axis: Axis,
    /// Max slew speed, degrees/second.
    max_speed: f32,
    /// Slew acceleration, degrees/second².
    accel: f32,
    travel: Travel,
}

/// The commanded angle (parent-local) a servo slews toward — the *intent*, written by aiming
/// (and, later, the ROADMAP Phase-2 controls layer). Position-mode for now; a velocity-mode
/// command is a future variant (NOTES.md). Kept separate from state: different writer, different
/// lifecycle.
#[derive(Component, Default)]
pub struct ServoCommand {
    pub target: f32,
}

/// A servo's live mechanism state — current angle and angular velocity of the slew. Owned by
/// `drive_servos`; never authored, never shared.
#[derive(Component, Default)]
pub struct ServoState {
    current: f32,
    velocity: f32,
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
            // The variant spec sheet (thrust, servo speeds, …) rides alongside the geometry;
            // `apply_tank_spec` applies it once both the asset and the rig are ready (ADR-0010).
            TankSpecHandle(asset_server.load("tiger_1/tiger_1.tank.ron")),
            Transform::from_xyz(10.0, 2.0, 5.0).with_rotation(Quat::from_rotation_z(0.7)),
            // The hull is a dynamic rigid body — Avian owns its Transform (ADR-0005). Its collider
            // comes from the model's `*_Collider` convex proxy, bound in on_tank_ready (ADR-0008).
            Tank,
            RigidBody::Dynamic,
        ))
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
        let Ok(name) = names.get(entity) else {
            continue;
        };
        let mut entity = commands.entity(entity);
        match name.as_str() {
            // Servos: bind the marker + the command/state slots here (structure). The `ServoSpec`
            // config (axis, speeds, travel) is per-variant data, applied from the `.tank.ron` spec
            // sheet by `apply_tank_spec` (ADR-0010) — not authored in code.
            "Turret" => {
                entity.insert((Turret, ServoCommand::default(), ServoState::default()));
            }
            "Gun" => {
                entity.insert((Gun, ServoCommand::default(), ServoState::default()));
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
            // Collision proxies (`*_Collider`, optionally split `_0/_1/...`): each becomes a
            // convex-hull collider on the Vehicle layer — part of the compound rigid body — and is
            // hidden, since it exists for physics, not rendering (ADR-0008). The glTF loader puts
            // the mesh on a child primitive entity, so build over this node's descendants (the
            // hierarchy constructor) rather than the node itself, which has no mesh handle.
            s if s.contains("_Collider") => {
                entity.insert((
                    ColliderConstructorHierarchy::new(ColliderConstructor::ConvexHullFromMesh)
                        .with_default_layers(CollisionLayers::new([Layer::Vehicle], LayerMask::ALL))
                        .with_default_density(ColliderDensity(HULL_DENSITY)),
                    Visibility::Hidden,
                ));
            }
            _ => {}
        }
    }
}

fn drive_servos(
    mut q: Query<(&mut Transform, &ServoSpec, &ServoCommand, &mut ServoState)>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (mut transform, spec, command, mut state) in &mut q {
        // `ServoSpec` authors angles in degrees (the human/skein unit); the runtime — the command,
        // the state, and the slew maths below — is radians. Convert the spec's angular quantities
        // once here, at the spec→runtime boundary.
        let max_speed = spec.max_speed.to_radians();
        let accel = spec.accel.to_radians();
        let travel = match spec.travel {
            Travel::Limited { min, max } => Travel::Limited {
                min: min.to_radians(),
                max: max.to_radians(),
            },
            Travel::Continuous => Travel::Continuous,
        };

        let prev = state.current;
        let error = match travel {
            Travel::Limited { .. } => command.target - state.current,
            Travel::Continuous => shortest_angle(command.target - state.current),
        };
        let braking_dist = (state.velocity * state.velocity) / (2.0 * accel);

        if error.abs() <= braking_dist {
            let dv = accel * dt;
            state.velocity = if state.velocity > 0.0 {
                (state.velocity - dv).max(0.0)
            } else {
                (state.velocity + dv).min(0.0)
            };
        } else {
            state.velocity += error.signum() * accel * dt;
            state.velocity = state.velocity.clamp(-max_speed, max_speed);
        }

        state.current += state.velocity * dt;
        if let Travel::Limited { min, max } = travel {
            state.current = state.current.clamp(min, max);
        }

        if error.abs() < 0.001 && state.velocity.abs() < 0.01 {
            state.velocity = 0.0;
            if let Travel::Limited { min, max } = travel {
                state.current = command.target.clamp(min, max);
            }
        }

        let delta = state.current - prev;
        transform.rotate_local(Quat::from_axis_angle(spec.axis.to_vec3(), delta));
    }
}

/// Wrap an angle difference into [-PI, PI] for shortest-path rotation.
fn shortest_angle(diff: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (diff + PI).rem_euclid(TAU) - PI
}
