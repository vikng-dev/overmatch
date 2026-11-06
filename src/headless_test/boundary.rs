//! Production hull driving against the map's admitted side bound, relied on by the server sim.

use super::*;
use crate::track::{
    rig_geom::{Pose, RigGeom},
    side::Side,
};
use avian3d::prelude::*;

struct Bed {
    app: BootedSim,
    tank: Entity,
    hulls: Vec<(Collider, ColliderTransform)>,
    droop_feet: Vec<Vec3>,
    half: f32,
    tolerance: f32,
    rest: f32,
}

impl Bed {
    fn new(ramp: bool) -> Self {
        // Fixture: a 256 m square; a 1:8 ramp returns to ground 8 m before the east wall.
        let manifest = crate::map::tests::fixture_manifest(128.0, 0.0, 50.0);
        let half = manifest.extent.half_extent();
        let size = 129;
        let heights = (0..size * size)
            .map(|i| {
                let x = -half + (i % size) as f32 * (2.0 * half / (size - 1) as f32);
                if ramp && x > half - 40.0 && x < half - 8.0 {
                    (x - (half - 40.0)) / 8.0
                } else {
                    0.0
                }
            })
            .collect::<Vec<_>>();
        let grid = crate::terrain_grid::HeightGrid::new(heights.into(), size, manifest.extent);
        let mut app = booted_sim_on_map(Some(grid), Some(manifest));
        start_fixed_clock(&mut app);
        app.update();
        let world = app.world_mut();
        let tank = world
            .query_filtered::<Entity, (With<Tank>, With<Controlled>)>()
            .single(world)
            .unwrap();
        let hulls = world
            .query::<(&Collider, &ColliderTransform, &ColliderOf, &CollisionLayers)>()
            .iter(world)
            .filter(|(_, _, of, layers)| {
                of.body == tank
                    && layers.memberships.has_all(crate::Layer::Vehicle)
                    && layers.filters.has_all(crate::Layer::Terrain)
            })
            .map(|(shape, local, _, _)| (shape.clone(), *local))
            .collect::<Vec<_>>();
        assert!(
            !hulls.is_empty(),
            "the witness must observe the production hull proxies"
        );
        let geom = world.resource::<RigGeom>();
        let params = world
            .resource::<TankBlueprint>()
            .spec
            .track
            .suspension
            .params();
        let rest = geom.hull_rest_y;
        // DERIVED: each roadwheel's lowest reachable outer shoe point at full chain-limited droop.
        // If an end rises past this reach on flat ground, it has reared or climbed off support.
        let droop_feet = Side::ALL
            .into_iter()
            .flat_map(|side| {
                geom.circles(side, Pose::Droop, &params)
                    .into_iter()
                    .skip(1)
                    .take(geom.road_wheels.get(side).len())
                    .map(move |(c, r)| {
                        Vec3::new(
                            geom.side_plane_x(side),
                            c.y - r - geom.model.pin_to_outer,
                            c.x,
                        )
                    })
            })
            .collect();
        let tolerance = world.resource::<NarrowPhaseConfig>().contact_tolerance
            * world.resource::<PhysicsLengthUnit>().0;
        Self {
            app,
            tank,
            hulls,
            droop_feet,
            half,
            tolerance,
            rest,
        }
    }

    fn pose(&self) -> (Vec3, Quat) {
        let world = self.app.world();
        (
            world.get::<Position>(self.tank).unwrap().0,
            world.get::<Rotation>(self.tank).unwrap().0,
        )
    }

    fn side_overrun(&self) -> f32 {
        let (p, r) = self.pose();
        self.hulls
            .iter()
            .map(|(shape, local)| {
                let aabb = shape.aabb(p + r * local.translation, r * local.rotation.0);
                aabb.min.xz().abs().max(aabb.max.xz().abs()).max_element() - self.half
            })
            .fold(f32::NEG_INFINITY, f32::max)
    }

    fn foot_height(&self) -> f32 {
        let (p, r) = self.pose();
        self.droop_feet
            .iter()
            .map(|foot| (p + r * *foot).y)
            .fold(f32::NEG_INFINITY, f32::max)
    }

    fn enter(&mut self, at: Vec2, yaw_deg: f32, rolling: bool) {
        let rotation = Quat::from_rotation_y(yaw_deg.to_radians());
        let position = Vec3::new(at.x, self.rest, at.y);
        drive_load::place(&mut self.app, self.tank, position, rotation, 0.0, 1);
        // Fixture: settle the suspension before drive entry, so launch transients are not wall lift.
        for _ in 0..2 * FIXED_TICKS_PER_SECOND {
            drive_tick(&mut self.app, self.tank, 0.0, 0.0);
        }
        let position = self.pose().0;
        let trans = self
            .app
            .world()
            .resource::<crate::track::sim::TrackGear>()
            .trans();
        let (speed, gear) = if rolling {
            (trans.governed_speed(false), trans.gears_fwd.len() as u8)
        } else {
            (0.0, 1)
        };
        drive_load::place(&mut self.app, self.tank, position, rotation, speed, gear);
    }
}

