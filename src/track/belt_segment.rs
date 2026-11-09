//! The belt's sprung outer face against loose pieces, in the game's tank driving physics (game
//! state only).
//!
//! Terrain and fixed construction meet the belt through the spring channel (`forces`) and the
//! outer-face stop (`bump_stop`). A loose piece meets it here, as a solver contact, so the piece
//! takes the equal and opposite reaction through Avian:
//!
//! - **Shape.** Per side, [`segments`] cuts the closed rest loop into [`SEGMENT_M`] pieces and
//!   keeps those over suspension travel: a convex slab across the shoe's measured faces, its outer
//!   face on the droop envelope (the rest outer face pushed out by the travel profile the spring
//!   channel reads, `envelope::wheel_travel_knots`). Built at spawn from construction data and hung
//!   on the hull with the stop (`bump_stop::spawn_track_colliders`; ADR-0014), on
//!   [`Layer::TrackBelt`], which meets only `Layer::Debris`.
//! - **Support: the running gear's law along the belt.** A shoe is a rigid plate: a piece under
//!   part of its width still loads the road wheels through the whole link, so coverage across the
//!   shoe moves where the force acts (the roll moment comes from the contact's position) and
//!   leaves the stiffness alone. The calibrated per-metre envelope law is a fact along the belt
//!   and width does not enter it. [`BeltSegment::shape_contacts`] gives a segment that law times
//!   its length (`ContactCompliance`, an OVERMATCH PATCH in the vendored solver);
//!   [`share_segment_laws`] shares it over every point in overlap, so pieces side by side or
//!   stacked cannot multiply the support. The same rigid plate keeps the ground from a second
//!   count: over the length a loose piece covers, its shadow in the spring channel's footprint
//!   partition spans the whole shoe, and the ground beside the piece is not touching the link. The
//!   sampled support differs at partial coverage of ground itself: a column quadrature, so a curb
//!   under one edge takes that column's share. Past the wheels' travel the piece meets the same outer-face stop as the
//!   ground. The end wheels have no travel and no segment: the stop is hard there from first
//!   touch.
//! - **Drivetrain.** Each segment carries the stop's `BeltFace`, so the shared hook moves its
//!   surface at belt speed and `stop_contacts::by_owner` reads its friction into the transmission
//!   with the stop's.

use avian3d::prelude::{
    AngularVelocity, Collider, CollisionLayers, ContactCompliance, ContactGraph, ContactManifold,
    ContactPair, LinearVelocity, Position, Rotation,
};
use bevy::prelude::*;

use super::derive::SuspensionParams;
use super::forces::TravelField;
use super::rig_geom::RigGeom;
use super::route::build_route;
use super::side::Side;
use crate::Layer;

/// Resolution: belt length one segment stands for (m). Below the 0.5 m size floor of a loose
/// piece, so a piece under the belt meets at least one whole segment face; finer costs colliders
/// and manifolds linearly.
const SEGMENT_M: f32 = 0.3;

/// A contact normal (unit, out of a segment) nearer its slab's end, side or inner faces than its
/// outer face came from a cut of the continuous belt, not from its surface. DERIVED: an edge of
/// the outer face meets a world face along any normal between the two faces' normals, which
/// are square to each other; the bisector parts the two.
const OUTER_FACE_COS: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// One sprung belt segment, a child collider of its hull.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct BeltSegment {
    /// Loop length this segment stands for (m), which scales the per-metre envelope law.
    pub(crate) length: f32,
    /// Body-local unit normal of the outer face, out of the loop.
    outward: Vec3,
    /// The outer face's plane, `outward · p` for every body-local point `p` on it (m).
    face: f32,
}

#[cfg(test)]
impl BeltSegment {
    /// Fixture: a 0.3 m segment whose outer face is y = 0, facing down.
    pub(crate) const FIXTURE: Self = Self {
        length: 0.3,
        outward: Vec3::NEG_Y,
        face: 0.0,
    };
}

/// The calibrated envelope law a segment carries per metre of belt, the spring channel's own
/// (`ForceParams::support_stiffness_per_m`, `support_damping_per_m`): published by whichever
/// driver calibrated it, the game's `TrackGear` or the sandbox's rig.
#[derive(Resource, Clone, Copy, Debug)]
pub(crate) struct SegmentLaw {
    /// N/m per metre of belt.
    pub(crate) stiffness_per_m: f32,
    /// N·s/m per metre of belt.
    pub(crate) damping_per_m: f32,
}

