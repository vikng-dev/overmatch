//! Sleeping and waking for [`PhysicsIsland`](super::PhysicsIsland)s.
//!
//! See [`IslandSleepingPlugin`].

use bevy::{
    app::{App, Plugin},
    ecs::{
        entity::Entity,
        entity_disabling::Disabled,
        error::Result,
        lifecycle::{Discard, HookContext, Insert},
        observer::On,
        query::{Changed, Has, Or, With, Without},
        resource::Resource,
        schedule::{
            IntoScheduleConfigs,
            common_conditions::{resource_changed, resource_exists},
        },
        system::{
            Command, Commands, Local, ParamSet, Query, Res, ResMut, SystemChangeTick, SystemState,
            lifetimeless::{SQuery, SResMut},
        },
        world::{DeferredWorld, Mut, Ref, World},
    },
    prelude::{Deref, DerefMut},
    time::Time,
};

use crate::{
    data_structures::bit_vec::BitVec,
    dynamics::{
        joints::joint_graph::JointGraph,
        solver::{
            constraint_graph::ConstraintGraph,
            islands::{BodyIslandNode, IslandId, PhysicsIslands},
            solver_body::{SolverBodies, SolverBody, SolverBodyIndex},
        },
    },
    prelude::*,
    schedule::{LastPhysicsTick, is_changed_after_tick},
    // OVERMATCH PATCH: the rest test runs per body on the compute pool.
    utils::{MIN_PAR_ITER_ENTITIES, ParallelQueryForEach},
};

/// A plugin for managing sleeping and waking of [`PhysicsIsland`](super::PhysicsIsland)s.
pub struct IslandSleepingPlugin;

impl Plugin for IslandSleepingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AwakeIslandBitVec>();
        app.init_resource::<TimeToSleep>();
        app.init_resource::<RestTest>(); // OVERMATCH PATCH

        // Insert `SleepThreshold` and `SleepTimer` for each body that has a solver body.
        app.register_required_components::<SolverBodyIndex, SleepThreshold>();
        app.register_required_components::<SolverBodyIndex, SleepTimer>();

        // Set up cached system states for sleeping and waking bodies or islands.
        let cached_system_state1 = CachedBodySleepingSystemState(SystemState::new(app.world_mut()));
        let cached_system_state2 =
            CachedIslandSleepingSystemState(SystemState::new(app.world_mut()));
        let cached_system_state3 = CachedIslandWakingSystemState(SystemState::new(app.world_mut()));
        app.insert_resource(cached_system_state1);
        app.insert_resource(cached_system_state2);
        app.insert_resource(cached_system_state3);

        // Set up hooks to automatically sleep/wake islands when `Sleeping` is added/removed.
        app.world_mut()
            .register_component_hooks::<Sleeping>()
            .on_add(sleep_on_add_sleeping)
            .on_remove(wake_on_remove_sleeping);

        app.add_observer(wake_on_replace_rigid_body);
        app.add_observer(wake_on_enable_rigid_body);

        app.add_systems(
            PhysicsSchedule,
            (
                update_sleeping_states,
                wake_islands_with_sleeping_disabled,
                wake_on_changed,
                wake_all_islands.run_if(resource_changed::<Gravity>),
                sleep_islands,
            )
                .chain()
                .run_if(resource_exists::<PhysicsIslands>)
                .in_set(PhysicsStepSystems::Sleeping),
        );
    }
}

fn sleep_on_add_sleeping(mut world: DeferredWorld, ctx: HookContext) {
    let Some(body_island) = world.get::<BodyIslandNode>(ctx.entity) else {
        return;
    };

    let island_id = body_island.island_id;

    // Check if the island is already sleeping.
    if let Some(island) = world
        .get_resource::<PhysicsIslands>()
        .and_then(|islands| islands.get(island_id))
        && island.is_sleeping
    {
        return;
    }

    world.commands().queue_silenced(SleepBody(ctx.entity));
}

