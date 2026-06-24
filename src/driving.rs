//! Driving: the raycast-wheel locomotion seed (ADR-0005). Each roadwheel's suspension ray does
//! double duty — its spring holds the hull up (support, implemented here) and, later, its normal
//! load feeds the drive friction. The hull rides on its wheels; the hull box is only a collision
//! shape and a bottoming-out safety floor.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::state::GameplaySet;
use crate::tank::{CenterOfMassAnchor, Roadwheel, Tank, TrackSide};

/// Suspension free length from the hub (m). Longer than the effective radius (~0.5166) so at rest
/// the spring is compressed enough to carry the tank's weight at the authored ride height.
const REST_LENGTH: f32 = 0.6;
/// Spring stiffness per wheel (N/m): ~16 wheels × this × static compression ≈ the tank's weight.
const STIFFNESS: f32 = 450_000.0;
/// Suspension damping per wheel (N·s/m), ~0.6 of critical, so it settles without bouncing.
const DAMPING: f32 = 50_000.0;

/// Max thrust per roadwheel at full throttle (N); ×16 wheels = total tractive force.
const MAX_THRUST: f32 = 12_500.0;
/// Rolling resistance per wheel (N per m/s of forward speed) — bounds top speed.
const ROLLING_RESISTANCE: f32 = 1_150.0;
/// Lateral grip per wheel (N per m/s of side-slip) — resists side-slip and yaw (skid-steer).
const LATERAL_GRIP: f32 = 60_000.0;
/// Coulomb coefficient: each wheel's total ground force is capped at MU × load (friction circle).
const MU: f32 = 0.9;
/// Input ramp (per second): turns binary keys into a smooth throttle/steer signal — and gives
/// the keyboard a taste of the analog mid-range on the way to full.
const INPUT_RAMP: f32 = 4.0;

pub fn plugin(app: &mut App) {
    app.init_resource::<DriveInput>()
        .add_systems(Update, (attach_suspension, set_center_of_mass))
        // Order matters within the fixed step: read input, settle springs (sets per-wheel load),
        // then drive (reads that load for the friction circle). All gated by the gameplay set.
        .add_systems(
            FixedUpdate,
            (read_drive_input, apply_suspension, apply_drive)
                .chain()
                .in_set(GameplaySet),
        );
}

/// Set the body's centre of mass from the authored `Center_Of_Mass` empty (the model owns it).
/// Runs once: the `Without<CenterOfMass>` filter retires it after the override is inserted.
fn set_center_of_mass(
    mut commands: Commands,
    tank: Query<(Entity, &GlobalTransform), (With<Tank>, Without<CenterOfMass>)>,
    anchor: Query<&GlobalTransform, With<CenterOfMassAnchor>>,
) {
    let Ok((entity, tank_transform)) = tank.single() else { return };
    let Ok(anchor) = anchor.single() else { return }; // empty not bound yet

    // The anchor's position in the tank's local frame is exactly Avian's COM offset.
    let local = tank_transform
        .affine()
        .inverse()
        .transform_point3(anchor.translation());
    commands.entity(entity).insert(CenterOfMass(local));
}

/// Per-roadwheel suspension state. Written by `apply_suspension`; the contact point + load are
/// what the drive friction will also read (one ray, both jobs). `contact: None` = wheel airborne.
#[derive(Component, Default)]
pub struct Suspension {
    /// Ground contact this tick (world) — where drive force is applied. `None` = airborne.
    pub contact: Option<Vec3>,
    /// Magnitude of the spring force currently applied (N) — the wheel's normal load.
    pub load: f32,
    /// Horizontal ground force applied this tick (thrust + friction), kept for the debug viz.
    pub drive_force: Vec3,
}

/// Attach `Suspension` reactively once the rig binds a `Roadwheel` (init, ungated).
fn attach_suspension(wheels: Query<Entity, Added<Roadwheel>>, mut commands: Commands) {
    for entity in &wheels {
        commands.entity(entity).insert(Suspension::default());
    }
}

