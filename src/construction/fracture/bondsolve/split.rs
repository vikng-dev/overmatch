//! Experiment for the game's environment destruction model (game state only): the physics engine
//! owns touching, this solve owns holding. Every group of chunks joined by holding joints is one
//! avian3d body; Avian resolves its resting, sliding, tipping, falling and blows. Inside a body,
//! [`hold`] answers only how hard each joint is pulled, from the body's weight, the contact forces
//! Avian reports on its faces ([`crate::contact_transfer`]) and its own rigid acceleration: one
//! linear answer a tick. A rest, a cracked seam or a fixing's bearing still squeezed shut inside a
//! body presses through a linear edge, re-checked once per tick as push only ([`judge`]); a joint
//! that opens can cut the body, and each remaining group becomes a new body.
//! Findings: `.agents/scratch/bestofboth/bob-split.md`.

use super::super::{Cause, Side, State, Structure};
use super::{
    DMat3, DVec3, Edge, INITIAL_RANK_PASSES, Krylov, Node, Problem, Progress, ROUNDS, Rim, Wrench,
    accelerating_frame, joint_edge,
};
use std::collections::HashMap;

/// Whether joint `j` still presses or holds its two standing sides together: not yet opened.
fn shut(structure: &Structure, state: &State, j: usize) -> bool {
    !state.separated[j] && state.live(&structure.joints[j])
}

/// Whether joint `j` joins its two sides into one body: an intact seam, or a cracked one still
/// shut and sticking: each alone stiffens all six relative motions of its two chunks. A rest
/// joins nothing; inside one body it presses as push only, between bodies it is Avian's, as a
/// slipping cracked seam is.
fn holding(structure: &Structure, state: &State, contact: &[Contact], j: usize) -> bool {
    structure.joints[j].seam().is_some()
        && shut(structure, state, j)
        && (!state.cracked[j] || contact[j] == Contact::Shut)
}

/// The standing chunks of `structure` grouped by holding joints, each group ascending.
pub(crate) fn groups(structure: &Structure, state: &State, contact: &[Contact]) -> Vec<Vec<usize>> {
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
        if let (Side::Chunk(b), true) = (joint.b, holding(structure, state, contact, j)) {
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

/// How a joint's push-only part (a rest, a cracked seam, or an intact fixing's bulk bearing)
/// stands inside its body, per structure joint. A rest or seam that opens leaves the body's graph
/// through `State::separated`; only a bearing opens here, its fixing still holding.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Contact {
    #[default]
    Shut,
    /// Sliding along the unit direction (construction frame) the shear it transmits points, with
    /// pair friction's force at the compression it had then, N. StandIn: the force stays at that
    /// compression and the part never sticks again; the rigid body cannot show the slip.
    Slipping(DVec3, f64),
    /// A fixing's bearing that stopped pressing; the fixing alone holds the joint.
    Open,
}

/// Which law reads an edge.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    /// An intact seam: its bulk compliance, or a fixing's slip slopes.
    Cohesive,
    /// A rest, a cracked seam or a fixing's bearing: bulk compliance, push only.
    PushOnly,
}

/// One body's bonded graph in the construction frame: its chunks, the shut joints among them and
/// to the ground, and the warm displacement field of its last [`hold`].
#[derive(Clone, Debug)]
pub(crate) struct Body {
    pub members: Vec<usize>,
    /// Per edge, its structure joint; an intact fixing has two edges, its own and its bearing's.
    pub joints: Vec<usize>,
    /// A ground joint or a buried chunk holds it: Avian keeps it static.
    pub anchored: bool,
    nodes: Vec<Node>,
    masses: Vec<f64>,
    own_inertia: Vec<DMat3>,
    edges: Vec<Edge>,
    parts: Vec<Part>,
    /// Per edge, the friction force a slipping push-only part transmits from `a` to `b`, N.
    friction: Vec<DVec3>,
    local: HashMap<usize, usize>,
    solve: Krylov,
    loads: Vec<Wrench>,
}

