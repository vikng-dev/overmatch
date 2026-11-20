//! The belt against loose pieces, in the game's tank driving physics (game state only).
//!
//! Terrain and standing construction meet the belt through the sampled support (`forces`) and the
//! outer-face stop (`bump_stop`). A loose piece meets one run collider per side, as a solver
//! contact, so the piece takes the equal and opposite reaction through Avian:
//!
//! - **Shape.** [`RunShape::build`]: the convex outline of the rest belt's outer face, extruded
//!   across the shoe's measured faces. Built at spawn from construction data and hung on the hull
//!   with the stop (`bump_stop::spawn_track_colliders`; ADR-0014), on [`run_layers`]: a run meets
//!   loose pieces and nothing else, and the sampled support reads no collider a run meets
//!   (`rubble::RubbleQuery::near`), so every solid has one channel. No side plate meets a loose
//!   piece (`bump_stop`); the stop does, as the hard limit at full bump travel.
//! - **Support.** The run is read as links of one pitch along its outline, and each link carries
//!   the running gear's calibrated law per metre (`forces::SupportLaw`, the sampled support's own)
//!   times its length, at the deepest overlap any contact gives it, once however many pieces press
//!   it ([`RunContacts`]). A piece's manifold is read on the one face of the convex run that faces
//!   its normal ([`SoftRun::facing`]). On a rim face its points are projected onto the face's
//!   outline edge, and its overlap runs along it, linear between them. A link takes the deepest
//!   that reaches on its length, nothing where that is zero or less. A link is rigid along its
//!   length, so one point loads a whole link. The contacts on a link share its stiffness and
//!   damping in proportion to their depth there. A contact's deepest points that bear split its
//!   share evenly, and a common rest length (`ContactCompliance::rest`) carries its force at the
//!   step's start, so its law is affine and its tangent stiffness the links' own; the solver leaves
//!   its other points inert. A contact on a lateral cap carries one link's law along its normal,
//!   outside the links' budget ([`Face::Cap`]). The datum is the rest face, so a piece's force adds
//!   to the ground's under the same belt, which the sampled support carries: a limit (#354), as
//!   the cap law is. The reading's limits are the design doc's
//!   (`.agents/docs/design/track-model/track-contact.md`).
//! - **Drivetrain.** A run binds its side's `BeltFace` to the contact coordinate. The owner
//!   commits the exchanged momentum; `stop_contacts::by_owner` reports traction to the Driver.
//!
//! Each run's laws come from a shared read of the contact graph, every run apart ([`read_runs`]);
//! one short step publishes them in the step-local law store ([`write_runs`]). Both run in the
//! solver, after Avian's solver-start wake and before it prepares the step's contact constraints:
//! a pair whose island a new contact wakes there is solved that step with the manifold it slept
//! with, so it is read then, not skipped as asleep.

use std::ops::Range;

use crate::avian_ext::compliance::{ContactCompliance, Laws};
use avian3d::collision::contact_types::{ContactId, ContactManifold};
use avian3d::prelude::{
    Collider, ColliderOf, CollisionLayers, ContactGraph, PhysicsSchedule, PhysicsStepSystems,
    Position, Rotation, SolverSystems,
};
use bevy::prelude::*;

use super::forces::SupportLaw;
use super::rig_geom::RigGeom;
use super::route::build_route;
use super::side::Side;
use super::stop_contacts::{Bearing, BeltFace};
use crate::Layer;

/// One side's run as a tank kind's track shapes cache it (`bump_stop::TrackColliders`): its
/// body-local collider, its moving face and its links.
#[derive(Clone)]
pub(crate) struct RunShape {
    collider: Collider,
    face: BeltFace,
    run: SoftRun,
}

impl RunShape {
    pub(crate) fn build(geom: &RigGeom, side: Side) -> Self {
        let outline = outline(geom, side);
        Self {
            collider: super::bump_stop::across_shoe(geom, side, &outline),
            run: SoftRun::new(&outline, geom.pitch),
            face: BeltFace::new(side, outline, Bearing::Whole),
        }
    }

    pub(crate) fn collider(&self) -> &Collider {
        &self.collider
    }

    /// What a run's entity carries beside its pose, parent and friction.
    pub(crate) fn bundle(&self) -> impl Bundle {
        (
            self.run.clone(),
            self.face.clone(),
            self.collider.clone(),
            run_layers(),
        )
    }
}

pub(crate) fn run_layers() -> CollisionLayers {
    CollisionLayers::new([Layer::TrackBelt], [Layer::Debris])
}

/// A run collider: one side's belt against loose pieces, and the links its contacts are read on.
#[derive(Component, Clone, Debug)]
#[require(RunContacts)]
pub(crate) struct SoftRun {
    /// The run's outline in the side plane `(z, y)`, counter-clockwise from its first vertex: one
    /// rim face of the collider per edge.
    edges: Vec<Edge>,
    /// The closed outline's links: its length over the belt's pitch, rounded.
    links: usize,
    /// The belt one link stands for (m): the outline's length over its links, so the links tile
    /// it exactly; it parts from the pitch by at most half a pitch over all the links (DERIVED).
    link: f32,
}

/// One edge of a run's outline in the side plane `(z, y)`.
#[derive(Clone, Copy, Debug)]
struct Edge {
    /// Its first vertex, and that vertex's arc from the outline's first (m).
    start: Vec2,
    arc: f32,
    /// Its length (m), its direction round the outline and its outward normal, both unit.
    length: f32,
    dir: Vec2,
    out: Vec2,
}

impl Edge {
    /// How far along this edge side-plane point `p`'s foot stands, clamped to the edge (m).
    fn foot(&self, p: Vec2) -> f32 {
        (p - self.start).dot(self.dir).clamp(0.0, self.length)
    }
}

