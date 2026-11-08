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
//! - **Support: the spring channel's law, per area of shoe face.** The calibrated per-metre
//!   envelope law `k'` holds over the shoe's width `W`, so it is `k'/W` per square metre of face:
//!   [`BeltSegment::shape_contacts`] gives a pair `k'/W` times the area of the segment's outer
//!   face the piece covers ([`BeltSegment::covered_area`], the piece's silhouette on the face
//!   clipped to it; `ContactCompliance`, an OVERMATCH PATCH in the vendored solver). A piece over
//!   the whole face takes `k' × length`, the sampled support's law; a narrower one takes its
//!   share, as the sampled support's covered cells do. The silhouette overstates a tilted piece's
//!   area until it embeds (DERIVED: the sampled support reads the depth profile instead); the
//!   force still grows from zero with penetration. [`share_segment_laws`] caps a segment's pairs
//!   at its whole law, so stacked pieces cannot multiply the support. Past the wheels' travel the
//!   piece meets the same outer-face stop as the ground. The end wheels have no travel and no
//!   segment: the stop is hard there from first touch.
//! - **Drivetrain.** Each segment carries the stop's `BeltFace`, so the shared hook moves its
//!   surface at belt speed and `stop_contacts::by_owner` reads its friction into the transmission
//!   with the stop's.

use avian3d::prelude::{
    Collider, CollisionLayers, ContactCompliance, ContactGraph, ContactManifold, Position, Rotation,
};
use bevy::prelude::*;

