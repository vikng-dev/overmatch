use avian3d::prelude::*;
use bevy::prelude::*;
use serde::Deserialize;

use crate::Layer;
use crate::tank::{Roadwheel, Tank, TankSim, WheelIndex, rig_world_pose};
use crate::trace::num;

use super::contact::sphere_cast_ground_contact;
use super::susp_trace;

/// Per-variant suspension characteristics, authored in the `.tank.ron` spec sheet (ADR-0010) and
/// applied to the hull. Required, no default (ADR-0011): the tank has no suspension until applied.
#[derive(Component, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuspensionParams {
    /// Suspension probe reach from the hub (m). Invariant: it exceeds `wheel_radius` so a resting
    /// wheel can find terrain, and `wheel_radius < ray_length + SPHERE_PROBE_RETRACT` for sphere-cast droop.
    pub ray_length: f32,
    /// Spring free length from the hub (m). Invariant: it exceeds `wheel_radius` at the authored ride height.
    pub rest_length: f32,
    /// Spring stiffness per wheel (N/m). Relationship: `wheel_count * stiffness * static_compression`
    /// carries the tank's weight at rest.
    pub stiffness: f32,
    /// Suspension damping per wheel (N·s/m).
    pub damping: f32,
    /// Effective roadwheel radius (m). The sphere probe uses it to round terrain edges.
    ///
    /// Invariant: it remains defaulted until the spec supplies a per-variant radius; the probe
    /// reconstruction preserves flat-ground ride height for any radius.
    #[serde(default = "default_wheel_radius")]
    pub wheel_radius: f32,
}

/// Default roadwheel radius (m) when a spec omits one.
fn default_wheel_radius() -> f32 {
    0.5166
}

/// Sphere-cast retract margin (m).
///
/// Invariant: it cancels out of flat-ground compression and only extends resolvable penetration.
const SPHERE_PROBE_RETRACT: f32 = 0.3;

/// Bump-stop stiffness multiplier for the linear spring.
const BUMP_STOP_STIFFNESS_RATIO: f32 = 15.0;
/// Bump-stop multiple of `M*g/(n*k)` at which the progressive bump stop begins.
///
/// Invariant: the cap leaves a nonzero ramp before the travel clamp; specs must keep static sag
/// below the resulting engage point.
const BUMP_STOP_ENGAGE_LOAD_RATIO: f32 = 1.25;
/// Maximum engage fraction of travel; preserves a nonzero ramp before the clamp.
const BUMP_STOP_LATEST_ENGAGE: f32 = 0.95;
/// DERIVED bump-stop damping: scale the linear damper by `sqrt(stiffness multiplier)`.
///
/// Invariant: the applied stop force is capped in [`apply_suspension`] to remain non-restitutive.
fn bump_stop_damping(params: &SuspensionParams) -> f32 {
    params.damping * BUMP_STOP_STIFFNESS_RATIO.sqrt()
}

/// Ground-probe geometry, selected once from `SUSPENSION_PROBE`.
///
/// Invariant: all peers must use the same value because it determines simulation contact distance.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub enum SuspensionProbe {
    /// Per-wheel line ray.
    Ray,
    /// Wheel-radius sphere cast.
    Sphere,
}

impl SuspensionProbe {
    /// Parse `SUSPENSION_PROBE` once. `ray`/`sphere` select the model; anything else (including
    /// unset) defaults to `Sphere` — the continuous-contact fix — with a warning for a typo'd value.
    pub(super) fn from_env() -> Self {
        match std::env::var("SUSPENSION_PROBE").as_deref() {
            Ok("ray") => Self::Ray,
            Ok("sphere") => Self::Sphere,
            Err(_) => Self::Sphere,
            Ok(other) => {
                warn!("SUSPENSION_PROBE=`{other}` unrecognised (want ray|sphere) — using sphere");
                Self::Sphere
            }
        }
    }
}

/// Announce the active probe at startup, loudly — it is sim-affecting, so a mismatched client and
/// server (one `ray`, one `sphere`) would diverge silently otherwise. Runs on both ends.
pub(super) fn log_suspension_probe(probe: Res<SuspensionProbe>) {
    info!(
        "SUSPENSION_PROBE={:?} — SIM-AFFECTING: client and server MUST match this value",
        *probe
    );
}

