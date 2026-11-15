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
    /// OVERMATCH PATCH: non-self leaf candidates visited before duplicate, layer and body
    /// filtering. A candidate can be visited from both endpoints.
    pub broad_phase_candidates: u32,
    /// Time spent updating contacts in the [narrow phase](crate::collision::narrow_phase).
    pub narrow_phase: Duration,
    /// The number of contacts.
    pub contact_count: u32,
    /// The number of contacts that were recycled from the previous timestep.
    pub recycled_count: u32,
    /// OVERMATCH PATCH: still, non-touching pairs visited with overlapping bounds
    /// (`narrow_phase::stationary_miss`).
    pub stationary_non_touching_pairs: u32,
    /// OVERMATCH PATCH: the geometry queries those pairs made, their inputs having changed.
    pub stationary_non_touching_queries: u32,
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
            (Self::RECYCLED_COUNT, self.recycled_count),
            (Self::BROAD_PHASE_CANDIDATES, self.broad_phase_candidates),
            (Self::STATIONARY_PAIRS, self.stationary_non_touching_pairs),
            (
                Self::STATIONARY_QUERIES,
                self.stationary_non_touching_queries,
            ),
        ]
    }
}

impl_diagnostic_paths! {
    impl CollisionDiagnostics {
        BROAD_PHASE: "avian/collision/broad_phase",
        NARROW_PHASE: "avian/collision/update_contacts",
        CONTACT_COUNT: "avian/collision/contact_count",
        RECYCLED_COUNT: "avian/collision/recycled_count",
        BROAD_PHASE_CANDIDATES: "avian/collision/broad_phase_candidates",
        STATIONARY_PAIRS: "avian/collision/stationary_non_touching_pairs",
        STATIONARY_QUERIES: "avian/collision/stationary_non_touching_queries",
    }
}
