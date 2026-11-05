//! Experiment for the game's environment destruction model (game state only): the physics engine
//! owns touching, this solve owns holding. Every group of chunks joined by holding joints is one
//! avian3d body; Avian resolves its resting, sliding, tipping, falling and blows. Inside a body,
//! [`hold`] answers only how hard each joint is pulled, from the body's weight, the contact forces
//! Avian reports on its faces ([`crate::contact_transfer`]) and its own rigid acceleration: no
//! contact inside, so one linear answer. A cracked seam still squeezed shut stays inside its body
//! as push only, re-checked once per tick ([`judge`]); one that opens cuts the body, and each
//! remaining group becomes a new body. Findings: `.agents/scratch/bestofboth/bob-split.md`.

use super::super::{Cause, Side, State, Structure};
use super::{
    DMat3, DVec3, Edge, INITIAL_RANK_PASSES, Krylov, Node, Problem, Progress, ROUNDS, Rim, Wrench,
    accelerating_frame, joint_edge,
};
use std::collections::HashMap;

/// Whether joint `j` still joins its two sides inside one body: a seam that has not opened.
/// A rest is Avian's, never the body's.
fn holding(structure: &Structure, state: &State, j: usize) -> bool {
    let joint = &structure.joints[j];
    joint.seam().is_some() && !state.separated[j] && state.live(joint)
}

/// The standing chunks of `structure` grouped by holding joints, each group ascending.
pub(crate) fn groups(structure: &Structure, state: &State) -> Vec<Vec<usize>> {
    let n = structure.chunks.len();
    let mut root: Vec<usize> = (0..n).collect();
    fn find(root: &mut [usize], mut c: usize) -> usize {
        while root[c] != c {
            root[c] = root[root[c]];
            c = root[c];
        }
        c
    }
    for (j, joint) in structure.joints.iter().enumerate() {
        if let (Side::Chunk(b), true) = (joint.b, holding(structure, state, j)) {
            let (ra, rb) = (find(&mut root, joint.a), find(&mut root, b));
            root[ra.max(rb)] = ra.min(rb);
        }
    }
    let mut by_root: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for c in (0..n).filter(|&c| state.standing(c)) {
        let r = find(&mut root, c);
        by_root.entry(r).or_default().push(c);
    }
    by_root.into_values().collect()
}

/// One body's bonded graph in the construction frame: its chunks, the holding joints among them
/// and to the ground, and the warm displacement field of its last [`hold`].
#[derive(Clone, Debug)]
pub(crate) struct Body {
    pub members: Vec<usize>,
    /// Per edge, its structure joint.
    pub joints: Vec<usize>,
    /// A ground joint or a buried chunk holds it: Avian keeps it static.
    pub anchored: bool,
    nodes: Vec<Node>,
    masses: Vec<f64>,
    own_inertia: Vec<DMat3>,
    edges: Vec<Edge>,
    local: HashMap<usize, usize>,
    solve: Krylov,
    loads: Vec<Wrench>,
}

