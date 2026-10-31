//! Servo mechanism. Window contract (design `armor-penetration-and-damage.md` §13.5): a `Limited`
//! travel is finite with `min <= 0 <= max`, certify refuses any other, and `drive_servos` keeps
//! `current` inside it on every tick; the envelope sweeps exactly that window.

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
    pub(crate) fn axis(self) -> Vec3 {
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
    /// The degree of freedom this servo drives.
    pub fn role(&self) -> ServoRole {
        self.role
    }

    /// Authored travel window converted to runtime radians; `None` means continuous rotation.
    pub fn travel_limits(&self) -> Option<(f32, f32)> {
        match self.travel {
            Travel::Limited { min, max } => Some((min.to_radians(), max.to_radians())),
            Travel::Continuous => None,
        }
    }

    /// Why this servo's authored travel breaks the window contract, or `None` when it keeps it.
    /// The contract: a `Limited` window is finite with `min <= 0 <= max` (a servo spawns at angle 0,
    /// so the window holds its own start), and `drive_servos` never lets `current` leave it.
    pub(crate) fn travel_window_fault(&self) -> Option<String> {
        let Travel::Limited { min, max } = self.travel else {
            return None;
        };
        // Written as a negated conjunction so NaN fails it too.
        (!(min.is_finite() && max.is_finite() && min <= 0.0 && max >= 0.0)).then(|| {
            format!(
                "travel Limited(min: {min}, max: {max}) must be finite with min <= 0 <= max \
                 (degrees): a servo spawns at angle 0, which the window must contain"
            )
        })
    }

    #[cfg(test)]
    pub(crate) fn test_limited(
        role: ServoRole,
        max_speed: f32,
        accel: f32,
        min: f32,
        max: f32,
    ) -> Self {
        Self {
            role,
            max_speed,
            accel,
            travel: Travel::Limited { min, max },
            requires: Vec::new(),
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

/// Client-local mechanism state for a replicated remote tank — permanent on replicas. Public
/// [`ServoAngles`](crate::net::protocol::ServoAngles) is the remote target stream; [`TankServos`]
/// is the server/single-player integrator and never reaches a client.
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

/// A servo node's local rotation at `angle` (radians): its authored rest rotation, then the turn
/// about the role's axis in the node's own frame, `rest * axis(angle)`. The one composition the
/// sim, the pen cam record's playback and every view of a record pose a servo with.
pub fn servo_pose(role: ServoRole, rest: Quat, angle: f32) -> Quat {
    rest * Quat::from_axis_angle(role.axis(), angle)
}

fn servo_rotation(spec: &ServoSpec, rest: &ServoRest, angle: f32) -> Quat {
    servo_pose(spec.role, rest.0, angle)
}

/// Restore sim-node transforms from rollback state before any fixed-step gameplay reader runs.
pub(crate) fn restore_servo_truth(
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
        // Replicas carry `RemoteServos` permanently and it takes precedence; the `TankServos` arm
        // serves the server and single-player, where no `RemoteServos` exists.
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
        let rotation = servo_rotation(spec, rest, state.current);
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
}

pub(crate) fn drive_servos(
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
            if let Travel::Limited { min, max } = travel {
                state.current = state.current.clamp(min, max);
            }
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
        let rotation = servo_rotation(spec, rest, state.current);
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
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

    /// Invariant: a `Limited` servo's `current` stays inside `travel_limits()` on every tick, for any
    /// start inside the window and any target, including one beyond either limit and one sweeping
    /// at full slew. The envelope sweeps only the window, so a mount outside it is a part the
    /// damage model's reachability test would call unreachable.
    #[test]
    fn a_limited_servo_never_leaves_its_window() {
        let mut world = World::new();
        let spec = ServoSpec::test_limited(ServoRole::Yaw, 90.0, 360.0, -20.0, 35.0);
        let (lo, hi) = spec.travel_limits().unwrap();
        let dt = 1.0 / 64.0;
        world.insert_resource(Time::<()>::default());
        let root = world
            .spawn(TankServos {
                states: vec![ServoState::default()],
            })
            .id();
        // `drive_servos` reads slew from the tank's volumes; an empty requirement evaluates to 1, but
        // a tank with no `TankVolumes` at all reads 0 and freezes the mount.
        world.spawn(crate::damage::VolumeOf(root));
        let servo = world
            .spawn((
                Transform::default(),
                spec,
                ServoRest(Quat::IDENTITY),
                ServoCommand::default(),
                ServoIndex(0),
                TankRoot(root),
            ))
            .id();
        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        for start in [lo, 0.0, hi, 0.5 * hi] {
            world.get_mut::<TankServos>(root).unwrap().states[0] =
                ServoState::test_new(start, start, 0.0);
            for tick in 0..3000 {
                // First half: random targets beyond both limits, re-rolled every other tick.
                // Then a target sweeping at the servo's full slew speed past the window. Last, targets
                // held a hair past one limit then the other (0.5 to 5 mrad), so the servo arrives
                // at the limit still moving and the overshoot-snap branch fires on a target beyond it.
                let target = if tick < 1000 {
                    (next() - 0.5) * 6.0
                } else if tick < 2000 {
                    (tick as f32 * dt * 90f32.to_radians()).sin() * 3.0
                } else {
                    let hold = (tick - 2000) / 100;
                    let past = 0.0005 * (1 + hold % 10) as f32;
                    if hold % 2 == 0 { hi + past } else { lo - past }
                };
                world.get_mut::<ServoCommand>(servo).unwrap().target = target;
                world
                    .resource_mut::<Time<()>>()
                    .advance_by(Duration::from_secs_f32(dt));
                world.run_system_once(drive_servos).unwrap();
                let current = world.get::<TankServos>(root).unwrap().states[0].current;
                assert!(
                    (lo..=hi).contains(&current),
                    "start {start} tick {tick} target {target}: {current} outside [{lo}, {hi}]"
                );
            }
        }
    }

    /// The window contract `travel_window_fault` names: inverted, non-finite and zero-excluding
    /// windows are refused; a continuous mount and a window holding 0 are not.
    #[test]
    fn the_window_contract_refuses_inverted_and_zero_excluding_travel() {
        let fault = |min, max| {
            ServoSpec::test_limited(ServoRole::Pitch, 1.0, 1.0, min, max).travel_window_fault()
        };
        assert!(fault(-10.0, 20.0).is_none());
        assert!(fault(0.0, 20.0).is_none());
        assert!(fault(-10.0, 0.0).is_none());
        assert!(fault(5.0, -5.0).is_some(), "inverted");
        assert!(fault(5.0, 20.0).is_some(), "excludes 0 from below");
        assert!(fault(-20.0, -5.0).is_some(), "excludes 0 from above");
        assert!(fault(f32::NAN, 1.0).is_some());
        assert!(fault(-1.0, f32::INFINITY).is_some());
        assert!(
            ServoSpec::test_continuous(ServoRole::Yaw, 1.0, 1.0)
                .travel_window_fault()
                .is_none()
        );
    }

    #[test]
    fn remote_servo_state_is_the_view_interpolation_source_when_both_integrators_exist() {
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
