use core::cell::Cell;
#[cfg(feature = "parallel")]
use core::cell::RefCell;

use crate::{
    collision::{
        collider::EnlargedAabb,
        contact_types::{
            ContactEdgeFlags, ContactId, PairManifolds, ReductionChurn, StationaryMiss,
            reduce_body_pair,
        },
    },
    data_structures::{bit_vec::BitVec, pair_key::PairKey},
    dynamics::solver::{
        constraint_graph::ConstraintGraph,
        islands::{BodyIslandNode, IslandId, PhysicsIslands, WakeIslands},
        joint_graph::JointGraph,
    },
    prelude::*,
};
use bevy::{
    ecs::{
        entity_disabling::Disabled,
        query::QueryData,
        system::{SystemParam, SystemParamItem, lifetimeless::Read},
    },
    prelude::*,
};
use thread_local::ThreadLocal;

#[derive(QueryData)]
struct ColliderQuery<C: AnyCollider> {
    entity: Entity,
    of: Option<Read<ColliderOf>>,
    shape: Ref<'static, C>,
    enlarged_aabb: Read<EnlargedAabb>,
    position: Read<Position>,
    rotation: Read<Rotation>,
    transform: Option<Read<ColliderTransform>>,
    layers: Read<CollisionLayers>,
    friction: Option<Read<Friction>>,
    restitution: Option<Read<Restitution>>,
    collision_margin: Option<Read<CollisionMargin>>,
    speculative_margin: Option<Read<SpeculativeMargin>>,
    is_sensor: Has<Sensor>,
}

#[derive(QueryData)]
struct RigidBodyQuery {
    entity: Entity,
    rb: Read<RigidBody>,
    position: Read<Position>,
    rotation: Read<Rotation>,
    center_of_mass: Read<ComputedCenterOfMass>,
    linear_velocity: Read<LinearVelocity>,
    angular_velocity: Read<AngularVelocity>,
    sleeping: Has<Sleeping>,
    // TODO: We should define these as purely collider components and not query for them here.
    friction: Option<Read<Friction>>,
    restitution: Option<Read<Restitution>>,
    collision_margin: Option<Read<CollisionMargin>>,
    speculative_margin: Option<Read<SpeculativeMargin>>,
    // OVERMATCH PATCH: body-pair contact reduction reads these.
    reducible: Has<ReducibleContacts>,
    sleep_threshold: Option<Read<SleepThreshold>>,
}

/// A system parameter for managing the narrow phase.
///
/// Responsibilities:
///
/// - Updates each active [`ContactPair`] in the [`ContactGraph`].
/// - Sends [collision events](crate::collision::collision_events) when colliders start or stop touching.
/// - Removes contact pairs from the [`ContactGraph`] when AABBs stop overlapping.
/// - Adds [`ContactManifold`]s to the [`ConstraintGraph`] when they are created.
/// - Removes [`ContactManifold`]s from the [`ConstraintGraph`] when they are destroyed.
#[derive(SystemParam)]
#[expect(missing_docs)]
pub struct NarrowPhase<'w, 's, C: AnyCollider> {
    collider_query: Query<'w, 's, ColliderQuery<C>, Without<ColliderDisabled>>,
    colliding_entities_query: Query<'w, 's, &'static mut CollidingEntities>,
    body_query: Query<'w, 's, RigidBodyQuery, Without<RigidBodyDisabled>>,
    body_islands:
        Query<'w, 's, &'static mut BodyIslandNode, Or<(With<Disabled>, Without<Disabled>)>>,
    pub contact_graph: ResMut<'w, ContactGraph>,
    pub joint_graph: ResMut<'w, JointGraph>,
    pub constraint_graph: ResMut<'w, ConstraintGraph>,
    pub islands: Option<ResMut<'w, PhysicsIslands>>,
    contact_status_bits: ResMut<'w, ContactStatusBits>,
    #[cfg(feature = "parallel")]
    thread_local_contact_status_bits: ResMut<'w, ThreadLocalContactStatusBits>,
    pub config: Res<'w, NarrowPhaseConfig>,
    default_friction: Res<'w, DefaultFriction>,
    default_restitution: Res<'w, DefaultRestitution>,
    length_unit: Res<'w, PhysicsLengthUnit>,
    // These are scaled by the length unit.
    default_speculative_margin: Local<'s, Scalar>,
    contact_tolerance: Local<'s, Scalar>,
    // OVERMATCH PATCH: owner-local tallies avoid contended per-pair atomics.
    miss_work: Local<'s, ThreadLocal<Cell<[u32; 2]>>>,
    pub(super) stationary_miss_work: Local<'s, [u32; 2]>,
    // OVERMATCH PATCH: reusable manifold buffers for body-pair reduction.
    reduction_entries: Local<'s, Vec<ReductionEntry>>,
    pub(super) reduction_churn: Local<'s, ReductionChurn>,
}

/// OVERMATCH PATCH: a reducible pair's manifolds, moved out for its body pair's reduction.
struct ReductionEntry {
    /// The two bodies, in ascending order: the group key.
    bodies: (Entity, Entity),
    /// Where the manifolds return: an index into the active pairs.
    pair_index: usize,
    /// The pair's manifold count before reduction.
    raw_count: usize,
    taken: PairManifolds,
}