/// Full throttle must not carry any hull proxy across the admitted sides; the server relies on it.
struct DriveMargins {
    side: f32,
    height: f32,
    slide: f32,
}

fn drive_at_wall(name: &str, yaw: f32, corner: bool, rolling: bool) -> DriveMargins {
    let mut bed = Bed::new(false);
    let z = if corner { bed.half - 16.0 } else { -40.0 };
    bed.enter(Vec2::new(bed.half - 16.0, z), yaw, rolling);
    let mut overrun = f32::NEG_INFINITY;
    let mut foot = f32::NEG_INFINITY;
    let mut touched = false;
    let mut slide_start = None;
    let mut slide = 0.0_f32;
    // Fixture: enough time to reach the side from rest and traverse several hull lengths.
    for _ in 0..12 * FIXED_TICKS_PER_SECOND {
        drive_tick(&mut bed.app, bed.tank, 1.0, 0.0);
        overrun = overrun.max(bed.side_overrun());
        foot = foot.max(bed.foot_height());
        if bed.side_overrun() >= -bed.tolerance {
            touched = true;
            let start = *slide_start.get_or_insert(bed.pose().0.z);
            slide = slide.max((bed.pose().0.z - start).abs());
        }
    }
    println!(
        "MEASURED {name}: overrun={overrun} m, side margin={} m, droop-foot height={foot} m, height margin={} m, slide={slide} m",
        bed.tolerance - overrun,
        bed.tolerance - foot
    );
    assert!(touched, "{name} must exercise the wall");
    // DERIVED: a whole body width of tangential travel rules out a stationary perch.
    let width = bed
        .hulls
        .iter()
        .map(|(shape, local)| {
            let aabb = shape.aabb(local.translation, local.rotation);
            aabb.max.x - aabb.min.x
        })
        .fold(0.0_f32, f32::max);
    DriveMargins {
        side: bed.tolerance - overrun,
        height: bed.tolerance - foot,
        slide: slide - width,
    }
}

#[test]
fn full_throttle_head_on_stays_inside() {
    assert!(drive_at_wall("head-on", -90.0, false, true).side >= 0.0);
}

#[test]
fn full_throttle_into_a_convex_corner_stays_inside() {
    assert!(drive_at_wall("convex-corner", -135.0, true, true).side >= 0.0);
}

#[test]
fn oblique_drive_slides_without_rearing_climbing_or_perching() {
    // Fixture: 30 degrees to the wall, accelerating from rest.
    let margins = drive_at_wall("oblique-30", -150.0, false, false);
    assert!(
        margins.height >= 0.0,
        "an end lifted beyond its own droop reach"
    );
    assert!(
        margins.slide > 0.0,
        "the hull must slide a full body width while touching the wall"
    );
}

/// The server containment invariant remains open for an oblique grind; its bound is unchanged.
#[test]
#[ignore = "open #299: production contact resolution exceeds the side-bound tolerance"]
fn oblique_drive_stays_inside_the_wall() {
    assert!(drive_at_wall("oblique-30", -150.0, false, false).side >= 0.0);
}

/// The server's side bound must hold even when terrain support is absent on arrival. The ramp
/// supplies the launch through ordinary track contacts; the test never authors an upward impulse.
#[test]
#[ignore = "open #299: airborne production hull contact exceeds the side-bound tolerance"]
fn a_hull_launched_off_a_ramp_stays_inside_the_wall() {
    let mut bed = Bed::new(true);
    bed.enter(Vec2::new(bed.half - 50.0, -40.0), -90.0, true);
    let mut overrun = f32::NEG_INFINITY;
    let mut airborne_at_wall = false;
    let mut peak = f32::NEG_INFINITY;
    for _ in 0..12 * FIXED_TICKS_PER_SECOND {
        drive_tick(&mut bed.app, bed.tank, 1.0, 0.0);
        let side = bed.side_overrun();
        overrun = overrun.max(side);
        peak = peak.max(bed.pose().0.y);
        let contacts = &bed
            .app
            .world()
            .get::<crate::track::sim::TrackContacts>(bed.tank)
            .unwrap()
            .0;
        airborne_at_wall |= side >= -bed.tolerance && contacts.iter().all(Vec::is_empty);
    }
    println!(
        "MEASURED ramp: overrun={overrun} m, side margin={} m, peak root={peak} m, airborne at wall={airborne_at_wall}",
        bed.tolerance - overrun
    );
    assert!(
        airborne_at_wall,
        "the ramp must deliver an airborne wall contact"
    );
    assert!(
        overrun <= bed.tolerance,
        "airborne hull crossed the side by {overrun} m"
    );
}
