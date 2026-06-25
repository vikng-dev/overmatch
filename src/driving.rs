//! Driving: the raycast-wheel locomotion seed (ADR-0005). Each roadwheel's suspension ray does
//! double duty — its spring holds the hull up (support, implemented here) and, later, its normal
//! load feeds the drive friction. The hull rides on its wheels; the hull box is only a collision
//! shape and a bottoming-out safety floor.

use avian3d::prelude::*;
use bevy::ecs::lifecycle::Add;
use bevy::prelude::*;
use serde::Deserialize;

use crate::state::GameplaySet;
use crate::tank::{CenterOfMassAnchor, Roadwheel, Tank, TrackSide};

/// Suspension free length from the hub (m). Longer than the effective radius (~0.5166) so at rest
/// the spring is compressed enough to carry the tank's weight at the authored ride height.
const REST_LENGTH: f32 = 0.6;
/// Spring stiffness per wheel (N/m): ~16 wheels × this × static compression ≈ the tank's weight.
const STIFFNESS: f32 = 450_000.0;
/// Suspension damping per wheel (N·s/m), ~0.6 of critical, so it settles without bouncing.
const DAMPING: f32 = 50_000.0;

/// Coulomb coefficient: each wheel's total ground force is capped at MU × load (friction circle).
/// Per-environment (the track-vs-ground surface pair), not per-tank — destined for the terrain
/// mechanic, not the model (ADR-0007, bucket 3).
const MU: f32 = 0.9;
/// Input ramp (per second): turns binary keys into a smooth throttle/steer signal — and gives
/// the keyboard a taste of the analog mid-range on the way to full. Universal feel (bucket 1).
const INPUT_RAMP: f32 = 4.0;
/// Below this contact planar speed (m/s) a wheel "grips": it plants a brush anchor and holds
/// statically instead of slipping. Above it, friction is kinetic (the skid / coast-down model).
/// This static↔kinetic gate is a Karnopp-style zero-velocity band — what lets a stopped tank
/// hold on a slope instead of creeping away. Universal feel (bucket 1).
const STICK_SPEED: f32 = 0.3;
/// A per-track command below this magnitude counts as "no drive" — the wheel holds rather than
/// driving, so a feather-touch doesn't switch off the hill-hold. Universal feel (bucket 1).
const COMMAND_DEADBAND: f32 = 0.02;

/// Per-variant drivetrain characteristics — this tank's locomotion spec sheet, read by
/// `apply_drive`. Authored in the tank's `.tank.ron` spec sheet (ADR-0010) and applied to the
/// hull; the code `Default` is the fallback used until the asset loads (or if a variant omits it).
#[derive(Component, Clone, Deserialize)]
pub struct Drivetrain {
    /// Max thrust per roadwheel at full throttle (N); ×16 wheels = total tractive force.
    pub max_thrust: f32,
    /// Longitudinal viscous term (N per m/s of forward speed): bounds top speed under thrust, and
    /// — throttle released, still rolling — IS the engine-brake / coast-down (heavy-glide dial).
    pub rolling_resistance: f32,
    /// Lateral grip (N per m/s of side-slip), kinetic regime — resists side-slip and yaw.
    pub lateral_grip: f32,
    /// Brush-anchor stiffness (N per m of slip): the static grip spring that holds the tank at rest.
    pub brush_stiffness: f32,
    /// Brush-anchor damping (N·s/m): settles the hold spring without buzzing at rest.
    pub brush_damping: f32,
}