impl core::borrow::Borrow<PairManifolds> for ReductionEntry {
    fn borrow(&self) -> &PairManifolds {
        &self.taken
    }
}

impl core::borrow::BorrowMut<PairManifolds> for ReductionEntry {
    fn borrow_mut(&mut self) -> &mut PairManifolds {
        &mut self.taken
    }
}

/// A bit vector for tracking contact status changes.
/// Set bits correspond to contact pairs that were either added or removed.
#[derive(Resource, Default, Deref, DerefMut)]
pub(super) struct ContactStatusBits(pub BitVec);

// TODO: We could just combine all the bit vectors into the first one
//       instead of having a separate `ContactStatusBits` resource.
/// A thread-local bit vector for tracking contact status changes.
/// Set bits correspond to contact pairs that were either added or removed.
///
/// The thread-local bit vectors are combined with the global [`ContactStatusBits`].
#[cfg(feature = "parallel")]
#[derive(Resource, Default, Deref, DerefMut)]
pub(super) struct ThreadLocalContactStatusBits(pub ThreadLocal<RefCell<BitVec>>);

impl<C: AnyCollider> NarrowPhase<'_, '_, C> {
    /// Updates the narrow phase.
    ///
    /// - Updates each active [`ContactPair`] in the [`ContactGraph`].
    /// - Sends [collision events](crate::collision::collision_events) when colliders start or stop touching.
    /// - Removes contact pairs from the [`ContactGraph`] when AABBs stop overlapping.
    /// - Adds [`ContactManifold`]s to the [`ConstraintGraph`] when they are created.
    /// - Removes [`ContactManifold`]s from the [`ConstraintGraph`] when they are destroyed.
    pub fn update<H: CollisionHooks>(
        &mut self,
        collision_started_writer: &mut MessageWriter<CollisionStart>,
        collision_ended_writer: &mut MessageWriter<CollisionEnd>,
        delta_secs: Scalar,
        hooks: &SystemParamItem<H>,
        context: &SystemParamItem<C::Context>,
        commands: &mut ParallelCommands,
    ) where
        for<'w, 's> SystemParamItem<'w, 's, H>: CollisionHooks,
    {
        // Cache default margins scaled by the length unit.
        if self.config.is_changed() || self.length_unit.is_changed() {
            *self.default_speculative_margin =
                self.length_unit.0 * self.config.default_speculative_margin;
            *self.contact_tolerance = self.length_unit.0 * self.config.contact_tolerance;
        }

        // Update contacts for all contact pairs.
        self.update_contacts::<H>(delta_secs, hooks, context, commands);

        // OVERMATCH PATCH: bounds each body pair's contacts before the constraint graph sees them.
        self.reduce_body_pair_contacts();

        // A later merge can retire an island ID before wakes are applied.
        let mut wake_bodies: Vec<Entity> = Vec::with_capacity(128);
        let mut sleeping_colliders = Vec::new();
        let mut contacts_to_link = Vec::new();

        // Process contact status changes, iterating over set bits serially to maintain determinism.
        //
        // Iterating over set bits is done efficiently with the "count trailing zeros" method:
        // https://lemire.me/blog/2018/02/21/iterating-over-set-bits-quickly/
        for (i, mut bits) in self.contact_status_bits.blocks().enumerate() {
            while bits != 0 {
                let trailing_zeros = bits.trailing_zeros();
                let contact_id = ContactId(i as u32 * 64 + trailing_zeros);

                let (contact_edge, contact_pair) = self
                    .contact_graph
                    .get_mut_by_id(contact_id)
                    .unwrap_or_else(|| panic!("Contact pair not found for {contact_id:?}"));

                // Three options:
                // 1. The AABBs are no longer overlapping, and the contact pair should be removed.
                // 2. The colliders started touching, and a collision started event should be sent.
                // 3. The colliders stopped touching, and a collision ended event should be sent.
                if contact_pair.aabbs_disjoint() {
                    // Send a collision ended event if the contact pair was touching.
                    let send_event = contact_edge
                        .flags
                        .contains(ContactEdgeFlags::TOUCHING | ContactEdgeFlags::CONTACT_EVENTS);
                    if send_event {
                        collision_ended_writer.write(CollisionEnd {
                            collider1: contact_pair.collider1,
                            collider2: contact_pair.collider2,
                            body1: contact_pair.body1,
                            body2: contact_pair.body2,
                        });
                    }

                    // Remove from `CollidingEntities`.
                    Self::remove_colliding_entities(
                        &mut self.colliding_entities_query,
                        contact_pair.collider1,
                        contact_pair.collider2,
                    );

                    let pair_key = PairKey::new(
                        contact_pair.collider1.index_u32(),
                        contact_pair.collider2.index_u32(),
                    );

                    if contact_pair.generates_constraints()
                        && let (Some(body1), Some(body2)) = (contact_pair.body1, contact_pair.body2)
                    {
                        let has_island = contact_edge.island.is_some();

                        // Remove the contact pair from the constraint graph.
                        for _ in 0..contact_edge.constraint_handles.len() {
                            self.constraint_graph.pop_manifold(
                                &mut self.contact_graph.edges,
                                contact_id,
                                body1,
                                body2,
                            );
                        }

                        // Unlink the contact pair from its island.
                        if has_island && let Some(islands) = &mut self.islands {
                            islands.remove_contact(
                                contact_id,
                                &mut self.body_islands,
                                &mut self.contact_graph.edges,
                                &self.joint_graph,
                            );
                        }
                    }

                    // Remove the contact edge from the contact graph.
                    self.contact_graph.remove_edge_by_id(&pair_key, contact_id);
                } else if contact_pair.collision_started() {
                    // Send collision started event.
                    if contact_edge.events_enabled() {
                        collision_started_writer.write(CollisionStart {
                            collider1: contact_pair.collider1,
                            collider2: contact_pair.collider2,
                            body1: contact_pair.body1,
                            body2: contact_pair.body2,
                        });
                    }

                    // Add to `CollidingEntities`.
                    Self::add_colliding_entities(
                        &mut self.colliding_entities_query,
                        contact_pair.collider1,
                        contact_pair.collider2,
                    );

                    // OVERMATCH PATCH: body-pair reduction may leave a touching pair no manifold.
                    debug_assert!(
                        !contact_pair.manifolds.is_empty()
                            || contact_pair
                                .flags
                                .contains(ContactPairFlags::REDUCIBLE_CONTACTS),
                        "Manifolds should not be empty when colliders start touching"
                    );

                    contact_edge.flags.set(ContactEdgeFlags::TOUCHING, true);
                    contact_pair
                        .flags
                        .set(ContactPairFlags::STARTED_TOUCHING, false);

                    if contact_pair.generates_constraints() {
                        // Add the contact pair to the constraint graph.
                        for _ in contact_pair.manifolds.iter() {
                            self.constraint_graph
                                .push_manifold(contact_edge, contact_pair);
                        }

                        contacts_to_link.push(contact_id);
                    }
                } else if contact_pair
                    .flags
                    .contains(ContactPairFlags::STOPPED_TOUCHING)
                {
                    // Send collision ended event.
                    if contact_edge.events_enabled() {
                        collision_ended_writer.write(CollisionEnd {
                            collider1: contact_pair.collider1,
                            collider2: contact_pair.collider2,
                            body1: contact_pair.body1,
                            body2: contact_pair.body2,
                        });
                    }

                    // Remove from `CollidingEntities`.
                    Self::remove_colliding_entities(
                        &mut self.colliding_entities_query,
                        contact_pair.collider1,
                        contact_pair.collider2,
                    );

                    debug_assert!(
                        contact_pair.manifolds.is_empty(),
                        "Manifolds should be empty when colliders stopped touching"
                    );

                    contact_edge.flags.set(ContactEdgeFlags::TOUCHING, false);
                    contact_pair
                        .flags
                        .set(ContactPairFlags::STOPPED_TOUCHING, false);

                    // Remove the contact pair from the constraint graph. OVERMATCH PATCH: the
                    // island link is tested on its own: a pair reduced to no manifold has one
                    // with no constraint handle.
                    if contact_pair.generates_constraints()
                        && (!contact_edge.constraint_handles.is_empty()
                            || contact_edge.island.is_some())
                        && let (Some(body1), Some(body2)) = (contact_pair.body1, contact_pair.body2)
                    {
                        for _ in 0..contact_edge.constraint_handles.len() {
                            self.constraint_graph.pop_manifold(
                                &mut self.contact_graph.edges,
                                contact_id,
                                body1,
                                body2,
                            );
                        }

                        // Unlink the contact pair from its island.
                        if let Some(islands) = &mut self.islands {
                            let island = islands.remove_contact(
                                contact_id,
                                &mut self.body_islands,
                                &mut self.contact_graph.edges,
                                &self.joint_graph,
                            );

                            // TODO: Do we need this?
                            if island.is_sleeping {
                                // Wake up the island if it was previously sleeping.
                                wake_bodies.push(
                                    island
                                        .head_body
                                        .expect("An island with a contact has a body"),
                                );
                            }
                        }
                    }
                } else if contact_pair.is_touching()
                    && contact_pair
                        .flags
                        .contains(ContactPairFlags::STARTED_GENERATING_CONSTRAINTS)
                {
                    // The contact pair should start generating constraints.
                    contact_pair
                        .flags
                        .set(ContactPairFlags::STARTED_GENERATING_CONSTRAINTS, false);

                    // Add the contact pair to the constraint graph.
                    for _ in contact_pair.manifolds.iter() {
                        self.constraint_graph
                            .push_manifold(contact_edge, contact_pair);
                    }

                    contacts_to_link.push(contact_id);
                } else if contact_pair.is_touching()
                    && contact_pair.generates_constraints()
                    && contact_pair.manifold_count_change > 0
                {
                    // The contact pair is still touching, but the manifold count has increased.
                    // Add the new manifolds to the constraint graph.
                    for _ in 0..contact_pair.manifold_count_change {
                        self.constraint_graph
                            .push_manifold(contact_edge, contact_pair);
                    }
                    contact_pair.manifold_count_change = 0;
                } else if contact_pair.is_touching()
                    && contact_pair.generates_constraints()
                    && contact_pair.manifold_count_change < 0
                {
                    // The contact pair is still touching, but the manifold count has decreased.
                    // Remove the excess manifolds from the constraint graph.
                    let removal_count = contact_pair.manifold_count_change.unsigned_abs() as usize;
                    contact_pair.manifold_count_change = 0;

                    if let (Some(body1), Some(body2)) = (contact_pair.body1, contact_pair.body2) {
                        for _ in 0..removal_count {
                            self.constraint_graph.pop_manifold(
                                &mut self.contact_graph.edges,
                                contact_id,
                                body1,
                                body2,
                            );
                        }
                    }
                }

                // Clear the least significant set bit.
                bits &= bits - 1;
            }
        }

        for contact_id in contacts_to_link {
            self.link_contact_to_island(contact_id, &mut wake_bodies, &mut sleeping_colliders);
        }

        // Dormant contacts leave no active manifold handles.
        sleeping_colliders.sort_unstable();
        sleeping_colliders.dedup();
        for collider in sleeping_colliders {
            self.contact_graph
                .sleep_entity_with(collider, |graph, pair| {
                    if pair.generates_constraints()
                        && let (Some(body1), Some(body2)) = (pair.body1, pair.body2)
                    {
                        let edge = graph
                            .get_edge_mut_by_id(pair.contact_id)
                            .expect("A transferred sleeping pair retains its contact edge");
                        for _ in 0..edge.constraint_handles.len() {
                            self.constraint_graph.pop_manifold(
                                &mut graph.edges,
                                pair.contact_id,
                                body1,
                                body2,
                            );
                        }
                    }
                });
        }

        if !wake_bodies.is_empty() {
            let mut islands_to_wake: Vec<IslandId> = wake_bodies
                .into_iter()
                .map(|body| {
                    self.body_islands
                        .get(body)
                        .expect("Contact bodies remain linked until narrow phase completes")
                        .island_id
                })
                .collect();
            islands_to_wake.sort_unstable();
            islands_to_wake.dedup();

            // Wake up the islands that were previously sleeping.
            commands.command_scope(|mut commands| {
                commands.queue(WakeIslands(islands_to_wake));
            });
        }
    }