/// The face of a run a manifold is read on.
#[derive(Clone, Copy, Debug)]
enum Face {
    /// The rim face over one outline edge, by its index.
    Rim(usize),
    /// Either lateral cap. `StandIn`: a cap contact carries one link's law along its normal, split
    /// over its points in overlap, outside the links' budget: the solid's lateral continuation,
    /// the rim's own at a flip for a piece alone on one link. The stop is the lateral limit
    /// (`.agents/docs/design/track-model/track-contact.md`).
    Cap,
}

impl SoftRun {
    /// The run of side-plane `outline`, closed and counter-clockwise, read as links of about
    /// `pitch`.
    fn new(outline: &[Vec2], pitch: f32) -> Self {
        let mut length = 0.0;
        let edges: Vec<Edge> = outline
            .iter()
            .zip(outline.iter().cycle().skip(1))
            .map(|(&a, &b)| {
                let dir = (b - a).normalize();
                let edge = Edge {
                    start: a,
                    arc: length,
                    length: a.distance(b),
                    dir,
                    out: Vec2::new(dir.y, -dir.x),
                };
                length += edge.length;
                edge
            })
            .collect();
        assert!(
            edges
                .iter()
                .zip(edges.iter().cycle().skip(1))
                .all(|(a, b)| a.dir.perp_dot(b.dir) > 0.0),
            "a run outline turns left at every vertex of the side plane (z, y): {outline:?}"
        );
        let links = (length / pitch).round() as usize;
        assert!(
            links >= 3,
            "a run outline {length} m round holds at least three links of {pitch} m"
        );
        Self {
            edges,
            links,
            link: length / links as f32,
        }
    }

    /// The face a manifold whose normal leaves the run along body-frame `out` is read on, by the
    /// narrow phase's own rule (parry's `ConvexPolyhedron::local_support_feature`): the face whose
    /// outward normal lies nearest `out`. An exact tie between two edges goes to the one whose
    /// outward normal points lowest in the side plane, then toward least `z`: a key of the
    /// outline's shape, so the law does not depend on where its numbering starts. Parry may take
    /// the other (`track-contact.md`, "One face per piece", bounds where its points then land).
    fn facing(&self, out: Vec3) -> Face {
        let side = Vec2::new(out.z, out.y);
        let (rim, nearest) = (self.edges.iter().map(|edge| edge.out.dot(side)).enumerate())
            .max_by(|a, b| {
                let (ka, kb) = (self.edges[a.0].out, self.edges[b.0].out);
                (a.1.total_cmp(&b.1))
                    .then(kb.y.total_cmp(&ka.y))
                    .then(kb.x.total_cmp(&ka.x))
            })
            .expect("a run outline has edges");
        if out.x.abs() > nearest {
            Face::Cap
        } else {
            Face::Rim(rim)
        }
    }

    /// Where along the closed outline side-plane point `p` lies on rim face `edge`: at its foot on
    /// the edge (m from the first vertex, up to the outline's length).
    fn arc_on(&self, edge: usize, p: Vec2) -> f32 {
        self.edges[edge].arc + self.edges[edge].foot(p)
    }

    /// The link holding arc `s` (m), round the closed outline.
    fn link_at(&self, s: f32) -> u32 {
        ((s / self.link).floor() as usize % self.links) as u32
    }

    /// The belt one link stands for (m).
    #[cfg(test)]
    pub(crate) fn link(&self) -> f32 {
        self.link
    }

    /// The run's outline in the side plane `(z, y)`, from its first vertex.
    #[cfg(test)]
    pub(crate) fn outline(&self) -> Vec<Vec2> {
        self.edges.iter().map(|edge| edge.start).collect()
    }

    /// Fixture: a run round a 3 m square, 96 links of 0.125 m (12 m round).
    #[cfg(test)]
    pub(crate) fn fixture() -> Self {
        let corners = [(0.0, 0.0), (3.0, 0.0), (3.0, 3.0), (0.0, 3.0)];
        Self::new(&corners.map(|(z, y)| Vec2::new(z, y)), 0.125)
    }
}

/// Resolution: the least turn a run outline's vertex keeps, as the sine of its angle (about 2°).
/// The raw route's near-collinear arc points make sliver faces, and the convex contact against a
/// piece answers them with loaded points 1.5–3 km deep (MEASURED with the raw route).
const OUTLINE_TURN_MIN: f32 = 0.035;

/// One side's run outline in the side plane `(z, y)`: the convex hull of the rest belt's outer
/// face, every vertex turning less than [`OUTLINE_TURN_MIN`] dropped.
fn outline(geom: &RigGeom, side: Side) -> Vec<Vec2> {
    let outer: Vec<(Vec2, f32)> = geom
        .rest
        .get(side)
        .iter()
        .map(|&(centre, radius)| (centre, radius + geom.model.pin_to_outer))
        .collect();
    let mut points = build_route(&outer, 0.0).pts;
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    points.dedup();
    let straight = |o: Vec2, a: Vec2, b: Vec2| {
        (a - o).perp_dot(b - a) <= OUTLINE_TURN_MIN * (a - o).length() * (b - a).length()
    };
    let chain = |points: &mut dyn Iterator<Item = Vec2>| {
        let mut out: Vec<Vec2> = Vec::new();
        for p in points {
            while out.len() >= 2 && straight(out[out.len() - 2], out[out.len() - 1], p) {
                out.pop();
            }
            out.push(p);
        }
        out.pop(); // The shared endpoint starts the other chain.
        out
    };
    let mut hull = chain(&mut points.iter().copied());
    hull.extend(chain(&mut points.iter().rev().copied()));
    hull
}

