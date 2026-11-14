//! The belt against loose pieces, in the game's tank driving physics (game state only).
//!
//! Terrain and standing construction meet the belt through the sampled support (`forces`) and the
//! outer-face stop (`bump_stop`). A loose piece meets one run collider per side, as a solver
//! contact, so the piece takes the equal and opposite reaction through Avian:
//!
//! - **Shape.** [`run_outline`]: the convex outline of the rest belt's outer face, extruded across
//!   the shoe's measured faces. Built at spawn from construction data and hung on the hull with
//!   the stop (`bump_stop::spawn_track_colliders`; ADR-0014), on [`crate::Layer::TrackBelt`], which
//!   meets only `Layer::Debris`.
//! - **Support.** The run is read as links of one pitch along its outline, and each link carries
//!   the running gear's calibrated law per metre (`belt_segment::SegmentLaw`, the sampled
//!   support's own) times its length, at the deepest overlap any contact gives it, once however
//!   many pieces press it ([`RunContacts`]). A contact's overlap runs along the belt through its
//!   points, linear between neighbours the shorter way round; a link takes the deepest that
//!   reaches on its length, nothing where that is zero or less. A link is rigid along its
//!   length, so one point loads a whole link. The contacts on a link share its law in proportion
//!   to their depth there. Only a contact's points in overlap reach the solver, and they carry
//!   its whole share. The datum is the rest face, so a piece's force adds to the ground's under
//!   the same belt, which the sampled support still carries. The stop meets loose pieces too, as
//!   the hard limit at full bump travel.
//! - **Drivetrain.** A run carries its side's `BeltFace`, so the shared hook moves its surface at
//!   belt speed and `stop_contacts::by_owner` reads its friction into the transmission.
//!
//! Each run's laws come from a shared read of the contact graph, every run apart ([`read_runs`]);
//! one short step then writes them into the manifolds ([`write_runs`]).
//!
//! Loose pieces meet nothing else of the track: the sampled support's footprint query skips them
//! (`rubble::RubbleQuery::near`) and the side plates' layers leave them out (`bump_stop`).

use std::ops::Range;

use avian3d::collision::contact_types::{ContactId, ContactManifold};
use avian3d::prelude::{
    ColliderOf, ContactCompliance, ContactGraph, PhysicsSchedule, PhysicsStepSystems, Position,
    Rotation,
};
use bevy::prelude::*;

use super::belt_segment::SegmentLaw;
use super::rig_geom::RigGeom;
use super::route::build_route;
use super::side::Side;

/// A run collider: one side's belt against loose pieces, and the links its contacts are read on.
#[derive(Component, Clone, Debug)]
#[require(RunContacts)]
pub(crate) struct SoftRun {
    /// The run's outline in the side plane `(z, y)`, each vertex with its arc length from the
    /// first along the closed outline (m).
    outline: Vec<(Vec2, f32)>,
    /// The closed outline's length (m).
    length: f32,
    /// The closed outline's links: its length over the belt's pitch, rounded.
    links: usize,
    /// The belt one link stands for (m): the outline's length over its links, so the links tile
    /// it exactly; it parts from the pitch by at most half a pitch over all the links (DERIVED).
    link: f32,
}

impl SoftRun {
    /// The run of side-plane `outline`, closed, read as links of about `pitch`.
    pub(crate) fn new(outline: &[Vec2], pitch: f32) -> Self {
        let mut length = 0.0;
        let outline: Vec<(Vec2, f32)> = outline
            .iter()
            .zip(outline.iter().cycle().skip(1))
            .map(|(&a, &b)| {
                let vertex = (a, length);
                length += a.distance(b);
                vertex
            })
            .collect();
        let links = (length / pitch).round() as usize;
        assert!(
            links >= 3,
            "a run outline {length} m round holds at least three links of {pitch} m"
        );
        Self {
            outline,
            length,
            links,
            link: length / links as f32,
        }
    }

