//! Driving: the raycast-wheel locomotion seed (ADR-0005). Each roadwheel's suspension ray does
//! double duty — its spring holds the hull up (support, implemented here) and, later, its normal
//! load feeds the drive friction. The hull rides on its wheels; the hull box is only a collision
//! shape and a bottoming-out safety floor.

use avian3d::prelude::*;
use bevy::ecs::lifecycle::Add;
use bevy::prelude::*;
use serde::Deserialize;

use crate::damage::{
    Capability, TankCapabilities, TankVolumes, VolumeFacets, capability_available,
};
use crate::state::GameplaySet;
use crate::tank::{CenterOfMassAnchor, Controlled, Roadwheel, Tank, TrackSide};

/// Coulomb coefficient: each wheel's total ground force is capped at MU × load (friction ellipse).
/// Per-environment (the track-vs-ground surface pair), not per-tank — destined for the terrain
/// mechanic, not the model (ADR-0007, bucket 3).
const MU: f32 = 0.9;
/// Lateral fraction of the friction ellipse: the sideways force budget is `LATERAL_GRIP_RATIO × MU ×
/// load`, modelling a track's turning-resistance coefficient μ_t against its longitudinal μ. Firm-
/// ground skid-steer theory (Wong/Merritt) puts μ_t ≈ 0.5 vs μ ≈ 0.9; this lower lateral grip is what
/// lets a heavy tank pivot at all — an isotropic circle nearly cancels the steer drive. Surface
/// property like [`MU`] (ADR-0007, bucket 3).
const LATERAL_GRIP_RATIO: f32 = 0.55;
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
/// `apply_drive`. Authored in the tank's `.tank.ron` spec sheet (ADR-0010); **required, with no
/// default** — a competitive sim must never run on guessed stats, so a failed spec load is fatal
/// (`report_failed_spec`) and a tank simply isn't driven until its `Drivetrain` has been applied.
#[derive(Component, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Per-variant suspension characteristics, authored in the `.tank.ron` spec sheet (ADR-0010) and
/// applied to the hull. Required, no default (ADR-0011): the tank has no suspension until applied.
#[derive(Component, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuspensionParams {
    /// How far a roadwheel's suspension ray reaches from the hub (m). Must exceed the effective
    /// radius (~0.5166) so it finds the ground at rest, with margin for droop. Used when the ray is
    /// built (`apply_tank_spec`), not per-step.
    pub ray_length: f32,
    /// Spring free length from the hub (m). Longer than the effective radius so at rest the spring
    /// is compressed enough to carry the tank's weight at the authored ride height.
    pub rest_length: f32,
    /// Spring stiffness per wheel (N/m): ~16 wheels × this × static compression ≈ the tank's weight.
    pub stiffness: f32,
    /// Suspension damping per wheel (N·s/m), ~0.6 of critical, so it settles without bouncing.
    pub damping: f32,
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
/// `on_tank_ready` adds `NoAutoCenterOfMass`, so this authored value is the body's COM outright —
/// the collision proxies' centroid does not dilute it (ADR-0011). Runs once: the
/// `Without<CenterOfMass>` filter retires it after the override is inserted.
fn set_center_of_mass(
    mut commands: Commands,
    tanks: Query<(Entity, &GlobalTransform), (With<Tank>, Without<CenterOfMass>)>,
    children: Query<&Children>,
    anchors: Query<&GlobalTransform, With<CenterOfMassAnchor>>,
) {
    // Per tank: find *its own* `Center_Of_Mass` anchor among its descendants (the rig hierarchy),
    // so each body's COM comes from its own model. Runs once per tank — the `Without<CenterOfMass>`
    // filter retires a tank after its override is inserted.
    for (entity, tank_transform) in &tanks {
        let Some(anchor) = children
            .iter_descendants(entity)
            .find_map(|d| anchors.get(d).ok())
        else {
            continue; // this tank's anchor empty not bound yet
        };

        // The anchor's position in the tank's local frame is exactly Avian's COM offset.
        let local = tank_transform
            .affine()
            .inverse()
            .transform_point3(anchor.translation());
        commands.entity(entity).insert(CenterOfMass(local));
    }
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
    // Runs for *every* tank — support is tank-agnostic (each body rides on its own wheels),
    // unlike drive input which is scoped to the controlled tank. The `&SuspensionParams` gates a
    // body in: no suspension until the spec is applied to the hull (ADR-0011 — no default spring).
    // Wheels likewise lack a `RayCaster` until then.
    mut bodies: Query<(Entity, Forces, &SuspensionParams), With<Tank>>,
    children: Query<&Children>,
    mut wheels: Query<(&RayCaster, &RayHits, &mut Suspension), With<Roadwheel>>,
) {
    for (body, mut forces, params) in &mut bodies {
        // Only this body's own roadwheels (its rig descendants) push on it — otherwise a second
        // tank's wheel hits would load this hull.
        for wheel in children.iter_descendants(body) {
            let Ok((ray, hits, mut suspension)) = wheels.get_mut(wheel) else {
                continue;
            };
            let Some(hit) = hits.iter_sorted().next() else {
                *suspension = Suspension::default();
                continue;
            };

            let compression = params.rest_length - hit.distance;
            if compression <= 0.0 {
                *suspension = Suspension::default();
                continue;
            }

            let dir = Vec3::from(ray.global_direction());
            let up = -dir;
            let contact = ray.global_origin() + dir * hit.distance;

            // Damped spring along the suspension axis. velocity_at_point gives the hull's speed at
            // the contact; its component along `up` is the compression rate (negative while
            // settling).
            let spring_speed = forces.velocity_at_point(contact).dot(up);
            let load = (params.stiffness * compression - params.damping * spring_speed).max(0.0);

            forces.apply_force_at_point(up * load, contact);
            suspension.contact = Some(contact);
            suspension.load = load;
        }
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
/// grip resisting side-slip — the whole vector capped on the friction ellipse (μ·load fore-aft, a
/// lower lateral budget sideways). Yaw, turning resistance, and weight transfer all emerge from
/// per-contact forces; nothing scripts the turn.
fn apply_drive(
    input: Res<DriveInput>,
    mut bodies: Query<
        (
            Entity,
            &GlobalTransform,
            Forces,
            &Drivetrain,
            Option<&TankVolumes>,
            Option<&TankCapabilities>,
            Has<Controlled>,
        ),
        With<Tank>,
    >,
    children: Query<&Children>,
    volumes: Query<VolumeFacets>,
    mut wheels: Query<(&Roadwheel, &mut Suspension)>,
) {
    // Per tank. `Drivetrain` is required per-variant data with no fallback (ADR-0010): we never
    // guess stats. It's absent only in the startup frames before the spec applies (a failed load is
    // fatal — see `report_failed_spec`), so a tank with no `Drivetrain` is simply not driven yet.
    for (body, tank_transform, mut forces, drivetrain, tank_volumes, tank_caps, controlled) in
        &mut bodies
    {
        // Drive input is the one player-specific part: only the controlled tank reads the live
        // throttle/steer. A drive-disabled tank parks (no hold either, like before); every other
        // tank gets zero command, so it holds in place via the brush anchor rather than driving off.
        if !capability_available(tank_volumes, tank_caps, Capability::Drive, &volumes) {
            for wheel in children.iter_descendants(body) {
                if let Ok((_, mut suspension)) = wheels.get_mut(wheel) {
                    suspension.drive_force = Vec3::ZERO;
                }
            }
            continue;
        }
        let (throttle, steer) = if controlled {
            (input.throttle, input.steer)
        } else {
            (0.0, 0.0)
        };

        // Ground-plane drive basis from the hull orientation: forward flattened onto the ground,
        // and right as forward rotated −90° about Y (avoids depending on a separate `right()`).
        let forward: Vec3 = tank_transform.forward().into();
        let forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        let right = Vec3::new(-forward.z, 0.0, forward.x);

        // Only this tank's own roadwheels (its rig descendants) — otherwise the other tank's wheels
        // would take this tank's drive.
        for wheel_entity in children.iter_descendants(body) {
            let Ok((wheel, mut suspension)) = wheels.get_mut(wheel_entity) else {
                continue;
            };
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
                TrackSide::Left => throttle + steer,
                TrackSide::Right => throttle - steer,
            }
            .clamp(-1.0, 1.0);
            let driving = command.abs() > COMMAND_DEADBAND;

            let velocity = forces.velocity_at_point(contact);
            let v_fwd = velocity.dot(forward);
            let v_lat = velocity.dot(right);

            // Static↔kinetic gate: below the stick speed the contact grips (plant an anchor and
            // hold); above it, it slips and friction is the kinetic skid / coast-down model.
            let gripping = v_fwd.hypot(v_lat) < STICK_SPEED;
            if !gripping {
                suspension.anchor = None;
            } else if suspension.anchor.is_none() {
                suspension.anchor = Some(contact);
            }

            // Friction ellipse: tracks grip hard fore-aft (full μ·load) but skid sideways at the
            // lower turning-resistance coefficient μ_t = ratio·μ (Wong/Merritt firm-ground
            // skid-steer). The lateral semi-axis is what lets a heavy tank pivot — an isotropic
            // circle nearly cancels the steer drive.
            let grip = MU * load;
            let grip_lat = grip * LATERAL_GRIP_RATIO;

            // Slip from the planted anchor, split into the ground-plane axes.
            let (mut d_fwd, mut d_lat) = match suspension.anchor {
                Some(anchor) => (
                    (contact - anchor).dot(forward),
                    (contact - anchor).dot(right),
                ),
                None => (0.0, 0.0),
            };

            // Bristle saturation (LuGre steady-state deflection) on the ellipse: a brush bristle
            // stretches only to its slip point — d_fwd to grip/k, d_lat to grip_lat/k. Past the
            // ellipse the bristle *trails* the contact at that fixed deflection (a smooth Coulomb
            // slide) instead of snapping back to zero, which removes the low-speed stick-slip cycle.
            if suspension.anchor.is_some() {
                let a_fwd = grip / drivetrain.brush_stiffness;
                let a_lat = grip_lat / drivetrain.brush_stiffness;
                let e = (d_fwd / a_fwd).powi(2) + (d_lat / a_lat).powi(2);
                if e > 1.0 {
                    let s = e.sqrt().recip();
                    d_fwd *= s;
                    d_lat *= s;
                    suspension.anchor = Some(contact - forward * d_fwd - right * d_lat);
                }
            }

            // Longitudinal: thrust when commanded (bleeding the anchor's forward slip so the static
            // spring doesn't fight the drive — the wheel "rolls"); else hold (static spring) or,
            // while still rolling, the engine-brake / coast-down.
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

            // Cap the tangential force on the friction ellipse (μ·load fore-aft, grip_lat sideways)
            // by scaling the vector onto its boundary. The bounded bristle rarely overshoots, so
            // this only trims the thrust+grip vector sum — and never resets the anchor (that snap is
            // the stick-slip source).
            let e = (f_fwd / grip).powi(2) + (f_lat / grip_lat).powi(2);
            if e > 1.0 {
                force *= e.sqrt().recip();
            }

            forces.apply_force_at_point(force, contact);
            suspension.drive_force = force;
        }
    }
}
