//! The scorecard's seam onto one track model (the game's tank driving physics, game state only).
//!
//! Everything the card sets or reads that belongs to a track model, and nothing else, lives here.
//! An arm that replaces the track internals edits this file and leaves the card unchanged. The
//! card itself reads only the shipped input (`TankCommand`), Avian body state, every solver
//! contact's momentum through `ContactTransfers` (normal and friction) and construction facts
//! (`RigGeom`, the tank spec, the transmission tables).
//!
//! This arm: the lean boat, with test-selected force resolution. Its belt support and grip act as forces applied outside
//! Avian's solver, reported per tick in `TrackContacts`.

use bevy::prelude::*;

use crate::ground_field::{ContactGround, GroundField};
use crate::track::bump_stop::{BumpStop, SidePlate};
use crate::track::sim::TrackContacts;
use crate::track::terrain::TrackField;

/// A force on the hull from one world body, at a world point.
#[derive(Clone, Copy)]
pub(super) struct WorldForce {
    /// The world body, or `None` for the terrain height field the track samples itself.
    pub(super) body: Option<Entity>,
    pub(super) point: Vec3,
    /// Newtons on the hull.
    pub(super) force: Vec3,
}

/// Whether this arm claims that a piece under load takes the equal and opposite reaction. The
/// boat does not: `track::rubble` clamps a resting piece's normal reaction by design.
pub(super) const CLAIMS_PIECE_REACTION: bool = false;

/// The role of one of the track model's own colliders on the tank, named from the marker its
/// spawner puts on it; `None` for a collider the track model does not spawn.
pub(super) fn collider_role(world: &World, collider: Entity) -> Option<&'static str> {
    if world.get::<BumpStop>(collider).is_some() {
        Some("bump stop")
    } else if world.get::<SidePlate>(collider).is_some() {
        Some("side plate")
    } else {
        None
    }
}

/// The tracked `scripts/track-resolution-scorecard` selects each experiment setting once per
/// test process. Without its selector the card uses the shipped force resolution.
fn resolution() -> crate::track::forces::ContactResolution {
    crate::track::resolution::configure().expect("the scorecard script supplies a valid resolution")
}

/// Select the force field before the fixture's first fixed tick and make the terrain uniform.
pub(super) fn set_ground(app: &mut App, ground: ContactGround) {
    crate::track::sim::set_test_resolution(app, resolution());
    let mut track = app.world_mut().resource_mut::<TrackField>();
    let field = track
        .field
        .take()
        .expect("the booted world built its track field");
    track.field = Some(field.with_ground(Some(GroundField::uniform(ground))));
}

/// The forces this tick that the track model applied to the hull outside Avian's solver. The
/// card adds every solver contact itself, friction included, so an arm whose contacts live in
/// the solver returns only what it applies beside them.
pub(super) fn belt_forces(app: &App, tank: Entity) -> Vec<WorldForce> {
    let world = app.world();
    let contacts = world
        .get::<TrackContacts>(tank)
        .expect("a driving tank reports its belt contacts");
    contacts
        .0
        .iter()
        .flatten()
        .map(|c| WorldForce {
            body: c.collider.map(|collider| {
                world
                    .get::<avian3d::prelude::ColliderOf>(collider)
                    .map_or(collider, |of| of.body)
            }),
            point: c.point,
            force: c.normal * c.load + c.tangential_force(),
        })
        .collect()
}

/// The arm's own work per tank this tick, beyond the production `Work` counts and the solver
/// contacts the card counts itself.
pub(super) fn work(app: &App, tank: Entity) -> Vec<(&'static str, f64)> {
    let contacts = app
        .world()
        .get::<TrackContacts>(tank)
        .expect("a driving tank reports its belt contacts");
    let count = contacts.0.iter().map(Vec::len).sum::<usize>() as f64;
    // DERIVED from `track::forces::emit_contact_forces`: a support and a tangent application
    // per contact.
    vec![
        ("belt_contacts", count),
        ("belt_force_applications", 2.0 * count),
    ]
}

/// Tick observations of the same one-lane wall pushes as the card. No force, climb or chatter
/// output is a gate; the CSV retains every tick for the owner's comparison.
#[test]
#[ignore = "wall contact observations; run beside the scorecard"]
fn wall_push_tick_observations() {
    use super::{Bed, DT, TICKS, WALL_HEIGHT, base_ground, flat, front_z, lanes};
    let lane_spans = {
        let bed = Bed::on_ground(Some(base_ground()), flat());
        lanes(bed.geom())
    };
    let setting = resolution();
    std::fs::create_dir_all("target/track-lean-session3").unwrap();
    for (side, lo, hi) in lane_spans {
        let mut bed = Bed::on_ground(Some(base_ground()), flat());
        let wall_face = front_z(bed.geom()) - 2.0; // Fixture: the card's approach distance.
        let wall = bed.spawn_static(
            Vec3::new((lo + hi) / 2.0, (WALL_HEIGHT - 1.0) / 2.0, wall_face - 1.0),
            Vec3::new(hi - lo, WALL_HEIGHT + 1.0, 2.0),
        );
        bed.place(Vec3::ZERO, Quat::IDENTITY);
        bed.start();
        let begin = bed.hull().position.y;
        let mut csv = String::from(
            "tick,time_s,normal_n,normal_y_n,friction_y_n,spring_y_n,rise_m,left_speed_m_s,right_speed_m_s,stop_overlap_m\n",
        );
        for tick in 0..8 * TICKS {
            bed.tick(1.0, 0.0);
            let (mut normal, mut friction) = (Vec3::ZERO, Vec3::ZERO);
            for (force, n) in bed.tank_solver_forces() {
                if force.body == Some(wall) {
                    let along = n * force.force.dot(n);
                    normal += along;
                    friction += force.force - along;
                }
            }
            let spring: Vec3 = belt_forces(&bed.app, bed.tank)
                .iter()
                .filter(|f| f.body == Some(wall))
                .map(|f| f.force)
                .sum();
            let world = bed.app.world();
            let drive = world
                .get::<crate::track::sim::TrackDrive>(bed.tank)
                .unwrap();
            let overlap = world
                .resource::<avian3d::prelude::ContactGraph>()
                .iter_active_touching()
                .filter(|pair| {
                    (pair.collider1 == wall && world.get::<BumpStop>(pair.collider2).is_some())
                        || (pair.collider2 == wall
                            && world.get::<BumpStop>(pair.collider1).is_some())
                })
                .flat_map(|pair| &pair.manifolds)
                .flat_map(|m| &m.points)
                .map(|p| p.penetration)
                .fold(0.0_f32, f32::max);
            csv += &format!(
                "{tick},{},{},{},{},{},{},{},{},{overlap}\n",
                tick as f32 * DT,
                normal.length(),
                normal.y,
                friction.y,
                spring.y,
                bed.hull().position.y - begin,
                drive.sides[0].speed,
                drive.sides[1].speed
            );
        }
        let path = format!(
            "target/track-lean-session3/wall-{}x{}-{side:?}.csv",
            setting.stations, setting.lateral_points
        );
        std::fs::write(&path, csv).unwrap();
        println!("MEASURED wall tick trace: {path}");
    }
}