    /// Where along the closed outline side-plane point `p` lies, at its nearest point on the
    /// outline: m from its first vertex, below the outline's length, so that every gap between
    /// two distinct arcs round the outline is positive.
    fn arc_of(&self, p: Vec2) -> f32 {
        let mut nearest = (f32::INFINITY, 0.0);
        for (i, &(a, at)) in self.outline.iter().enumerate() {
            let edge = self.outline[(i + 1) % self.outline.len()].0 - a;
            let t = ((p - a).dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
            let gap = p.distance_squared(a + edge * t);
            if gap < nearest.0 {
                nearest = (gap, at + t * edge.length());
            }
        }
        // The last edge's end is the first vertex, at the outline's length.
        nearest.1 % self.length
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
}

/// Resolution: the least turn a run outline's vertex keeps, as the sine of its angle (about 2°).
/// The raw route's near-collinear arc points make sliver faces, and the convex contact against a
/// piece answers them with loaded points 1.5–3 km deep (MEASURED with the raw route, spike R).
const OUTLINE_TURN_MIN: f32 = 0.035;

/// One side's run outline in the side plane `(z, y)`: the convex hull of the rest belt's outer
/// face, every vertex turning less than [`OUTLINE_TURN_MIN`] dropped.
pub(crate) fn run_outline(geom: &RigGeom, side: Side) -> Vec<Vec2> {
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
    /// Its links in overlap in [`RunContacts::depths`].
    depths: Range<usize>,
    /// Its points' summed overlap (m), and how many are in overlap: the points the solver gets.
    overlap: f32,
    count: usize,
}

/// One run's touching contacts this step and the law each takes, rebuilt every step from the
/// contact graph. Its buffers keep their capacity from step to step.
#[derive(Component, Default)]
pub(crate) struct RunContacts {
    manifolds: Vec<Manifold>,
    /// Each manifold's links in overlap, `(link, depth)` (m, above zero), ascending by link.
    depths: Vec<(u32, f32)>,
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
    /// Each manifold's law, in `manifolds` order: none for one with no point in overlap.
    laws: Vec<Option<ContactCompliance>>,
}

impl RunContacts {
    fn clear(&mut self) {
        self.manifolds.clear();
        self.depths.clear();
        self.laws.clear();
    }

    /// Add one manifold of `run`, `index` of pair `pair`, from its points `(arc, overlap)`. Its
    /// overlap runs through its points, the deepest where two stand together, linear between
    /// neighbours across every gap but the widest, so the shorter way round; each link takes the
    /// deepest it reaches on the link's length. Gaps that tie for the widest are all left open:
    /// which way round a contact runs then depends on nothing but the gaps, never on where the
    /// run's seam lies.
    fn push(
        &mut self,
        run: &SoftRun,
        (pair, index): (ContactId, usize),
        points: impl Iterator<Item = (f32, f32)>,
    ) {
        self.points.clear();
        self.points.extend(points);
        // Summed by value, so the order the points came in cannot move the sum.
        self.points.sort_unstable_by(|a, b| a.1.total_cmp(&b.1));
        let (overlap, count) = self
            .points
            .iter()
            .filter(|p| p.1 > 0.0)
            .fold((0.0, 0), |(sum, n), p| (sum + p.1, n + 1));
        self.points
            .sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(b.1.total_cmp(&a.1)));
        self.points.dedup_by(|b, a| a.0 == b.0);
        let k = self.points.len();
        let gap = |i: usize| match self.points.get(i + 1) {
            Some(next) => next.0 - self.points[i].0,
            None => self.points[0].0 + run.length - self.points[i].0,
        };
        let widest = (0..k).map(gap).fold(0.0, f32::max);
        self.span.clear();
        for i in 0..k {
            let (s0, d0) = self.points[i];
            self.span.push((run.link_at(s0), d0));
            let width = gap(i);
            if width >= widest {
                continue;
            }
            let (s1, d1) = (s0 + width, self.points[(i + 1) % k].1);
            let at = |s: f32| d0 + (d1 - d0) * ((s - s0) / width);
            let (first, last) = ((s0 / run.link).floor(), (s1 / run.link).floor());
            for link in first as usize..=last as usize {
                let lo = s0.max(link as f32 * run.link);
                let hi = s1.min((link + 1) as f32 * run.link);
                self.span
                    .push(((link % run.links) as u32, at(lo).max(at(hi))));
            }
        }
        // Deepest first on each link, then one entry per link.
        self.span
            .sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
        self.span.dedup_by_key(|entry| entry.0);
        let start = self.depths.len();
        self.depths
            .extend(self.span.iter().filter(|entry| entry.1 > 0.0));
        self.manifolds.push(Manifold {
            pair,
            index,
            depths: start..self.depths.len(),
            overlap,
            count,
        });
    }

    /// Each manifold's law under the running gear's `law`: a link carries `k'·link` at its deepest
    /// depth and `c'·link`, shared among the manifolds on it in proportion to their depth there;
    /// a manifold's points in overlap together carry its shares at their own overlaps and at a
    /// common closing speed. A manifold with none in overlap reaches no link and takes no law.
    fn solve(&mut self, run: &SoftRun, law: &SegmentLaw) {
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
            self.laws.push(if manifold.count == 0 {
                None
            } else {
                Some(ContactCompliance {
                    stiffness: stiffness * area / manifold.overlap,
                    damping: damping * length / manifold.count as f32,
                })
            });
        }
    }
}