    // Contact discovery alone is not a wake source. An awake non-static
    // participant supplies one; otherwise the new sleeping contact stays outside the solver.
    fn link_contact_to_island(
        &mut self,
        contact_id: ContactId,
        wake_bodies: &mut Vec<Entity>,
        sleeping_colliders: &mut Vec<Entity>,
    ) {
        // IslandPlugin is optional.
        let Some(islands) = &mut self.islands else {
            return;
        };
        let (_, pair) = self
            .contact_graph
            .get_by_id(contact_id)
            .expect("The contact being linked exists in the contact graph");
        let participants = [(pair.collider1, pair.body1), (pair.collider2, pair.body2)];
        // Disabled bodies are absent from body_query and supply no wake source.
        let has_awake_body = participants.iter().any(|(_, body)| {
            body.is_some_and(|body| {
                self.body_query
                    .get(body)
                    .is_ok_and(|body| !body.rb.is_static() && !body.sleeping)
            })
        });
        // A standalone collider pair need not belong to an island.
        if let Some(island) = islands.add_contact(
            contact_id,
            &mut self.body_islands,
            &mut self.contact_graph,
            &mut self.joint_graph,
        ) && island.is_sleeping
        {
            if has_awake_body {
                wake_bodies.push(
                    island
                        .head_body
                        .expect("An island with a contact has a body"),
                );
            } else {
                for (collider, body) in participants {
                    if body.is_some_and(|body| {
                        self.body_query
                            .get(body)
                            .is_ok_and(|body| !body.rb.is_static() && body.sleeping)
                    }) {
                        sleeping_colliders.push(collider);
                    }
                }
            }
        }
    }

