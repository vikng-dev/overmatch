//! Size metrics associated with a [`RigidBody`] and its colliders.
//!
//! See [`BodySizeMetrics`] for more information.
//!
//! [`RigidBody`]: crate::dynamics::rigid_body::RigidBody

use core::marker::PhantomData;
use core::sync::atomic::{AtomicU32, Ordering};

use bevy::{
    ecs::{entity::EntityHashSet, query::QueryEntityError, system::StaticSystemParam},
    prelude::*,
};

use crate::{
    collision::collider::{
        AnyCollider, ColliderContext,
        collider_hierarchy::{ColliderOf, RigidBodyColliders},
        collider_transform::ColliderTransform,
    },
    dynamics::rigid_body::mass_properties::components::ComputedCenterOfMass,
    math::ToRealPrecision,
    schedule::{PhysicsSchedule, PhysicsStepSystems},
};

/// Size metrics associated with a [`RigidBody`] and its colliders.
///
/// These can be used for various purposes, such as determining thresholds
/// for [Continuous Collision Detection (CCD)][CCD], [sleeping], and contact recycling.
///
/// The values are automatically computed and updated by the [`BodySizeMetricsPlugin`].
///
/// [`RigidBody`]: crate::dynamics::rigid_body::RigidBody
/// [CCD]: crate::dynamics::ccd
/// [sleeping]: crate::dynamics::sleeping
#[derive(Component, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct BodySizeMetrics {
    /// A conservative minimum thickness used by [Continuous Collision Detection (CCD)][CCD]
    /// to determine how far the body can move in a single timestep before it might start to
    /// tunnel through geometry.
    ///
    /// This is the minimum [`ccd_thickness`] of the colliders attached to the body,
    /// and typically corresponds to the minimum distance from the centroid
    /// of any given shape to its surface.
    ///
    /// [CCD]: crate::dynamics::ccd
    /// [`ccd_thickness`]: crate::collision::collider::AnyCollider::ccd_thickness_with_context
    pub ccd_thickness: f32,

    /// The maximum distance from the center of mass of the body to the surface
    /// of any of its colliders.
    ///
    /// Typically corresponds to the radius of the sphere formed by sweeping
    /// the body about its center of mass.
    ///
    /// This can be useful for determining the maximum velocity that a point
    /// on the body can have, which can be used for sleeping and contact recycling
    /// thresholds, for example.
    pub sweep_radius: f32,
}

impl Default for BodySizeMetrics {
    fn default() -> Self {
        Self {
            ccd_thickness: f32::INFINITY,
            sweep_radius: 0.0,
        }
    }
}

/// A plugin for computing and updating [`BodySizeMetrics`] for rigid bodies.
#[derive(Default)]
pub struct BodySizeMetricsPlugin<C: AnyCollider> {
    _phantom: PhantomData<C>,
}

impl<C: AnyCollider> Plugin for BodySizeMetricsPlugin<C> {
    fn build(&self, app: &mut App) {
        app.init_resource::<BodySizeMetricUpdates>(); // OVERMATCH PATCH

        // Update body size metrics before they are consumed by the solver and continuous collision
        // detection. Allowing ambiguities lets multiple collision backends coexist.
        app.add_systems(
            PhysicsSchedule,
            update_body_size_metrics::<C>
                .before(PhysicsStepSystems::Solver)
                .ambiguous_with_all(),
        );
    }
}

/// OVERMATCH PATCH: test instrumentation, the bodies whose metrics the last pass of
/// [`update_body_size_metrics`] recomputed. Only the game's guard reads it
/// (`src/tests/avian_body_size_metrics.rs`). Cost: one relaxed atomic add per recomputed body and
/// one resource write per step. Why it is not test-only: `OVERMATCH_PATCH.md`, "Body size
/// metrics once per body".
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BodySizeMetricUpdates(pub u32);

type BodySizeMetricsData = (
    &'static mut BodySizeMetrics,
    &'static RigidBodyColliders,
    &'static ComputedCenterOfMass,
);

fn update_body_size_metrics<C: AnyCollider>(
    mut bodies: Query<BodySizeMetricsData>,
    // Body size metrics only need updating when the center of mass
    // or one of the colliders changes.
    moved_centers: Query<Entity, (With<BodySizeMetrics>, Changed<ComputedCenterOfMass>)>,
    colliders: Query<(Entity, &C, &ColliderTransform)>,
    changed_colliders: Query<&ColliderOf, Or<(Changed<C>, Changed<ColliderTransform>)>>,
    context: StaticSystemParam<C::Context>,
    mut stale: Local<EntityHashSet>,
    mut updates: ResMut<BodySizeMetricUpdates>,
) {
    let context = context.into_inner();

    // Computes the minimum CCD thickness and maximum sweep radius over a body's colliders.
    let compute = |rb_colliders: &RigidBodyColliders, com: &ComputedCenterOfMass| {
        let mut ccd_thickness: f32 = f32::INFINITY;
        let mut sweep_radius: f32 = 0.0;

        for (entity, collider, collider_transform) in colliders.iter_many(rb_colliders).matched() {
            // Compute the CCD thickness
            let ctx = ColliderContext::new(entity, &context);
            let thickness = collider.ccd_thickness_with_context(ctx);
            ccd_thickness = ccd_thickness.min(thickness);

            // Compute the sweep radius
            let ctx = ColliderContext::new(entity, &context);
            let point = com.0 - collider_transform.translation;
            let distance_to_com = collider.max_distance_to_point_with_context(point.real(), ctx);
            sweep_radius = sweep_radius.max(distance_to_com);
        }

        BodySizeMetrics {
            ccd_thickness,
            sweep_radius,
        }
    };

    // OVERMATCH PATCH: a stale body is recomputed once, however many of its colliders changed.
    // Upstream recomputed it once per changed collider, each time over all its colliders. Each
    // stale body owns its metrics and reads only its own colliders, so they run on the compute
    // pool; a step with no stale body dispatches nothing.
    stale.clear();
    stale.extend(&moved_centers);
    stale.extend(changed_colliders.iter().map(|collider_of| collider_of.body));
    if stale.is_empty() {
        updates.0 = 0;
        return;
    }
    let recomputed = AtomicU32::new(0);
    bodies
        .par_iter_many_unique_mut(&*stale)
        .for_each(|body| match body {
            Ok((mut size_metrics, rb_colliders, com)) => {
                *size_metrics = compute(rb_colliders, com);
                recomputed.fetch_add(1, Ordering::Relaxed);
            }
            // A changed collider's body that is despawned, disabled or holds no metrics has none
            // to refresh.
            Err(QueryEntityError::NotSpawned(_) | QueryEntityError::QueryDoesNotMatch(..)) => {}
            Err(e @ QueryEntityError::AliasedMutability(_)) => {
                unreachable!("a set of distinct bodies cannot alias: {e}")
            }
        });
    updates.0 = recomputed.into_inner();
}