/// One touching manifold of a run this step.
struct Manifold {
    pair: ContactId,
    /// Its index in its pair's manifolds.
    index: usize,
    face: Face,
    /// Its links in overlap in [`RunContacts::depths`]: none on a cap.
    depths: Range<usize>,
    /// Its points' overlaps in [`RunContacts::overlaps`].
    overlaps: Range<usize>,
}

/// One run's touching contacts this step and the law each takes, rebuilt every step from the
/// contact graph. Its buffers keep their capacity from step to step.
#[derive(Component, Default)]
struct RunContacts {
    manifolds: Vec<Manifold>,
    /// Each manifold's links in overlap, `(link, depth)` (m, above zero), ascending by link.
    depths: Vec<(u32, f32)>,
    /// Each manifold's points' overlaps (m; a separated point's is negative), deepest first.
    overlaps: Vec<f32>,
    /// One manifold's points as `(arc, overlap)` (m; a separated point's overlap is negative).
    points: Vec<(f32, f32)>,
    /// One manifold's depth on each link its profile reaches, once per knot or stretch.
    span: Vec<(u32, f32)>,
    /// Every manifold's link depths grouped by link, each group ascending; where each link's
    /// group starts (one past the last link: the end); a cursor for the grouping.
    grouped: Vec<f32>,
    starts: Vec<usize>,
    cursor: Vec<usize>,
    /// Per link with a group: its deepest depth and its depths' sum (m).
    deepest: Vec<f32>,
    total: Vec<f32>,
    /// One manifold's terms per link: its deepest depth times the manifold's share there (m), and
    /// that share.
    terms: Vec<(f32, f32)>,
    /// Each manifold's law, in `manifolds` order.
    laws: Vec<ContactCompliance>,
}

impl RunContacts {
    fn clear(&mut self) {
        self.manifolds.clear();
        self.depths.clear();
        self.overlaps.clear();
        self.laws.clear();
    }

    /// Add `manifold`, `index` of pair `pair`, as the narrow phase left it, on `run` hung on a body
    /// at `body` (position, rotation); `first` says the run is the pair's first collider, which
    /// the normal leaves. On a rim face its overlap runs along the straight edge through its feet,
    /// the deepest where two stand together, linear between neighbours, and each link takes the
    /// deepest it reaches on the link's length; a cap reaches no link.
    fn read(
        &mut self,
        run: &SoftRun,
        (position, rotation): (Vec3, Quat),
        (pair, index): (ContactId, usize),
        manifold: &ContactManifold,
        first: bool,
    ) {
        let to_body = rotation.inverse();
        let leaving = if first {
            manifold.normal
        } else {
            -manifold.normal
        };
        let face = run.facing(to_body * leaving);
        // By value, so the order the points came in cannot move the law's sums.
        let first = self.overlaps.len();
        self.overlaps
            .extend(manifold.points.iter().map(|point| point.penetration));
        self.overlaps[first..].sort_unstable_by(|a, b| b.total_cmp(a));
        let start = self.depths.len();
        if let Face::Rim(edge) = face {
            self.points.clear();
            self.points.extend(manifold.points.iter().map(|point| {
                let local = to_body * (point.point - position);
                (
                    run.arc_on(edge, Vec2::new(local.z, local.y)),
                    point.penetration,
                )
            }));
            self.points
                .sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(b.1.total_cmp(&a.1)));
            self.points.dedup_by(|b, a| a.0 == b.0);
            self.span.clear();
            self.span
                .extend(self.points.iter().map(|&(s, d)| (run.link_at(s), d)));
            for knots in self.points.windows(2) {
                let [(s0, d0), (s1, d1)] = [knots[0], knots[1]];
                let at = |s: f32| d0 + (d1 - d0) * ((s - s0) / (s1 - s0));
                for link in (s0 / run.link).floor() as usize..=(s1 / run.link).floor() as usize {
                    let lo = s0.max(link as f32 * run.link);
                    let hi = s1.min((link + 1) as f32 * run.link);
                    self.span
                        .push(((link % run.links) as u32, at(lo).max(at(hi))));
                }
            }
            self.span
                .sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
            self.span.dedup_by_key(|entry| entry.0);
            self.depths
                .extend(self.span.iter().filter(|entry| entry.1 > 0.0));
        }
        self.manifolds.push(Manifold {
            pair,
            index,
            face,
            depths: start..self.depths.len(),
            overlaps: first..self.overlaps.len(),
        });
    }

    /// Each manifold's law under the running gear's `law`: a link carries `k'·link` and `c'·link`
    /// at its deepest depth, shared among the manifolds on it in proportion to their depth there.
    /// A manifold's shares are its stiffness and damping, and its links' deepest depths weighted by
    /// them its force at the step's start; its points that bear take them ([`shared`]). A cap
    /// manifold carries one link's law over its points in overlap ([`Face::Cap`]). A manifold with
    /// none in overlap takes the zero law, which the solver leaves inert; `None` would be a rigid
    /// contact.
    fn solve(&mut self, run: &SoftRun, law: &SupportLaw) {
        let links = run.links;
        // Grouped by link (a counting sort), each group by value, so a link's sum runs in one
        // order whatever order the manifolds came in.
        self.starts.clear();
        self.starts.resize(links + 1, 0);
        for &(link, _) in &self.depths {
            self.starts[link as usize + 1] += 1;
        }
        for link in 0..links {
            self.starts[link + 1] += self.starts[link];
        }
        self.cursor.clear();
        self.cursor.extend_from_slice(&self.starts[..links]);
        self.grouped.clear();
        self.grouped.resize(self.depths.len(), 0.0);
        for &(link, depth) in &self.depths {
            let at = &mut self.cursor[link as usize];
            self.grouped[*at] = depth;
            *at += 1;
        }
        self.deepest.resize(links, 0.0);
        self.total.resize(links, 0.0);
        for link in 0..links {
            let group = &mut self.grouped[self.starts[link]..self.starts[link + 1]];
            group.sort_unstable_by(f32::total_cmp);
            if let Some(&deepest) = group.last() {
                self.deepest[link] = deepest;
                self.total[link] = group.iter().sum();
            }
        }
        let (stiffness, damping) = (law.stiffness_per_m * run.link, law.damping_per_m * run.link);
        for manifold in &self.manifolds {
            self.terms.clear();
            self.terms.extend(
                self.depths[manifold.depths.clone()]
                    .iter()
                    .map(|&(link, depth)| {
                        let share = depth / self.total[link as usize];
                        (self.deepest[link as usize] * share, share)
                    }),
            );
            // Summed by value, so the links' numbering, which starts at the seam, cannot move it.
            self.terms
                .sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
            let (area, length) = self.terms.iter().fold((0.0, 0.0), |(area, length), term| {
                (area + term.0, length + term.1)
            });
            let overlaps = &self.overlaps[manifold.overlaps.clone()];
            let bearing = overlaps.iter().filter(|&&overlap| overlap > 0.0).count();
            self.laws.push(match manifold.face {
                Face::Rim(_) if length > 0.0 => shared(
                    overlaps,
                    (stiffness * length, damping * length),
                    area / length,
                ),
                Face::Cap if bearing > 0 => ContactCompliance {
                    stiffness: stiffness / bearing as f32,
                    damping: damping / bearing as f32,
                    rest: 0.0,
                },
                _ => ContactCompliance {
                    stiffness: 0.0,
                    damping: 0.0,
                    rest: 0.0,
                },
            });
        }
    }
}