    /// OVERMATCH PATCH: reduces the contacts between each pair of [`ReducibleContacts`] bodies
    /// (`contact_types::reduction`). Pairs group by their two bodies, whose groups own disjoint
    /// pairs and reduce in parallel; moving the manifolds out needs no unsafe aliasing. Runs
    /// after the pair update and before status bits are read, so the constraint graph sees only
    /// the reduced manifolds. A pair losing all of them stays touching, keeping its events and
    /// its island link.
    fn reduce_body_pair_contacts(&mut self) {
        *self.reduction_churn = ReductionChurn::default();
        let entries = &mut *self.reduction_entries;
        entries.clear();
        for (pair_index, pair) in self.contact_graph.active_pairs_mut().iter_mut().enumerate() {
            if pair.manifolds.is_empty()
                || !pair.flags.contains(ContactPairFlags::REDUCIBLE_CONTACTS)
                || pair.flags.contains(ContactPairFlags::MODIFY_CONTACTS)
                || !pair.generates_constraints()
            {
                continue;
            }
            let (body1, body2) = pair
                .body1
                .zip(pair.body2)
                .expect("a reducible pair has both bodies");
            let manifolds = core::mem::take(&mut pair.manifolds);
            entries.push(ReductionEntry {
                bodies: (body1.min(body2), body1.max(body2)),
                pair_index,
                raw_count: manifolds.len(),
                taken: PairManifolds {
                    colliders: (
                        pair.collider1.min(pair.collider2),
                        pair.collider1.max(pair.collider2),
                    ),
                    reversed: body2 < body1,
                    manifolds,
                },
            });
        }
        if entries.is_empty() {
            return;
        }
        entries.sort_unstable_by_key(|entry| entry.bodies);

        let unit = self.length_unit.0;
        let bodies = &self.body_query;
        // A landing is any point closing past the slower of the two bodies' rest speeds, the
        // comparison a contact reader makes per side (`Sleeping` compares the same threshold).
        let rest = |body: Entity| {
            bodies
                .get(body)
                .ok()
                .and_then(|body| body.sleep_threshold.map(|t| t.linear as Scalar * unit))
        };
        let mut groups: Vec<(Scalar, ReductionChurn, &mut [ReductionEntry])> = entries
            .chunk_by_mut(|a, b| a.bodies == b.bodies)
            .map(|group| {
                let (first, second) = group[0].bodies;
                let rest_speed = [first, second]
                    .into_iter()
                    .filter_map(&rest)
                    .fold(Scalar::INFINITY, Scalar::min);
                (rest_speed, ReductionChurn::default(), group)
            })
            .collect();
        crate::utils::par_for_each(&mut groups, 8, |_, (rest_speed, churn, group)| {
            *churn = reduce_body_pair(group, *rest_speed, unit);
        });
        for (_, churn, _) in groups {
            *self.reduction_churn += churn;
        }

        let pairs = self.contact_graph.active_pairs_mut();
        for entry in entries.drain(..) {
            let pair = &mut pairs[entry.pair_index];
            let removed = entry.raw_count - entry.taken.manifolds.len();
            pair.manifolds = entry.taken.manifolds;
            if removed > 0 {
                pair.manifold_count_change -= removed as i16;
                if pair.manifold_count_change != 0 {
                    self.contact_status_bits.set(pair.contact_id.0 as usize);
                }
            }
        }
    }