impl Default for Drivetrain {
    fn default() -> Self {
        Self {
            max_thrust: 12_500.0,
            rolling_resistance: 1_150.0,
            lateral_grip: 60_000.0,
            brush_stiffness: 250_000.0,
            brush_damping: 25_000.0,
        }
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<DriveInput>()
        .add_observer(attach_suspension)
        .add_systems(Update, set_center_of_mass)
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
    let Ok((entity, tank_transform)) = tank.single() else {
        return;
    };
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
    /// Brush-anchor: the world point the contact "gripped" while near rest. `Some` = gripping
    /// (static friction holds the tank here); `None` = slipping (kinetic) or airborne.
    pub anchor: Option<Vec3>,
}

/// Attach `Suspension` the moment the rig binds a `Roadwheel` (observer, ungated).
fn attach_suspension(add: On<Add, Roadwheel>, mut commands: Commands) {
    commands.entity(add.entity).insert(Suspension::default());
}

/// Damped-spring suspension: each grounded wheel pushes the hull up at its contact point, so
/// ride height, pitch, roll, and weight transfer all emerge from the per-wheel springs.
fn apply_suspension(
    mut body: Query<Forces, With<Tank>>,
    mut wheels: Query<(&RayCaster, &RayHits, &mut Suspension), With<Roadwheel>>,
) {
    let Ok(mut forces) = body.single_mut() else {
        return;
    };

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

fn read_drive_input(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut input: ResMut<DriveInput>,
) {
    let axis = |pos: KeyCode, neg: KeyCode| {
        keys.pressed(pos) as i8 as f32 - keys.pressed(neg) as i8 as f32
    };
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
    drivetrain: Option<Single<&Drivetrain>>,
    mut body: Query<(&GlobalTransform, Forces), With<Tank>>,
    mut wheels: Query<(&Roadwheel, &mut Suspension)>,
) {
    let Ok((tank_transform, mut forces)) = body.single_mut() else {
        return;
    };

    // The model authors `Drivetrain` on its part (the hull); fall back to the code default until it
    // does (or if a variant omits it). Read by type — where it sits in the rig is irrelevant here.
    let default = Drivetrain::default();
    let drivetrain = match drivetrain {
        Some(d) => d.into_inner(),
        None => &default,
    };

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
            suspension.anchor = None;
            continue;
        }

        // Additive differential: D adds to the left track and subtracts from the right, so steer
        // yaws the nose the same way regardless of throttle, and a pure steer pivots in place.
        let command = match wheel.side {
            TrackSide::Left => input.throttle + input.steer,
            TrackSide::Right => input.throttle - input.steer,
        }
        .clamp(-1.0, 1.0);
        let driving = command.abs() > COMMAND_DEADBAND;

        let velocity = forces.velocity_at_point(contact);
        let v_fwd = velocity.dot(forward);
        let v_lat = velocity.dot(right);

        // Static↔kinetic gate: below the stick speed the contact grips (plant an anchor and hold);
        // above it, it slips and friction is the kinetic skid / coast-down model.
        let gripping = v_fwd.hypot(v_lat) < STICK_SPEED;
        if !gripping {
            suspension.anchor = None;
        } else if suspension.anchor.is_none() {
            suspension.anchor = Some(contact);
        }

        // Slip from the planted anchor, split into the ground-plane axes.
        let (d_fwd, d_lat) = match suspension.anchor {
            Some(anchor) => (
                (contact - anchor).dot(forward),
                (contact - anchor).dot(right),
            ),
            None => (0.0, 0.0),
        };

        // Longitudinal: thrust when commanded (bleeding the anchor's forward slip so the static
        // spring doesn't fight the drive — the wheel "rolls"); else hold (static spring) or, while
        // still rolling, the engine-brake / coast-down.
        let f_fwd = if driving {
            if let Some(anchor) = suspension.anchor {
                suspension.anchor = Some(anchor + forward * d_fwd);
            }
            command * drivetrain.max_thrust - drivetrain.rolling_resistance * v_fwd
        } else if gripping {
            -drivetrain.brush_stiffness * d_fwd - drivetrain.brush_damping * v_fwd
        } else {
            -drivetrain.rolling_resistance * v_fwd
        };

        // Lateral: static spring holds the tracks fixed at rest (kills sideways creep); kinetic
        // stiff grip resists side-slip and yaw while moving (skid steer).
        let f_lat = if gripping {
            -drivetrain.brush_stiffness * d_lat - drivetrain.brush_damping * v_lat
        } else {
            -drivetrain.lateral_grip * v_lat
        };

        let mut force = forward * f_fwd + right * f_lat;

        // Friction circle: ground can't supply more than μ × load of tangential force. Past it the
        // grip breaks loose — re-plant the anchor at the contact so it re-grips from here.
        let grip = MU * load;
        if force.length() > grip {
            force = force.normalize_or_zero() * grip;
            if suspension.anchor.is_some() {
                suspension.anchor = Some(contact);
            }
        }

        forces.apply_force_at_point(force, contact);
        suspension.drive_force = force;
    }
}
