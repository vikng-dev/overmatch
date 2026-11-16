//! GUARD for the fork's prepared collider mass (`vendor/avian3d-0.8.0-overmatch`,
//! `OVERMATCH_PATCH.md`, "Prepared collider mass"). The game's environment physics prepares the
//! mass of each construction chunk collider that is no sensor off the system thread
//! (`Collider::with_mass_properties_cache`), and Avian's insertion hook reads that value instead of
//! integrating the shape. Invariant: a prepared value is what the shape gives at insertion, and is
//! never returned for another density, scale or shape. Who relies on it: every piece body whose
//! mass, centre and inertia aggregate from those colliders.

use avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::ComputeMassProperties3d;
use avian3d::prelude::*;
use bevy::prelude::*;

use super::avian_child_pose_churn::physics_app;

/// Fails on a fork whose prepared value is installed wrong or outlives a change of density, scale
/// or shape. Whether the hook reads the prepared value or integrates again is cost, not state, and
/// is not observed here.
#[test]
fn prepared_contact_mass_is_valid_at_insertion_and_invalidates_with_its_inputs() {
    // Fixture: an asymmetric convex box exposes centre and inertia as well as mass.
    let points = [0.0, 1.0]
        .into_iter()
        .flat_map(|x| {
            [0.0, 2.0]
                .into_iter()
                .flat_map(move |y| [0.0, 3.0].map(move |z| Vec3::new(x, y, z)))
        })
        .collect::<Vec<_>>();
    let hull = Collider::convex_hull(&points).expect("eight box corners span a hull");
    let density = ColliderDensity::default().0;
    let expected = hull.mass_properties(density);
    let prepared = hull.clone().with_mass_properties_cache(density);
    let mut app = physics_app();
    let entity = app.world_mut().spawn((Transform::default(), prepared)).id();
    assert_eq!(
        *app.world()
            .get::<ColliderMassProperties>(entity)
            .expect("Avian's insertion hook installs collider mass"),
        ColliderMassProperties::from(expected)
    );
    let mut prepared = app
        .world()
        .get::<Collider>(entity)
        .expect("the inserted collider")
        .clone();
    assert_eq!(
        prepared.mass_properties(density * 2.0),
        hull.mass_properties(density * 2.0),
        "a prepared value was returned for another density"
    );
    prepared.set_scale(Vec3::splat(2.0), 10);
    let mut scaled = hull.clone();
    scaled.set_scale(Vec3::splat(2.0), 10);
    assert_eq!(
        prepared.mass_properties(density),
        scaled.mass_properties(density),
        "a prepared value outlived a change of scale"
    );
    // A fresh prepared value: the scale change above has dropped the first.
    let mut reshaped = hull.with_mass_properties_cache(density);
    let replaced = Collider::sphere(1.0);
    reshaped.set_shape(replaced.shape().clone());
    assert_eq!(
        reshaped.mass_properties(density),
        replaced.mass_properties(density),
        "a prepared value outlived a change of shape"
    );
}