/// The law handing a manifold's points stiffness and damping `(stiffness, damping)` in all at a
/// mean compression `depth` (m, above zero), over the most of its deepest points that all bear it
/// (`overlaps`, deepest first): `n` of them take `1/n` each and a common rest length that puts
/// their mean compression at `depth`. So force, stiffness and damping are the manifold's whole,
/// and the law affine. The deepest point alone bears at `depth`; under the law for `n`, the next
/// point bears nothing (DERIVED: were it to bear, the rest for one more point, no smaller, would
/// let them all bear).
fn shared(overlaps: &[f32], (stiffness, damping): (f32, f32), depth: f32) -> ContactCompliance {
    let mut sum = overlaps[0];
    let mut law = ContactCompliance {
        stiffness,
        damping,
        rest: depth - sum,
    };
    for (n, &overlap) in overlaps.iter().enumerate().skip(1) {
        sum += overlap;
        let points = (n + 1) as f32;
        let more = ContactCompliance {
            stiffness: stiffness / points,
            damping: damping / points,
            rest: depth - sum / points,
        };
        if more.bears(overlap) {
            law = more;
        }
    }
    law
}

/// Read each run's touching contacts from a shared read of the contact graph and work out the
/// law each takes, every run apart: a run's laws read only its own contacts.
fn read_runs(
    graph: Res<ContactGraph>,
    mut runs: Query<(Entity, &ColliderOf, &SoftRun, &mut RunContacts)>,
    bodies: Query<(&Position, &Rotation)>,
    law: Option<Res<SupportLaw>>,
) {
    let law = law.as_deref();
    runs.par_iter_mut()
        .for_each(|(run, of, soft, mut contacts)| {
            contacts.clear();
            let (position, rotation) = bodies
                .get(of.body)
                .expect("a run hangs on its spawn-built body");
            for edge in graph.contact_edges_with(run) {
                // A pair still asleep after the solver-start wake is not solved this step.
                if !edge.is_touching() || edge.is_sleeping() {
                    continue;
                }
                let pair = graph
                    .get_pair_by_edge(edge)
                    .expect("a contact edge names its pair");
                for (index, manifold) in pair.manifolds.iter().enumerate() {
                    contacts.read(
                        soft,
                        (position.0, rotation.0),
                        (edge.id, index),
                        manifold,
                        edge.collider1 == run,
                    );
                }
            }
            if !contacts.manifolds.is_empty() {
                let law = law.expect("a run meets a piece only after its support law is published");
                contacts.solve(soft, law);
            }
        });
}