impl Body {
    /// The body of `members` (one entry of [`groups`]). Every edge is linear: a cracked seam that
    /// is still shut keeps its bulk stiffness, and a fixing's bulk bearing stands in parallel with
    /// its slip slopes. StandIn: that bearing is taken closed; it re-opens with the seam alone.
    pub(crate) fn of(structure: &Structure, state: &State, members: Vec<usize>) -> Self {
        let local: HashMap<_, _> = members.iter().enumerate().map(|(i, &c)| (c, i)).collect();
        let mut nodes: Vec<_> = members
            .iter()
            .map(|&c| Node {
                centre: structure.chunks[c].centroid.as_dvec3(),
                fixed: structure.chunks[c].buried,
            })
            .collect();
        let mut edges = Vec::new();
        let mut joints = Vec::new();
        for (j, joint) in structure.joints.iter().enumerate() {
            let Some(&a) = local.get(&joint.a) else {
                continue; // Another body's joint.
            };
            let b = match joint.b {
                Side::Chunk(b) => match local.get(&b) {
                    Some(&b) => Some(b),
                    None => continue, // Across a cut: Avian's contact.
                },
                Side::Ground => None,
            };
            if !holding(structure, state, j) {
                continue; // A rest or an opened seam: Avian's contact.
            }
            let mut edge = joint_edge(structure, j, a, b, nodes[a].centre, false);
            if let Some(bearing) = edge.bearing.take() {
                edge.normal_stiffness += bearing.normal;
                edge.shear_stiffness += bearing.shear;
            }
            edges.push(edge);
            joints.push(j);
        }
        let anchored = nodes.iter().any(|n| n.fixed) || edges.iter().any(|e| e.b.is_none());
        if !anchored {
            // A free body's six rigid modes are gauge freedom once its resultant is balanced.
            nodes[0].fixed = true;
        }
        Self {
            masses: members
                .iter()
                .map(|&c| f64::from(structure.chunks[c].mass))
                .collect(),
            own_inertia: members
                .iter()
                .map(|&c| {
                    DMat3::from_diagonal(DVec3::splat(structure.chunks[c].intrinsic_inertia()))
                })
                .collect(),
            members,
            joints,
            anchored,
            nodes,
            edges,
            local,
            solve: Krylov::default(),
            loads: Vec::new(),
        }
    }

    /// The local node of structure chunk `c`.
    pub(crate) fn node(&self, c: usize) -> Option<usize> {
        self.local.get(&c).copied()
    }

    /// The body's centre of mass in the construction frame, m.
    pub(crate) fn centre(&self) -> DVec3 {
        let mass: f64 = self.masses.iter().sum();
        self.nodes
            .iter()
            .zip(&self.masses)
            .map(|(n, m)| n.centre * *m)
            .sum::<DVec3>()
            / mass
    }

    /// The arm from node `i`'s centre to `point`, both in the construction frame.
    pub(crate) fn arm(&self, i: usize, point: DVec3) -> DVec3 {
        point - self.nodes[i].centre
    }
}

/// One [`hold`]: each edge's resultant, the wrench side `a`'s loads pass through the joint at its
/// patch centre, and the sparse work it took.
#[derive(Clone, Debug, Default)]
pub(crate) struct Held {
    pub resultants: Vec<Wrench>,
    pub products: usize,
    pub joint_visits: usize,
}

/// THE INNER SOLVE: one body's joint resultants under `applied` (per node, the contact wrenches
/// about its centre), its weight along `gravity` (m/s², body frame) and, for a free body, its
/// rigid acceleration and spin `spin` (rad/s, body frame). Unchanged loads cost zero products.
/// The joints are linear and the gauge fixed, so the system is positive definite and converges;
/// a factorized solve of the same `K u = f` replaces this body's Krylov without changing callers.
pub(crate) fn hold(body: &mut Body, applied: &[Wrench], gravity: DVec3, spin: DVec3) -> Held {
    let mut loads: Vec<_> = applied
        .iter()
        .zip(&body.masses)
        .map(|(&load, &mass)| {
            let mut load = load;
            load.linear += gravity * mass;
            load
        })
        .collect();
    if !body.anchored {
        accelerating_frame(
            &body.nodes,
            &body.masses,
            &body.own_inertia,
            spin,
            &mut loads,
        );
    }
    let mut held = Held::default();
    if loads != body.loads {
        body.solve.changed();
        body.loads = loads;
    }
    let problem = Problem {
        nodes: &body.nodes,
        edges: &body.edges,
        loads: &body.loads,
    };
    let budget = INITIAL_RANK_PASSES * 6 * body.nodes.len();
    while body.solve.progress != Progress::Equilibrated {
        assert!(
            held.products < budget,
            "a gauge-fixed linear bonded body converges within its rank budget"
        );
        held.products += body.solve.advance(&problem, ROUNDS);
        held.joint_visits += body.solve.joint_visits;
        assert_ne!(
            body.solve.progress,
            Progress::Mechanism,
            "a bonded body's stiffness is positive definite once its gauge is fixed"
        );
    }
    held.resultants = body
        .edges
        .iter()
        .map(|e| e.resultant(&body.nodes, &body.solve.answer))
        .collect();
    held
}

