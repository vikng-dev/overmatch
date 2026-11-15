//! The rest energy law. A body rests when Avian's separate linear and angular velocity test passes
//! or when its kinetic energy per mass is under that of its rest speed:
//! `v² + ω·I_world·ω/m < (SleepThreshold::linear · PhysicsLengthUnit)²`. The separate test misreads
//! a slender body's roll as motion; the energy test does not, and the separate test stays because
//! it is the more permissive one for large bodies.
//!
//! Who relies on it: the game's scenery and debris rest (`OVERMATCH_PATCH.md`, "Rest energy"), and
//! the game's own rest census, which compares with Avian's verdict.

use avian3d::dynamics::solver::solver_body::SolverBody;
use avian3d::prelude::*;

/// Twice the body's kinetic energy per mass, in squared speed units: `v² + ω·I·ω/m`, with `I` the
/// computed inertia turned to the world frame by `rotation`. The factor 1/2 cancels against the
/// rest speed's energy. Physical inertia, not the solver's lock-masked inertia: a locked axis
/// stops acceleration, while a prescribed velocity still moves the body. Zero and infinite mass and
/// inertia follow the computed properties' zero-reciprocal convention, so they contribute no
/// rotational term.
pub fn kinetic_speed_squared(
    body: &SolverBody,
    mass: ComputedMass,
    angular_inertia: ComputedAngularInertia,
    rotation: Rotation,
) -> f32 {
    let rotational = body
        .angular_velocity
        .dot(angular_inertia.rotated(rotation.0).tensor() * body.angular_velocity);
    body.linear_velocity.length_squared() + rotational * mass.inverse()
}
