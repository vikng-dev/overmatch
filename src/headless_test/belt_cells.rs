//! Counts-only witness for belt support on another data-spawned tank's hull proxy.

use super::*;
use crate::track::{rig_geom::RigGeom, side::Side, sim::TrackContacts};
use avian3d::prelude::{Collider, ColliderAabb, ColliderOf, Position, Rotation};

/// The belt spring channel must reach the other tank's declared hull proxy. The fixture
/// derives its placement from the proxy surface and running gear.
#[test]
fn belt_bears_on_another_spawned_hull_proxy() {
    let mut app = booted_sim();
    let world = app.world_mut();
    let tanks: Vec<_> = world
        .query_filtered::<Entity, With<Tank>>()
        .iter(world)
        .collect();
    let upper = tanks[0];
    let lower = tanks[1];
    let declared: Vec<_> = world
        .resource::<TankBlueprint>()
        .spec
        .colliders
        .iter()
        .map(|c| c.node.clone())
        .collect();
    let (proxy, _) = world
        .query::<(Entity, &ColliderAabb, &ColliderOf, &ChildOf)>()
        .iter(world)
        .filter(|(_, _, of, parent)| {
            of.body == lower
                && world
                    .get::<Name>(parent.parent())
                    .is_some_and(|n| declared.iter().any(|s| s == n.as_str()))
        })
        .map(|(e, aabb, _, _)| (e, aabb.min.y + aabb.max.y))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .expect("spawned tank has a declared hull proxy");
    let shape = world.get::<Collider>(proxy).unwrap().shape_scaled();
    let (vertices, indices) = shape.as_convex_polyhedron().unwrap().to_trimesh();
    let proxy_at = world.get::<Position>(proxy).unwrap().0;
    let proxy_rot = world.get::<Rotation>(proxy).unwrap().0;
    let lower_at = world.get::<Position>(lower).unwrap().0;
    let surface = indices
        .iter()
        .map(|tri| tri.map(|i| proxy_at + proxy_rot * vertices[i as usize]))
        .filter(|[a, b, c]| (*b - *a).cross(*c - *a).y > 0.0)
        .map(|tri| tri.into_iter().sum::<Vec3>() / 3.0)
        .max_by(|a, b| a.y.total_cmp(&b.y))
        .unwrap()
        - lower_at;
    let geom = world.resource::<RigGeom>();
    let offset = geom.grip_columns(Side::Left)[1].0;
    let rest_y = geom.hull_rest_y;
    let engage = world
        .resource::<TankBlueprint>()
        .spec
        .track
        .suspension
        .engage;
    // Fixture: elevate both tanks clear of the field; half-ramp overlap makes the belt bear.
    let lower_pose = Vec3::Y * 10.0;
    let upper_pose = lower_pose + surface + Vec3::new(-offset, rest_y - engage * 0.5, 0.0);
    start_fixed_clock(&mut app);
    let mut bore = false;
    // Fixture: repeated prescribed poses isolate per-tick partition work from divergent motion.
    for _ in 0..8 {
        super::drive_load::place(&mut app, lower, lower_pose, Quat::IDENTITY, 0.0, 1);
        super::drive_load::place(&mut app, upper, upper_pose, Quat::IDENTITY, 0.0, 1);
        app.update();
        let world = app.world();
        bore |= world
            .get::<TrackContacts>(upper)
            .unwrap()
            .0
            .iter()
            .flatten()
            .any(|c| c.collider == Some(proxy));
    }
    assert!(
        bore,
        "the upper belt must bear on the lower tank's data-spawned hull proxy"
    );
}