/// What a tick's [`judge`] changed at one joint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Change {
    /// Cohesion lost; the faces still press shut and stay in the body as push only.
    Cracked(Cause),
    /// The seam opened: cohesion lost, if it had any, and the faces part.
    Opened(Cause),
}

/// Why a shut seam's resultant `carried` is not push only, or `None` while it is: compression,
/// its pressure centre on the face, and its shear within pair friction `mu`. Twist is not read
/// (StandIn: a shut seam's torsional friction is not modelled).
fn opening(edge: &Edge, carried: Wrench, mu: f64) -> Option<Cause> {
    let n = edge.normal;
    let compression = -carried.linear.dot(n);
    if compression <= 0.0 {
        return Some(Cause::LiftForce);
    }
    if (carried.linear + n * compression).length() > mu * compression {
        return Some(Cause::Slide);
    }
    let bending = carried.angular - n * carried.angular.dot(n);
    let centre = bending.cross(n) / compression;
    let inside = match &edge.patch.rim {
        Rim::Circle(radius) => centre.length() <= *radius,
        Rim::Polygon(corners) => {
            let edges = || corners.iter().zip(corners.iter().cycle().skip(1));
            let turn: f64 = edges().map(|(p, q)| n.dot(p.cross(*q))).sum();
            edges().all(|(p, q)| n.dot((*q - *p).cross(centre - *p)) * turn.signum() >= 0.0)
        }
    };
    (!inside).then_some(Cause::Tip)
}

/// Once per tick, each joint of a held body against the data: an intact seam cracks past its
/// tensile strength or its cohesion plus pair friction on its compression; a cracked seam stays
/// shut while [`opening`] finds it push only, and opens otherwise. Only sustained readings: every load here is
/// a force over the tick, Avian's contact transfer included.
pub(crate) fn judge(
    structure: &Structure,
    state: &State,
    body: &Body,
    held: &Held,
) -> Vec<(usize, Change)> {
    let mut changes = Vec::new();
    for ((&j, edge), &carried) in body.joints.iter().zip(&body.edges).zip(&held.resultants) {
        let joint = &structure.joints[j];
        let seam = joint
            .seam()
            .expect("only seams hold inside a body; rests are Avian's contacts");
        let other = match joint.b {
            Side::Chunk(b) => structure.chunks[b].material.friction,
            Side::Ground => structure.chunks[joint.a].material.friction,
        };
        let mu = f64::from(crate::substances::pair_friction(
            structure.chunks[joint.a].material.friction,
            other,
        ));
        let crack = if state.cracked[j] {
            None
        } else {
            let (tension, shear) = edge.stress(carried, edge.normal);
            let compression = (-carried.linear.dot(edge.normal) / edge.area).max(0.0);
            // A fixing's friction is its parallel bearing's, not added to its cohesion.
            let friction = if seam.fixing.is_some() { 0.0 } else { mu };
            if tension > f64::from(seam.tensile) {
                Some(Cause::Tension)
            } else if shear > f64::from(seam.shear.cohesion) + friction * compression {
                Some(Cause::Shear)
            } else {
                continue; // Holds.
            }
        };
        match (crack, opening(edge, carried, mu)) {
            (Some(cause), None) => changes.push((j, Change::Cracked(cause))),
            (Some(cause), Some(_)) => changes.push((j, Change::Opened(cause))),
            (None, Some(cause)) => changes.push((j, Change::Opened(cause))),
            (None, None) => {} // Still shut.
        }
    }
    changes
}

mod tests;