fn wake_on_remove_sleeping(mut world: DeferredWorld, ctx: HookContext) {
    let Some(body_island) = world.get::<BodyIslandNode>(ctx.entity) else {
        return;
    };

    let island_id = body_island.island_id;

    // Check if the island is already awake.
    if let Some(island) = world
        .get_resource::<PhysicsIslands>()
        .and_then(|islands| islands.get(island_id))
        && !island.is_sleeping
    {
        return;
    }

    // OVERMATCH PATCH: this body has lost `Sleeping` by the time the queued wake runs, which
    // restarts only sleeping bodies' rest intervals.
    world
        .get_mut::<SleepTimer>(ctx.entity)
        .expect("every island body retains its SleepTimer")
        .0 = 0.0;
    world.commands().queue_silenced(WakeBody(ctx.entity));
}

fn wake_on_replace_rigid_body(
    trigger: On<Discard<RigidBody>>,
    mut commands: Commands,
    query: Query<&BodyIslandNode>,
) {
    let Ok(body_island) = query.get(trigger.entity) else {
        return;
    };

    commands.queue(WakeIslands(vec![body_island.island_id]));
}

fn wake_on_enable_rigid_body(
    trigger: On<Insert<BodyIslandNode>>,
    mut commands: Commands,
    mut query: Query<
        (&BodyIslandNode, &mut SleepTimer, Has<Sleeping>),
        Or<(With<Disabled>, Without<Disabled>)>,
    >,
) {
    let Ok((body_island, mut sleep_timer, is_sleeping)) = query.get_mut(trigger.entity) else {
        return;
    };

    if is_sleeping {
        commands.entity(trigger.entity).try_remove::<Sleeping>();
    }

    // Reset the sleep timer and wake up the island.
    if sleep_timer.0 > 0.0 {
        sleep_timer.0 = 0.0;
        commands.queue(WakeIslands(vec![body_island.island_id]));
    }
}

/// A bit vector that stores which islands are kept awake and which are allowed to sleep.
#[derive(Resource, Default, Deref, DerefMut)]
pub(crate) struct AwakeIslandBitVec(pub(crate) BitVec);

fn wake_islands_with_sleeping_disabled(
    mut awake_island_bit_vec: ResMut<AwakeIslandBitVec>,
    mut query: Query<
        (&BodyIslandNode, &mut SleepTimer),
        Or<(
            With<SleepingDisabled>,
            With<Disabled>,
            With<RigidBodyDisabled>,
        )>,
    >,
) {
    // Wake up all islands that have a body with `SleepingDisabled`.
    for (body_island, mut sleep_timer) in &mut query {
        awake_island_bit_vec.set_and_grow(body_island.island_id.0 as usize);

        // Reset the sleep timer.
        sleep_timer.0 = 0.0;
    }
}

// OVERMATCH PATCH (hook): the rest test.
/// The test that decides whether an awake body rests this step: passing it advances the body's
/// [`SleepTimer`], failing it restarts the timer. The default is Avian's own, [`velocity_rest`].
#[derive(Resource, Clone, Copy, Debug)]
pub struct RestTest(pub RestTestFn);

/// A rest test of the body's solver state, its [`SleepThreshold`], the [`PhysicsLengthUnit`] and
/// the body's computed mass, angular inertia and rotation.
pub type RestTestFn =
    fn(&SolverBody, &SleepThreshold, f32, ComputedMass, ComputedAngularInertia, Rotation) -> bool;

impl Default for RestTest {
    fn default() -> Self {
        Self(velocity_rest)
    }
}

/// Avian's rest test: linear speed under [`SleepThreshold::linear`] times the length unit, and
/// angular speed under [`SleepThreshold::angular`]. A negative threshold keeps its sign and never
/// passes.
pub fn velocity_rest(
    solver_body: &SolverBody,
    sleep_threshold: &SleepThreshold,
    length_unit: f32,
    _: ComputedMass,
    _: ComputedAngularInertia,
    _: Rotation,
) -> bool {
    let length_unit_squared = length_unit * length_unit;
    let lin_vel_squared = solver_body.linear_velocity.length_squared();
    #[cfg(feature = "2d")]
    let ang_vel_squared = solver_body.angular_velocity * solver_body.angular_velocity;
    #[cfg(feature = "3d")]
    let ang_vel_squared = solver_body.angular_velocity.length_squared();

    // Keep signs.
    let lin_threshold_squared = sleep_threshold.linear * sleep_threshold.linear.abs();
    let ang_threshold_squared = sleep_threshold.angular * sleep_threshold.angular.abs();

    lin_vel_squared < length_unit_squared * lin_threshold_squared
        && ang_vel_squared < ang_threshold_squared
}

