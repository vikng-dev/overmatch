//! Observation probe for the game's tank driving physics: hull-first contact with a thin wall.
//! The shipped plugin and construction are used unchanged. Fixture dimensions below pose the
//! question; the acceptance range is a judgement of plausible contact, not an earlier output.

use super::*;

fn hull_front(bed: &mut Bed) -> f32 {
    let tank = bed.tank;
    let world = bed.app.world_mut();
    let body_pos = world.get::<Position>(tank).expect("tank pose").0;
    let body_rot = *world.get::<Rotation>(tank).expect("tank rotation");
    world
        .query::<(&ColliderOf, &Collider, &ColliderTransform, &CollisionLayers)>()
        .iter(world)
        .filter(|(of, _, _, layers)| of.body == tank && layers.memberships.has_all(Layer::Vehicle))
        .map(|(_, collider, local, _)| {
            collider
                .aabb(
                    body_pos + body_rot * local.translation,
                    body_rot * local.rotation,
                    0.0,
                )
                .min
                .z
        })
        .reduce(f32::min)
        .expect("the tank has solid hull proxies")
}

#[test]
#[ignore = "observation: thin-wall CCD probe, run before and after engine changes"]
fn hull_first_thin_wall() {
    // Fixture: a central wall misses the belts, is 2 cm thick and taller than the hull.
    // A quarter metre of peak intrusion is already an implausible first contact; passing the
    // wall or remaining embedded is unacceptable at all three approach speeds.
    for speed in [6.0, 8.0, 10.0] {
        let mut bed = resting_on_flat();
        let face = hull_front(&mut bed) - 0.1;
        let wall = bed.spawn_static(Vec3::new(0.0, 2.0, face - 0.01), Vec3::new(0.5, 4.0, 0.02));
        bed.app.update();
        bed.app
            .world_mut()
            .entity_mut(bed.tank)
            .remove::<Sleeping>()
            .insert(LinearVelocity(Vec3::NEG_Z * speed));
        let mut deepest = 0.0_f32;
        let mut first_touch = None;
        for _ in 0..TICKS {
            bed.tick(0.0, 0.0);
            deepest = deepest.max(face - hull_front(&mut bed));
            if first_touch.is_none() {
                first_touch = bed.first_touch(wall);
            }
        }
        let final_depth = face - hull_front(&mut bed);
        println!(
            "CCD_PROBE speed={speed} peak_intrusion_m={deepest} final_intrusion_m={final_depth} velocity={:?} first_touch={first_touch:?}",
            bed.hull().velocity
        );
        assert!(
            deepest < 0.25 && final_depth < 0.1 && first_touch.is_some(),
            "implausible thin-wall contact at {speed} m/s"
        );
    }
}
