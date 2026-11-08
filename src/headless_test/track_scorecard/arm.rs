//! The scorecard's seam onto one track model (the game's tank driving physics, game state only).
//!
//! Everything the card sets or reads that belongs to a track model, and nothing else, lives here.
//! An arm that replaces the track internals edits this file and leaves the card unchanged. The
//! card itself reads only the shipped input (`TankCommand`), Avian body state, every solver
//! contact's momentum through `ContactTransfers` (normal and friction) and construction facts
//! (`RigGeom`, the tank spec, the transmission tables).
//!
//! This arm: the lean boat on the ground, solver contacts for pieces, with test-selected force
//! resolution. Its belt support and grip on terrain and fixed construction act as forces applied
//! outside Avian's solver, reported per tick in `TrackContacts`; loose pieces meet its sprung belt
//! segments (`track::belt_segment`) as solver contacts, which the card reads itself.

use bevy::prelude::*;

use crate::ground_field::{ContactGround, GroundField};
use crate::track::belt_segment::BeltSegment;
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

/// Whether this arm claims that a piece under load takes the equal and opposite reaction. It
/// does: a piece meets the belt segments as an ordinary two-body solver contact.
pub(super) const CLAIMS_PIECE_REACTION: bool = true;

/// The role of one of the track model's own colliders on the tank, named from the marker its
/// spawner puts on it; `None` for a collider the track model does not spawn.
pub(super) fn collider_role(world: &World, collider: Entity) -> Option<&'static str> {
    if world.get::<BeltSegment>(collider).is_some() {
        Some("belt segment")
    } else if world.get::<BumpStop>(collider).is_some() {
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

/// Belt-segment work (observation, no gate): per tank per tick, the segment colliders in the
/// broad phase, their broad-phase pairs (contact-graph edges, after the layer filter), their
/// narrow-phase pairs (touching), manifolds and solver points; on open ground, driving onto the
/// card's pile and parked on it, and parked on the card's layer. Also the most manifolds the tank
/// has with other moving bodies in one step: Avian 0.7 colours each such manifold into one of
/// `DYNAMIC_COLOR_COUNT` (20) colours that hold the tank once, and solves the rest serially in
/// its overflow colour.
#[test]
#[ignore = "work-count observation; run beside the scorecard"]
fn segment_work_counts() {
    use super::{Bed, TICKS, base_ground, flat, pile_ahead, pile_under};
    use avian3d::prelude::{ColliderOf, ContactGraph, ContactPairFlags};

    #[derive(Default)]
    struct Tally {
        sums: [f64; 5],
        ticks: usize,
        dynamic_manifolds_max: usize,
    }
    fn count(bed: &mut Bed, tally: &mut Tally) {
        let tank = bed.tank;
        let world = bed.app.world_mut();
        let segments: Vec<Entity> = world
            .query_filtered::<(Entity, &ColliderOf), With<BeltSegment>>()
            .iter(world)
            .filter(|(_, of)| of.body == tank)
            .map(|(entity, _)| entity)
            .collect();
        let graph = world.resource::<ContactGraph>();
        tally.ticks += 1;
        tally.sums[0] += segments.len() as f64;
        for segment in segments {
            for edge in graph.contact_edges_with(segment) {
                tally.sums[1] += 1.0;
                if !edge.is_touching() {
                    continue;
                }
                tally.sums[2] += 1.0;
                let pair = graph
                    .get_pair_by_edge(edge)
                    .expect("a contact edge names its pair");
                tally.sums[3] += pair.manifolds.len() as f64;
                tally.sums[4] +=
                    pair.manifolds.iter().map(|m| m.points.len()).sum::<usize>() as f64;
            }
        }
        let dynamic = graph
            .iter_active_touching()
            .filter(|pair| pair.body1 == Some(tank) || pair.body2 == Some(tank))
            .filter(|pair| {
                !pair
                    .flags
                    .intersects(ContactPairFlags::STATIC1 | ContactPairFlags::STATIC2)
            })
            .map(|pair| pair.manifolds.len())
            .sum::<usize>();
        tally.dynamic_manifolds_max = tally.dynamic_manifolds_max.max(dynamic);
    }
    fn report(case: &str, tally: &Tally) {
        let [colliders, broad, narrow, manifolds, points] =
            tally.sums.map(|s| s / tally.ticks as f64);
        println!(
            "MEASURED segment work, {case}, per tank per tick over {} ticks: \
             {colliders:.1} segment colliders, {broad:.2} broad-phase pairs, {narrow:.2} \
             narrow-phase pairs, {manifolds:.2} manifolds, {points:.2} solver points; at most \
             {} manifolds with moving bodies in one step",
            tally.ticks, tally.dynamic_manifolds_max
        );
    }

    // Each phase's bed drops before the next boots: a booted world holds the boot lease.
    {
        let mut bed = Bed::on_ground(Some(base_ground()), flat());
        bed.place(Vec3::new(0.0, 0.0, 300.0), Quat::IDENTITY);
        bed.start();
        for _ in 0..5 * TICKS {
            bed.tick(1.0, 0.0);
        }
        let mut tally = Tally::default();
        for _ in 0..5 * TICKS {
            bed.tick(1.0, 0.0);
            count(&mut bed, &mut tally);
        }
        report("open ground, full throttle, seconds 5 to 10", &tally);
    }
    {
        let mut bed = Bed::on_ground(Some(base_ground()), flat());
        let (_, centre) = pile_ahead(&mut bed);
        bed.place(Vec3::ZERO, Quat::IDENTITY);
        bed.start();
        for _ in 0..2 * TICKS {
            bed.tick(0.0, 0.0);
        }
        let mut tally = Tally::default();
        for _ in 0..15 * TICKS {
            bed.tick(1.0, 0.0);
            count(&mut bed, &mut tally);
            if bed.hull().position.z < centre {
                break;
            }
        }
        report("driving onto the pile, full throttle", &tally);
        for _ in 0..2 * TICKS {
            let moving = bed.hull().speed() > crate::track::transmission::PARK_ENGAGE_SPEED;
            bed.tick(if moving { -1.0 } else { 0.0 }, 0.0);
        }
        let mut tally = Tally::default();
        for _ in 0..4 * TICKS {
            bed.tick(0.0, 0.0);
            count(&mut bed, &mut tally);
        }
        report("parked on the pile", &tally);
    }
    let mut bed = Bed::on_ground(Some(base_ground()), flat());
    let level = pile_under(&mut bed);
    bed.place(Vec3::new(0.0, level, 0.0), Quat::IDENTITY);
    bed.start();
    let mut tally = Tally::default();
    for _ in 0..4 * TICKS {
        bed.tick(0.0, 0.0);
        count(&mut bed, &mut tally);
    }
    report("parked on the layer under its tracks", &tally);
}