use super::derive::SuspensionParams;
use super::forces::TravelField;
use super::rig_geom::RigGeom;
use super::route::build_route;
use super::side::Side;
use super::world_contact::convex::{clip, signed_area};
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
    /// Loop length this segment stands for (m): its share of the per-metre envelope law.
    length: f32,
    /// Body-local unit normal of the outer face, out of the loop.
    outward: Vec3,
    /// The outer face's plane, `outward · p` for every body-local point `p` on it (m).
    face: f32,
    /// Body-local unit tangent of the outer face along the belt.
    along: Vec3,
    /// `along · p` of the outer face's start (m).
    start: f32,
    /// The shoe's lateral bounds, body-local x (m).
    lateral: (f32, f32),
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
    let count = (length / SEGMENT_M).round().max(3.0) as usize;
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
            let t = (b - a).normalize();
            let along = Vec3::new(0.0, t.y, t.x);
            let start = Vec3::new(0.0, a.y, a.x);
            Some((
                collider,
                BeltSegment {
                    length: spacing,
                    outward,
                    face: outward.dot(start),
                    along,
                    start: along.dot(start),
                    lateral: (x_in.min(x_out), x_in.max(x_out)),
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

/// The convex hull of `points` in the plane, counter-clockwise, by Andrew's monotone chain.
fn convex_hull(mut points: Vec<Vec2>) -> Vec<Vec2> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let chain = |points: &mut dyn Iterator<Item = Vec2>| {
        let mut hull: Vec<Vec2> = Vec::new();
        for p in points {
            while let [.., a, b] = hull[..] {
                if (b - a).perp_dot(p - a) > 0.0 {
                    break;
                }
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop(); // Each end starts the other chain.
        hull
    };
    let mut hull = chain(&mut points.iter().copied());
    hull.extend(chain(&mut points.iter().rev().copied()));
    hull
}

impl BeltSegment {
    /// The area of this segment's outer face that the convex `points` (world) cover (m²): their
    /// silhouette on the face, clipped to it. The face's frame is along the belt and across the
    /// shoe, with its start at zero.
    pub(crate) fn covered_area(
        &self,
        points: impl Iterator<Item = Vec3>,
        (position, rotation): (&Position, &Rotation),
    ) -> f32 {
        let to_body = rotation.0.inverse();
        let silhouette = convex_hull(
            points
                .map(|p| {
                    let local = to_body * (p - position.0);
                    Vec2::new(self.along.dot(local) - self.start, local.x)
                })
                .collect(),
        );
        let polygon: Vec<Vec3> = silhouette.iter().map(|p| p.extend(1.0)).collect();
        let (lo, hi) = self.lateral;
        // Homogeneous half-planes of the face rectangle, each kept on its nonnegative side.
        let clipped = [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(-1.0, 0.0, self.length),
            Vec3::new(0.0, 1.0, -lo),
            Vec3::new(0.0, -1.0, hi),
        ]
        .into_iter()
        .fold(polygon, |polygon, plane| clip(&polygon, plane));
        signed_area(&clipped).abs()
    }

    /// The face's whole area (m²).
    fn face_area(&self) -> f32 {
        self.length * (self.lateral.1 - self.lateral.0)
    }

    /// Shape one pair's manifolds on this segment (`sign` −1 when it is collider 1): the world
    /// touches the shoe only at its outer face, and the pair takes the envelope law `(k', c')`
    /// (per metre of belt) over the part of the face it covers, `covered` (m²), shared over its
    /// points. A pair covering none of the face has no contact: its manifolds are emptied.
    pub(crate) fn shape_contacts(
        &self,
        manifolds: &mut Vec<ContactManifold>,
        (position, rotation): (&Position, &Rotation),
        sign: f32,
        (stiffness, damping): (f32, f32),
        covered: f32,
    ) {
        let points = manifolds.iter().map(|m| m.points.len()).sum::<usize>();
        if points == 0 {
            return; // Nothing touches this step.
        }
        if covered <= 0.0 {
            manifolds.clear(); // No face area under the piece: nothing for the law to act on.
            return;
        }
        // A piece covering the whole face takes the segment's length of the per-metre law.
        let share = self.length * (covered / self.face_area()).min(1.0) / points as f32;
        let law = ContactCompliance {
            stiffness: stiffness * share,
            damping: damping * share,
        };
        // A slab's end, side and inner faces are the cut of a continuous belt, never a surface of
        // it: a normal from them turns to the outer face's, its depth the other body's point past
        // that face.
        let outward = rotation.0 * self.outward;
        let face = self.face + outward.dot(position.0);
        for manifold in manifolds.iter_mut() {
            let normal = manifold.normal;
            if (normal * -sign).dot(outward) < OUTER_FACE_COS {
                for point in &mut manifold.points {
                    // `point` is midway between the two surfaces along `normal`, 1 into 2.
                    let other = point.point + sign * normal * (point.penetration / 2.0);
                    point.penetration = face - outward.dot(other);
                }
                manifold.normal = outward * -sign;
            }
            manifold.compliance = Some(law);
        }
    }
}

/// Cap each segment's contacts this step at its whole law: pieces covering more of its face
/// than it has (stacked or overlapping silhouettes) share it, so a segment never carries more
/// than its length's stiffness. The hook sees one pair at a time, so the cap needs every pair of
/// the segment at once. Serial over segments with a contact: the shared contact graph is mutated
/// between the narrow phase and the solver's constraint preparation, exclusive by construction.
pub(crate) fn share_segment_laws(
    mut graph: ResMut<ContactGraph>,
    segments: Query<(Entity, &BeltSegment)>,
    law: Option<Res<SegmentLaw>>,
    mut pairs: Local<Vec<(Entity, Entity)>>,
) {
    let Some(law) = law else {
        return; // No driver has calibrated a belt: nothing is shaped yet.
    };
    for (entity, segment) in &segments {
        pairs.clear();
        let mut total = 0.0_f32;
        for edge in graph.contact_edges_with(entity) {
            if !edge.is_touching() || edge.is_sleeping() {
                continue; // A sleeping pair keeps its last step's law.
            }
            let pair = graph
                .get_pair_by_edge(edge)
                .expect("a contact edge names its pair");
            total += pair
                .manifolds
                .iter()
                .filter_map(|m| {
                    m.compliance
                        .map(|law| law.stiffness * m.points.len() as f32)
                })
                .sum::<f32>();
            pairs.push((pair.collider1, pair.collider2));
        }
        let whole = law.stiffness_per_m * segment.length;
        if pairs.len() < 2 || total <= whole {
            continue; // A pair's own coverage is at most the face; the sum is within it.
        }
        for &(a, b) in pairs.iter() {
            let (_, pair) = graph
                .get_mut(a, b)
                .expect("a pair read this step is still in the graph");
            for manifold in &mut pair.manifolds {
                if let Some(law) = &mut manifold.compliance {
                    law.stiffness *= whole / total;
                    law.damping *= whole / total;
                }
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

    /// A segment's stiffness follows the shoe face a piece covers, the sampled support's own
    /// per-area law. Whatever relies on it (the ride height on small pieces) reads the area this
    /// returns: a mutant that gives every pair the whole face, or ignores the shoe's bounds,
    /// fails here. FIXTURE DATA: a 0.3 m × 0.6 m face on the ground, boxes below it.
    #[test]
    fn a_segment_covers_only_the_face_area_under_a_piece() {
        let segment = BeltSegment {
            length: 0.3,
            outward: Vec3::NEG_Y,
            face: 0.0,
            along: Vec3::Z,
            start: 0.0,
            lateral: (-0.3, 0.3),
        };
        let pose = (&Position(Vec3::ZERO), &Rotation::default());
        let boxed = |x: (f32, f32), z: (f32, f32)| {
            [x.0, x.1].into_iter().flat_map(move |x| {
                [z.0, z.1]
                    .into_iter()
                    .flat_map(move |z| [Vec3::new(x, -0.1, z), Vec3::new(x, -0.3, z)])
            })
        };
        let area = |x, z| segment.covered_area(boxed(x, z), pose);
        // The whole face, by a piece larger than it.
        assert!((area((-1.0, 1.0), (-1.0, 1.0)) - segment.face_area()).abs() < 1e-6);
        // Half the shoe's width.
        assert!((area((0.0, 1.0), (-1.0, 1.0)) - segment.face_area() / 2.0).abs() < 1e-6);
        // A piece narrower than the shoe and shorter than the segment.
        assert!((area((-0.05, 0.05), (0.1, 0.2)) - 0.1 * 0.1).abs() < 1e-6);
        // Beyond the face's end and beyond the shoe's edge.
        assert_eq!(area((-1.0, 1.0), (0.4, 0.5)), 0.0);
        assert_eq!(area((0.5, 1.0), (-1.0, 1.0)), 0.0);
    }
}
