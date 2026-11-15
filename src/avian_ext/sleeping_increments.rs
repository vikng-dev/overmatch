//! Sleeping bodies' velocity increments. Avian builds every body's world-frame velocity increment
//! each step (constant forces and accelerations, then gravity, then the scaling by the substep
//! length in `pre_process_velocity_increments`) but clears only the bodies in the solver. A
//! sleeping body is out of the solver and keeps its `VelocityIntegrationData`, so stock Avian
//! compounds its increment step after step and carries the stale value into its first step awake:
//! about `N·g·h²/(1 − h)` extra velocity (DERIVED; `h` the substep length in s, `N` the substeps).
//!
//! The law here: a body out of the solver keeps across the step exactly the increment it held when
//! the step began, and Avian's build of it is discarded. An increment held at the step's start is
//! a force or acceleration written to a sleeper since the last step (the waking `Forces` call wakes
//! the island only in `PhysicsStepSystems::Sleeping`, after this step's solve), and it acts in the
//! body's first step awake, unscaled until then. Who relies on it: the server's momentum accounts,
//! which close on a resting body's wake tick, and every force written to a resting body.

use avian3d::dynamics::integrator::{IntegrationSystems, VelocityIntegrationData};
use avian3d::dynamics::rigid_body::forces::ForceSystems;
use avian3d::dynamics::solver::solver_body::SolverBodyIndex;
use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::utils::Parallel;

/// The non-zero increments of bodies out of the solver at the start of the step's build, by body.
#[derive(Resource, Default)]
struct Held(Parallel<Vec<(Entity, Vec3, Vec3)>>);

/// Installs the hold and the release in Avian's `PhysicsSchedule`.
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Held>();
    app.get_schedule_mut(PhysicsSchedule)
        .expect("avian_ext::plugin is added after Avian's PhysicsPlugins")
        .add_systems((
            hold.in_set(IntegrationSystems::UpdateVelocityIncrements)
                .before(ForceSystems::ApplyConstantForces),
            release.in_set(IntegrationSystems::ClearVelocityIncrements),
        ));
}

fn held(data: &VelocityIntegrationData) -> bool {
    data.linear_increment != Vec3::ZERO || data.angular_increment != Vec3::ZERO
}

/// Records each body out of the solver whose increment is not zero before Avian builds on it.
fn hold(
    bodies: Query<(Entity, &VelocityIntegrationData), Without<SolverBodyIndex>>,
    held_now: Res<Held>,
) {
    bodies.par_iter().for_each(|(body, data)| {
        if held(data) {
            held_now.0.borrow_local_mut().push((
                body,
                data.linear_increment,
                data.angular_increment,
            ));
        }
    });
}

/// Gives every body out of the solver back the increment [`hold`] recorded, zero if none: Avian's
/// own clear, which runs beside this one, reaches only the bodies in the solver.
fn release(
    mut bodies: Query<&mut VelocityIntegrationData, Without<SolverBodyIndex>>,
    mut held_then: ResMut<Held>,
) {
    bodies.par_iter_mut().for_each(|mut data| {
        if held(&data) {
            data.linear_increment = Vec3::ZERO;
            data.angular_increment = Vec3::ZERO;
        }
    });
    for (body, linear, angular) in held_then.0.drain() {
        let mut data = bodies
            .get_mut(body)
            .expect("no body enters or leaves the solver between the increments' build and clear");
        data.linear_increment = linear;
        data.angular_increment = angular;
    }
}