fn update_sleeping_states(
    mut awake_island_bit_vec: ResMut<AwakeIslandBitVec>,
    mut islands: ResMut<PhysicsIslands>,
    solver_bodies: Res<SolverBodies>,
    mut query: Query<
        (
            &mut SleepTimer,
            &SleepThreshold,
            &SolverBodyIndex,
            &BodyIslandNode,
            // OVERMATCH PATCH: the rest test's inputs.
            &ComputedMass,
            &ComputedAngularInertia,
            &Rotation,
        ),
        (Without<Sleeping>, Without<SleepingDisabled>),
    >,
    length_unit: Res<PhysicsLengthUnit>,
    time_to_sleep: Res<TimeToSleep>,
    time: Res<Time>,
    rest_test: Res<RestTest>, // OVERMATCH PATCH
    // OVERMATCH PATCH: each body's island and rest time, committed after the parallel pass.
    mut rested: Local<bevy::utils::Parallel<Vec<(IslandId, f32)>>>,
) {
    let delta_secs = time.delta_secs();

    islands.split_candidate_sleep_timer = 0.0;

    // OVERMATCH PATCH: a body's timer is its own, so the rest test runs per body on the compute
    // pool, above upstream's floor for its per-body loops.
    query.par_for_each_mut(
        MIN_PAR_ITER_ENTITIES,
        |(mut sleep_timer, sleep_threshold, index, island_data, mass, inertia, rotation)| {
            // OVERMATCH PATCH: an index is inserted after its slot is pushed and removed with it,
            // so a miss is broken bookkeeping, not a body to skip.
            let solver_body = solver_bodies
                .get(*index)
                .expect("every indexed awake body has a live SolverBodies slot");
            // OVERMATCH PATCH: the rest test is a hook.
            if (rest_test.0)(
                solver_body,
                sleep_threshold,
                length_unit.0,
                *mass,
                *inertia,
                *rotation,
            ) {
                // Increment the sleep timer.
                sleep_timer.0 += delta_secs;
            } else {
                // Reset the sleep timer if the body is moving.
                sleep_timer.0 = 0.0;
            }
            rested.scope(|out| out.push((island_data.island_id, sleep_timer.0)));
        },
    );

    // OVERMATCH PATCH: the islands' effects commit here, in any order to the same result: the wake
    // bits are a union, and the split candidate is the sleepiest island, a tie going to the lesser
    // island id. It replaces the standing candidate under upstream's condition.
    let mut sleepiest: Option<(f32, IslandId)> = None;
    for (island_id, rested) in rested.drain() {
        if rested < time_to_sleep.0 {
            // Keep the island awake.
            awake_island_bit_vec.set_and_grow(island_id.0 as usize);
        } else if let Some(island) = islands.get(island_id)
            // The body wants to sleep, but its island needs splitting first.
            && island.constraints_removed > 0
            && sleepiest.is_none_or(|(timer, id)| {
                rested > timer || (rested == timer && island_id < id)
            })
        {
            sleepiest = Some((rested, island_id));
        }
    }
    if let Some((rested, island_id)) = sleepiest
        && rested > islands.split_candidate_sleep_timer
    {
        // This island is now the sleepiest candidate for splitting.
        islands.split_candidate = Some(island_id);
        islands.split_candidate_sleep_timer = rested;
    }
}

fn sleep_islands(
    mut awake_island_bit_vec: ResMut<AwakeIslandBitVec>,
    mut islands: ResMut<PhysicsIslands>,
    mut commands: Commands,
    mut sleep_buffer: Local<Vec<IslandId>>,
    mut wake_buffer: Local<Vec<IslandId>>,
) {
    // Clear the buffers.
    sleep_buffer.clear();
    wake_buffer.clear();

    // Sleep islands that are not in the awake bit vector.
    for island in islands.iter_mut() {
        if awake_island_bit_vec.get(island.id.0 as usize) {
            if island.is_sleeping {
                wake_buffer.push(island.id);
            }
        } else if !island.is_sleeping && island.constraints_removed == 0 {
            // The island does not have a pending split, so it can go to sleep.
            sleep_buffer.push(island.id);
        }
    }

    // Sleep islands.
    if !sleep_buffer.is_empty() {
        let sleep_buffer = sleep_buffer.clone();
        commands.queue(|world: &mut World| {
            SleepIslands(sleep_buffer).apply(world);
        });
    }

    // Wake islands.
    if !wake_buffer.is_empty() {
        let wake_buffer = wake_buffer.clone();
        commands.queue(|world: &mut World| {
            WakeIslands(wake_buffer).apply(world);
        });
    }

    // Reset the awake island bit vector.
    awake_island_bit_vec.set_bit_count_and_clear(islands.len());
}