/// Hand one manifold its law. The solver acts on every point it gets whose separation is not
/// positive (`ContactNormalPart::solve_impulse`), so a point at zero overlap would take damping,
/// and a point the narrow phase found apart could close mid-step, each with a share counted for
/// the points in overlap only. So the solver gets only those: a point at or past zero overlap
/// pushes nothing this step and is read again at the next narrow phase. A manifold with none in
/// overlap gets no point and no law.
fn hand_over(manifold: &mut ContactManifold, law: Option<ContactCompliance>) {
    manifold.points.retain(|point| point.penetration > 0.0);
    manifold.compliance = law;
}

/// Read each run's touching contacts from a shared read of the contact graph and work out the
/// law each takes, every run apart: a run's laws read only its own contacts.
pub(crate) fn read_runs(
    graph: Res<ContactGraph>,
    mut runs: Query<(Entity, &ColliderOf, &SoftRun, &mut RunContacts)>,
    bodies: Query<(&Position, &Rotation)>,
    law: Option<Res<SegmentLaw>>,
) {
    let law = law.as_deref();
    runs.par_iter_mut()
        .for_each(|(run, of, soft, mut contacts)| {
            contacts.clear();
            let (position, rotation) = bodies
                .get(of.body)
                .expect("a run hangs on its spawn-built body");
            let side_plane = |p: Vec3| {
                let local = rotation.0.inverse() * (p - position.0);
                Vec2::new(local.z, local.y)
            };
            for edge in graph.contact_edges_with(run) {
                if !edge.is_touching() || edge.is_sleeping() {
                    continue;
                }
                let pair = graph
                    .get_pair_by_edge(edge)
                    .expect("a contact edge names its pair");
                for (index, manifold) in pair.manifolds.iter().enumerate() {
                    let points = manifold
                        .points
                        .iter()
                        .map(|p| (soft.arc_of(side_plane(p.point)), p.penetration));
                    contacts.push(soft, (edge.id, index), points);
                }
            }
            if !contacts.manifolds.is_empty() {
                let law = law.expect("a run meets a piece only after its support law is published");
                contacts.solve(soft, law);
            }
        });
}