impl Body {
    /// The body of `members` (one entry of [`groups`]), its push-only parts as `contact` holds
    /// them. Every edge is linear: intact seams carry their own stiffness, and every push-only part
    /// its bulk, which [`judge`] re-checks once per tick.
    pub(crate) fn of(
        structure: &Structure,
        state: &State,
        contact: &[Contact],
        members: Vec<usize>,
    ) -> Self {
        let local: HashMap<_, _> = members.iter().enumerate().map(|(i, &c)| (c, i)).collect();
        let mut nodes: Vec<_> = members
            .iter()
            .map(|&c| Node {
                centre: structure.chunks[c].centroid.as_dvec3(),
                fixed: structure.chunks[c].buried,
            })
            .collect();
        let (mut edges, mut parts, mut friction, mut joints) = (vec![], vec![], vec![], vec![]);
        for (j, joint) in structure.joints.iter().enumerate() {
            let Some(&a) = local.get(&joint.a) else {
                continue; // Another body's joint.
            };
            let b = match joint.b {
                Side::Chunk(b) => match local.get(&b) {
                    Some(&b) => Some(b),
                    None => continue, // Across a cut: Avian's contact.
                },
                // Only a holding joint anchors; the ground under a rest is Avian's contact.
                Side::Ground if holding(structure, state, contact, j) => None,
                Side::Ground => continue,
            };
            if !shut(structure, state, j) {
                continue; // Opened: Avian's contact, if the sides still meet.
            }
            let cohesive = joint.seam().is_some() && !state.cracked[j];
            let mut bulk = joint_edge(structure, j, a, b, nodes[a].centre, true);
            bulk.contact = None; // Push only is judged once per tick, never projected.
            if cohesive {
                let mut own = joint_edge(structure, j, a, b, nodes[a].centre, false);
                let bearing = own.bearing.take().is_some();
                edges.push(own);
                parts.push(Part::Cohesive);
                friction.push(DVec3::ZERO);
                joints.push(j);
                if !bearing {
                    continue; // An intrinsic seam's bulk is its own stiffness.
                }
            }
            let slip = match contact[j] {
                Contact::Open => continue,
                Contact::Shut => DVec3::ZERO,
                Contact::Slipping(direction, force) => {
                    bulk.slip = Some(direction);
                    direction * force
                }
            };
            edges.push(bulk);
            parts.push(Part::PushOnly);
            friction.push(slip);
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
            parts,
            friction,
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
/// patch centre (a slipping part's friction included), and the sparse work it took.
#[derive(Clone, Debug, Default)]
pub(crate) struct Held {
    pub resultants: Vec<Wrench>,
    pub products: usize,
    pub joint_visits: usize,
    /// The graph lost stiffness in some direction: a part of the body is free to move inside
    /// it, which no judge may read. Open: that part must leave as its own body.
    pub mechanism: bool,
}

/// THE INNER SOLVE: one body's joint resultants under `applied` (per node, the contact wrenches
/// about its centre), its weight along `gravity` (m/s², body frame) and, for a free body, its
/// rigid acceleration and spin `spin` (rad/s, body frame). Unchanged loads cost zero products.
/// The joints are linear and the gauge fixed, so the system converges unless a slipping part
/// leaves a direction without stiffness ([`Held::mechanism`]). A factorized solve of the same
/// `K u = f` replaces this body's Krylov without changing callers.
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
    // A slipping part transmits its friction as a fixed pair of forces at its patch.
    for (edge, &friction) in body.edges.iter().zip(&body.friction) {
        loads[edge.a].add(
            Wrench::at(friction, edge.centre - body.nodes[edge.a].centre),
            -1.0,
        );
        if let Some(b) = edge.b {
            loads[b].add(
                Wrench::at(friction, edge.centre - body.nodes[b].centre),
                1.0,
            );
        }
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
        if body.solve.progress == Progress::Mechanism {
            held.mechanism = true;
            return held;
        }
    }
    held.resultants = body
        .edges
        .iter()
        .zip(&body.friction)
        .map(|(e, &friction)| {
            let mut carried = e.resultant(&body.nodes, &body.solve.answer);
            carried.linear += friction;
            carried
        })
        .collect();
    held
}

/// What a tick's [`judge`] changed at one joint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Change {
    /// Cohesion lost; the faces still press shut and stay in the body as push only.
    Cracked(Cause),
    /// The joint opened: cohesion lost, if it had any, and the faces part.
    Opened(Cause),
    /// Its push-only part now stands so ([`Contact`]); an opened bearing leaves its fixing holding.
    Contact(Contact),
}

/// Why a push-only resultant `carried` is not push only, or `None` while it is: compression, its
/// pressure centre on the face, and its shear within pair friction `mu`. Twist is not read
/// (StandIn: torsional friction is not modelled).
fn opening(edge: &Edge, carried: Wrench, mu: f64) -> Option<Cause> {
    let n = edge.normal;
    let compression = -carried.linear.dot(n);
    if compression <= 0.0 {
        return Some(Cause::LiftForce);
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
    if !inside {
        return Some(Cause::Tip);
    }
    // Roundoff margin of a part already slipping at exactly its friction.
    let slack = 1.0 + 64.0 * f64::EPSILON;
    ((carried.linear + n * compression).length() > mu * compression * slack).then_some(Cause::Slide)
}

/// Once per tick, each edge of a held body against the data. An intact seam cracks past its
/// tensile strength or its cohesion plus pair friction on its compression (a fixing's friction is
/// its bearing's, never added to its cohesion); a cracked intrinsic seam that no longer presses
/// opens with it, and a cracked fixing whose bearing is open opens. A push-only part that stops
/// pressing or tips opens (a bearing opens alone); one whose shear passes friction slips, keeping
/// its compression. Only sustained readings: every load here is a force over the tick.
pub(crate) fn judge(
    structure: &Structure,
    state: &State,
    contact: &[Contact],
    body: &Body,
    held: &Held,
) -> Vec<(usize, Change)> {
    let mut changes = Vec::new();
    for (((&j, edge), &carried), &part) in body
        .joints
        .iter()
        .zip(&body.edges)
        .zip(&held.resultants)
        .zip(&body.parts)
    {
        let joint = &structure.joints[j];
        let other = match joint.b {
            Side::Chunk(b) => structure.chunks[b].material.friction,
            Side::Ground => structure.chunks[joint.a].material.friction,
        };
        let mu = f64::from(crate::substances::pair_friction(
            structure.chunks[joint.a].material.friction,
            other,
        ));
        let n = edge.normal;
        match (part, joint.seam()) {
            (Part::Cohesive, Some(seam)) => {
                let (tension, shear) = edge.stress(carried, n);
                let compression = (-carried.linear.dot(n) / edge.area).max(0.0);
                let friction = if seam.fixing.is_some() { 0.0 } else { mu };
                let cause = if tension > f64::from(seam.tensile) {
                    Cause::Tension
                } else if shear > f64::from(seam.shear.cohesion) + friction * compression {
                    Cause::Shear
                } else {
                    continue; // Holds.
                };
                let opens = match seam.fixing {
                    Some(_) => contact[j] == Contact::Open,
                    None => matches!(
                        opening(edge, carried, mu),
                        Some(Cause::LiftForce | Cause::Tip)
                    ),
                };
                changes.push((
                    j,
                    if opens {
                        Change::Opened(cause)
                    } else {
                        Change::Cracked(cause)
                    },
                ));
            }
            (Part::Cohesive, None) => unreachable!("a rest has no cohesive part"),
            (Part::PushOnly, seam) => {
                let bearing = seam.is_some_and(|s| s.fixing.is_some()) && !state.cracked[j];
                match opening(edge, carried, mu) {
                    None => {}
                    Some(Cause::Slide) => {
                        let compression = -carried.linear.dot(n);
                        let shear = carried.linear + n * compression;
                        changes.push((
                            j,
                            Change::Contact(Contact::Slipping(shear.normalize(), mu * compression)),
                        ));
                    }
                    Some(_) if bearing => changes.push((j, Change::Contact(Contact::Open))),
                    Some(cause) => changes.push((j, Change::Opened(cause))),
                }
            }
        }
    }
    changes
}

/// Commit one tick's [`judge`] changes. A fixing can crack and lose its bearing in one tick; a
/// cracked joint whose push-only part has opened has no part left, so it separates.
pub(crate) fn apply(state: &mut State, contact: &mut [Contact], changes: &[(usize, Change)]) {
    for &(j, change) in changes {
        match change {
            Change::Cracked(_) => state.cracked[j] = true,
            Change::Opened(_) => {
                state.cracked[j] = true;
                state.separated[j] = true;
            }
            Change::Contact(part) => contact[j] = part,
        }
        state.separated[j] |= state.cracked[j] && contact[j] == Contact::Open;
    }
}

mod tests;