/// Publish each run's laws in the step-local store; manifold geometry remains the narrow phase's.
fn write_runs(mut laws: ResMut<Laws>, runs: Query<&RunContacts>) {
    for contacts in &runs {
        for (manifold, &law) in contacts.manifolds.iter().zip(&contacts.laws) {
            laws.set(manifold.pair, manifold.index, law);
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PhysicsSchedule,
        (read_runs, write_runs)
            .chain()
            .in_set(PhysicsStepSystems::Solver)
            .after(SolverSystems::PrepareSolverBodies)
            .before(SolverSystems::PrepareContactConstraints),
    );
}

#[cfg(test)]
mod tests {
    use avian3d::collision::contact_types::{ContactEdge, ContactEdgeFlags, ContactPoint};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::tasks::{ComputeTaskPool, TaskPool};

    use super::*;

    /// A run meets loose pieces and nothing else: the sampled support carries every other solid,
    /// terrain included, so a run meeting one would carry it a second time. Fails if the run's
    /// layers admit terrain, a hull, a standing construction or another run, or stop admitting a
    /// loose piece.
    #[test]
    fn a_run_meets_loose_pieces_and_nothing_else() {
        use avian3d::prelude::LayerMask;
        let run = run_layers();
        assert!(run.interacts_with(crate::construction::breakage::piece_layers()));
        for (other, layers) in [
            (
                "terrain",
                CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
            ),
            (
                "a hull",
                CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
            ),
            (
                "a standing construction",
                crate::construction::element_layers(),
            ),
            ("another run", run),
        ] {
            assert!(!run.interacts_with(layers), "a run meets {other}");
        }
    }

    /// Fixture: a law of 400 kN/m and 40 kN·s/m per metre.
    const LAW: SupportLaw = SupportLaw {
        stiffness_per_m: 4.0e5,
        damping_per_m: 4.0e4,
    };

    /// The arc `links` links round the run from its seam (m).
    fn at(links: f32) -> f32 {
        links * SoftRun::fixture().link
    }

    /// One manifold given by its face and its points `(arc, overlap)` on the fixture run.
    type Given<'a> = (Face, &'a [(f32, f32)]);

    /// One manifold by its normal leaving the run and its exposed points `(point, overlap)`, in
    /// the run's body frame.
    type Exposed = (Vec3, Vec<(Vec3, f32)>);

    /// A world holding `run` on a body at the origin under the fixture law, touching one piece per
    /// manifold. Returns the world and the manifolds' pairs, in order.
    fn world_on(run: SoftRun, manifolds: &[Exposed]) -> (World, Vec<ContactId>) {
        ComputeTaskPool::get_or_init(TaskPool::default);
        let mut world = World::new();
        world.init_resource::<Laws>();
        world.insert_resource(LAW);
        let body = world
            .spawn((
                Position::default(),
                Rotation::default(),
                Transform::IDENTITY,
            ))
            .id();
        let collider = world
            .spawn((ColliderOf { body }, run, Transform::IDENTITY))
            .id();
        let mut graph = ContactGraph::default();
        let pairs = manifolds
            .iter()
            .map(|(normal, points)| {
                let points =
                    (points.iter()).map(|&(at, overlap)| ContactPoint::new(at, at, at, overlap));
                let manifold = ContactManifold::new(points, *normal);
                let mut edge = ContactEdge::new(collider, world.spawn_empty().id());
                edge.flags.insert(ContactEdgeFlags::TOUCHING);
                graph
                    .add_edge_with(edge, |pair| pair.manifolds.push(manifold.clone()))
                    .expect("each piece meets the run once")
            })
            .collect();
        world.insert_resource(graph);
        (world, pairs)
    }

    /// [`world_on`] the fixture run, each manifold's points standing on the outline at their arcs,
    /// its normal leaving the run through its face.
    fn world(manifolds: &[Given]) -> (World, Vec<ContactId>) {
        let run = SoftRun::fixture();
        let exposed: Vec<Exposed> = manifolds
            .iter()
            .map(|&(face, points)| {
                let normal = match face {
                    Face::Rim(edge) => Vec3::new(0.0, run.edges[edge].out.y, run.edges[edge].out.x),
                    Face::Cap => Vec3::X,
                };
                let points = points.iter().map(|&(s, overlap)| {
                    let last = run.edges.last().expect("a run outline has edges");
                    let s = s.rem_euclid(last.arc + last.length);
                    let edge = run
                        .edges
                        .iter()
                        .rfind(|edge| edge.arc <= s)
                        .expect("the first edge starts at arc zero");
                    let p = edge.start + edge.dir * (s - edge.arc);
                    (Vec3::new(0.0, p.y, p.x), overlap)
                });
                (normal, points.collect())
            })
            .collect();
        world_on(run, &exposed)
    }

    /// Each manifold's law as `read_runs` reads it, in `pairs` order.
    fn read(world: &mut World, pairs: &[ContactId]) -> Vec<ContactCompliance> {
        world.run_system_once(read_runs).expect("the read runs");
        let contacts = world
            .query::<&RunContacts>()
            .single(world)
            .expect("the world holds one run");
        pairs
            .iter()
            .map(|&pair| {
                let at = contacts.manifolds.iter().position(|m| m.pair == pair);
                contacts.laws[at.expect("every touching manifold is read")]
            })
            .collect()
    }

    fn laws(manifolds: &[Given]) -> Vec<ContactCompliance> {
        let (mut world, pairs) = world(manifolds);
        read(&mut world, &pairs)
    }

    /// What one manifold's points that bear `law` carry at their own overlaps: their springs'
    /// force (N), stiffness (N/m) and damping (N·s/m), each summed.
    fn carries(law: &ContactCompliance, points: &[(f32, f32)]) -> [f32; 3] {
        points
            .iter()
            .filter(|p| law.bears(p.1))
            .fold([0.0; 3], |[f, k, c], p| {
                [
                    f + law.stiffness * (p.1 + law.rest),
                    k + law.stiffness,
                    c + law.damping,
                ]
            })
    }

    /// What the manifolds carry together ([`carries`]).
    fn carried(manifolds: &[Given]) -> [f32; 3] {
        laws(manifolds)
            .iter()
            .zip(manifolds)
            .map(|(law, (_, points))| carries(law, points))
            .fold([0.0; 3], |[f, k, c], [df, dk, dc]| [f + df, k + dk, c + dc])
    }

    /// Whether two readings agree to f32 round-off.
    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter()
            .zip(&b)
            .all(|(a, b)| (a - b).abs() <= b.abs() * 1e-5)
    }

    fn bits(laws: &[ContactCompliance]) -> Vec<[u32; 3]> {
        laws.iter()
            .map(|law| [law.stiffness, law.damping, law.rest].map(f32::to_bits))
            .collect()
    }

    /// A piece split in two carries what the whole carries: a link carries the law once, at the
    /// deepest overlap reaching it, its stiffness and damping whole. Links 26 to 46, inside the
    /// square's second edge, pressed `d` deep by one piece carry `k'·d`, `k'` and `c'` per metre of
    /// their 21 links; so do two pieces side by side across the shoe, and two end to end split
    /// inside link 36. A piece tilted along the belt, cut inside link 36, a piece clear of the belt
    /// past link 36, split where it leaves it, and a piece `d` and `3d` deep across link 30, split
    /// at `2d`, carry what they carry whole, to f32 round-off; the last carries one link's `k'`,
    /// `c'` and `3d·k'`. Fails if a link's law adds up over its manifolds (side by side doubles
    /// it), if a manifold loads links past its own (a per-manifold floor loads the split link
    /// twice), if a contact's points are read at their links before its overlap runs between them
    /// (the tilted cut steepens both halves), if a link at or below zero carries (the clear half
    /// pushes), or if a manifold's stiffness is its force over its overlap (the one link's whole
    /// carries `1.5·k'`, its halves `1.52·k'`).
    #[test]
    fn a_piece_split_in_two_carries_what_the_whole_carries() {
        let d = 0.04;
        let link = SoftRun::fixture().link;
        let (k, c) = (LAW.stiffness_per_m * link, LAW.damping_per_m * link);
        let edge = Face::Rim(1);
        let level = |link: f32| (at(link), d);
        let tilt = |link: f32| (at(link), 0.01 + 0.002 * (link - 26.0));
        let clear = |link: f32| (at(link), d * (36.0 - link) / 10.0);
        let steep = |depth: f32| (at(30.0 + 0.3 * depth / d), depth);
        let whole = carried(&[(edge, &[level(26.0), level(26.0), level(46.0), level(46.0)])]);
        let flat = [21.0 * k * d, 21.0 * k, 21.0 * c];
        assert!(close(whole, flat), "one piece: {whole:?} against {flat:?}");
        let one = carried(&[(edge, &[steep(d), steep(3.0 * d)])]);
        let link_law = [3.0 * k * d, k, c];
        assert!(
            close(one, link_law),
            "one link: {one:?} against {link_law:?}"
        );
        for (layout, whole, parts) in [
            (
                "side by side",
                whole,
                carried(&[
                    (edge, &[level(26.0), level(46.0)]),
                    (edge, &[level(26.0), level(46.0)]),
                ]),
            ),
            (
                "end to end",
                carried(&[(edge, &[level(26.0), level(46.0)])]),
                carried(&[
                    (edge, &[level(26.0), level(36.5)]),
                    (edge, &[level(36.5), level(46.0)]),
                ]),
            ),
            (
                "tilted, cut inside link 36",
                carried(&[(edge, &[tilt(26.0), tilt(46.0)])]),
                carried(&[
                    (edge, &[tilt(26.0), tilt(36.5)]),
                    (edge, &[tilt(36.5), tilt(46.0)]),
                ]),
            ),
            (
                "clear past link 36",
                carried(&[(edge, &[clear(26.0), clear(46.0)])]),
                carried(&[
                    (edge, &[clear(26.0), clear(36.0)]),
                    (edge, &[clear(36.0), clear(46.0)]),
                ]),
            ),
            (
                "d and 3d across one link, cut at 2d",
                one,
                carried(&[
                    (edge, &[steep(d), steep(2.0 * d)]),
                    (edge, &[steep(2.0 * d), steep(3.0 * d)]),
                ]),
            ),
        ] {
            assert!(
                whole[0] > 0.0 && close(parts, whole),
                "{layout}: the parts carry {parts:?} against the whole's {whole:?}"
            );
        }
    }

    /// The order the solver hands manifolds and points over means nothing: permuting the points
    /// within each manifold, and the manifolds, gives every manifold its law bit for bit. Link 5
    /// holds a deep manifold and two shallow ones whose depths round away when added in arrival
    /// order (0.5 m, then 2⁻²⁵ m twice: each addition rounds back to 0.5) but not in ascending
    /// order (2⁻²⁴ + 0.5 is exact). Fails if a link's sum, or a manifold's summed overlap, runs
    /// in the order the solver handed them over.
    #[test]
    fn permuting_points_or_manifolds_changes_nothing() {
        let tiny = 2.0_f32.powi(-25);
        let manifolds: [&[(f32, f32)]; 4] = [
            &[(at(5.0), 0.5), (at(9.0), 0.3), (at(5.0), -0.1)],
            &[(at(5.0), tiny)],
            &[(at(5.0), tiny), (at(7.0), tiny)],
            &[(at(8.0), 0.05), (at(12.0), -0.02), (at(10.0), 0.01)],
        ];
        let reference = bits(&laws(&manifolds.map(|points| (Face::Rim(0), points))));
        for order in [[3, 2, 1, 0], [1, 3, 0, 2], [2, 0, 3, 1]] {
            let reversed: Vec<Vec<(f32, f32)>> = order
                .iter()
                .map(|&m| manifolds[m].iter().rev().copied().collect())
                .collect();
            let permuted: Vec<Given> = reversed
                .iter()
                .map(|points| (Face::Rim(0), points.as_slice()))
                .collect();
            let got = bits(&laws(&permuted));
            for (k, &m) in order.iter().enumerate() {
                assert_eq!(
                    got[k], reference[m],
                    "order {order:?}: manifold {m}'s law moved"
                );
            }
        }
    }

    /// The edge ending at the seam reads as any other edge: its far corner stands at the outline's
    /// full length, the seam, and a contact reaching it spans its own edge alone. Two manifolds on
    /// the square's last edge, one reaching that corner, each carry what the same two carry on its
    /// second edge, to f32 round-off (the arcs are not dyadic). Fails if an arc is taken round the
    /// closed outline (the corner reads as arc zero, and the contact spans the run the long way),
    /// or if the link past the last does not wrap to the first.
    #[test]
    fn the_edge_ending_at_the_seam_reads_as_any_other_edge() {
        let run = SoftRun::fixture();
        let read = |edge: usize| {
            let from = run.edges[edge].arc;
            let a = [(from + 0.3, 0.03), (from + 1.7, -0.01), (from + 2.35, 0.04)];
            let b = [(from + 2.1, 0.02), (from + 3.0, 0.025)];
            let face = Face::Rim(edge);
            let laws = laws(&[(face, &a), (face, &b)]);
            [carries(&laws[0], &a), carries(&laws[1], &b)]
        };
        let (second, last) = (read(1), read(3));
        assert!(second[0][0] > 0.0, "the contact carries nothing");
        for (k, (last, second)) in last.iter().zip(&second).enumerate() {
            assert!(
                close(*last, *second),
                "manifold {k}: {last:?} on the last edge against {second:?} on the second"
            );
        }
    }

    /// An exact tie between two faces goes by the outline's shape, not its numbering: the law is the
    /// same whichever vertex the outline starts from. On the square, a piece `d` deep whose normal
    /// bisects the corner at the origin, its point exposed half the overlap inside, and a piece
    /// pressing the bottom face beside that corner carry the same force, stiffness and damping
    /// under each of the outline's four numberings, the seam 24 links further on each time. Fails
    /// if a tie goes to the last edge: one numbering then reads the bisecting piece on the left
    /// face, at link 95, and the two pieces carry twice what they carry sharing link 0.
    #[test]
    fn a_tie_between_faces_goes_by_the_outline_not_its_numbering() {
        let d = 0.1;
        let corners =
            [(0.0, 0.0), (3.0, 0.0), (3.0, 3.0), (0.0, 3.0)].map(|(z, y)| Vec2::new(z, y));
        let inside = d / 2.0 * std::f32::consts::FRAC_1_SQRT_2;
        let manifolds: [Exposed; 2] = [
            (
                Vec3::new(0.0, -1.0, -1.0).normalize(),
                vec![(Vec3::new(0.0, inside, inside), d)],
            ),
            (Vec3::NEG_Y, vec![(Vec3::new(0.0, 0.0, 0.0625), d)]),
        ];
        let carried = |start: usize| {
            let outline: Vec<Vec2> = (0..4).map(|k| corners[(start + k) % 4]).collect();
            let (mut world, pairs) = world_on(SoftRun::new(&outline, 0.125), &manifolds);
            let laws = read(&mut world, &pairs);
            laws.iter()
                .zip(&manifolds)
                .fold([0.0; 3], |sum, (law, (_, points))| {
                    let points: Vec<(f32, f32)> =
                        points.iter().map(|&(_, overlap)| (0.0, overlap)).collect();
                    let [f, k, c] = carries(law, &points);
                    [sum[0] + f, sum[1] + k, sum[2] + c]
                })
        };
        let first = carried(0);
        assert!(first[0] > 0.0, "the pieces carry nothing");
        for start in 1..4 {
            let got = carried(start);
            assert!(
                close(got, first),
                "the outline from vertex {start}: {got:?} against {first:?} from vertex 0"
            );
        }
    }

    /// A cap contact carries one link's law along its normal, split over its points in overlap
    /// however many parry gives it, and leaves the rim's links alone. Two points `d` deep, and three
    /// with a fourth apart, carry `k'·d`, `k'` and `c'` per metre of one link, as one point `d`
    /// deep alone on its link carries on the rim, where a piece reaching three links at that depth
    /// carries three times as much; and a rim contact on the links they stand by carries what it
    /// carries without them. Fails if a cap's law grows with its points, or if a cap takes a share
    /// of a link's budget from the rim.
    #[test]
    fn a_cap_contact_carries_one_links_law_outside_the_links() {
        let d = 0.04;
        let link = SoftRun::fixture().link;
        let one = [
            LAW.stiffness_per_m * link * d,
            LAW.stiffness_per_m * link,
            LAW.damping_per_m * link,
        ];
        for (n, points) in [
            (1.0, &[(at(12.5), d)][..]),
            (3.0, &[(at(12.5), d), (at(14.5), d)]),
        ] {
            let got = carried(&[(Face::Rim(0), points)]);
            let want = one.map(|v| n * v);
            assert!(
                close(got, want),
                "{n} links on the rim: {got:?} against {want:?}"
            );
        }
        let rim: &[(f32, f32)] = &[(at(12.0), d), (at(13.0), d)];
        let alone = carried(&[(Face::Rim(0), rim)]);
        let two: &[(f32, f32)] = &[(at(12.5), d), (at(60.5), d)];
        let four: &[(f32, f32)] = &[
            (at(12.5), d),
            (at(12.7), d),
            (at(60.5), d),
            (at(30.0), -0.01),
        ];
        for cap in [two, four] {
            let laws = laws(&[(Face::Rim(0), rim), (Face::Cap, cap)]);
            let (carried, rim_carried) = (carries(&laws[1], cap), carries(&laws[0], rim));
            assert!(
                close(carried, one),
                "{} points: {carried:?} against {one:?}",
                cap.len()
            );
            assert!(
                close(rim_carried, alone),
                "{} points: the rim carries {rim_carried:?} against {alone:?} alone",
                cap.len()
            );
        }
    }

    /// A pair asleep at the narrow phase and woken by Avian at the solver's start takes its law in
    /// the step it is solved in: the run reads its kept manifold once the wake has returned it to
    /// the solver. Fixture: a piece resting on a hull's run, both asleep, and a striker meeting the
    /// hull's skid, which wakes their island at the solver's start. Fails if the laws are read
    /// before that wake: the woken pair is skipped as asleep and solved as a rigid contact.
    #[test]
    fn a_pair_woken_at_the_solvers_start_takes_its_law() {
        use avian3d::dynamics::solver::constraint_graph::ConstraintGraph;
        use avian3d::prelude::{LinearVelocity, RigidBody, Sleeping};
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            TransformPlugin,
            AssetPlugin::default(),
            crate::offline_physics(),
            crate::contact_transfer::sim_plugin,
        ))
        .init_asset::<Mesh>()
        .insert_resource(LAW);
        plugin(&mut app);
        crate::headless_test::start_fixed_clock(&mut app);
        crate::headless_test::finish_plugins(&mut app);
        let world = app.world_mut();
        // Fixture: ground whose top is y = 0; a hull on a 1 × 0.2 × 3 m skid, its run a 1 m wide
        // box over the fixture outline (z, y in [0, 3]) above the skid; a 0.4 m piece on the run.
        world.spawn((
            RigidBody::Static,
            Collider::cuboid(20.0, 1.0, 20.0),
            Transform::from_xyz(0.0, -0.5, 0.0),
        ));
        let hull = world
            .spawn((RigidBody::Dynamic, Transform::from_xyz(0.0, 0.2, 0.0)))
            .id();
        world.spawn((
            ChildOf(hull),
            Collider::cuboid(1.0, 0.2, 3.0),
            Transform::from_xyz(0.0, -0.1, 1.5),
        ));
        let run = world
            .spawn((
                ChildOf(hull),
                SoftRun::fixture(),
                Collider::cuboid(1.0, 3.0, 3.0),
                run_layers(),
                Transform::from_xyz(0.0, 1.5, 1.5),
            ))
            .id();
        let piece = world
            .spawn((
                RigidBody::Dynamic,
                Collider::cuboid(0.4, 0.4, 0.4),
                crate::construction::breakage::piece_layers(),
                Transform::from_xyz(0.0, 3.4, 1.5),
            ))
            .id();
        let asleep = |app: &App| {
            let world = app.world();
            [hull, piece].map(|body| world.get::<Sleeping>(body).is_some()) == [true, true]
        };
        for _ in 0..640 {
            app.update();
            if asleep(&app) {
                break;
            }
        }
        assert!(
            asleep(&app),
            "fixture: the hull and the piece must come to rest"
        );
        let (edge, _) = (app.world().resource::<ContactGraph>())
            .get(run, piece)
            .expect("fixture: the piece rests on the run");
        assert!(edge.is_sleeping(), "fixture: the run and the piece sleep");
        let contact = edge.id;
        // Fixture: a striker 5 mm into the skid's side, closing on it.
        app.world_mut().spawn((
            RigidBody::Dynamic,
            Collider::cuboid(0.2, 0.2, 0.2),
            Transform::from_xyz(0.595, 0.1, 1.5),
            LinearVelocity(Vec3::NEG_X),
        ));
        for _ in 0..4 {
            app.update();
            if app.world().get::<Sleeping>(hull).is_none() {
                break;
            }
        }
        let world = app.world();
        assert!(
            world.get::<Sleeping>(hull).is_none(),
            "fixture: the striker wakes the hull"
        );
        let points: Vec<bool> = (world.resource::<ConstraintGraph>().colors.iter())
            .flat_map(|color| &color.contact_constraints)
            .filter(|constraint| constraint.contact_id == contact)
            .flat_map(|constraint| &constraint.points)
            .map(|point| point.normal_part.compliant)
            .collect();
        assert!(
            !points.is_empty(),
            "fixture: the woken pair is solved in the step it wakes"
        );
        assert!(
            points.iter().all(|&compliant| compliant),
            "the woken pair was solved rigid: {points:?}"
        );
    }

    /// Handing a manifold its law changes none of its points, so the law is a function of the
    /// manifold as the narrow phase left it, as a pair woken at the solver's start needs: read
    /// again after the hand-over, it is the same bit for bit. A manifold out of overlap takes the
    /// zero law, never none, which the solver would read as a rigid contact. Fails if the hand-over
    /// drops a point at or past zero overlap (the profile through it, and so the law, moves), or
    /// leaves a manifold without a law.
    #[test]
    fn handing_a_law_over_leaves_the_manifold_it_was_read_from() {
        let touching: &[(f32, f32)] = &[(at(26.0), 0.04), (at(30.5), 0.0), (at(34.0), -0.01)];
        let apart: &[(f32, f32)] = &[(at(40.0), 0.0), (at(44.0), -0.01)];
        let (mut world, pairs) = world(&[(Face::Rim(1), touching), (Face::Rim(1), apart)]);
        let manifolds = |world: &World| -> Vec<ContactManifold> {
            let graph = world.resource::<ContactGraph>();
            pairs
                .iter()
                .map(|&pair| graph.get_by_id(pair).expect("a pair stays").1.manifolds[0].clone())
                .collect()
        };
        let before = manifolds(&world);
        let law = read(&mut world, &pairs);
        world.run_system_once(write_runs).expect("the write runs");
        let after = manifolds(&world);
        for (before_index, ((before, after), law)) in
            before.iter().zip(&after).zip(&law).enumerate()
        {
            assert_eq!(after.points, before.points, "the hand-over changed a point");
            assert_eq!(
                world
                    .resource::<Laws>()
                    .get(pairs[before_index], 0)
                    .map(|stored| (stored.stiffness, stored.damping, stored.rest)),
                Some((law.stiffness, law.damping, law.rest)),
                "a manifold went without its law"
            );
        }
        let zero = ContactCompliance {
            stiffness: 0.0,
            damping: 0.0,
            rest: 0.0,
        };
        assert_eq!(law[1], zero, "a manifold out of overlap carries");
        assert_eq!(
            bits(&read(&mut world, &pairs)),
            bits(&law),
            "read again, the law moved"
        );
    }
}