/// Damped-spring suspension: each grounded wheel pushes the hull up at its contact point, so
/// ride height, pitch, roll, and weight transfer all emerge from the per-wheel springs.
fn apply_suspension(
    mut body: Query<Forces, With<Tank>>,
    mut wheels: Query<(&RayCaster, &RayHits, &mut Suspension), With<Roadwheel>>,
) {
    let Ok(mut forces) = body.single_mut() else { return };

    for (ray, hits, mut suspension) in &mut wheels {
        let Some(hit) = hits.iter_sorted().next() else {
            *suspension = Suspension::default();
            continue;
        };

        let compression = REST_LENGTH - hit.distance;
        if compression <= 0.0 {
            *suspension = Suspension::default();
            continue;
        }

        let dir = Vec3::from(ray.global_direction());
        let up = -dir;
        let contact = ray.global_origin() + dir * hit.distance;

        // Damped spring along the suspension axis. velocity_at_point gives the hull's speed at the
        // contact; its component along `up` is the compression rate (negative while settling).
        let spring_speed = forces.velocity_at_point(contact).dot(up);
        let load = (STIFFNESS * compression - DAMPING * spring_speed).max(0.0);

        forces.apply_force_at_point(up * load, contact);
        suspension.contact = Some(contact);
        suspension.load = load;
    }
}

/// Smoothed driver intent in [-1, 1]: throttle (W/S) and steer (D/A). Ramped from the raw keys
/// so it's controller-ready and the keyboard eases through the analog range.
#[derive(Resource, Default)]
struct DriveInput {
    throttle: f32,
    steer: f32,
}

fn read_drive_input(keys: Res<ButtonInput<KeyCode>>, time: Res<Time>, mut input: ResMut<DriveInput>) {
    let axis = |pos: KeyCode, neg: KeyCode| keys.pressed(pos) as i8 as f32 - keys.pressed(neg) as i8 as f32;
    let target_throttle = axis(KeyCode::KeyW, KeyCode::KeyS);
    let target_steer = axis(KeyCode::KeyD, KeyCode::KeyA);
    let step = INPUT_RAMP * time.delta_secs();
    input.throttle = approach(input.throttle, target_throttle, step);
    input.steer = approach(input.steer, target_steer, step);
}

/// Move `current` toward `target` by at most `step`.
fn approach(current: f32, target: f32, step: f32) -> f32 {
    if current < target {
        (current + step).min(target)
    } else {
        (current - step).max(target)
    }
}

/// Differential-thrust drive with skid-steer friction. Each grounded wheel applies, at its
/// contact: longitudinal thrust (its track's command) minus rolling resistance, plus lateral
/// grip resisting side-slip — the whole vector capped at the friction circle (μ × load). Yaw,
/// turning resistance, and weight transfer all emerge from per-contact forces; nothing scripts
/// the turn.
fn apply_drive(
    input: Res<DriveInput>,
    mut body: Query<(&GlobalTransform, Forces), With<Tank>>,
    mut wheels: Query<(&Roadwheel, &mut Suspension)>,
) {
    let Ok((tank_transform, mut forces)) = body.single_mut() else { return };

    // Ground-plane drive basis from the hull orientation: forward flattened onto the ground,
    // and right as forward rotated −90° about Y (avoids depending on a separate `right()`).
    let forward: Vec3 = tank_transform.forward().into();
    let forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let right = Vec3::new(-forward.z, 0.0, forward.x);

    for (wheel, mut suspension) in &mut wheels {
        let (Some(contact), load) = (suspension.contact, suspension.load) else {
            continue;
        };
        if load <= 0.0 {
            suspension.drive_force = Vec3::ZERO;
            continue;
        }

        // Additive differential: D adds to the left track and subtracts from the right, so steer
        // yaws the nose the same way regardless of throttle, and a pure steer pivots in place.
        let command = match wheel.side {
            TrackSide::Left => input.throttle + input.steer,
            TrackSide::Right => input.throttle - input.steer,
        }
        .clamp(-1.0, 1.0);

        let velocity = forces.velocity_at_point(contact);
        let f_long = command * MAX_THRUST - ROLLING_RESISTANCE * velocity.dot(forward);
        let f_lat = -LATERAL_GRIP * velocity.dot(right);
        let mut force = forward * f_long + right * f_lat;

        // Friction circle: ground can't supply more than μ × load of tangential force.
        let grip = MU * load;
        if force.length() > grip {
            force = force.normalize() * grip;
        }

        forces.apply_force_at_point(force, contact);
        suspension.drive_force = force;
    }
}
