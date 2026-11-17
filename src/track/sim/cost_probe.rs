//! Brackets for the ignored drivetrain cost observation; absent from normal fixtures and play.

use super::*;
use std::time::Instant;

#[derive(Resource, Default)]
pub(crate) struct Meter {
    since: Option<Instant>,
    pub(crate) ns: u128,
}

fn start(mut meter: ResMut<Meter>) {
    assert!(meter.since.replace(Instant::now()).is_none());
}

fn end(mut meter: ResMut<Meter>) {
    meter.ns += meter
        .since
        .take()
        .expect("a cost bracket was opened")
        .elapsed()
        .as_nanos();
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<Meter>()
        .add_systems(
            FixedUpdate,
            (
                start.before(wake_driven_tanks),
                end.after(commit_track_contacts),
            )
                .in_set(SimPhase::DrivingForces),
        )
        .add_systems(
            SubstepSchedule,
            (
                start
                    .after(SubstepSolverSystems::WarmStart)
                    .before(advance_drivetrains),
                end.after(update_belt_surfaces)
                    .before(SubstepSolverSystems::SolveConstraints),
            ),
        )
        .add_systems(
            PhysicsSchedule,
            (
                start.before(commit_drivetrains),
                end.after(publish_engine_rpm),
            )
                .in_set(SolverSystems::PostSubstep),
        );
}