#[derive(Resource)]
struct CachedBodySleepingSystemState(
    SystemState<(
        SQuery<&'static mut BodyIslandNode, Or<(With<Disabled>, Without<Disabled>)>>,
        SQuery<&'static RigidBodyColliders>,
        SResMut<PhysicsIslands>,
        SResMut<ContactGraph>,
        SResMut<JointGraph>,
    )>,
);

/// A [`Command`] that forces a [`RigidBody`] and its [`PhysicsIsland`][super::PhysicsIsland] to be [`Sleeping`].
pub struct SleepBody(pub Entity);

impl Command for SleepBody {
    type Out = Result;
    fn apply(self, world: &mut World) -> Result {
        if let Ok(entity) = world.get_entity(self.0) {
            if let Some(island_id) = entity.get::<BodyIslandNode>().map(|node| node.island_id) {
                world.try_resource_scope(|world, mut state: Mut<CachedBodySleepingSystemState>| {
                    let (
                        mut body_islands,
                        body_colliders,
                        mut islands,
                        contact_graph,
                        joint_graph,
                    ) = state.0.get_mut(world).unwrap();

                    let Some(island) = islands.get_mut(island_id) else {
                        return;
                    };

                    // The island must be split before it can be woken up.
                    // Note that this is expensive.
                    if island.constraints_removed > 0 {
                        islands.split_island(
                            island_id,
                            &mut body_islands,
                            &body_colliders,
                            &contact_graph,
                            &joint_graph,
                        );
                    }

                    // The ID of the body's island might have changed due to the split,
                    // so we need to retrieve it again.
                    let island_id = body_islands.get(self.0).map(|node| node.island_id).unwrap();

                    // Sleep the island.
                    SleepIslands(vec![island_id]).apply(world);
                });
                Ok(())
            } else {
                Err(format!(
                    "Tried to sleep entity {:?} that is not a body or does not belong to an island",
                    self.0
                )
                .into())
            }
        } else {
            Err(format!("Tried to sleep entity {:?} that does not exist", self.0).into())
        }
    }
}

#[derive(Resource)]
struct CachedIslandSleepingSystemState(
    SystemState<(
        SQuery<(
            &'static BodyIslandNode,
            &'static mut SleepTimer,
            Option<&'static RigidBodyColliders>,
        )>,
        SResMut<PhysicsIslands>,
        SResMut<ContactGraph>,
        SResMut<ConstraintGraph>,
    )>,
);

/// A [`Command`] that makes the [`PhysicsIsland`](super::PhysicsIsland)s with the given IDs sleep if they are not already sleeping.
pub struct SleepIslands(pub Vec<IslandId>);

