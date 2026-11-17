//! Contact-pass visits from Avian's public graph, at each pass boundary. The diagnostics consumer
//! needs dispatched constraints, including zero-restitution no-ops, rather than contact points.

use avian3d::dynamics::solver::constraint_graph::{COLOR_OVERFLOW_INDEX, ConstraintGraph};
use avian3d::dynamics::solver::schedule::SubstepSolverSystems;
use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::tasks::ComputeTaskPool;

/// Graph-length readers only: surface-velocity writes leave every count invariant.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VisitCounts;

/// Last physics step's constraint visits, split by Avian's serial/parallel dispatch decision.
#[derive(Resource, Default)]
pub struct ContactVisits {
    pub serial: u32,
    pub parallel: u32,
}

/// Installs counters at the public pass boundaries; graph lengths are read when that pass ends,
/// and the next pass cannot start before its count is recorded. The final snapshot precedes
/// impulse writeback, which takes the constraint graph mutably.
pub fn plugin(app: &mut App) {
    app.init_resource::<ContactVisits>()
        .add_systems(PhysicsSchedule, reset.in_set(PhysicsStepSystems::First))
        .add_systems(
            PhysicsSchedule,
            record
                .in_set(VisitCounts)
                .after(SolverSystems::Restitution)
                .before(SolverSystems::StoreContactImpulses)
                .in_set(PhysicsStepSystems::Solver),
        )
        .add_systems(
            SubstepSchedule,
            (
                record
                    .after(SubstepSolverSystems::WarmStart)
                    .before(SubstepSolverSystems::SolveConstraints),
                record
                    .after(SubstepSolverSystems::SolveConstraints)
                    .before(SubstepSolverSystems::Relax),
                record.after(SubstepSolverSystems::Relax),
            )
                .in_set(VisitCounts),
        );
}

fn reset(mut visits: ResMut<ContactVisits>) {
    *visits = ContactVisits::default();
}

fn record(graph: Res<ConstraintGraph>, mut visits: ResMut<ContactVisits>) {
    // Resolution: Avian 0.8.0's contact-pass floor (`solver/plugin.rs`), chosen by the engine for
    // dispatch overhead. The root dependency keeps Avian's default `parallel` feature enabled.
    const PARALLEL_FLOOR: usize = 64;
    let workers = ComputeTaskPool::get().thread_num();
    // One length per color, bounded by Avian's graph-color count; no constraint is walked.
    for (index, color) in graph.colors[..=COLOR_OVERFLOW_INDEX].iter().enumerate() {
        let count = color.contact_constraints.len();
        if index == COLOR_OVERFLOW_INDEX || workers == 1 || count < PARALLEL_FLOOR {
            visits.serial += count as u32;
        } else {
            visits.parallel += count as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    /// The work account mirrors a pinned engine dispatch contract. A dependency changing either
    /// its floor or its single-worker rule must force a re-derivation, even if fixture sizes still
    /// happen to fall on the same side of the new floor.
    #[test]
    fn cost_dispatch_matches_the_pinned_engine_rule() {
        let solver =
            include_str!("../../vendor/avian3d-0.8.0-overmatch/src/dynamics/solver/plugin.rs");
        let dispatch = include_str!("../../vendor/avian3d-0.8.0-overmatch/src/utils.rs");
        assert_eq!(
            solver
                .matches("par_for_each(&mut color.contact_constraints, 64,")
                .count(),
            3
        );
        assert!(dispatch.contains("if task_pool_.thread_num() == 1 || slice.len() < min_len"));
    }
}