/// What a segment meets: loose pieces only.
pub(crate) fn segment_layers() -> CollisionLayers {
    CollisionLayers::new([Layer::TrackBelt], [Layer::Debris])
}

/// One side's sprung segments in body coordinates, cut from the rest loop of `geom`.
pub(crate) fn segments(
    geom: &RigGeom,
    side: Side,
    params: &SuspensionParams,
) -> Vec<(Collider, BeltSegment)> {
    let rest = geom.rest.get(side);
    let mut pins = build_route(rest, geom.belt_len()).pts;
    let first = *pins.first().expect("a certified running gear has a route");
    if pins.last() != Some(&first) {
        pins.push(first);
    }
    let knots =
        super::envelope::wheel_travel_knots(rest, geom.side_droop_travel(side, params).effective);
    let travel = TravelField { knots: &knots };
    let reach = knots.iter().map(|&(_, t)| t).fold(0.0, f32::max);
    // DERIVED: a convex meeting a plane leaves along its axis of least overlap, so the outer face
    // stays the exit only while the slab is over twice as deep as the deepest overlap the stop
    // admits (the whole droop plus the bump reserve). Inward depth from the pin line:
    let inward = 2.0 * (reach + params.bump_stop);
    let length = super::route::polyline_len(&pins);
    let count = (length / SEGMENT_M).round() as usize;
    assert!(
        count >= 3,
        "the {side:?} belt loop is {length} m: a certified running gear closes a loop of at least \
         three segments"
    );
    let spacing = length / count as f32;
    let stations: Vec<Vec2> = (0..count)
        .map(|i| at_arc(&pins, i as f32 * spacing))
        .collect();
    // Shoelace sign: the outward side of the loop's direction of travel.
    let area: f32 = pins.windows(2).map(|w| w[0].perp_dot(w[1])).sum();
    let outward = |t: Vec2| if area > 0.0 { -t.perp() } else { t.perp() };
    let normal = |i: usize| {
        let prev = stations[(i + count - 1) % count];
        let next = stations[(i + 1) % count];
        outward((next - prev).normalize())
    };
    let cols = geom.grip_columns(side);
    let (x_in, x_out) = (cols[0].0, cols[2].0);
    // The spring channel's travel: ground-facing stations only, as its probes.
    let droop = |i: usize| {
        let (p, n) = (stations[i], normal(i));
        if n.y < 0.0 { travel.at(p.x) } else { 0.0 }
    };
    let faces = |i: usize| {
        let (p, n) = (stations[i], normal(i));
        [p + n * (geom.model.pin_to_outer + droop(i)), p - n * inward]
    };
    (0..count)
        .filter_map(|i| {
            let j = (i + 1) % count;
            if droop(i) == 0.0 && droop(j) == 0.0 {
                return None; // No travel: the stop is the belt's contact from first touch.
            }
            let points: Vec<Vec3> = faces(i)
                .into_iter()
                .chain(faces(j))
                .flat_map(|p| [Vec3::new(x_in, p.y, p.x), Vec3::new(x_out, p.y, p.x)])
                .collect();
            let collider = Collider::convex_hull(points).unwrap_or_else(|| {
                panic!("the {side:?} belt segment {i} is degenerate: the shoe has width and depth")
            });
            let (a, b) = (faces(i)[0], faces(j)[0]);
            let n = outward((b - a).normalize());
            let outward = Vec3::new(0.0, n.y, n.x);
            Some((
                collider,
                BeltSegment {
                    length: spacing,
                    outward,
                    face: outward.dot(Vec3::new(0.0, a.y, a.x)),
                },
            ))
        })
        .collect()
}

/// The point at arc length `s` along the open polyline `pins`.
fn at_arc(pins: &[Vec2], mut s: f32) -> Vec2 {
    for w in pins.windows(2) {
        let len = w[0].distance(w[1]);
        if s <= len && len > 0.0 {
            return w[0].lerp(w[1], s / len);
        }
        s -= len;
    }
    *pins.last().expect("a route has points")
}

