//! Observation probe for the game's tank driving physics: hull-first contact with a thin wall.
//! The shipped plugin and construction are unchanged; acceptance follows the engine's travel
//! bound, while peak and settled distances are observations.

use avian3d::collision::collider::contact_query::contact;

use super::*;

fn hull_wall_distance(bed: &mut Bed, wall_shape: &Collider, wall_pos: Vec3) -> f32 {
    let tank = bed.tank;
    let world = bed.app.world_mut();
    let body_pos = world.get::<Position>(tank).expect("tank pose").0;
    let body_rot = *world.get::<Rotation>(tank).expect("tank rotation");
    world
        .query::<(&ColliderOf, &Collider, &ColliderTransform, &CollisionLayers)>()
        .iter(world)
        .filter(|(of, _, _, layers)| of.body == tank && layers.memberships.has_all(Layer::Vehicle))
        .map(|(_, collider, local, _)| {
            -contact(
                collider,
                body_pos + body_rot * local.translation,
                body_rot * local.rotation,
                wall_shape,
                wall_pos,
                Quat::IDENTITY,
                f32::MAX,
            )
            .expect("hull proxies and the wall support convex contact queries")
            .expect("an unlimited prediction distance includes the finite hull/wall separation")
            .penetration
        })
        .reduce(f32::min)
        .expect("the tank has solid hull proxies")
}

#[test]
#[ignore = "observation: thin-wall CCD probe, run before and after engine changes"]
fn hull_first_thin_wall() {
    // Fixture: a central 2 cm wall misses the belts and is taller than the hull. Pose it at
    // 0.1 m signed separation using shape queries, so hull tilt cannot change the start gap.
    for speed in [6.0, 8.0, 10.0] {
        let mut bed = resting_on_flat();
        let size = Vec3::new(0.5, 4.0, 0.02);
        let wall_shape = Collider::cuboid(size.x, size.y, size.z);
        let mut wall_pos = Vec3::new(0.0, 2.0, bed.hull().position.z);
        // Fixture bracket: from inside the hull back to clear space on the approaching side.
        let mut near = wall_pos.z;
        let mut far = near - 20.0;
        assert!(hull_wall_distance(&mut bed, &wall_shape, wall_pos) < 0.1);
        wall_pos.z = far;
        assert!(hull_wall_distance(&mut bed, &wall_shape, wall_pos) > 0.1);
        // Resolution: micrometre placement error is negligible beside a tick's travel.
        while near - far > 1e-6 {
            let mid = (near + far) * 0.5;
            if mid == near || mid == far {
                break; // Adjacent floats already provide the finest representable fixture pose.
            }
            wall_pos.z = mid;
            if hull_wall_distance(&mut bed, &wall_shape, wall_pos) > 0.1 {
                far = mid;
            } else {
                near = mid;
            }
        }
        wall_pos.z = far;
        let wall = bed.spawn_static(wall_pos, size);
        bed.app.update();
        bed.app
            .world_mut()
            .entity_mut(bed.tank)
            .remove::<Sleeping>()
            .insert(LinearVelocity(Vec3::NEG_Z * speed));
        let step = bed
            .app
            .world()
            .resource::<Time<Fixed>>()
            .timestep()
            .as_secs_f32();
        let mut peak = 0.0_f32;
        let mut first_touch = None;
        for _ in 0..TICKS {
            bed.tick(0.0, 0.0);
            peak = peak.max(-hull_wall_distance(&mut bed, &wall_shape, wall_pos));
            first_touch = first_touch.or_else(|| bed.first_touch(wall));
            assert!(
                bed.center_of_mass().z > wall_pos.z,
                "the hull tunneled through its wall at {speed} m/s"
            );
        }
        let settled = -hull_wall_distance(&mut bed, &wall_shape, wall_pos);
        println!(
            "CCD_PROBE speed={speed} peak_intrusion_m={peak} settled_intrusion_m={settled} tick_travel_m={} velocity={:?} first_touch={first_touch:?}",
            speed * step,
            bed.hull().velocity
        );
        // Engine contract: low-risk discrete contact may intrude by one tick of travel.
        // Resolution: allow float roundoff in the shape query at the travel bound.
        assert!(
            peak <= speed * step + 1e-4 && first_touch.is_some(),
            "implausible thin-wall contact at {speed} m/s"
        );
    }
}
