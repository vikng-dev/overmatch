use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::model::{TankRoot, TankServos};
use super::view::{ViewOf, ViewServo};
use crate::damage::{Requirement, TankVolumes, VolumeFacets, evaluate, part_qualities};

#[derive(Clone, Copy, Deserialize)]
pub enum Travel {
    Limited { min: f32, max: f32 },
    Continuous,
}

/// Aiming degree of freedom and its local rotation axis.
#[derive(Component, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ServoRole {
    Yaw,
    Pitch,
}

impl ServoRole {
    fn axis(self) -> Vec3 {
        match self {
            ServoRole::Yaw => Vec3::Y,
            ServoRole::Pitch => Vec3::X,
        }
    }
}

/// Authored one-axis motor limits. Specs use degrees; runtime state uses radians.
#[derive(Component, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServoSpec {
    /// Role-based binding and rotation-axis source.
    pub(super) role: ServoRole,
    /// Max slew speed, degrees/second.
    max_speed: f32,
    /// Slew acceleration, degrees/second².
    accel: f32,
    travel: Travel,
    /// Damage/crew requirement whose effectiveness scales slew speed.
    #[serde(default)]
    pub(crate) requires: Requirement,
}

impl ServoSpec {
    /// Authored travel window converted to runtime radians; `None` means continuous rotation.
    pub fn travel_limits(&self) -> Option<(f32, f32)> {
        match self.travel {
            Travel::Limited { min, max } => Some((min.to_radians(), max.to_radians())),
            Travel::Continuous => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_continuous(role: ServoRole, max_speed: f32, accel: f32) -> Self {
        Self {
            role,
            max_speed,
            accel,
            travel: Travel::Continuous,
            requires: Vec::new(),
        }
    }
}

/// Parent-local target angle written by aiming and consumed by the fixed-step mechanism.
#[derive(Component, Default)]
pub struct ServoCommand {
    pub target: f32,
}

/// Root-resident servo mechanism state. `drive_servos` writes fixed-step truth to the sim node;
/// `interpolate_servos` blends `previous` to `current` only on its view node.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct ServoState {
    current: f32,
    /// The angle at the previous fixed tick — the render interpolation's blend-from.
    previous: f32,
    velocity: f32,
}

/// Authored rotation at angle zero. Absolute pose writes avoid accumulating rotation deltas.
#[derive(Component, Clone, Copy)]
pub struct ServoRest(pub Quat);

/// This servo's slot in its tank's [`TankServos::states`], assigned in sorted-name order.
#[derive(Component, Clone, Copy)]
pub struct ServoIndex(pub usize);

/// Client-local mechanism state for a non-predicted remote tank. Public [`ServoAngles`](crate::net::
/// protocol::ServoAngles) remains the remote target stream; keeping its integrator separate means a
/// late `Predicted` marker can wait for an untouched authoritative [`TankServos`] snapshot.
#[derive(Component, Clone, PartialEq, Debug, Default)]
pub(crate) struct RemoteServos(pub Vec<ServoState>);

impl RemoteServos {
    pub(super) fn for_count(count: usize) -> Self {
        Self(vec![ServoState::default(); count])
    }
}

impl ServoState {
    /// The servo's current angle (radians, parent-local) — its live mechanism position. Read by the
    /// gunner sight to clamp how far the aim intent may lead the gun (the on-screen margin).
    pub fn current(&self) -> f32 {
        self.current
    }

    /// Canonical fixed-field order consumed by the passive divergence hash.
    pub(crate) fn hash_fields(&self) -> [f32; 3] {
        let Self {
            current,
            previous,
            velocity,
        } = *self;
        [current, previous, velocity]
    }

    /// Rollback-trigger comparison with physical tolerance. The bit-exact servo gate stormed on
    /// ULP-scale aim jitter (~6e-8 rad observed) that the coarse hull bars already forgive; the aim
    /// angle and its rate now compare within small physical bands (far below aim/hit resolution),
    /// and the render-interpolation `previous` (a view-only blend-from, never sim-affecting) is
    /// excluded entirely. Matching raw bits stay equal so NaN payloads compare equal. The
    /// determinism state hash still consumes the raw `hash_fields`; tolerance lives only here.
    pub(crate) fn rollback_eq(&self, other: &Self, current_band: f32, velocity_band: f32) -> bool {
        fn within(a: f32, b: f32, eps: f32) -> bool {
            a.to_bits() == b.to_bits() || (a - b).abs() <= eps
        }
        within(self.current, other.current, current_band)
            && within(self.velocity, other.velocity, velocity_band)
    }