impl Command for SleepIslands {
    type Out = ();
    fn apply(self, world: &mut World) {
        if self.0.is_empty() {
            return;
        }
        world.try_resource_scope(|world, mut state: Mut<CachedIslandSleepingSystemState>| {
            let (bodies, mut islands, mut contact_graph, mut constraint_graph) =
                state.0.get_mut(world).unwrap();

            let mut bodies_to_sleep = Vec::<(Entity, Sleeping)>::new();
            let mut colliders_to_sleep = Vec::<Entity>::new();

            // First, mark every island in this batch as sleeping and gather their bodies and colliders.
            for island_id in self.0 {
                if let Some(island) = islands.get_mut(island_id) {
                    if island.is_sleeping {
                        // The island is already sleeping, no need to sleep it again.
                        continue;
                    }

                    island.is_sleeping = true;

                    let mut body = island.head_body;

                    while let Some(entity) = body {
                        let Ok((body_island, _, colliders)) = bodies.get(entity) else {
                            body = None;
                            continue;
                        };

                        if let Some(colliders) = colliders {
                            colliders_to_sleep.extend(colliders);
                        }

                        bodies_to_sleep.push((entity, Sleeping));
                        body = body_island.next;
                    }
                }
            }

            // A pair may only be put to sleep once its other body is also asleep or immovable.
            let is_other_body_asleep = |other_body: Option<Entity>| -> bool {
                let Some(other) = other_body else {
                    return true;
                };
                match bodies.get(other) {
                    Ok((body_island, _, _)) => islands
                        .get(body_island.island_id)
                        .is_none_or(|island| island.is_sleeping),
                    Err(_) => true,
                }
            };

            // Transfer the contact pairs to the sleeping set, and remove touching contacts
            // from the constraint graph.
            for collider in colliders_to_sleep {
                contact_graph.sleep_entity_with(
                    collider,
                    |_graph, contact_pair| {
                        // Remove touching contacts from the constraint graph.
                        if !contact_pair.is_touching() || !contact_pair.generates_constraints() {
                            return;
                        }
                        if let (Some(body1), Some(body2)) = (contact_pair.body1, contact_pair.body2)
                        {
                            constraint_graph.remove_contact(contact_pair.contact_id, body1, body2);
                        }
                    },
                    // OVERMATCH PATCH: a miss stays active, so the narrow phase reads its inputs
                    // every step.
                    |edge, other| edge.is_touching() && is_other_body_asleep(other),
                );
            }

            // Batch insert `Sleeping` to the bodies.
            world.insert_batch(bodies_to_sleep);
        });
    }
}

#[derive(Resource)]
struct CachedIslandWakingSystemState(
    SystemState<(
        SQuery<(
            &'static BodyIslandNode,
            &'static mut SleepTimer,
            Option<&'static RigidBodyColliders>,
            Has<Sleeping>, // OVERMATCH PATCH
        )>,
        SResMut<PhysicsIslands>,
        SResMut<ContactGraph>,
        SResMut<ConstraintGraph>,
    )>,
);

/// A [`Command`] that wakes up a [`RigidBody`] and its [`PhysicsIsland`](super::PhysicsIsland) if it is [`Sleeping`].
pub struct WakeBody(pub Entity);

impl Command for WakeBody {
    type Out = Result;
    fn apply(self, world: &mut World) -> Result {
        if let Ok(entity) = world.get_entity(self.0) {
            if let Some(body_island) = entity.get::<BodyIslandNode>() {
                WakeIslands(vec![body_island.island_id]).apply(world);
                Ok(())
            } else {
                Err(format!(
                    "Tried to wake entity {:?} that is not a body or does not belong to an island",
                    self.0
                )
                .into())
            }
        } else {
            Err(format!("Tried to wake entity {:?} that does not exist", self.0).into())
        }
    }
}

/// A [`Command`] that wakes up the [`PhysicsIsland`](super::PhysicsIsland)s with the given IDs if they are sleeping.
pub struct WakeIslands(pub Vec<IslandId>);

impl Command for WakeIslands {
    type Out = ();
    fn apply(self, world: &mut World) {
        if self.0.is_empty() {
            return;
        }
        world.try_resource_scope(|world, mut state: Mut<CachedIslandWakingSystemState>| {
            let (mut bodies, mut islands, mut contact_graph, mut constraint_graph) =
                state.0.get_mut(world).unwrap();

            let mut bodies_to_wake = Vec::<Entity>::new();

            for island_id in self.0 {
                if let Some(island) = islands.get_mut(island_id) {
                    if !island.is_sleeping {
                        // The island is not sleeping, no need to wake it up.
                        continue;
                    }

                    island.is_sleeping = false;

                    let mut body = island.head_body;

                    while let Some(entity) = body {
                        // OVERMATCH PATCH: a broken link panics rather than leave the island
                        // partly awake.
                        let (body_island, mut sleep_timer, colliders, was_sleeping) =
                            bodies.get_mut(entity).expect(
                                "every linked island body retains BodyIslandNode and SleepTimer",
                            );

                        // Transfer the contact pairs to the awake set, and add touching contacts to the constraint graph.
                        if let Some(colliders) = colliders {
                            for collider in colliders {
                                contact_graph.wake_entity_with(collider, |_graph, contact_pair| {
                                    // Add touching contacts to the constraint graph.
                                    if !contact_pair.is_touching()
                                        || !contact_pair.generates_constraints()
                                    {
                                        return;
                                    }
                                    for _ in contact_pair.manifolds.iter() {
                                        constraint_graph.push_manifold(contact_pair);
                                    }
                                });
                            }
                        }

                        bodies_to_wake.push(entity);
                        body = body_island.next;
                        // OVERMATCH PATCH: a merged island's awake members keep their rest time.
                        if was_sleeping {
                            sleep_timer.0 = 0.0;
                        }
                    }
                }
            }

            // Remove `Sleeping` from the bodies.
            bodies_to_wake.into_iter().for_each(|entity| {
                world.entity_mut(entity).remove::<Sleeping>();
            });
        });
    }
}

