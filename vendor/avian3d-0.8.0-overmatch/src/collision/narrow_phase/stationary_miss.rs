//! OVERMATCH PATCH (module): the stationary non-touching pair cache.
//!
//! A pair of built-in colliders whose geometry query found no contact while both sides held still
//! keeps every input that query and its point pruning read, as a [`StationaryMiss`] on the pair.
//! While the pair stays non-touching and still, a visit whose inputs equal the kept ones skips the
//! query: the query is a pure function of them, so it would miss again. Any changed input, touching
//! geometry or a hook-rejected contact queries as upstream does.
//!
//! A side holds still when its collider is standalone (static), or its body is static, sleeping or
//! at zero linear and angular velocity. A side whose body the narrow phase reads as absent is
//! unknown, never still. Sleep alone is no input: a wake changes no pose, shape or velocity, so it
//! cannot change the result; a wake that moves the body changes its inputs.
//!
//! Only the built-in collider ([`Collider`]) is cached: a custom backend's `Context` may feed its
//! query state the key does not hold. The key is never serialized; a new or deserialized pair has
//! none, and pair removal drops it. Bounds, layers and constraint eligibility are decided before the
//! key is compared, every step.
//!
//! Counts, same-input comparison and reproduction: the game's `upstream/avian-still-pairs.md`.

use core::cell::Cell;

use bevy::ecs::{change_detection::Tick, entity::Entity};
use thread_local::ThreadLocal;

use crate::prelude::*;

/// Every input of a pair's geometry query and point pruning, beside the shapes themselves: the
/// colliders' world poses, their shapes' change ticks and owners, the summed collision margin, the
/// speculative distance (contact tolerance and any speculative CCD included), the bodies' relative
/// linear velocity and angular velocities, the anchor inputs (collider offsets from their bodies,
/// rotated centers of mass) and the step length.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StationaryMiss {
    pub poses: [(Position, Rotation); 2],
    pub shapes: [Tick; 2],
    pub owners: [Option<Entity>; 2],
    pub collision_margin: f32,
    pub speculative_distance: f32,
    pub relative_linear_velocity: Vector,
    pub angular_velocities: [AngularVelocity; 2],
    pub anchor_inputs: [Vector; 4],
    pub delta_secs: f32,
}

/// Whether the narrow phase's collider type is the built-in one, whose query reads no `Context`.
pub(super) fn cacheable<C: AnyCollider>() -> bool {
    #[cfg(feature = "default-collider")]
    return core::any::TypeId::of::<C>() == core::any::TypeId::of::<Collider>();
    #[cfg(not(feature = "default-collider"))]
    return false;
}

/// This step's tallies, per thread: still non-touching pairs visited, and the geometry queries they
/// made. Summed into `CollisionDiagnostics::{stationary_non_touching_pairs,
/// stationary_non_touching_queries}` once per step.
#[derive(Default)]
pub(super) struct MissWork(ThreadLocal<Cell<[u32; 2]>>);

impl MissWork {
    /// Counts a still non-touching pair visited, or a geometry query such a pair makes; the query
    /// count sits beside the query it counts.
    pub(super) fn count(&self, pair: bool, query: bool) {
        if pair || query {
            let work = self.0.get_or_default();
            let [pairs, queries] = work.get();
            work.set([pairs + u32::from(pair), queries + u32::from(query)]);
        }
    }

    /// The step's `[pairs, queries]`, every thread's tally zeroed for the next.
    pub(super) fn take(&mut self) -> [u32; 2] {
        self.0.iter_mut().fold([0, 0], |[pairs, queries], work| {
            let [p, q] = work.take();
            [pairs + p, queries + q]
        })
    }
}