    /// Construct non-default state for divergence-hash tests without exposing production writers.
    #[cfg(test)]
    pub(crate) fn test_new(current: f32, previous: f32, velocity: f32) -> Self {
        Self {
            current,
            previous,
            velocity,
        }
    }
}

fn servo_rotation(spec: &ServoSpec, rest: &ServoRest, angle: f32) -> Quat {
    rest.0 * Quat::from_axis_angle(spec.role.axis(), angle)
}

/// Restore sim-node transforms from rollback state before any fixed-step gameplay reader runs.
pub(super) fn restore_servo_truth(
    mut q: Query<(
        &mut Transform,
        &ServoSpec,
        &ServoRest,
        &ServoIndex,
        &TankRoot,
    )>,
    servos: Query<&TankServos>,
    remote_servos: Query<&RemoteServos>,
) {
    for (mut transform, spec, rest, slot, root) in &mut q {
        // A late-role replica can temporarily carry both. Its public remote mechanism remains live
        // until `net::rig` promotes the body and removes `RemoteServos`, leaving the untouched
        // authority snapshot as the predicted integrator seed.
        let state = remote_servos
            .get(root.0)
            .ok()
            .and_then(|servos| servos.0.get(slot.0))
            .or_else(|| {
                servos
                    .get(root.0)
                    .ok()
                    .and_then(|servos| servos.states.get(slot.0))
            });
        let Some(state) = state else {
            continue;
        };
        transform.rotation = servo_rotation(spec, rest, state.current);
    }
}

pub(super) fn drive_servos(
    mut q: Query<(
        &mut Transform,
        &ServoSpec,
        &ServoRest,
        &ServoCommand,
        &ServoIndex,
        &TankRoot,
    )>,
    mut servos: Query<&mut TankServos>,
    mut remote_servos: Query<&mut RemoteServos>,
    tanks: Query<&TankVolumes>,
    facets: Query<VolumeFacets>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (mut transform, spec, rest, command, slot, root) in &mut q {
        let mut remote = remote_servos.get_mut(root.0).ok();
        let mut authoritative = if remote.is_none() {
            servos.get_mut(root.0).ok()
        } else {
            None
        };
        let state = remote
            .as_mut()
            .and_then(|servos| servos.0.get_mut(slot.0))
            .or_else(|| {
                authoritative
                    .as_mut()
                    .and_then(|servos| servos.states.get_mut(slot.0))
            });
        let Some(state) = state else { continue };
        // Preserve the prior fixed-step value for view interpolation.
        state.previous = state.current;

        // Requirement effectiveness scales slew; zero freezes the mount.
        let slew = tanks
            .get(root.0)
            .map(|tv| evaluate(&spec.requires, &part_qualities(tv, &facets)))
            .unwrap_or(0.0);

        // Specs author degrees; runtime state and commands use radians.
        let max_speed = spec.max_speed.to_radians() * slew;
        let accel = spec.accel.to_radians();
        let travel = match spec.travel {
            Travel::Limited { min, max } => Travel::Limited {
                min: min.to_radians(),
                max: max.to_radians(),
            },
            Travel::Continuous => Travel::Continuous,
        };

        let error = match travel {
            Travel::Limited { .. } => command.target - state.current,
            Travel::Continuous => shortest_angle(command.target - state.current),
        };

        // Snap an overshooting step to prevent a discrete limit cycle around the target.
        let step = state.velocity * dt;
        if step.abs() >= error.abs() && error.abs() > 0.0 {
            state.current += error;
            state.velocity = 0.0;
        } else {
            // Braking envelope: v = sqrt(2a * distance), capped by authored speed.
            let target_speed = (2.0 * accel * error.abs()).sqrt().min(max_speed);
            let desired_velocity = error.signum() * target_speed;
            let dv = accel * dt;
            state.velocity += (desired_velocity - state.velocity).clamp(-dv, dv);

            state.current += state.velocity * dt;
            if let Travel::Limited { min, max } = travel {
                state.current = state.current.clamp(min, max);
            }
        }

        // Scale the deadband to one step's resolution instead of a fixed unreachable epsilon.
        let settle = accel * dt * dt;
        if error.abs() < settle && state.velocity.abs() < accel * dt {
            state.velocity = 0.0;
            if let Travel::Limited { min, max } = travel {
                state.current = command.target.clamp(min, max);
            }
        }

        // The sim node always carries fixed-step truth; smoothing writes the view tree only.
        transform.rotation = servo_rotation(spec, rest, state.current);
    }
}

/// The render half of the fixed-clock servo split: blend last tick's angle to this tick's by the
/// fixed clock's overstep and write the **view** node's `Transform` — smooth mechanism motion at
/// any frame rate, exactly how Avian renders the hull between physics ticks. Along the shortest
/// arc, so a continuous mount's ±π wrap doesn't spin the long way round.
///
/// Writes VIEW nodes only (design §6C): the sim servo node's `Transform` is pure tick truth,
/// written by `drive_servos`/`restore_servo_truth` alone, so no sim reader can ever see a
/// render-blended pose. The view node resolves its sim source through [`ViewOf`]; a launched
/// turret's view node loses `ViewServo` at detach and drops out of this write set.
pub(super) fn interpolate_servos(
    time: Res<Time<Fixed>>,
    mut views: Query<(&mut Transform, &ViewOf), With<ViewServo>>,
    servos: Query<(&ServoSpec, &ServoRest, &ServoIndex, &TankRoot)>,
    states: Query<&TankServos>,
    remote_states: Query<&RemoteServos>,
) {
    let alpha = time.overstep_fraction();
    for (mut transform, view_of) in &mut views {
        let Ok((spec, rest, slot, root)) = servos.get(view_of.0) else {
            continue;
        };
        let state = remote_states
            .get(root.0)
            .ok()
            .and_then(|servos| servos.0.get(slot.0))
            .or_else(|| {
                states
                    .get(root.0)
                    .ok()
                    .and_then(|servos| servos.states.get(slot.0))
            });
        let Some(state) = state else {
            continue;
        };
        let angle = state.previous + shortest_angle(state.current - state.previous) * alpha;
        // Guarded write: a settled mount must not re-dirty the view transform every frame.
        let rotation = servo_rotation(spec, rest, angle);
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
}

/// Wrap an angle difference into [-PI, PI] for shortest-path rotation.
pub fn shortest_angle(diff: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (diff + PI).rem_euclid(TAU) - PI
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    #[test]
    fn remote_servo_state_remains_the_view_interpolation_source_during_late_promotion() {
        let mut world = World::new();
        let authority = TankServos {
            states: vec![ServoState::test_new(-1.0, -1.0, -1.0)],
        };
        let root = world
            .spawn((
                authority.clone(),
                RemoteServos(vec![ServoState::test_new(1.0, 0.0, 0.5)]),
            ))
            .id();
        let servo = world
            .spawn((
                ServoSpec::test_continuous(ServoRole::Yaw, 90.0, 180.0),
                ServoRest(Quat::IDENTITY),
                ServoIndex(0),
                TankRoot(root),
            ))
            .id();
        let view = world
            .spawn((Transform::default(), ViewOf(servo), ViewServo))
            .id();

        let mut fixed = Time::<Fixed>::from_seconds(1.0);
        fixed.accumulate_overstep(Duration::from_millis(500));
        world.insert_resource(fixed);
        world.run_system_once(interpolate_servos).unwrap();

        let rendered = world.get::<Transform>(view).unwrap().rotation;
        let expected = Quat::from_axis_angle(Vec3::Y, shortest_angle(1.0) * 0.5);
        assert_eq!(
            rendered.to_array().map(f32::to_bits),
            expected.to_array().map(f32::to_bits),
            "the established remote state must still provide the interpolated pose",
        );
        assert_eq!(
            world.get::<TankServos>(root).unwrap(),
            &authority,
            "remote presentation must not alter the arriving authority snapshot",
        );
    }
}
