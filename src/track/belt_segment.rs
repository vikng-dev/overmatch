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
//!   [`share_segment_laws`] shares it over every point the segment touches, so pieces side by
//!   side or stacked cannot multiply the support. The sampled support differs at partial
//!   coverage: over ground it is a column quadrature, so a curb under one edge takes that
//!   column's share. Past the wheels' travel the piece meets the same outer-face stop as the
//!   ground. The end wheels have no travel and no segment: the stop is hard there from first
//!   touch.
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
    length: f32,
    /// Body-local unit normal of the outer face, out of the loop.
    outward: Vec3,
    /// The outer face's plane, `outward · p` for every body-local point `p` on it (m).
    face: f32,
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
    /// touches the shoe only at its outer face, and each point takes the envelope law
    /// `(k', c')` (per metre of belt) over this segment's length, shared over the pair's points.
    /// [`share_segment_laws`] then shares it over every pair the segment touches.
    pub(crate) fn shape_contacts(
        &self,
        manifolds: &mut [ContactManifold],
        (position, rotation): (&Position, &Rotation),
        sign: f32,
        (stiffness, damping): (f32, f32),
    ) {
        let points = manifolds.iter().map(|m| m.points.len()).sum::<usize>();
        if points == 0 {
            return; // Nothing touches this step.
        }
        let law = ContactCompliance {
            stiffness: stiffness * self.length / points as f32,
            damping: damping * self.length / points as f32,
        };
        // A slab's end, side and inner faces are the cut of a continuous belt, never a surface of
        // it: a normal from them turns to the outer face's, its depth the other body's point past
        // that face.
        let outward = rotation.0 * self.outward;
        let face = self.face + outward.dot(position.0);
        for manifold in manifolds {
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

/// Share each segment's law over every point it touches this step, across all its pairs: the
/// hook sees one pair at a time, so a segment over two pieces would otherwise carry its length's
/// stiffness twice. Serial over segments with a contact: the shared contact graph is mutated
/// between the narrow phase and the solver's constraint preparation, exclusive by construction.
pub(crate) fn share_segment_laws(
    mut graph: ResMut<ContactGraph>,
    segments: Query<Entity, With<BeltSegment>>,
    mut shared: Local<Vec<(Entity, Entity, usize)>>,
) {
    for segment in &segments {
        shared.clear();
        let mut total = 0;
        for edge in graph.contact_edges_with(segment) {
            if !edge.is_touching() || edge.is_sleeping() {
                continue; // A sleeping pair keeps its last step's shared law.
            }
            let pair = graph
                .get_pair_by_edge(edge)
                .expect("a contact edge names its pair");
            let points = pair
                .manifolds
                .iter()
                .filter(|m| m.compliance.is_some())
                .map(|m| m.points.len())
                .sum::<usize>();
            if points > 0 {
                total += points;
                shared.push((pair.collider1, pair.collider2, points));
            }
        }
        if shared.len() < 2 {
            continue; // One pair already holds the segment's whole law.
        }
        for &(a, b, points) in shared.iter() {
            let (_, pair) = graph
                .get_mut(a, b)
                .expect("a pair read this step is still in the graph");
            let share = points as f32 / total as f32;
            for manifold in &mut pair.manifolds {
                if let Some(law) = &mut manifold.compliance {
                    law.stiffness *= share;
                    law.damping *= share;
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
