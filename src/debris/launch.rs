//! Free-body momentum exchange in the game's environment physics (game state only).
//! Occupied geometry supplies mass and inertia; field impulses supply free-group motion.
//! Body sweeps exchange momentum before physics in `construction::sweep`.

use crate::construction::fracture::Load;
use avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::{
    ComputeMassProperties3d, MassProperties3d,
};
use avian3d::prelude::*;
use bevy::prelude::*;

/// One freed group's collider frame and departure in construction terms. The mass and inertia are those of these exact colliders at their data's density.
#[derive(Debug)]
pub(crate) struct Launch {
    pub(crate) centroid: Vec3,
    /// Actual occupied centre of mass in the construction frame, including collider rounding.
    pub(crate) centre: Vec3,
    pub(crate) colliders: Vec<(usize, Collider)>,
    pub(crate) velocity: Vec3,
    pub(crate) spin: Vec3,
    pub(crate) mass: f32,
    pub(crate) inertia: Mat3,
}

/// What a collider contributes to its body's mass, inertia and centre: the occupied `hull`'s at
/// `density`, whatever shape the collider itself meets contacts with.
pub(crate) fn occupied_mass(hull: &Collider, density: f32) -> (Mass, AngularInertia, CenterOfMass) {
    let props = hull.mass_properties(density);
    (
        Mass(props.mass),
        AngularInertia::new_with_local_frame(
            props.principal_angular_inertia,
            props.local_inertial_frame,
        ),
        CenterOfMass(props.center_of_mass),
    )
}

/// Occupied geometry and field impulses in a frame about `centroid`. Contacts carry force only;
/// their momentum is already integrated by physics or the construction sweep.
pub(crate) fn launch_colliders(
    centroid: Vec3,
    colliders: Vec<(usize, Collider)>,
    density: impl Fn(usize) -> f32,
    loads: &[Load],
) -> Launch {
    let total = colliders
        .iter()
        .map(|(c, shape)| shape.mass_properties(density(*c)))
        .fold(MassProperties3d::default(), |sum, part| sum + part);
    assert!(total.mass > 0.0, "a freed group has occupied mass");
    let inverse_inertia = total.angular_inertia_tensor().inverse();
    let com = centroid + total.center_of_mass;
    let mut impulse = Vec3::ZERO;
    let mut angular = Vec3::ZERO;
    for load in loads {
        // A press, or a field carrying only crushing energy, launches no momentum.
        if load.blow == Vec3::ZERO {
            continue;
        }
        let launched = load.blow;
        impulse += launched;
        angular += (load.at - com).cross(launched);
    }
    Launch {
        centroid,
        centre: com,
        colliders,
        velocity: impulse / total.mass,
        spin: inverse_inertia * angular,
        mass: total.mass,
        inertia: total.angular_inertia_tensor().to_mat3(),
    }
}