#[derive(Component, Default, Clone, PartialEq, Debug)]
pub struct Suspension {
    /// Ground contact this tick (world) — where drive force is applied. `None` = airborne.
    pub contact: Option<Vec3>,
    /// Magnitude of the spring force currently applied (N) — the wheel's normal load.
    pub load: f32,
    /// Horizontal ground force applied this tick (thrust + friction), kept for the debug viz.
    pub drive_force: Vec3,
}

/// Apply per-wheel damped-spring forces from fresh terrain queries.
///
/// Invariant: query tick-truth `Position`/`Rotation` directly, never delayed query-result
/// components; the terrain layer must remain static for a mid-step query to be trustworthy.
pub(super) fn apply_suspension(
    spatial: SpatialQuery,
    probe: Res<SuspensionProbe>,
    gravity: Res<Gravity>,
    time: Res<Time>,
    mut bodies: Query<
        (
            Entity,
            &Position,
            &Rotation,
            Forces,
            &ComputedMass,
            &ComputedAngularInertia,
            &ComputedCenterOfMass,
            &SuspensionParams,
            &mut TankSim,
        ),
        With<Tank>,
    >,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    mut wheels: Query<(&WheelIndex, &mut Suspension), With<Roadwheel>>,
) {
    let filter = SpatialQueryFilter::from_mask(Layer::Terrain);
    // Suspension-force recorder (`susp_trace`): the per-run counter, this tick's row join key.
    let trace_tick = if susp_trace::enabled() {
        susp_trace::next_tick()
    } else {
        0
    };
    for (body, position, rotation, mut forces, mass, inertia, com, params, mut sim) in &mut bodies {
        // No valid support force exists without finite mass; clear contacts and brush anchors.
        if !(mass.value() > 0.0 && mass.value().is_finite()) {
            for wheel in children.iter_descendants(body) {
                if let Ok((wheel_slot, mut suspension)) = wheels.get_mut(wheel) {
                    *suspension = Suspension::default();
                    if let Some(anchor) = sim.anchors.get_mut(wheel_slot.0) {
                        *anchor = None;
                    }
                }
            }
            continue;
        }
        // Geometric travel limit: the hub stays at least one wheel radius above contact.
        let max_travel = params.rest_length - params.wheel_radius;
        // DERIVED from per-body mass, gravity, wheel count, and stiffness; preserve a ramp before
        // the travel limit.
        let wheel_count = sim.anchors.len().max(1) as f32;
        let static_compression =
            mass.value() * gravity.0.length() / (wheel_count * params.stiffness);
        let engage = (BUMP_STOP_ENGAGE_LOAD_RATIO * static_compression)
            .min(BUMP_STOP_LATEST_ENGAGE * max_travel);
        // The cap uses `closing / (dt * (n/M + (r×n)·I⁻¹·(r×n)))`.
        // Invariant: scaling linear compliance by wheel count bounds simultaneous wheel impulses;
        // the rotational term prevents a corner contact from exceeding its effective mass.
        let inv_inertia_world = inertia.rotated(rotation.0).inverse();
        let com_world = position.0 + rotation.0 * com.0;
        let dt = time.delta_secs();
        // Only this body's rig descendants may apply support or retain a brush anchor.
        for wheel in children.iter_descendants(body) {
            let Ok((wheel_slot, mut suspension)) = wheels.get_mut(wheel) else {
                continue;
            };
            let unsupported = |suspension: &mut Suspension, sim: &mut TankSim| {
                *suspension = Suspension::default();
                if let Some(anchor) = sim.anchors.get_mut(wheel_slot.0) {
                    *anchor = None;
                }
            };
            let Some((origin, wheel_rotation)) =
                rig_world_pose(wheel, body, position.0, rotation.0, &parents, &locals)
            else {
                unsupported(&mut suspension, &mut sim);
                continue;
            };
            // Reject corrupt pose data before it can poison force accumulation.
            if !origin.is_finite() {
                unsupported(&mut suspension, &mut sim);
                continue;
            }
            let Ok(down) = Dir3::new(wheel_rotation * Vec3::NEG_Y) else {
                unsupported(&mut suspension, &mut sim);
                continue;
            };
            let dir = Vec3::from(down);

            // `ground_distance` is hub-to-ground along the cast axis; `contact` receives drive force.
            // Invariant: sphere reconstruction is delegated to the tested helper so its flat-ground
            // compression matches the ray probe. See ADR-0015.
            let probed = match *probe {
                SuspensionProbe::Ray => spatial
                    .cast_ray(origin, down, params.ray_length, true, &filter)
                    .map(|hit| (hit.distance, origin + dir * hit.distance)),
                SuspensionProbe::Sphere => spatial
                    .cast_shape(
                        &Collider::sphere(params.wheel_radius),
                        origin - dir * SPHERE_PROBE_RETRACT,
                        Quat::IDENTITY,
                        down,
                        &ShapeCastConfig {
                            max_distance: params.ray_length + SPHERE_PROBE_RETRACT
                                - params.wheel_radius,
                            ..default()
                        },
                        &filter,
                    )
                    .map(|hit| {
                        sphere_cast_ground_contact(
                            origin,
                            dir,
                            params.wheel_radius,
                            SPHERE_PROBE_RETRACT,
                            hit.distance,
                            hit.point1,
                            hit.normal1,
                        )
                    }),
            };
            let Some((ground_distance, contact)) = probed else {
                unsupported(&mut suspension, &mut sim);
                continue;
            };

            let raw_compression = params.rest_length - ground_distance;
            if raw_compression <= 0.0 {
                unsupported(&mut suspension, &mut sim);
                continue;
            }

            // The linear spring cannot compress beyond the wheel's geometric travel limit.
            let compression = raw_compression.min(max_travel);

            let up = -dir;

            let spring_speed = forces.velocity_at_point(contact).dot(up);
            let spring_force = params.stiffness * compression;
            let damper_force = -params.damping * spring_speed;
            let mut load = spring_force + damper_force;
            let mut trace_stop = 0.0_f32;
            let mut trace_capped = false;

            // The progressive stop uses unclamped compression and may only absorb closing momentum.
            if raw_compression > engage {
                let over = raw_compression - engage;
                let ramp = (over / (max_travel - engage)).min(1.0);
                let stop = BUMP_STOP_STIFFNESS_RATIO * params.stiffness * over
                    - bump_stop_damping(params) * spring_speed * ramp;
                let closing = (-spring_speed).max(0.0);
                let lever = (contact - com_world).cross(up);
                let inv_effective_mass =
                    wheel_count * mass.inverse() + lever.dot(inv_inertia_world * lever);
                let cap = closing / (dt * inv_effective_mass);
                // A non-finite effective mass cannot safely enter `clamp`.
                let applied = if cap.is_finite() {
                    stop.clamp(0.0, cap)
                } else {
                    0.0
                };
                trace_stop = applied;
                trace_capped = stop > cap;
                load += applied;
            }

            let load = load.max(0.0);

            if trace_tick != 0 {
                let hull_v = forces.linear_velocity();
                let hull_w = forces.angular_velocity();
                susp_trace::write(&serde_json::json!({
                    "k": "s",
                    "n": trace_tick,
                    "w": wheel_slot.0,
                    "c": num(raw_compression),
                    "cc": num(compression),
                    "ss": num(spring_speed),
                    "fs": num(spring_force),
                    "fd": num(damper_force),
                    "fb": num(trace_stop),
                    "cap": trace_capped,
                    "clip": num((spring_force + damper_force + trace_stop).min(0.0)),
                    "ld": num(load),
                    "cy": num(contact.y),
                    "py": num(position.0.y),
                    "vy": num(hull_v.y),
                    "wx": num(hull_w.x),
                    "wz": num(hull_w.z),
                    "oy": num(origin.y),
                    "gd": num(ground_distance),
                    "we": wheel.to_bits(),
                }));
            }

            forces.apply_force_at_point(up * load, contact);
            suspension.contact = Some(contact);
            suspension.load = load;
        }
    }
}