#[cfg(feature = "2d")]
type ConstantForceChanges = Or<(
    Changed<ConstantForce>,
    Changed<ConstantTorque>,
    Changed<ConstantLinearAcceleration>,
    Changed<ConstantAngularAcceleration>,
    Changed<ConstantLocalForce>,
    Changed<ConstantLocalLinearAcceleration>,
)>;
#[cfg(feature = "3d")]
type ConstantForceChanges = Or<(
    Changed<ConstantForce>,
    Changed<ConstantTorque>,
    Changed<ConstantLinearAcceleration>,
    Changed<ConstantAngularAcceleration>,
    Changed<ConstantLocalForce>,
    Changed<ConstantLocalTorque>,
    Changed<ConstantLocalLinearAcceleration>,
    Changed<ConstantLocalAngularAcceleration>,
)>;

/// Removes the [`Sleeping`] component from sleeping bodies when properties like
/// position, rotation, velocity and external forces are changed by the user.
fn wake_on_changed(
    mut query: ParamSet<(
        // These could've been changed by physics too.
        // We need to ignore non-user changes.
        Query<
            (
                Ref<Position>,
                Ref<Rotation>,
                Ref<LinearVelocity>,
                Ref<AngularVelocity>,
                Ref<SleepTimer>,
                &BodyIslandNode,
            ),
            (
                With<Sleeping>,
                Or<(
                    Changed<Position>,
                    Changed<Rotation>,
                    Changed<LinearVelocity>,
                    Changed<AngularVelocity>,
                    Changed<SleepTimer>,
                )>,
            ),
        >,
        // These are not modified by the physics engine
        // and don't need special handling.
        Query<&BodyIslandNode, Or<(ConstantForceChanges, Changed<GravityScale>)>>,
    )>,
    mut awake_island_bit_vec: ResMut<AwakeIslandBitVec>,
    last_physics_tick: Res<LastPhysicsTick>,
    system_tick: SystemChangeTick,
) {
    let this_run = system_tick.this_run();

    for (pos, rot, lin_vel, ang_vel, sleep_timer, body_island) in &query.p0() {
        if is_changed_after_tick(pos, last_physics_tick.0, this_run)
            || is_changed_after_tick(rot, last_physics_tick.0, this_run)
            || is_changed_after_tick(lin_vel, last_physics_tick.0, this_run)
            || is_changed_after_tick(ang_vel, last_physics_tick.0, this_run)
            || is_changed_after_tick(sleep_timer, last_physics_tick.0, this_run)
        {
            awake_island_bit_vec.set_and_grow(body_island.island_id.0 as usize);
        }
    }

    for body_island in &query.p1() {
        awake_island_bit_vec.set_and_grow(body_island.island_id.0 as usize);
    }
}

/// Wakes up all sleeping [`PhysicsIsland`](super::PhysicsIsland)s. Triggered automatically when [`Gravity`] is changed.
fn wake_all_islands(mut commands: Commands, islands: Res<PhysicsIslands>) {
    let sleeping_islands: Vec<IslandId> = islands
        .iter()
        .filter_map(|island| island.is_sleeping.then_some(island.id))
        .collect();

    if !sleeping_islands.is_empty() {
        commands.queue(WakeIslands(sleeping_islands));
    }
}