impl BeltSegment {
    /// Shape one pair's manifolds on this segment (`sign` −1 when it is collider 1): the world
    /// touches the shoe only at its outer face, and each point in overlap takes the envelope law
    /// `(k', c')` (per metre of belt) over this segment's length, shared over the pair's points.
    /// [`share_segment_laws`] then shares it over every pair the segment touches.
    pub(crate) fn shape_contacts(
        &self,
        manifolds: &mut [ContactManifold],
        (position, rotation): (&Position, &Rotation),
        sign: f32,
        (stiffness, damping): (f32, f32),
    ) {
        let outward = rotation.0 * self.outward;
        let face = self.face + outward.dot(position.0);
        for manifold in manifolds.iter_mut() {
            let normal = manifold.normal;
            if (normal * -sign).dot(outward) < OUTER_FACE_COS {
                for point in &mut manifold.points {
                    let other = point.point + sign * normal * (point.penetration / 2.0);
                    point.penetration = face - outward.dot(other);
                }
                manifold.normal = outward * -sign;
            }
        }
        // A compliant point at positive separation applies nothing, and the speculative margin
        // grows with speed, so only overlap divides the law. A pair with none keeps it whole: a
        // point that crosses within the step still has a law to evaluate.
        let overlapping = overlapping_points(manifolds).max(1) as f32;
        let law = ContactCompliance {
            stiffness: stiffness * self.length / overlapping,
            damping: damping * self.length / overlapping,
        };
        for manifold in manifolds {
            manifold.compliance = Some(law);
        }
    }
}

fn overlapping_points(manifolds: &[ContactManifold]) -> usize {
    manifolds
        .iter()
        .flat_map(|manifold| &manifold.points)
        .filter(|point| point.penetration > 0.0)
        .count()
}

/// The narrow phase stated each point's approach speed before the hook chose its final normal, and
/// `ContactTransfers` publishes it for landings (`debris::crumble`); restates it along that normal.
fn state_final_approach(
    pair: &mut ContactPair,
    bodies: &Query<(&LinearVelocity, &AngularVelocity)>,
) {
    // A collider with no rigid body is fixed, and has no velocity to read.
    let at_anchor = |body: Option<Entity>, anchor: Vec3| {
        body.map_or(Vec3::ZERO, |body| {
            let (linear, angular) = bodies
                .get(body)
                .expect("a contact's rigid body carries its velocities");
            linear.0 + angular.0.cross(anchor)
        })
    };
    let (body1, body2) = (pair.body1, pair.body2);
    for manifold in &mut pair.manifolds {
        let normal = manifold.normal;
        for point in &mut manifold.points {
            point.normal_speed =
                (at_anchor(body2, point.anchor2) - at_anchor(body1, point.anchor1)).dot(normal);
        }
    }
}

