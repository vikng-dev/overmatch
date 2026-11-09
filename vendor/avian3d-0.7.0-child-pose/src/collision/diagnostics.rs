use bevy::{
    diagnostic::DiagnosticPath,
    prelude::{ReflectResource, Resource},
    reflect::Reflect,
};
use core::time::Duration;

use crate::diagnostics::{PhysicsDiagnostics, impl_diagnostic_paths};

/// Diagnostics for collision detection.
#[derive(Resource, Debug, Default, Reflect)]
#[reflect(Resource, Debug)]
pub struct CollisionDiagnostics {
    /// Time spent finding potential collision pairs in the [broad phase](crate::collision::broad_phase).
    pub broad_phase: Duration,
    /// Non-self leaf candidates visited before duplicate, layer and body filtering.
    /// A candidate can be visited from both endpoints.
    pub broad_phase_candidates: u32,
    /// Time spent updating contacts in the [narrow phase](crate::collision::narrow_phase).
    pub narrow_phase: Duration,
    /// The number of contacts.
    pub contact_count: u32,
    // OVERMATCH PATCH: active misses, before geometry, with both owners static,
    // sleeping or exactly zero linear/angular velocity. Body-query gaps are excluded.
    /// Stationary non-touching active pairs examined by narrow phase this step.
    pub stationary_non_touching_pairs: u32,
    /// Geometry-query calls among those pairs (after bounds/layer admission).
    pub stationary_non_touching_queries: u32,
    // OVERMATCH PATCH: body-pair reduction's groups and warm-start selection counters.
    /// Body-pair groups of at least two manifolds offered for reduction this step.
    pub reduction_groups: u32,
    /// Of those, groups left whole because a point landed.
    pub reduction_landing_whole: u32,
    /// Groups left whole because they spanned more support normals than the ceiling.
    pub reduction_ceiling_whole: u32,
    /// Groups left whole because a point they would remove overlapped past the slop.
    pub reduction_overlap_whole: u32,
    /// Points holding a matched warm start that body-pair reduction removed this step.
    pub reduction_warm_points_removed: u32,
    /// Points body-pair reduction kept this step with no warm start.
    pub reduction_cold_points_kept: u32,
}

impl PhysicsDiagnostics for CollisionDiagnostics {
    fn timer_paths(&self) -> Vec<(&'static DiagnosticPath, Duration)> {
        vec![
            (Self::BROAD_PHASE, self.broad_phase),
            (Self::NARROW_PHASE, self.narrow_phase),
        ]
    }

    fn counter_paths(&self) -> Vec<(&'static DiagnosticPath, u32)> {
        vec![
            (Self::CONTACT_COUNT, self.contact_count),
            (Self::BROAD_PHASE_CANDIDATES, self.broad_phase_candidates),
            (
                Self::STATIONARY_NON_TOUCHING_PAIRS,
                self.stationary_non_touching_pairs,
            ),
            (
                Self::STATIONARY_NON_TOUCHING_QUERIES,
                self.stationary_non_touching_queries,
            ),
            (Self::REDUCTION_GROUPS, self.reduction_groups),
            (Self::REDUCTION_LANDING_WHOLE, self.reduction_landing_whole),
            (Self::REDUCTION_CEILING_WHOLE, self.reduction_ceiling_whole),
            (Self::REDUCTION_OVERLAP_WHOLE, self.reduction_overlap_whole),
            (
                Self::REDUCTION_WARM_POINTS_REMOVED,
                self.reduction_warm_points_removed,
            ),
            (
                Self::REDUCTION_COLD_POINTS_KEPT,
                self.reduction_cold_points_kept,
            ),
        ]
    }
}

impl_diagnostic_paths! {
    impl CollisionDiagnostics {
        BROAD_PHASE: "avian/collision/broad_phase",
        NARROW_PHASE: "avian/collision/update_contacts",
        BROAD_PHASE_CANDIDATES: "avian/collision/broad_phase_candidates",
        CONTACT_COUNT: "avian/collision/contact_count",
        STATIONARY_NON_TOUCHING_PAIRS: "avian/collision/stationary_non_touching_pairs",
        STATIONARY_NON_TOUCHING_QUERIES: "avian/collision/stationary_non_touching_queries",
        REDUCTION_GROUPS: "avian/collision/reduction_groups",
        REDUCTION_LANDING_WHOLE: "avian/collision/reduction_landing_whole",
        REDUCTION_CEILING_WHOLE: "avian/collision/reduction_ceiling_whole",
        REDUCTION_OVERLAP_WHOLE: "avian/collision/reduction_overlap_whole",
        REDUCTION_WARM_POINTS_REMOVED: "avian/collision/reduction_warm_points_removed",
        REDUCTION_COLD_POINTS_KEPT: "avian/collision/reduction_cold_points_kept",
    }
}
