//! Spawn-built servo mechanisms. `drive_servos` keeps limited travel inside its admitted window;
//! sim angles and velocities use radians. Law and input provenance: design
//! `armor-penetration-and-damage.md` §7c.

use bevy::prelude::*;
use serde::Deserialize;

use super::model::{Controlled, TankRoot, TankServos};
use super::view::{ViewOf, ViewServo};
use crate::damage::{
    Part, Requirement, TankVolumes, VolumeFacets, evaluate, live_quality, part_facts,
};
use crate::provenance::Sourced;
use crate::track::sim::EngineRpm;
use crate::track::transmission::{RPM_TO_RAD, TransmissionParams};

#[derive(Clone, Copy, Deserialize)]
#[serde(from = "TravelDegrees")]
pub enum Travel {
    Limited { min: f32, max: f32 },
    Continuous,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum TravelDegrees {
    Limited { min: f32, max: f32 },
    Continuous,
}

impl From<TravelDegrees> for Travel {
    fn from(value: TravelDegrees) -> Self {
        match value {
            TravelDegrees::Limited { min, max } => Self::Limited {
                min: min.to_radians(),
                max: max.to_radians(),
            },
            TravelDegrees::Continuous => Self::Continuous,
        }
    }
}

fn angular_rate<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Sourced<f32>, D::Error> {
    let rate = Sourced::<f32>::deserialize(d)?.0;
    let radians = rate.to_radians();
    if !radians.is_finite() || radians <= 0.0 {
        return Err(serde::de::Error::custom(
            "hand_rate must be finite and positive in degrees/s and radians/s; author a representable rate",
        ));
    }
    Ok(Sourced(radians))
}

/// Construction selects a motor law; all runtime angular quantities use radians.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
enum ServoDrive {
    Hand {
        #[serde(deserialize_with = "angular_rate")]
        hand_rate: Sourced<f32>,
    },
    Powered {
        power_share: Sourced<f32>,
        stall_torque: Sourced<f32>,
        bearing_friction: Sourced<f32>,
        /// Moment of inertia (kg m²) of everything this servo turns, about its own axis.
        moving_inertia_kgm2: Sourced<f32>,
        #[serde(deserialize_with = "angular_rate")]
        hand_rate: Sourced<f32>,
    },
}

/// Spawn-built construction, never loaded from a view asset or replicated as mechanism state.
#[derive(Component)]
pub(crate) struct ServoMechanism {
    engine: TransmissionParams,
}

impl ServoMechanism {
    /// `engine` is the nominal transmission the sheet's admission parsed.
    pub(super) fn new(engine: &TransmissionParams) -> Self {
        Self {
            engine: engine.clone(),
        }
    }
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

/// One-axis mechanism; authored degrees are converted at deserialization.
#[derive(Component, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServoSpec {
    /// Role-based binding and rotation-axis source.
    pub(super) role: ServoRole,
    drive: ServoDrive,
    travel: Travel,
    /// In the game's damage model (game state only), this requirement scales slew effectiveness.
    /// Required: `[]` supplies full operator work; powered work still depends on the engine.
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
            Travel::Limited { min, max } => Some((min, max)),
            Travel::Continuous => None,
        }
    }

    /// Why this servo's drive facts or travel violate admission, or `None` when they hold.
    /// The contract: a `Limited` window is finite with `min <= 0 <= max` (a servo spawns at angle 0,
    /// so the window holds its own start), and `drive_servos` never lets `current` leave it.
    pub(crate) fn admission_fault(&self) -> Option<String> {
        if let ServoDrive::Powered {
            power_share,
            stall_torque,
            bearing_friction,
            moving_inertia_kgm2,
            hand_rate,
        } = self.drive
        {
            for (field, value) in [
                ("power_share", power_share.0),
                ("stall_torque", stall_torque.0),
                ("bearing_friction", bearing_friction.0),
                ("moving_inertia_kgm2", moving_inertia_kgm2.0),
            ] {
                if !value.is_finite() || value <= 0.0 {
                    return Some(format!(
                        "drive.Powered.{field} must be finite and positive; author a positive drive fact"
                    ));
                }
            }
            if power_share.0 > 1.0 || stall_torque.0 <= bearing_friction.0 {
                return Some("drive.Powered needs power_share <= 1 and stall_torque > bearing_friction; correct the drive facts".into());
            }
            let hand_work = hand_rate.0 * bearing_friction.0;
            if !hand_work.is_finite() || hand_work <= 0.0 {
                return Some("drive.Powered.hand_rate * bearing_friction must yield finite positive hand work; author representable rate and friction facts".into());
            }
            // The unoperated coast's deceleration (rad/s²); positive, as both facts are.
            if !(bearing_friction.0 / moving_inertia_kgm2.0).is_normal() {
                return Some("drive.Powered.bearing_friction / moving_inertia_kgm2 must be a normal positive f32 deceleration; author representable friction and inertia facts".into());
            }
        }
        let Travel::Limited { min, max } = self.travel else {
            return None;
        };
        // Written as a negated conjunction so NaN fails it too.
        (!(min.is_finite() && max.is_finite() && min <= 0.0 && max >= 0.0)).then(|| {
            format!(
                "travel Limited(min: {}, max: {}) must be finite with min <= 0 <= max \
                 (degrees): a servo spawns at angle 0, which the window must contain",
                min.to_degrees(),
                max.to_degrees()
            )
        })
    }

    /// Fixture hand drive; test callers supply degrees at the same edge as authored data.
    #[cfg(test)]
    pub(crate) fn test_limited(role: ServoRole, rate: f32, min: f32, max: f32) -> Self {
        Self {
            role,
            drive: ServoDrive::Hand {
                hand_rate: Sourced(rate.to_radians()),
            },
            travel: Travel::Limited {
                min: min.to_radians(),
                max: max.to_radians(),
            },
            requires: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_continuous(role: ServoRole, rate: f32) -> Self {
        Self {
            travel: Travel::Continuous,
            ..Self::test_limited(role, rate, 0.0, 0.0)
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
#[derive(Clone, Copy, PartialEq, Debug, Default)]
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

/// Client-local presentation state, permanent on replicas. Non-owners consume published angles;
/// the controlled tank integrates its own ahead-running aim. [`TankServos`] remains authoritative.
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

/// World-space origin and angle-zero orientation for a servo's parent-local command. Current
/// ancestor turns belong to the frame; this servo's current rotation does not.
pub(crate) fn servo_aim_frame<F: bevy::ecs::query::QueryFilter>(
    servo: Entity,
    root: Entity,
    root_position: Vec3,
    root_rotation: Quat,
    rest: &ServoRest,
    parents: &Query<&ChildOf>,
    locals: &Query<&Transform, F>,
) -> (Vec3, Quat) {
    let parent = parents
        .get(servo)
        .expect("a registered servo has its authored parent")
        .parent();
    let (position, rotation) =
        super::model::rig_world_pose(parent, root, root_position, root_rotation, parents, locals)
            .expect("a registered servo's parent chain reaches its root");
    let local = locals
        .get(servo)
        .expect("a registered servo has its authored local transform");
    (position + rotation * local.translation, rotation * rest.0)
}

fn servo_rotation(spec: &ServoSpec, rest: &ServoRest, angle: f32) -> Quat {
    servo_pose(spec.role, rest.0, angle)
}

/// A tank root's one servo integrator: `RemoteServos` on a client replica, `TankServos` on the
/// authority (the server and single-player). Spawn builds exactly one on every tank root, empty
/// for a sheet with no servos; `TankServos` never replicates and a client attachment never builds
/// it, so a root carrying both, or neither, is a broken spawn.
pub(crate) fn selected<S>(remote: Option<S>, authority: Option<S>) -> S {
    match (remote, authority) {
        (Some(states), None) | (None, Some(states)) => states,
        (Some(_), Some(_)) => panic!(
            "a servo root carries one integrator: RemoteServos on a replica, TankServos on the authority"
        ),
        (None, None) => {
            panic!("a registered servo's root carries its authoritative or remote mechanism state")
        }
    }
}

fn slot_state(states: &[ServoState], slot: usize) -> &ServoState {
    states
        .get(slot)
        .expect("every registered servo slot belongs to its selected mechanism state")
}

fn registered_state<'a>(
    root: Entity,
    slot: usize,
    authority: &'a Query<&TankServos>,
    remote: &'a Query<&RemoteServos>,
) -> &'a ServoState {
    let states = selected(
        remote.get(root).ok().map(|remote| &remote.0),
        authority.get(root).ok().map(|authority| &authority.states),
    );
    slot_state(states, slot)
}

/// Restore sim-node transforms from rollback state before any fixed-step gameplay reader runs.
/// Each node writes only its own transform, so nodes restore on the compute pool.
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
    q.par_iter_mut()
        .for_each(|(mut transform, spec, rest, slot, root)| {
            let state = registered_state(root.0, slot.0, &servos, &remote_servos);
            let rotation = servo_rotation(spec, rest, state.current);
            if transform.rotation != rotation {
                transform.rotation = rotation;
            }
        });
}

/// Each tank integrates its own slots from its own crew, modules and crank and this update's
/// commands, and writes only its own integrator, so tanks evaluate on the compute pool (Bevy
/// 0.19.1's `QueryParIter`); a tank's slots step in slot order.
pub(crate) fn drive_servos(
    time: Res<Time>,
    mut nodes: Query<(
        Entity,
        &mut Transform,
        &ServoSpec,
        &ServoRest,
        &ServoCommand,
        &ServoIndex,
        &TankRoot,
    )>,
    mut tanks: Query<
        (
            Entity,
            Option<&mut TankServos>,
            Option<&mut RemoteServos>,
            Option<&TankVolumes>,
            Has<Controlled>,
            Option<(&ServoMechanism, &EngineRpm)>,
        ),
        Or<(With<TankServos>, With<RemoteServos>)>,
    >,
    facets: Query<VolumeFacets>,
    // This update's servo nodes as (root, slot, node), sorted; kept for its capacity.
    mut owned: Local<Vec<(Entity, usize, Entity)>>,
) {
    let dt = time.delta_secs();
    owned.clear();
    owned.extend(
        nodes
            .iter()
            .map(|(node, .., slot, root)| (root.0, slot.0, node)),
    );
    owned.sort_unstable();
    tanks
        .par_iter_mut()
        .for_each(|(root, authority, remote, volumes, controlled, drive)| {
            let servos = &owned[owned.partition_point(|&(owner, ..)| owner < root)
                ..owned.partition_point(|&(owner, ..)| owner <= root)];
            if servos.is_empty() {
                // No slot to step: every servo of this root left with a detached assembly, or its
                // sheet declares none.
                return;
            }
            // A replica this client does not drive presents published angles.
            let published = remote.is_some() && !controlled;
            let states = selected(
                remote.map(|remote| &mut remote.into_inner().0),
                authority.map(|authority| &mut authority.into_inner().states),
            );
            // The game's damage model: requirements scale operator work; an engine in action
            // supplies whole power (`module_quality`). One reading serves every slot.
            let facts = (!published).then(|| {
                part_facts(
                    volumes.expect(
                        "an admitted tank with a servo carries its volume relationship target",
                    ),
                    &facets,
                )
            });
            for &(_, slot, node) in servos {
                let (_, _, spec, _, command, ..) = nodes
                    .get(node)
                    .expect("gathered from this query in this update");
                let state = states
                    .get_mut(slot)
                    .expect("every registered servo slot belongs to its selected mechanism state");
                state.previous = state.current;
                let Some(facts) = &facts else {
                    // A published angle is resolved truth, not an aim command with momentum.
                    state.current = command.target;
                    state.velocity = 0.0;
                    continue;
                };
                let slew = evaluate(&spec.requires, &facts.quality);
                let power = match spec.drive {
                    ServoDrive::Hand { .. } => 0.0,
                    ServoDrive::Powered { .. } => {
                        let (mechanism, rpm) = drive.expect(
                            "a powered servo's root has spawn-built construction and replicated \
                             crank speed",
                        );
                        let quality = live_quality(&facts.quality, Part::Engine);
                        // Accessory capacity saturates at the governed budget; fuel cut does not
                        // stop a turning crank. The missing pump map is stated in design
                        // section 7c.
                        let crank = rpm.rpm().min(mechanism.engine.engine.governed_rpm);
                        mechanism.engine.torque_at(crank) * crank * RPM_TO_RAD * quality
                    }
                };
                advance_servo(state, spec, command.target, power, slew, dt);
            }
        });
    // The sim node always carries fixed-step truth; smoothing writes the view tree only.
    nodes
        .par_iter_mut()
        .for_each(|(_, mut transform, spec, rest, _, slot, root)| {
            let (_, authority, remote, ..) = tanks.get(root.0).expect(
                "a registered servo's root carries its authoritative or remote mechanism state",
            );
            let states = selected(
                remote.map(|remote| &remote.0),
                authority.map(|authority| &authority.states),
            );
            let rotation = servo_rotation(spec, rest, slot_state(states, slot.0).current);
            if transform.rotation != rotation {
                transform.rotation = rotation;
            }
        });
}

/// Implicit acceleration solves I Δω/Δt = min(Tstall, P/ω) - Tbearing.
/// The positive quadratic root prevents a discrete step oscillating across the power equilibrium.
/// Infinite stall torque denotes hand work, whose torque curve is not declared.
fn powered_velocity(
    speed: f32,
    power: f32,
    stall: f32,
    friction: f32,
    inertia: f32,
    dt: f32,
) -> f32 {
    let (speed, power, stall, friction, inertia, dt) = (
        f64::from(speed),
        f64::from(power),
        f64::from(stall),
        f64::from(friction),
        f64::from(inertia),
        f64::from(dt),
    );
    let step = dt / inertia;
    let stalled = speed + (stall - friction) * step;
    if stalled * stall <= power {
        stalled as f32
    } else {
        let b = speed - friction * step;
        let c = power * step;
        let root = (b * b + 4.0 * c).sqrt();
        // Rationalize the root when b is negative to avoid subtracting equal magnitudes.
        (if b >= 0.0 {
            (b + root) / 2.0
        } else {
            2.0 * c / (root - b)
        }) as f32
    }
}

fn advance_servo(
    state: &mut ServoState,
    spec: &ServoSpec,
    target: f32,
    engine_power: f32,
    slew: f32,
    dt: f32,
) {
    assert!(
        dt.is_finite() && dt >= 0.0 && engine_power.is_finite() && engine_power >= 0.0,
        "servo time and available engine power are finite nonnegative simulation facts"
    );
    if dt == 0.0 {
        // No elapsed simulation time, including a fresh test world.
        return;
    }
    let error = match spec.travel {
        Travel::Limited { min, max } => target.clamp(min, max) - state.current,
        Travel::Continuous => shortest_angle(target - state.current),
    };
    let hand_rate = match spec.drive {
        ServoDrive::Hand { hand_rate } | ServoDrive::Powered { hand_rate, .. } => hand_rate.0,
    };
    state.velocity = match spec.drive {
        // No operator, no controller: the command neither drives nor brakes the mount. Its stored
        // momentum runs down under bearing friction, the only stopping torque the construction
        // declares (no brake, no self-locking gear), and static friction holds it at rest.
        // Resolution limit: a decrement under half the speed's f32 ulp leaves it unchanged, so
        // above about 2^24 * Tf/I * dt the coast stalls; DERIVED over 1e5 rad/s for the shipped
        // moments at 64 Hz. Admission keeps Tf/I itself a normal f32.
        ServoDrive::Powered {
            bearing_friction,
            moving_inertia_kgm2,
            ..
        } if slew == 0.0 => {
            let braking = bearing_friction.0 / moving_inertia_kgm2.0 * dt;
            state.velocity.signum() * (state.velocity.abs() - braking).max(0.0)
        }
        ServoDrive::Powered {
            power_share,
            stall_torque,
            bearing_friction,
            moving_inertia_kgm2,
            ..
        } => {
            let (friction, inertia) = (bearing_friction.0, moving_inertia_kgm2.0);
            let motor_power = engine_power * power_share.0 * slew;
            // Hand rate names sustained motion against the same bearing: P = Tf*ω.
            // No handwheel torque curve is supplied; do not borrow the powered motor's stall.
            let hand_power = hand_rate * slew * friction;
            let hand = hand_power > motor_power;
            let (power, stall) = if hand {
                (hand_power, f32::INFINITY)
            } else {
                (motor_power, stall_torque.0)
            };
            let speed = state.velocity.abs();
            let braking_torque = if hand {
                // Hand work can cease; only bearing friction is known to brake this path.
                0.0
            } else if speed * stall <= power {
                stall
            } else {
                power / speed
            };
            // Current torque is a lower bound on braking as speed falls. Reserve this tick's
            // travel as well as v²/(2a): v*dt + v²/(2a) <= |error|. Rationalize near arrival.
            let brake_accel = (braking_torque + friction) / inertia;
            let step = f64::from(brake_accel) * f64::from(dt);
            let work = 2.0 * f64::from(brake_accel) * f64::from(error.abs());
            let desired =
                ((work / ((step * step + work).sqrt() + step)) as f32).min(error.abs() / dt);
            if state.velocity * error < 0.0 {
                state.velocity.signum() * (speed - brake_accel * dt).max(0.0)
            } else if speed > desired {
                state.velocity.signum() * (speed - brake_accel * dt).max(desired)
            } else {
                error.signum()
                    * powered_velocity(speed, power, stall, friction, inertia, dt).min(desired)
            }
        }
        // Kinematic: no declared inertia stores momentum, so an absent operator holds it at once.
        ServoDrive::Hand { .. } => {
            if error == 0.0 {
                0.0
            } else {
                error.signum() * (hand_rate * slew).min(error.abs() / dt)
            }
        }
    };
    state.current += state.velocity * dt;
    if let Travel::Limited { min, max } = spec.travel {
        let limited = state.current.clamp(min, max);
        // A mechanical stop can absorb momentum; an aim command cannot.
        if limited != state.current {
            state.current = limited;
            state.velocity = 0.0;
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
        let (spec, rest, slot, root) = servos.get(view_of.0).expect(
            "ViewServo links a live sim servo; detach removes the view's servo binding in the same flush",
        );
        let state = registered_state(root.0, slot.0, &states, &remote_states);
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
    // Fixture angles, rates, time steps and tolerances below exercise generic mechanism invariants.
    use std::time::Duration;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// Fixture moving inertia (kg m²) of [`powered_spec`]'s mount.
    const FIXTURE_INERTIA: f32 = 10.0;

    fn powered_spec(share: f32, stall: f32, friction: f32) -> ServoSpec {
        // Fixture: a small powered mount with a one-degree/s hand drive.
        ServoSpec {
            drive: ServoDrive::Powered {
                power_share: Sourced(share),
                stall_torque: Sourced(stall),
                bearing_friction: Sourced(friction),
                moving_inertia_kgm2: Sourced(FIXTURE_INERTIA),
                hand_rate: Sourced(1f32.to_radians()),
            },
            ..ServoSpec::test_continuous(ServoRole::Yaw, 1.0)
        }
    }

    /// `spec` with its powered mount's moving inertia (kg m²) replaced.
    fn with_inertia(mut spec: ServoSpec, inertia: f32) -> ServoSpec {
        let ServoDrive::Powered {
            moving_inertia_kgm2,
            ..
        } = &mut spec.drive
        else {
            unreachable!("the fixture mount is powered")
        };
        *moving_inertia_kgm2 = Sourced(inertia);
        spec
    }

    /// A powered mount's declared bearing friction (N m) and moving inertia (kg m²).
    fn bearing_and_inertia(spec: &ServoSpec) -> (f32, f32) {
        let ServoDrive::Powered {
            bearing_friction,
            moving_inertia_kgm2,
            ..
        } = spec.drive
        else {
            unreachable!("the fixture mount is powered")
        };
        (bearing_friction.0, moving_inertia_kgm2.0)
    }

    /// A world `drive_servos` runs in: the clock it reads and the compute pool its per-tank
    /// evaluation batches on.
    fn sim_world() -> World {
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world
    }

    fn powered_world(engine_share: f32) -> (World, Entity, Entity, Entity) {
        let mut world = sim_world();
        let (root, servo, engine) = spawn_powered(&mut world, engine_share);
        (world, root, servo, engine)
    }

    /// One tank with one powered mount: (root, servo, engine).
    fn spawn_powered(world: &mut World, engine_share: f32) -> (Entity, Entity, Entity) {
        use crate::ballistics::ComponentHealth;
        use crate::damage::{FunctionRole, VolumeOf};
        // Fixture drive facts; no shipped vehicle output determines the assertions.
        let template = crate::bake::selected_spec();
        let mut mechanism =
            ServoMechanism::new(&template.validate().expect("the selected sheet is admitted"));
        // Fixture engine inputs replace every engine field the servo reads; unused transmission
        // construction comes from the admitted template and does not determine the expectations.
        mechanism.engine.engine.torque_nm = vec![(600.0, 1000.0), (2400.0, 1000.0)];
        mechanism.engine.engine.governed_rpm = 2400.0;
        let rpm = EngineRpm(600);
        let root = world.spawn((TankServos::for_count(1), mechanism, rpm)).id();
        let mut health = ComponentHealth::full(10);
        health.set_held((health.capacity() as f32 * engine_share) as u32);
        let engine = world
            .spawn((
                FunctionRole::Engine,
                crate::damage::ModuleSupply::Whole,
                health,
                VolumeOf(root),
            ))
            .id();
        let servo = world
            .spawn((
                Transform::default(),
                powered_spec(0.001, 100.0, 20.0),
                ServoRest(Quat::IDENTITY),
                ServoCommand::default(),
                ServoIndex(0),
                TankRoot(root),
            ))
            .id();
        (root, servo, engine)
    }

    /// Tanks evaluate independently on the compute pool: each integrates only its own slots from
    /// its own crew, modules and crank, so a tank steps beside another exactly as it steps alone.
    /// The tanks differ in a per-tank fact, so one stepped with the other's facts fails.
    #[test]
    fn each_tank_integrates_only_its_own_slots() {
        // Fixture: both mounts move at 0.5 rad/s; the second tank's engine is out of action.
        let engine_shares = [1.0, 0.0];
        let prepare = |world: &mut World, engine_share: f32| {
            let (root, servo, _) = spawn_powered(world, engine_share);
            world.get_mut::<TankServos>(root).unwrap().states[0] =
                ServoState::test_new(0.0, 0.0, 0.5);
            world.get_mut::<ServoCommand>(servo).unwrap().target = 2.9;
            root
        };
        let step = |world: &mut World| {
            world
                .resource_mut::<Time<()>>()
                .advance_by(Duration::from_secs_f32(1.0 / 64.0));
            world.run_system_once(drive_servos).unwrap();
        };
        let alone = engine_shares.map(|share| {
            let mut world = sim_world();
            let root = prepare(&mut world, share);
            step(&mut world);
            world.get::<TankServos>(root).unwrap().states[0]
        });
        let mut world = sim_world();
        let roots = engine_shares.map(|share| prepare(&mut world, share));
        step(&mut world);
        let together = roots.map(|root| world.get::<TankServos>(root).unwrap().states[0]);
        assert_ne!(alone[0], alone[1], "the fixture tanks step differently");
        assert_eq!(together, alone);
    }

    fn tick_powered(world: &mut World, root: Entity, servo: Entity) -> ServoState {
        // Fixture: a moving command maintains enough travel for the steady-speed law.
        let current = world.get::<TankServos>(root).unwrap().states[0].current;
        world.get_mut::<ServoCommand>(servo).unwrap().target = current + 2.9;
        world
            .resource_mut::<Time<()>>()
            .advance_by(Duration::from_secs_f32(1.0 / 64.0));
        world.run_system_once(drive_servos).unwrap();
        world.get::<TankServos>(root).unwrap().states[0]
    }

    /// The game's damage model: authority reads the engine HP share every tick. An engine out
    /// of action leaves only the operator's authored hand drive, including after powered motion.
    #[test]
    fn an_engine_out_of_action_brakes_to_the_hand_rate() {
        let (mut world, root, servo, engine) = powered_world(1.0);
        for _ in 0..512 {
            tick_powered(&mut world, root, servo);
        }
        let mut velocity = world.get::<TankServos>(root).unwrap().states[0].velocity;
        world
            .get_mut::<crate::ballistics::ComponentHealth>(engine)
            .unwrap()
            .set_held(0);
        let spec = world.get::<ServoSpec>(servo).unwrap();
        let ServoDrive::Powered { hand_rate, .. } = spec.drive else {
            unreachable!()
        };
        let (friction, inertia) = bearing_and_inertia(spec);
        for _ in 0..4096 {
            let state = tick_powered(&mut world, root, servo);
            // Bearing friction alone is a lower bound on coast-down time; hand work can only
            // reduce that braking, never erase the stored momentum in the game's damage model.
            assert!(state.velocity >= velocity - friction / inertia / 64.0);
            velocity = state.velocity;
        }
        assert!((velocity - hand_rate.0).abs() < hand_rate.0 * 0.001);
    }

    /// The game's damage model: the best available path sets bearing work. Authority must
    /// keep turning above the governor cut; a reduced engine powers it whole (provisional,
    /// playtest fork F20) and a knocked-out one leaves hand work.
    #[test]
    fn crank_speed_and_quality_select_the_best_available_work() {
        // Fixture flat curve: 1000 N m, governed at 2400 rpm; no engine-control width is assumed.
        // Fixture shares: 0.1 holds one HP of the ten-HP fixture pool.
        for (rpm, share) in [
            (0, 1.0),
            (600, 0.1),
            (600, 0.0),
            (2400, 1.0),
            (2600, 1.0),
            (3000, 1.0),
        ] {
            let (mut world, root, servo, _) = powered_world(share);
            let in_action = if share > 0.0 { 1.0 } else { 0.0 };
            // Fixture power share keeps steady speed below the controller's stopping envelope at
            // the shortest-arc command's finite lead; this test isolates available work.
            let power = 0.0001;
            let ServoDrive::Powered { power_share, .. } =
                &mut world.get_mut::<ServoSpec>(servo).unwrap().drive
            else {
                unreachable!()
            };
            *power_share = Sourced(power);
            world.get_mut::<EngineRpm>(root).unwrap().0 = rpm;
            for _ in 0..4096 {
                tick_powered(&mut world, root, servo);
            }
            let expected = (1000.0 * f32::from(rpm).min(2400.0) * RPM_TO_RAD * in_action * power
                / 20.0)
                .max(1f32.to_radians());
            let state = world.get::<TankServos>(root).unwrap().states[0];
            assert!(
                (state.velocity - expected).abs() < expected * 0.001,
                "rpm={rpm}, engine share={share}: {} vs {expected}",
                state.velocity
            );
        }
    }

    /// A non-owner replica presents published truth even when local engine/operator facts
    /// in the game's damage model are stale. Presentation must invent no momentum past it.
    #[test]
    fn a_replica_consumes_truth_instead_of_simulating_a_second_drive() {
        let (mut world, root, servo, _) = powered_world(1.0);
        world
            .entity_mut(root)
            .remove::<TankServos>()
            .insert(RemoteServos(vec![ServoState::test_new(0.0, 0.0, 0.55)]));
        world.get_mut::<ServoSpec>(servo).unwrap().requires =
            vec![crate::damage::Group::Single(Part::Gunner)];
        for target in [0.01, 0.01, -0.02] {
            world.get_mut::<ServoCommand>(servo).unwrap().target = target;
            world
                .resource_mut::<Time<()>>()
                .advance_by(Duration::from_secs_f32(1.0 / 64.0));
            world.run_system_once(drive_servos).unwrap();
            let state = world.get::<RemoteServos>(root).unwrap().0[0];
            assert_eq!(state.current, target);
            assert_eq!(state.velocity, 0.0);
        }
    }

    /// The controlled replica's ahead-running input is still a command. Its exception to
    /// published-angle presentation must retain the drive law.
    #[test]
    fn the_controlled_replica_integrates_its_own_aim() {
        let (mut world, root, servo, _) = powered_world(1.0);
        world
            .entity_mut(root)
            .remove::<TankServos>()
            .insert((super::super::model::Controlled, RemoteServos::for_count(1)));
        world.get_mut::<ServoCommand>(servo).unwrap().target = 0.5;
        world
            .resource_mut::<Time<()>>()
            .advance_by(Duration::from_secs_f32(1.0 / 64.0));
        world.run_system_once(drive_servos).unwrap();
        let state = world.get::<RemoteServos>(root).unwrap().0[0];
        assert!(state.current > 0.0 && state.current < 0.5);
        assert!(state.velocity > 0.0);
    }

    /// At unchanged supply, aim consumers rely on monotone arrival within float angle
    /// resolution, on the hand arm (no engine power) as on the motor's. A command jump retains
    /// the separate momentum contract.
    #[test]
    fn a_constant_target_is_approached_within_angle_resolution() {
        // Fixture drive, inertia, target and solver steps, spanning the stall/power knee.
        let spec = powered_spec(1.0, 100.0, 20.0);
        for dt in [1.0 / 32.0, 1.0 / 64.0, 1.0 / 128.0] {
            for power in [0.0, 1.0, 10.0, 1000.0] {
                for target in [-0.5_f32, 0.5] {
                    let mut state = ServoState::default();
                    // Fixture allowance: subtraction, velocity rounding, product and addition
                    // each contribute at most one relative f32 epsilon at this angle scale.
                    let roundoff = 4.0 * f32::EPSILON * target.abs();
                    for _ in 0..8192 {
                        let previous = state.current;
                        advance_servo(&mut state, &spec, target, power, 1.0, dt);
                        assert!(
                            state.current * target.signum() <= target.abs() + roundoff,
                            "crossed {target}: {}",
                            state.current
                        );
                        assert!((state.current - previous) * target.signum() >= -roundoff);
                    }
                    assert!((state.current - target).abs() < 1e-6);
                }
            }
        }
    }

    /// A travel stop absorbs momentum. Removing its velocity reset must leave this red,
    /// independently of a later controller tick that could hide the missed collision.
    #[test]
    fn a_mechanical_stop_absorbs_velocity_on_impact() {
        let mut spec = powered_spec(1.0, 100.0, 20.0);
        spec.travel = Travel::Limited {
            min: -0.5,
            max: 0.5,
        };
        for direction in [-1.0, 1.0] {
            let mut state = ServoState::test_new(direction * 0.499, 0.0, direction);
            advance_servo(&mut state, &spec, direction, 1000.0, 1.0, 1.0 / 64.0);
            assert_eq!(state.current, direction * 0.5);
            assert_eq!(state.velocity, 0.0);
        }
    }

    /// Before the power knee, Δω = (Tstall - Tfriction)*dt/I. The mount's startup response
    /// therefore halves when its own declared moving inertia doubles.
    #[test]
    fn declared_moving_inertia_sets_stall_acceleration() {
        let mut speeds = Vec::new();
        for inertia in [FIXTURE_INERTIA, 2.0 * FIXTURE_INERTIA] {
            let (mut world, root, servo, _) = powered_world(1.0);
            let mut spec = world.get_mut::<ServoSpec>(servo).unwrap();
            *spec = with_inertia(spec.clone(), inertia);
            let state = tick_powered(&mut world, root, servo);
            let expected = (100.0 - 20.0) / inertia / 64.0;
            assert!((state.velocity - expected).abs() < 1e-6);
            speeds.push(state.velocity);
        }
        eprintln!(
            "MEASURED fixture startup steps: base={} rad/s, doubled inertia={} rad/s",
            speeds[0], speeds[1]
        );
        assert_eq!(speeds[1], speeds[0] / 2.0);
    }

    /// The game's damage model: with its operator's HP pool emptied, a moving powered mount has no
    /// controller, so the command neither drives nor brakes it. Its momentum runs down under the
    /// only stopping torque its construction declares, bearing friction: each tick removes Tf/I·dt
    /// until rest, which it reaches by v0·I/Tf and keeps. The owner's ahead-running replica
    /// integrates the same coast.
    #[test]
    fn an_unoperated_powered_mount_coasts_to_rest_under_bearing_friction() {
        // Fixture: the mechanism moves at 0.5 rad/s from the zero pose.
        let moving = ServoState::test_new(0.0, 0.0, 0.5);
        for fixed in [false, true] {
            let mut runs = Vec::new();
            for owner in [false, true] {
                let (mut world, root, servo, _) = powered_world(1.0);
                let (friction, inertia) =
                    bearing_and_inertia(world.get::<ServoSpec>(servo).unwrap());
                let braking = friction / inertia / 64.0;
                let rest = (moving.velocity / braking).ceil() as usize;
                // Fixture commands: far ahead of the pose every tick, or fixed at the continuous
                // law's coast distance v0²·I/(2·Tf), beyond where the per-tick coast rests. A
                // controller reaches the fixed one only by braking less than friction does.
                let stop = moving.velocity * moving.velocity * inertia / (2.0 * friction);
                let command = |pose: f32| if fixed { stop } else { pose + 2.9 };
                world.get_mut::<ServoSpec>(servo).unwrap().requires =
                    vec![crate::damage::Group::Single(Part::Gunner)];
                if owner {
                    world
                        .entity_mut(root)
                        .remove::<TankServos>()
                        .insert((super::super::model::Controlled, RemoteServos(vec![moving])));
                } else {
                    world.get_mut::<TankServos>(root).unwrap().states[0] = moving;
                }
                let state = |world: &World| match owner {
                    true => world.get::<RemoteServos>(root).unwrap().0[0],
                    false => world.get::<TankServos>(root).unwrap().states[0],
                };
                let mut run = Vec::new();
                let mut velocity = moving.velocity;
                for _ in 0..rest + 4 {
                    world.get_mut::<ServoCommand>(servo).unwrap().target =
                        command(state(&world).current);
                    world
                        .resource_mut::<Time<()>>()
                        .advance_by(Duration::from_secs_f32(1.0 / 64.0));
                    world.run_system_once(drive_servos).unwrap();
                    let now = state(&world);
                    // Fixture allowance: one f32 rounding of the speed subtraction.
                    let coast = (velocity - braking).max(0.0);
                    assert!(
                        (now.velocity - coast).abs() <= f32::EPSILON * velocity,
                        "{} rad/s where friction leaves {coast}",
                        now.velocity
                    );
                    velocity = now.velocity;
                    run.push(now);
                }
                assert_eq!(velocity, 0.0);
                runs.push(run);
            }
            assert_eq!(
                runs[0], runs[1],
                "the owner's replica coasts as the authority does"
            );
        }
    }

    /// A hand mount declares no moving inertia: with its operator's HP pool emptied it stores no
    /// momentum to coast on and holds its pose.
    #[test]
    fn an_unoperated_hand_mount_holds() {
        let (mut world, root, servo, _) = powered_world(1.0);
        // Fixture: Gunner is an absent source and the mechanism was moving.
        world.get_mut::<TankServos>(root).unwrap().states[0] = ServoState::test_new(0.0, 0.0, 0.5);
        let mut spec = world.get_mut::<ServoSpec>(servo).unwrap();
        spec.drive = ServoSpec::test_continuous(ServoRole::Yaw, 1.0).drive;
        spec.requires = vec![crate::damage::Group::Single(Part::Gunner)];
        let state = tick_powered(&mut world, root, servo);
        assert_eq!(state.current, 0.0);
        assert_eq!(state.velocity, 0.0);
    }

    /// Operator throughput limits available work, not the drive's physical stall torque. A
    /// partial operator must retain stall acceleration while reaching a proportional steady rate.
    #[test]
    fn operator_throughput_limits_power_without_changing_stall_torque() {
        // Fixture engine work and drive facts; one-step speed stays below the power knee.
        let power = 1000.0 * 600.0 * RPM_TO_RAD;
        let spec = powered_spec(0.001, 100.0, 20.0);
        for operator in [1.0, 0.5] {
            let mut state = ServoState::default();
            advance_servo(&mut state, &spec, 2.9, power, operator, 1.0 / 64.0);
            assert_eq!(state.velocity, (100.0 - 20.0) / 10.0 / 64.0);
            for _ in 0..2048 {
                let target = state.current + 2.9;
                advance_servo(&mut state, &spec, target, power, operator, 1.0 / 64.0);
            }
            let expected = power * 0.001 * operator / 20.0;
            assert!((state.velocity - expected).abs() < expected * 0.001);
        }
    }

    /// A commanded target is not a mechanical stop. A jumping aim intent cannot arrest inertia
    /// faster than the available motor and bearing torque, even when it jumps onto the current pose.
    #[test]
    fn a_command_jump_preserves_momentum_until_the_drive_can_brake_it() {
        // Fixture torque = 100 N m, bearing = 20 N m, I = 10 kg m²; Δω <= 12*dt rad/s.
        let spec = powered_spec(1.0, 100.0, 20.0);
        let dt = 1.0 / 64.0;
        for target in [0.0, 0.001, -0.001] {
            let mut state = ServoState::test_new(0.0, 0.0, 1.0);
            advance_servo(&mut state, &spec, target, 1000.0, 1.0, dt);
            assert_eq!(state.velocity, 1.0 - 12.0 * dt);
            assert!(state.current > target);
        }
    }

    /// The controller must settle a constant command and reverse without a separate speed cap.
    /// Available torque, rather than crossing the command angle, controls its velocity changes.
    #[test]
    fn a_powered_mount_settles_and_reverses_within_its_window() {
        let mut spec = powered_spec(1.0, 100.0, 20.0);
        // Fixture window, step and inertia; target changes include both mechanical limits.
        spec.travel = Travel::Limited {
            min: -1.0,
            max: 1.0,
        };
        let mut state = ServoState::default();
        for target in [0.5, -0.5, 2.0, -2.0, 0.0] {
            for _ in 0..2048 {
                advance_servo(&mut state, &spec, target, 1000.0, 1.0, 1.0 / 64.0);
                assert!((-1.0..=1.0).contains(&state.current));
            }
            assert!((state.current - target.clamp(-1.0, 1.0)).abs() < 1e-6);
            assert!(state.velocity.abs() < 1e-6);
        }
    }

    /// Invariant: a `Limited` servo's `current` stays inside `travel_limits()` on every tick, for any
    /// start inside the window and any target, including one beyond either limit and one sweeping
    /// at full slew.
    #[test]
    fn a_limited_servo_never_leaves_its_window() {
        let mut world = sim_world();
        let spec = ServoSpec::test_limited(ServoRole::Yaw, 90.0, -20.0, 35.0);
        let (lo, hi) = spec.travel_limits().unwrap();
        let dt = 1.0 / 64.0;
        let root = world
            .spawn(TankServos {
                states: vec![ServoState::default()],
            })
            .id();
        // The game's damage model: an explicitly empty requirement evaluates to full capability.
        // The relationship still supplies the complete root bundle the mechanism reads.
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
                // held just beyond alternating limits (Fixture: 0.5 to 5 mrad), exercising
                // target admission at both ends of the mechanical window.
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

    /// Travel refusals must quote the authored unit so an artist can repair the sheet directly.
    #[test]
    fn a_travel_refusal_reports_authored_degrees() {
        let spec = ServoSpec::test_limited(ServoRole::Pitch, 1.0, 90.0, 180.0);
        let err = spec.admission_fault().unwrap();
        assert!(
            err.contains("min: 90") && err.contains("max: 180") && err.contains("degrees"),
            "{err}"
        );
    }

    /// Admission protects the integrator from nonrepresentable hand rates and drive facts.
    #[test]
    fn drive_facts_refuse_invalid_values_before_play() {
        for value in ["0.0", "-1.0", "NaN", "inf", "1e-45"] {
            let row = format!(
                "(role: Yaw, drive: Hand(hand_rate: ({value}, Fixture(\"rate\"))), travel: Continuous, requires: [])"
            );
            assert!(ron::from_str::<ServoSpec>(&row).is_err());
        }
        // Fixture power-drive facts; each admitted drive must overcome its own bearing friction.
        for (share, stall, friction) in [
            (0.0, 10.0, 1.0),
            (2.0, 10.0, 1.0),
            (0.1, 1.0, 1.0),
            (0.1, 10.0, 0.0),
        ] {
            let spec = powered_spec(share, stall, friction);
            assert!(spec.admission_fault().is_some());
        }
        // A powered row states its own moving load: absent, it does not parse; unusable, it is
        // refused by name.
        let row = "(role: Yaw, drive: Powered(power_share: (0.1, Fixture(\"share\")), \
                   stall_torque: (10.0, Fixture(\"stall\")), bearing_friction: (1.0, \
                   Fixture(\"bearing\")), hand_rate: (1.0, Fixture(\"rate\"))), \
                   travel: Continuous, requires: [])";
        let err = ron::from_str::<ServoSpec>(row).err().unwrap().to_string();
        assert!(err.contains("moving_inertia_kgm2"), "{err}");
        for inertia in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let fault = with_inertia(powered_spec(0.1, 10.0, 1.0), inertia)
                .admission_fault()
                .unwrap();
            assert!(fault.contains("moving_inertia_kgm2"), "{fault}");
        }
        // Fixture individually representable facts whose coast deceleration overflows, is
        // subnormal, or underflows.
        for (friction, inertia) in [(1e30, 1e-10), (1e-20, 1e20), (1e-30, 1e30)] {
            let spec = with_inertia(powered_spec(0.1, 2.0 * friction, friction), inertia);
            let fault = spec.admission_fault().unwrap();
            assert!(fault.contains("/ moving_inertia_kgm2"), "{fault}");
        }
        // Fixture individually representable facts whose derived hand work over/underflows.
        for (rate, friction) in [(1e30, 1e20), (1e-20, 1e-30)] {
            let mut spec = powered_spec(1.0, 2.0 * friction, friction);
            let ServoDrive::Powered { hand_rate, .. } = &mut spec.drive else {
                unreachable!()
            };
            *hand_rate = Sourced(rate);
            let fault = spec.admission_fault().unwrap();
            assert!(fault.contains("hand_rate") && fault.contains("bearing_friction"));
        }
    }

    /// One slot-0 servo on a root carrying `integrators`. Fixed-step restoration and integration
    /// both rely on the root's one integrator owning every registered slot.
    fn servo_on(integrators: impl Bundle) -> World {
        let mut world = sim_world();
        let root = world.spawn(integrators).id();
        world.spawn((
            Transform::default(),
            ServoSpec::test_continuous(ServoRole::Yaw, 1.0),
            ServoRest(Quat::IDENTITY),
            ServoCommand::default(),
            ServoIndex(0),
            TankRoot(root),
        ));
        world
    }

    /// A replica whose remote vector lacks the registered slot.
    fn missing_remote_slot() -> World {
        servo_on(RemoteServos(vec![]))
    }

    /// A root carrying both integrators, which no spawn path builds.
    fn two_integrators() -> World {
        servo_on((TankServos::for_count(1), RemoteServos::for_count(1)))
    }

    #[test]
    #[should_panic(
        expected = "every registered servo slot belongs to its selected mechanism state"
    )]
    fn restoration_refuses_a_slot_missing_from_the_selected_source() {
        missing_remote_slot()
            .run_system_once(restore_servo_truth)
            .unwrap();
    }

    #[test]
    #[should_panic(
        expected = "every registered servo slot belongs to its selected mechanism state"
    )]
    fn integration_refuses_a_slot_missing_from_the_selected_source() {
        missing_remote_slot().run_system_once(drive_servos).unwrap();
    }

    #[test]
    #[should_panic(expected = "a servo root carries one integrator")]
    fn restoration_refuses_a_root_with_two_integrators() {
        two_integrators()
            .run_system_once(restore_servo_truth)
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "a servo root carries one integrator")]
    fn integration_refuses_a_root_with_two_integrators() {
        two_integrators().run_system_once(drive_servos).unwrap();
    }

    /// Presentation relies on the source/view lifetime contract; an orphan must not freeze at its
    /// last pose after a missing source is silently mistaken for a loading state.
    #[test]
    #[should_panic(expected = "ViewServo links a live sim servo")]
    fn interpolation_refuses_an_orphaned_binding() {
        let mut world = World::new();
        world.insert_resource(Time::<Fixed>::default());
        let source = world.spawn_empty().id();
        world.spawn((Transform::default(), ViewOf(source), ViewServo));
        world.run_system_once(interpolate_servos).unwrap();
    }

    /// The window contract `admission_fault` names: inverted, non-finite and zero-excluding
    /// windows are refused; a continuous mount and a window holding 0 are not.
    #[test]
    fn the_window_contract_refuses_inverted_and_zero_excluding_travel() {
        let fault =
            |min, max| ServoSpec::test_limited(ServoRole::Pitch, 1.0, min, max).admission_fault();
        assert!(fault(-10.0, 20.0).is_none());
        assert!(fault(0.0, 20.0).is_none());
        assert!(fault(-10.0, 0.0).is_none());
        assert!(fault(5.0, -5.0).is_some(), "inverted");
        assert!(fault(5.0, 20.0).is_some(), "excludes 0 from below");
        assert!(fault(-20.0, -5.0).is_some(), "excludes 0 from above");
        assert!(fault(f32::NAN, 1.0).is_some());
        assert!(fault(-1.0, f32::INFINITY).is_some());
        assert!(
            ServoSpec::test_continuous(ServoRole::Yaw, 1.0)
                .admission_fault()
                .is_none()
        );
    }

    /// A replica's view blends its remote integrator's last two fixed-step angles.
    #[test]
    fn a_replica_view_interpolates_its_remote_state() {
        let mut world = World::new();
        let root = world
            .spawn(RemoteServos(vec![ServoState::test_new(1.0, 0.0, 0.5)]))
            .id();
        let servo = world
            .spawn((
                ServoSpec::test_continuous(ServoRole::Yaw, 90.0),
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
            "the remote state provides the interpolated pose",
        );
    }
}