/// Finish every pair a segment touches, after the narrow phase: state each point's approach along
/// its final normal, and share the segment's whole law over every point in overlap, so pieces side
/// by side or stacked cannot multiply its support. Runs after the narrow phase, which owns the
/// graph, and before the solver reads the laws: serial, one pass over the contact graph, for the
/// 14.5 narrow pairs a tank on a pile (MEASURED).
pub(crate) fn share_segment_laws(
    mut graph: ResMut<ContactGraph>,
    segments: Query<Entity, With<BeltSegment>>,
    bodies: Query<(&LinearVelocity, &AngularVelocity)>,
    mut shared: Local<Vec<(Entity, Entity, usize)>>,
) {
    for segment in &segments {
        shared.clear();
        let mut total = 0;
        for edge in graph.contact_edges_with(segment) {
            if !edge.is_touching() || edge.is_sleeping() {
                continue;
            }
            let pair = graph
                .get_pair_by_edge(edge)
                .expect("a contact edge names its pair");
            let points = overlapping_points(&pair.manifolds);
            total += points;
            shared.push((pair.collider1, pair.collider2, points));
        }
        for &(a, b, points) in shared.iter() {
            let (_, pair) = graph
                .get_mut(a, b)
                .expect("a pair read this step is still in the graph");
            state_final_approach(pair, &bodies);
            if points == 0 {
                continue; // Its points apply nothing, so nothing of the law is theirs to share.
            }
            let share = points as f32 / total as f32;
            for manifold in &mut pair.manifolds {
                let law = manifold
                    .compliance
                    .as_mut()
                    .expect("a segment's touching pair was shaped by the contact hook this step");
                law.stiffness *= share;
                law.damping *= share;
            }
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    use avian3d::prelude::{PhysicsSchedule, PhysicsStepSystems};
    app.add_systems(
        PhysicsSchedule,
        share_segment_laws
            .after(PhysicsStepSystems::NarrowPhase)
            .before(PhysicsStepSystems::Solver),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::collision::contact_types::{ContactEdge, ContactEdgeFlags, ContactPairFlags};
    use avian3d::prelude::ContactPoint;
    use bevy::ecs::system::RunSystemOnce;

    /// Fixture law and segment.
    const LAW: (f32, f32) = (2.0e6, 1.0e5);
    const SEGMENT: BeltSegment = BeltSegment::FIXTURE;
    const WHOLE: (f32, f32) = (LAW.0 * SEGMENT.length, LAW.1 * SEGMENT.length);

    fn shaped(mut manifolds: Vec<ContactManifold>, sign: f32) -> Vec<ContactManifold> {
        SEGMENT.shape_contacts(
            &mut manifolds,
            (&Position::default(), &Rotation::default()),
            sign,
            LAW,
        );
        manifolds
    }

    /// `(height of the world point, penetration)` for each point.
    fn manifold(points: &[(f32, f32)], normal: Vec3) -> ContactManifold {
        ContactManifold::new(
            points.iter().map(|&(y, penetration)| {
                ContactPoint::new(Vec3::ZERO, Vec3::ZERO, Vec3::Y * y, penetration)
            }),
            normal,
        )
    }

    fn law(manifold: &ContactManifold) -> (f32, f32) {
        let law = manifold
            .compliance
            .expect("a shaped manifold carries the law");
        (law.stiffness, law.damping)
    }

    /// A compliant point at positive separation applies nothing, and the speculative margin
    /// grows with speed: a share for it would soften the segment with speed and step the law
    /// when a point is pruned. The law covers a pair's manifolds together.
    #[test]
    fn speculative_points_take_no_share_of_the_law() {
        for sign in [-1.0, 1.0] {
            let normal = SEGMENT.outward * -sign;
            let manifolds = shaped(
                vec![
                    manifold(&[(0.0, 0.01), (0.0, -0.02)], normal),
                    manifold(&[(0.0, 0.01), (0.0, -0.02)], normal),
                ],
                sign,
            );
            for manifold in &manifolds {
                assert_eq!(law(manifold), (WHOLE.0 / 2.0, WHOLE.1 / 2.0));
            }
        }
    }

    /// The rewrite states each point's depth to the outer face, so that depth is the one that
    /// says who shares the law. Fixture: a side-face normal; the first point is proud of the
    /// outer face only after the rewrite, the second only before it.
    #[test]
    fn a_point_counts_by_its_depth_to_the_outer_face() {
        for sign in [-1.0, 1.0] {
            let manifolds = shaped(
                vec![manifold(
                    &[(0.02, -0.01), (-0.02, 0.01), (0.01, -0.01)],
                    Vec3::X,
                )],
                sign,
            );
            assert_eq!(manifolds[0].normal, SEGMENT.outward * -sign);
            assert_eq!(law(&manifolds[0]), (WHOLE.0 / 2.0, WHOLE.1 / 2.0));
        }
    }

    /// The solver divides by `c + hk`: a pair with no point in overlap keeps a law it can
    /// evaluate should one cross within the step.
    #[test]
    fn a_wholly_speculative_pair_keeps_a_finite_law() {
        let manifolds = shaped(vec![manifold(&[(0.0, -0.01)], Vec3::NEG_Y)], -1.0);
        assert_eq!(law(&manifolds[0]), WHOLE);
    }

    fn contact_world() -> World {
        let mut world = World::new();
        world.init_resource::<ContactGraph>();
        world
    }

    fn touching(
        world: &mut World,
        (collider1, collider2): (Entity, Entity),
        bodies: (Entity, Entity),
        manifolds: Vec<ContactManifold>,
    ) {
        let mut manifolds = Some(manifolds);
        let mut graph = world.resource_mut::<ContactGraph>();
        graph
            .add_edge_with(ContactEdge::new(collider1, collider2), |pair| {
                pair.body1 = Some(bodies.0);
                pair.body2 = Some(bodies.1);
                pair.manifolds = manifolds.take().expect("one pair per edge");
                pair.flags.insert(ContactPairFlags::TOUCHING);
            })
            .expect("a new edge");
        graph
            .get_edge_mut(collider1, collider2)
            .expect("the edge just added")
            .flags
            .insert(ContactEdgeFlags::TOUCHING);
    }

    fn finish(world: &mut World) {
        world.run_system_once(share_segment_laws).unwrap();
    }

    /// A segment's whole law is shared over every point that overlaps, across every piece, so
    /// pieces side by side cannot multiply its support; a pair still clear of the face takes no
    /// part in the division. Fixture: two overlapping points on one piece, one on another.
    #[test]
    fn the_law_is_shared_over_every_overlapping_point_the_segment_touches() {
        let mut world = contact_world();
        let segment = world.spawn(SEGMENT).id();
        let at_rest = (LinearVelocity::ZERO, AngularVelocity::ZERO);
        let hull = world.spawn(at_rest).id();
        let pieces: [Entity; 3] = std::array::from_fn(|_| world.spawn(at_rest).id());
        let down = Vec3::NEG_Y;
        for (piece, points) in pieces.into_iter().zip([
            vec![(0.0, 0.01), (0.0, 0.02), (0.0, -0.02)],
            vec![(0.0, 0.01)],
            vec![(0.0, -0.01), (0.0, -0.02)],
        ]) {
            let manifolds = shaped(vec![manifold(&points, down)], -1.0);
            touching(&mut world, (segment, piece), (hull, piece), manifolds);
        }
        finish(&mut world);
        let graph = world.resource::<ContactGraph>();
        let carried = |piece: Entity| {
            let (_, pair) = graph.get(segment, piece).expect("the pair");
            let points = pair.manifolds[0]
                .points
                .iter()
                .filter(|p| p.penetration > 0.0)
                .count() as f32;
            let (k, c) = law(&pair.manifolds[0]);
            (points * k, points * c, k)
        };
        let (one, two, clear) = (carried(pieces[0]), carried(pieces[1]), carried(pieces[2]));
        assert!((one.0 + two.0 - WHOLE.0).abs() < WHOLE.0 * 1e-5);
        assert!((one.1 + two.1 - WHOLE.1).abs() < WHOLE.1 * 1e-5);
        assert_eq!(one.2, two.2, "every overlapping point takes an equal share");
        assert_eq!(clear.2, WHOLE.0);
    }

    /// The narrow phase states `normal_speed` along the normal it found, and landings are priced
    /// from it (`debris::crumble`). A redirected contact closes along its final normal, at the
    /// velocity of each body's own anchor. Fixture: the hull sinks at 2 m/s and spins so its
    /// anchor rises 1 m/s, onto a piece at rest: it closes at 1 m/s whichever collider is first.
    #[test]
    fn a_redirected_contact_states_its_approach_along_the_final_normal() {
        for segment_first in [true, false] {
            let sign = if segment_first { -1.0 } else { 1.0 };
            let mut world = contact_world();
            let segment = world.spawn(SEGMENT).id();
            let hull = world
                .spawn((LinearVelocity(Vec3::NEG_Y * 2.0), AngularVelocity(Vec3::Z)))
                .id();
            let piece = world
                .spawn((LinearVelocity::ZERO, AngularVelocity::ZERO))
                .id();
            let (anchor1, anchor2) = if segment_first {
                (Vec3::X, Vec3::ZERO)
            } else {
                (Vec3::ZERO, Vec3::X)
            };
            let side_face = ContactManifold::new(
                [ContactPoint::new(anchor1, anchor2, Vec3::Y * 0.01, 0.01)],
                Vec3::X,
            );
            let manifolds = shaped(vec![side_face], sign);
            let (colliders, bodies) = if segment_first {
                ((segment, piece), (hull, piece))
            } else {
                ((piece, segment), (piece, hull))
            };
            touching(&mut world, colliders, bodies, manifolds);
            finish(&mut world);
            let (_, pair) = world
                .resource::<ContactGraph>()
                .get(colliders.0, colliders.1)
                .expect("the pair");
            let speed = pair.manifolds[0].points[0].normal_speed;
            assert!(
                (speed + 1.0).abs() < 1e-5,
                "segment first {segment_first}: {speed}"
            );
        }
    }
}