/// Write each run's laws into its manifolds: the one step that holds the graph exclusively.
pub(crate) fn write_runs(mut graph: ResMut<ContactGraph>, runs: Query<&RunContacts>) {
    for contacts in &runs {
        for (manifold, law) in contacts.manifolds.iter().zip(&contacts.laws) {
            let (_, pair) = graph
                .get_mut_by_id(manifold.pair)
                .expect("a pair read this step is still in the graph");
            hand_over(&mut pair.manifolds[manifold.index], *law);
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PhysicsSchedule,
        (read_runs, write_runs)
            .chain()
            .after(PhysicsStepSystems::NarrowPhase)
            .after(super::belt_segment::share_segment_laws)
            .before(PhysicsStepSystems::Solver),
    );
}

#[cfg(test)]
mod tests {
    use avian3d::collision::contact_types::ContactPoint;
    use avian3d::dynamics::solver::contact::ContactNormalPart;

    use super::*;

    /// Fixture: a run of 96 links of 0.125 m (12 m round), and a law of 400 kN/m and
    /// 40 kN·s/m per metre.
    fn run() -> SoftRun {
        SoftRun {
            outline: Vec::new(),
            length: 12.0,
            links: 96,
            link: 0.125,
        }
    }

    const LAW: SegmentLaw = SegmentLaw {
        stiffness_per_m: 4.0e5,
        damping_per_m: 4.0e4,
    };

    /// The arc `links` links round the run from its seam (m).
    fn at(links: f32) -> f32 {
        links * run().link
    }

    /// Each manifold's law, for manifolds given by their points `(arc, overlap)`.
    fn laws(contacts: &[&[(f32, f32)]]) -> Vec<Option<ContactCompliance>> {
        let run = run();
        let mut read = RunContacts::default();
        for (i, points) in contacts.iter().enumerate() {
            read.push(&run, (ContactId(i as u32), 0), points.iter().copied());
        }
        read.solve(&run, &LAW);
        read.laws
    }

    /// The force the manifolds' springs carry together at their own overlaps (N), and their
    /// dampers at a common closing speed of 1 m/s (N), on the points the solver gets.
    fn carried(contacts: &[&[(f32, f32)]]) -> (f32, f32) {
        laws(contacts)
            .iter()
            .zip(contacts)
            .map(|(law, points)| {
                let overlapping = points.iter().filter(|p| p.1 > 0.0);
                match law {
                    Some(law) => (
                        law.stiffness * overlapping.clone().map(|p| p.1).sum::<f32>(),
                        law.damping * overlapping.count() as f32,
                    ),
                    None => {
                        assert_eq!(overlapping.count(), 0, "a manifold in overlap took no law");
                        (0.0, 0.0)
                    }
                }
            })
            .fold((0.0, 0.0), |(k, c), (dk, dc)| (k + dk, c + dc))
    }

    /// Whether two `(spring, damper)` readings agree to f32 round-off.
    fn close((a, b): (f32, f32), (x, y): (f32, f32)) -> bool {
        (a - x).abs() <= x.abs() * 1e-5 && (b - y).abs() <= y.abs() * 1e-5
    }

    fn bits(laws: &[Option<ContactCompliance>]) -> Vec<Option<(u32, u32)>> {
        laws.iter()
            .map(|law| law.map(|law| (law.stiffness.to_bits(), law.damping.to_bits())))
            .collect()
    }

    /// A piece split in two carries what the whole carries: a link carries the law once, at the
    /// deepest overlap reaching it. Links 16 to 40 pressed `d` deep by one piece carry `k'·d` and
    /// `c'` per metre of their 25 links; so do two pieces side by side across the shoe, and two
    /// end to end split inside link 28. A piece tilted along the belt, cut inside link 28, and a
    /// piece clear of the belt past link 20, split where it leaves it, carry what they carry
    /// whole, to f32 round-off. Fails if a link's law adds up over its manifolds (side by side
    /// doubles it), if a manifold loads links past its own (a per-manifold floor loads the split
    /// link twice), if a contact's points are read at their links before its overlap runs between
    /// them (the tilted cut steepens both halves), or if a link at or below zero carries (the
    /// clear half pushes).
    #[test]
    fn a_piece_split_in_two_carries_what_the_whole_carries() {
        let d = 0.04;
        let flat = (
            LAW.stiffness_per_m * run().link * 25.0 * d,
            LAW.damping_per_m * run().link * 25.0,
        );
        let level = |link: f32| (at(link), d);
        let tilt = |link: f32| (at(link), 0.01 + 0.002 * (link - 16.0));
        let clear = |link: f32| (at(link), d * (20.0 - link) / 10.0);
        let whole = carried(&[&[level(16.0), level(16.0), level(40.0), level(40.0)]]);
        assert!(close(whole, flat), "one piece: {whole:?} against {flat:?}");
        for (layout, whole, parts) in [
            (
                "side by side",
                carried(&[&[level(16.0), level(16.0), level(40.0), level(40.0)]]),
                carried(&[&[level(16.0), level(40.0)], &[level(16.0), level(40.0)]]),
            ),
            (
                "end to end",
                carried(&[&[level(16.0), level(40.0)]]),
                carried(&[&[level(16.0), level(28.5)], &[level(28.5), level(40.0)]]),
            ),
            (
                "tilted, cut inside link 28",
                carried(&[&[tilt(16.0), tilt(40.0)]]),
                carried(&[&[tilt(16.0), tilt(28.5)], &[tilt(28.5), tilt(40.0)]]),
            ),
            (
                "clear past link 20",
                carried(&[&[clear(10.0), clear(30.0)]]),
                carried(&[&[clear(10.0), clear(20.0)], &[clear(20.0), clear(30.0)]]),
            ),
        ] {
            assert!(
                whole.0 > 0.0 && close(parts, whole),
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
        let reference = bits(&laws(&manifolds));
        for order in [[3, 2, 1, 0], [1, 3, 0, 2], [2, 0, 3, 1]] {
            let reversed: Vec<Vec<(f32, f32)>> = order
                .iter()
                .map(|&m| manifolds[m].iter().rev().copied().collect())
                .collect();
            let permuted: Vec<&[(f32, f32)]> = reversed.iter().map(Vec::as_slice).collect();
            let got = bits(&laws(&permuted));
            for (k, &m) in order.iter().enumerate() {
                assert_eq!(
                    got[k], reference[m],
                    "order {order:?}: manifold {m}'s law moved"
                );
            }
        }
    }

    /// A contact across the run's closing seam equals the same contact away from it: the run is
    /// closed, so moving every point by whole links moves no law, bit for bit. Fails if a
    /// manifold's span walks the long way round, reads the seam as the run's whole length, or
    /// sums its links in their numbering, which starts at the seam.
    #[test]
    fn a_contact_across_the_seam_equals_the_same_contact_away_from_it() {
        let links = run().links as f32;
        let layout = |first: f32| -> Vec<Vec<(f32, f32)>> {
            let point = |offset: f32, depth: f32| (at((first + offset) % links), depth);
            vec![
                vec![point(0.25, 0.03), point(5.5, -0.01), point(2.75, 0.04)],
                vec![point(3.0, 0.02), point(7.5, 0.02)],
            ]
        };
        let read = |first: f32| {
            let layout = layout(first);
            let slices: Vec<&[(f32, f32)]> = layout.iter().map(Vec::as_slice).collect();
            (bits(&laws(&slices)), carried(&slices))
        };
        let (away, carried) = read(20.0);
        assert!(carried.0 > 0.0, "the contact carries nothing");
        assert_eq!(read(links - 3.0).0, away, "across the seam the laws moved");
    }

    /// A contact whose points stand exactly half the run apart runs neither way round, wherever
    /// the seam lies: each half is as short as the other, so each point loads only its own link.
    /// Manifold A presses links 0 and 48 `d` deep, mid-link, and B link 24 between them; shifted
    /// round the run by every whole link, together they carry `k'·d` and `c'` per metre of three
    /// links, each law the same bit for bit. Fails if a tie picks a half by the seam (A spans the
    /// half past its first gap from the seam: over B at some shifts and not at others), or if A
    /// spans either half.
    #[test]
    fn a_contact_half_the_run_round_spans_neither_half() {
        let d = 0.04;
        let links = run().links as f32;
        let three = (
            LAW.stiffness_per_m * run().link * 3.0 * d,
            LAW.damping_per_m * run().link * 3.0,
        );
        let read = |shift: f32| {
            let point = |link: f32| (at((link + shift) % links), d);
            let a: &[(f32, f32)] = &[point(0.5), point(48.5)];
            let b: &[(f32, f32)] = &[point(24.5)];
            (bits(&laws(&[a, b])), carried(&[a, b]))
        };
        let (unshifted, carried) = read(0.0);
        assert!(
            close(carried, three),
            "carried {carried:?} against {three:?}"
        );
        for shift in 1..run().links {
            assert_eq!(
                read(shift as f32).0,
                unshifted,
                "shifted {shift} links round, the laws moved"
            );
        }
    }

    /// Fixture: one point's mass along the normal (kg), and one substep (s): 64 Hz in six.
    const MASS: f32 = 200.0;
    const SUBSTEP: f32 = 1.0 / 384.0;

    /// The points the solver gets from a manifold of points `(arc, overlap)` handed `law`, and the
    /// normal impulse it puts through them in one substep closing at 1 m/s, each point built as
    /// `ContactConstraint::generate` builds a compliant one, at its own overlap.
    fn solved(points: &[(f32, f32)], law: Option<ContactCompliance>) -> (usize, f32) {
        let mut manifold = ContactManifold::new(
            points.iter().map(|&(_, overlap)| {
                ContactPoint::new(Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, overlap)
            }),
            Vec3::Y,
        );
        hand_over(&mut manifold, law);
        let impulse = manifold
            .points
            .iter()
            .map(|point| {
                let spring = manifold
                    .compliance
                    .expect("a manifold that hands the solver a point carries its law")
                    .coefficients(MASS, SUBSTEP);
                ContactNormalPart {
                    impulse: 0.0,
                    total_impulse: 0.0,
                    effective_mass: MASS,
                    softness: spring,
                    compliant: true,
                    spring,
                }
                .solve_impulse(
                    -point.penetration,
                    Vec3::NEG_Y,
                    Vec3::Y,
                    true,
                    f32::INFINITY,
                    SUBSTEP,
                )
            })
            .sum();
        (manifold.points.len(), impulse)
    }

    /// The solver gets exactly the points a manifold's law is shared over. A piece pressing link
    /// 16 `d` deep and touching link 24 at zero overlap puts through the solver, bit for bit, the
    /// impulse the same law puts through its point at link 16 alone; a piece touching only at zero
    /// overlap or apart takes no law and hands the solver no point. Fails if a point at zero
    /// overlap reaches the solver, which damps it under closing speed (its separation is not
    /// positive) with a share counted for the points in overlap only, so the manifold damps twice;
    /// or if a manifold with no point in overlap reaches it at all.
    #[test]
    fn only_points_in_overlap_reach_the_solver() {
        let d = 0.04;
        let touching: &[(f32, f32)] = &[(at(16.0), d), (at(24.0), 0.0)];
        let law = laws(&[touching])[0];
        let alone = solved(&[(at(16.0), d)], law);
        assert!(alone.1 > 0.0, "the point in overlap took no impulse");
        assert_eq!(
            solved(touching, law).1.to_bits(),
            alone.1.to_bits(),
            "the point at zero overlap moved the manifold's impulse"
        );
        let apart: &[(f32, f32)] = &[(at(16.0), 0.0), (at(20.0), -0.01)];
        let law = laws(&[apart])[0];
        assert_eq!(law, None, "a piece out of overlap took a law");
        assert_eq!(
            solved(apart, law).0,
            0,
            "a piece out of overlap reached the solver"
        );
    }
}