    /// Adds the colliding entities to their respective [`CollidingEntities`] components.
    fn add_colliding_entities(
        query: &mut Query<&mut CollidingEntities>,
        entity1: Entity,
        entity2: Entity,
    ) {
        if let Ok(mut colliding_entities1) = query.get_mut(entity1) {
            colliding_entities1.insert(entity2);
        }
        if let Ok(mut colliding_entities2) = query.get_mut(entity2) {
            colliding_entities2.insert(entity1);
        }
    }

    /// Removes the colliding entities from their respective [`CollidingEntities`] components.
    fn remove_colliding_entities(
        query: &mut Query<&mut CollidingEntities>,
        entity1: Entity,
        entity2: Entity,
    ) {
        if let Ok(mut colliding_entities1) = query.get_mut(entity1) {
            colliding_entities1.remove(&entity2);
        }
        if let Ok(mut colliding_entities2) = query.get_mut(entity2) {
            colliding_entities2.remove(&entity1);
        }
    }

    /// Updates contacts for all contact pairs in the [`ContactGraph`].
    ///
    /// Also updates the [`ContactStatusBits`] resource to track status changes for each contact pair.
    /// Set bits correspond to contact pairs that were either added or removed,
    /// while unset bits correspond to contact pairs that were persisted.
    ///
    /// The order of contact pairs is preserved.
    fn update_contacts<H: CollisionHooks>(
        &mut self,
        delta_secs: Scalar,
        hooks: &SystemParamItem<H>,
        collider_context: &SystemParamItem<C::Context>,
        par_commands: &mut ParallelCommands,
    ) where
        for<'w, 's> SystemParamItem<'w, 's, H>: CollisionHooks,
    {
        self.miss_work.iter_mut().for_each(|work| work.set([0, 0]));

        // Contact bit vecs must be sized based on the full contact capacity,
        // not the number of active contact pairs, because pair indices
        // are unstable and can be invalidated when pairs are removed.
        let bit_count = self.contact_graph.edges.raw_edges().len();

        // Clear the bit vector used to track status changes for each contact pair.
        self.contact_status_bits.set_bit_count_and_clear(bit_count);

        #[cfg(feature = "parallel")]
        self.thread_local_contact_status_bits
            .iter_mut()
            .for_each(|context| {
                let bit_vec_mut = &mut context.borrow_mut();
                bit_vec_mut.set_bit_count_and_clear(bit_count);
            });

        // Compute contacts for all contact pairs in parallel or serially
        // based on the `parallel` feature.
        //
        // Constraints are also generated for each contact pair that is touching
        // or expected to start touching.
        //
        // If parallelism is used, status changes are written to thread-local bit vectors,
        // where set bits correspond to contact pairs that were added or removed.
        // At the end, the bit vectors are combined using bit-wise OR.
        //
        // If parallelism is not used, status changes are instead written
        // directly to the global bit vector.
        //
        // TODO: An alternative to thread-local bit vectors could be to have one larger bit vector
        //       and to chunk it into smaller bit vectors for each thread. Might not be any faster though.
        crate::utils::par_for_each(self.contact_graph.active_pairs_mut(), 64, |_i, contacts| {
            let contact_id = contacts.contact_id.0 as usize;

            #[cfg(not(feature = "parallel"))]
            let status_change_bits = &mut self.contact_status_bits;

            // TODO: Move this out of the chunk iteration? Requires refactoring `par_for_each!`.
            #[cfg(feature = "parallel")]
            // Get the thread-local narrow phase context.
            let mut thread_context = self
                .thread_local_contact_status_bits
                .get_or(|| {
                    // No thread-local bit vector exists for this thread yet.
                    // Create a new one with the same capacity as the global bit vector.
                    let mut contact_status_bits = BitVec::new(bit_count);
                    contact_status_bits.set_bit_count_and_clear(bit_count);
                    RefCell::new(contact_status_bits)
                })
                .borrow_mut();
            #[cfg(feature = "parallel")]
            let status_change_bits = &mut *thread_context;

            // Get the colliders for the contact pair.
            let Ok([collider1, collider2]) = self
                .collider_query
                .get_many([contacts.collider1, contacts.collider2])
            else {
                return;
            };

            // OVERMATCH PATCH: stationarity is evaluated before this pair's geometry query.
            // Standalone colliders are static. A body-query gap is unknown, not stationary.
            let body1_bundle = collider1
                .of
                .and_then(|&ColliderOf { body }| self.body_query.get(body).ok());
            let body2_bundle = collider2
                .of
                .and_then(|&ColliderOf { body }| self.body_query.get(body).ok());
            let stationary_miss = !contacts.is_touching()
                && [
                    (collider1.of, body1_bundle.as_ref()),
                    (collider2.of, body2_bundle.as_ref()),
                ]
                .into_iter()
                .all(|(of, body)| {
                    of.is_none()
                        || body.is_some_and(|body| {
                            body.rb.is_static()
                                || body.sleeping
                                || (body.linear_velocity.0 == LinearVelocity::ZERO.0
                                    && body.angular_velocity.0 == AngularVelocity::ZERO.0)
                        })
                });
            let work = self.miss_work.get_or(|| Cell::new([0, 0]));
            if stationary_miss {
                let [pairs, queries] = work.get();
                work.set([pairs + 1, queries]);
            }

            // Check if the AABBs of the colliders still overlap and the contact pair is valid.
            let overlap = collider1.enlarged_aabb.intersects(collider2.enlarged_aabb);

            // Also check if the collision layers are still compatible and the contact pair is valid.
            // TODO: Ideally, we would have fine-grained change detection for `CollisionLayers`
            //       rather than checking it for every pair here.
            if !overlap || !collider1.layers.interacts_with(*collider2.layers) {
                // The AABBs no longer overlap. The contact pair should be removed.
                contacts.flags.set(ContactPairFlags::DISJOINT_AABB, true);
                status_change_bits.set(contact_id);
            } else {
                // The AABBs overlap. Compute contacts.

                // The rigid body's friction, restitution, collision margin, and speculative margin
                // will be used if the collider doesn't have them specified.
                let (
                    is_static1,
                    collider_offset1,
                    world_com1,
                    mut lin_vel1,
                    ang_vel1,
                    rb_friction1,
                    rb_collision_margin1,
                    rb_speculative_margin1,
                ) = body1_bundle
                    .as_ref()
                    .map(|body| {
                        (
                            body.rb.is_static(),
                            collider1.position.0 - body.position.0,
                            body.rotation * body.center_of_mass.0,
                            body.linear_velocity.0,
                            body.angular_velocity.0,
                            body.friction,
                            body.collision_margin,
                            body.speculative_margin,
                        )
                    })
                    .unwrap_or_default();
                let (
                    is_static2,
                    collider_offset2,
                    world_com2,
                    mut lin_vel2,
                    ang_vel2,
                    rb_friction2,
                    rb_collision_margin2,
                    rb_speculative_margin2,
                ) = body2_bundle
                    .as_ref()
                    .map(|body| {
                        (
                            body.rb.is_static(),
                            collider2.position.0 - body.position.0,
                            body.rotation * body.center_of_mass.0,
                            body.linear_velocity.0,
                            body.angular_velocity.0,
                            body.friction,
                            body.collision_margin,
                            body.speculative_margin,
                        )
                    })
                    .unwrap_or_default();

                // Store these to avoid having to query for the bodies
                // when processing status changes for the constraint graph.
                contacts.flags.set(ContactPairFlags::STATIC1, is_static1);
                contacts.flags.set(ContactPairFlags::STATIC2, is_static2);

                // If either collider is a sensor or either body is disabled, no constraints should be generated.
                let is_disabled = body1_bundle.is_none()
                    || body2_bundle.is_none()
                    || collider1.is_sensor
                    || collider2.is_sensor;

                if !is_disabled && !contacts.generates_constraints() {
                    // This can happen when a sensor is removed, or when a collider is attached to a body.
                    contacts
                        .flags
                        .set(ContactPairFlags::STARTED_GENERATING_CONSTRAINTS, true);
                    status_change_bits.set(contact_id);
                }

                contacts
                    .flags
                    .set(ContactPairFlags::GENERATE_CONSTRAINTS, !is_disabled);
                // OVERMATCH PATCH: both bodies must be non-static and allow it.
                let reducible = [&body1_bundle, &body2_bundle].into_iter().all(|body| {
                    body.as_ref()
                        .is_some_and(|body| body.reducible && !body.rb.is_static())
                });
                contacts.flags.set(
                    ContactPairFlags::REDUCIBLE_CONTACTS,
                    contacts.body1.is_some() && contacts.body2.is_some() && reducible,
                );

                // Get combined friction and restitution coefficients of the colliders
                // or the bodies they are attached to. Fall back to the global defaults.
                let friction = collider1
                    .friction
                    .or(rb_friction1)
                    .copied()
                    .unwrap_or(self.default_friction.0)
                    .combine(
                        collider2
                            .friction
                            .or(rb_friction2)
                            .copied()
                            .unwrap_or(self.default_friction.0),
                    )
                    .dynamic_coefficient;
                let restitution = collider1
                    .restitution
                    .copied()
                    .unwrap_or(self.default_restitution.0)
                    .combine(
                        collider2
                            .restitution
                            .copied()
                            .unwrap_or(self.default_restitution.0),
                    )
                    .coefficient;

                // Use the collider's own collision margin if specified, and fall back to the body's
                // collision margin.
                //
                // The collision margin adds artificial thickness to colliders for performance
                // and stability. See the `CollisionMargin` documentation for more details.
                let collision_margin1 = collider1
                    .collision_margin
                    .or(rb_collision_margin1)
                    .map_or(0.0, |margin| margin.0);
                let collision_margin2 = collider2
                    .collision_margin
                    .or(rb_collision_margin2)
                    .map_or(0.0, |margin| margin.0);
                let collision_margin_sum = collision_margin1 + collision_margin2;

                // Use the collider's own speculative margin if specified, and fall back to the body's
                // speculative margin.
                //
                // The speculative margin is used to predict contacts that might happen during the frame.
                // This is used for speculative collision. See the CCD and `SpeculativeMargin` documentation
                // for more details.
                let speculative_margin1 = collider1
                    .speculative_margin
                    .map_or(rb_speculative_margin1.map(|margin| margin.0), |margin| {
                        Some(margin.0)
                    });
                let speculative_margin2 = collider2
                    .speculative_margin
                    .map_or(rb_speculative_margin2.map(|margin| margin.0), |margin| {
                        Some(margin.0)
                    });

                let relative_linear_velocity: Vector;

                // Compute the effective speculative margin, clamping it based on velocities and the maximum bound.
                let effective_speculative_margin = {
                    let speculative_margin1 =
                        speculative_margin1.unwrap_or(*self.default_speculative_margin);
                    let speculative_margin2 =
                        speculative_margin2.unwrap_or(*self.default_speculative_margin);
                    let inv_delta_secs = delta_secs.recip();

                    // Clamp velocities to the maximum speculative margins.
                    if speculative_margin1 < Scalar::MAX {
                        lin_vel1 = lin_vel1.clamp_length_max(speculative_margin1 * inv_delta_secs);
                    }
                    if speculative_margin2 < Scalar::MAX {
                        lin_vel2 = lin_vel2.clamp_length_max(speculative_margin2 * inv_delta_secs);
                    }

                    // Compute the effective margin based on how much the bodies
                    // are expected to move relative to each other.
                    relative_linear_velocity = lin_vel2 - lin_vel1;
                    // OVERMATCH PATCH: contact_tolerance floors the margin for both the query
                    // and point pruning, including a resting pair with no relative velocity.
                    (delta_secs * relative_linear_velocity.length()).max(*self.contact_tolerance)
                };

                let max_contact_distance = effective_speculative_margin + collision_margin_sum;

                // Custom backends may read arbitrary Context state, so only the built-in
                // backend has a geometry-only cache key.
                #[cfg(feature = "default-collider")]
                let local_geometry =
                    core::any::TypeId::of::<C>() == core::any::TypeId::of::<Collider>();
                #[cfg(not(feature = "default-collider"))]
                let local_geometry = false;
                let stationary_state =
                    (stationary_miss && local_geometry).then(|| StationaryMiss {
                        poses: [
                            (*collider1.position, *collider1.rotation),
                            (*collider2.position, *collider2.rotation),
                        ],
                        shapes: [
                            collider1.shape.last_changed(),
                            collider2.shape.last_changed(),
                        ],
                        owners: [
                            body1_bundle.as_ref().map(|body| body.entity),
                            body2_bundle.as_ref().map(|body| body.entity),
                        ],
                        collision_margin: collision_margin_sum,
                        speculative_margin: effective_speculative_margin,
                        relative_linear_velocity,
                        angular_velocities: [AngularVelocity(ang_vel1), AngularVelocity(ang_vel2)],
                        anchor_inputs: [collider_offset1, collider_offset2, world_com1, world_com2],
                        delta_secs,
                    });
                if stationary_state.is_some()
                    && contacts.stationary_miss.as_deref() == stationary_state.as_ref()
                {
                    // Bounds/layers and constraint eligibility were checked above. An unchanged
                    // miss has no manifolds, impulses or contact transitions to update.
                    return;
                }
                contacts.stationary_miss = None;

                let was_touching = contacts.flags.contains(ContactPairFlags::TOUCHING);

                // Save the old manifolds for warm starting.
                let old_manifolds = contacts.manifolds.clone();

                // TODO: It'd be good to persist the manifolds and let Parry match contacts.
                //       This isn't currently done because it requires using Parry's contact manifold type.
                // Compute the contact manifolds using the effective speculative margin.
                let context = ContactManifoldContext::new(
                    collider1.entity,
                    collider2.entity,
                    collider_context,
                );
                if stationary_miss {
                    let [pairs, queries] = work.get();
                    work.set([pairs, queries + 1]);
                }
                collider1.shape.contact_manifolds_with_context(
                    &*collider2.shape,
                    collider1.position.0,
                    *collider1.rotation,
                    collider2.position.0,
                    *collider2.rotation,
                    max_contact_distance,
                    &mut contacts.manifolds,
                    context,
                );

                // Transform and prune contact data.
                contacts.manifolds.retain_mut(|manifold| {
                    // Set the initial surface properties.
                    manifold.friction = friction;
                    manifold.restitution = restitution;
                    #[cfg(feature = "2d")]
                    {
                        manifold.tangent_speed = 0.0;
                    }
                    #[cfg(feature = "3d")]
                    {
                        manifold.tangent_velocity = Vector::ZERO;
                    }
                    // OVERMATCH PATCH: compliance belongs to this step's hook, never carried over.
                    manifold.compliance = None;

                    let normal = manifold.normal;

                    // Transform and prune contact points.
                    manifold.retain_points_mut(|point| {
                        // Transform contact points to be relative to the centers of mass of the bodies.
                        point.anchor1 = point.anchor1 + collider_offset1 - world_com1;
                        point.anchor2 = point.anchor2 + collider_offset2 - world_com2;

                        // Add the collision margin to the penetration depth.
                        point.penetration += collision_margin_sum;

                        // Compute the relative velocity along the contact normal.
                        #[cfg(feature = "2d")]
                        let relative_velocity = relative_linear_velocity
                            + ang_vel2 * point.anchor2.perp()
                            - ang_vel1 * point.anchor1.perp();
                        #[cfg(feature = "3d")]
                        let relative_velocity = relative_linear_velocity
                            + ang_vel2.cross(point.anchor2)
                            - ang_vel1.cross(point.anchor1);
                        let normal_speed = relative_velocity.dot(normal);
                        point.normal_speed = normal_speed;

                        // Keep the contact if (1) the separation distance is below the required threshold,
                        // or if (2) the bodies are expected to come into contact within the next time step.
                        -point.penetration < effective_speculative_margin || {
                            let delta_distance = normal_speed * delta_secs;
                            delta_distance - point.penetration < effective_speculative_margin
                        }
                    });

                    // Prune extra contact points.
                    #[cfg(feature = "3d")]
                    if manifold.points.len() > 4 {
                        manifold.prune_points();
                    }

                    !manifold.points.is_empty()
                });

                // Check if the colliders are now touching.
                let geometry_miss = contacts.manifolds.is_empty();
                let mut touching = !geometry_miss;

                if touching && contacts.flags.contains(ContactPairFlags::MODIFY_CONTACTS) {
                    par_commands.command_scope(|mut commands| {
                        touching = hooks.modify_contacts(contacts, &mut commands);
                    });
                    if !touching {
                        contacts.manifolds.clear();
                    }
                }

                contacts.flags.set(ContactPairFlags::TOUCHING, touching);
                // A hook-rejected contact must be reconsidered when its outside state changes.
                if geometry_miss {
                    contacts.stationary_miss = stationary_state.map(Box::new);
                }

                // TODO: Manifold reduction

                // TODO: This condition is pretty arbitrary, mainly to skip dense trimeshes.
                //       If we let Parry handle contact matching, this wouldn't be needed.
                if contacts.manifolds.len() <= 4 && self.config.match_contacts {
                    // TODO: Cache this?
                    let distance_threshold = 0.1 * self.length_unit.0;

                    // OVERMATCH PATCH: choose across all old manifolds once; later manifolds
                    // must not overwrite a closer predecessor from an earlier one.
                    for manifold in contacts.manifolds.iter_mut() {
                        manifold.match_contacts(
                            old_manifolds
                                .iter()
                                .flat_map(|manifold| manifold.points.iter()),
                            distance_threshold,
                        );
                    }
                }

                // Record how many manifolds were added or removed.
                // This is used to add or remove contact constraints in the `ConstraintGraph` for persistent contacts.
                contacts.manifold_count_change =
                    (contacts.manifolds.len() as i32 - old_manifolds.len() as i32) as i16;

                // TODO: For unmatched contacts, apply any leftover impulses from the previous frame.

                if touching && !was_touching {
                    // The colliders started touching.
                    contacts.flags.set(ContactPairFlags::STARTED_TOUCHING, true);
                    status_change_bits.set(contact_id);
                } else if !touching && was_touching {
                    // The colliders stopped touching.
                    contacts.flags.set(ContactPairFlags::STOPPED_TOUCHING, true);
                    status_change_bits.set(contact_id);
                } else if contacts.manifold_count_change != 0 {
                    // The manifold count changed, but the colliders are still touching.
                    // This is used to add or remove contact constraints in the `ConstraintGraph`.
                    status_change_bits.set(contact_id);
                }
            };
        });

        *self.stationary_miss_work = self.miss_work.iter_mut().fold([0, 0], |sum, work| {
            let work = work.get();
            [sum[0] + work[0], sum[1] + work[1]]
        });

        #[cfg(feature = "parallel")]
        {
            // Combine the thread-local bit vectors serially using bit-wise OR.
            self.thread_local_contact_status_bits
                .iter_mut()
                .for_each(|context| {
                    let contact_status_bits = context.borrow();
                    self.contact_status_bits.or(&contact_status_bits);
                });
        }
    }
}
