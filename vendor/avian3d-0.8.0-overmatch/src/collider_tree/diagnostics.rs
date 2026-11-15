use bevy::{
    diagnostic::DiagnosticPath,
    prelude::{ReflectResource, Resource},
    reflect::Reflect,
};
use core::time::Duration;

use crate::diagnostics::{PhysicsDiagnostics, impl_diagnostic_paths};

/// Diagnostics for [collider trees](crate::collider_tree).
#[derive(Resource, Debug, Default, Reflect)]
#[reflect(Resource, Debug)]
pub struct ColliderTreeDiagnostics {
    /// Time spent optimizing [collider trees](crate::collider_tree).
    pub optimize: Duration,
    /// Time spent updating AABBs and BVH nodes.
    pub update: Duration,
    // OVERMATCH PATCH: the moved-collider pass's examined count, the one observable of its prune.
    /// Colliders `update_moved_collider_aabbs` examined this step: those whose `Position`,
    /// `Rotation` or collider changed since its previous run (every collider when it cannot prune).
    pub moved_aabb_candidates: u32,
}

impl PhysicsDiagnostics for ColliderTreeDiagnostics {
    fn timer_paths(&self) -> Vec<(&'static DiagnosticPath, Duration)> {
        vec![(Self::OPTIMIZE, self.optimize), (Self::UPDATE, self.update)]
    }

    fn counter_paths(&self) -> Vec<(&'static DiagnosticPath, u32)> {
        vec![(Self::MOVED_AABB_CANDIDATES, self.moved_aabb_candidates)]
    }
}

impl_diagnostic_paths! {
    impl ColliderTreeDiagnostics {
        OPTIMIZE: "avian/collider_tree/optimize",
        UPDATE: "avian/collider_tree/update",
        MOVED_AABB_CANDIDATES: "avian/collider_tree/moved_aabb_candidates",
    }
}
