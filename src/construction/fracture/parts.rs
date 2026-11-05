//! Parts decide what stands, in the game's environment destruction model (game state only).
//!
//! A part ([`Part`]) is the standing chunks of one baked element that intact bonds inside it
//! join, its buried chunks a fixed part of their own: one rigid node with six degrees. An
//! interface ([`Interface`]) is every live joint crossing between two parts, or from a part to
//! the ground; its stiffness and capacity are its joints', so damage inside a part reaches an
//! interface only through the joint area that remains.
//!
//! [`Assessment`] holds one owner's part graph, its sparse LU factor and the loads it last
//! solved. Bonded joints and push-only joints (rests and cracked seams) share one exact linear
//! solve. Each tick first settles the contact active set: a push-only face whose friction wrench
//! passes its Coulomb capacity slips ([`Slip`]), keeping its normal stiffness and carrying
//! `μ N` against its slip as a term of the system itself, and a face that lifts or tips opens;
//! each change is solved again, and a face only goes stick to slip to open, so the set is final
//! within one pass per face. A part nothing else holds tangentially is a mechanism: its slipping
//! rests open. Only then are bonds judged: an interface whose bonded joints reach their strength
//! cracks whole and bears on as push-only. Connectivity ([`super::loose_groups`]) releases what
//! lost its support, and the physics engine owns contact between separate bodies. Unchanged loads
//! within [`LOAD_TOLERANCE`] on unchanged topology cost nothing. [`Kernel`] is the chunk-scale
//! bonded solve inside one part. The construction design's "Parts decide what stands" owns the
//! model.

use bevy::math::{DMat3, DVec3, Vec3};
use std::collections::BTreeMap;

use super::bondsolve::{Edge, Node, Wrench, accelerating_frame, joint_edge};
use super::{Cause, Failure, Load, Side, State, Structure};

mod kernel;
pub(crate) use kernel::Kernel;

#[cfg(test)]
mod tests;

/// Resolution: a part load that moved by less than this fraction of its owner's weight (a moment,
/// of that weight times the owner's radius of gyration) since the last solve reuses that solve.
/// It is 100 × the 1e-7 contact-reading noise MEASURED by the split experiment
/// (`.agents/scratch/bestofboth/bob-split.md`), which defeats an exact-equality skip.
pub(crate) const LOAD_TOLERANCE: f64 = 1e-5;

/// Resolution: a face whose normal resultant is within this fraction of its owner's weight
/// neither pulls nor presses (a moment, of that weight times the owner's radius of gyration,
/// neither bends it). It is 400 × the 2.5e-12 relative equation residual MEASURED for the
/// house's factor (`.agents/scratch/bestofboth/bob-factor.md`).
const IDLE: f64 = 1e-9;

/// Resolution: a pivot below this fraction of its coordinate's elastic diagonal (its stiffness
/// without friction, never zero or below where any stiffness holds it) is singular within
/// roundoff, a mechanism. It is 400 × the 2.5e-12 relative equation residual MEASURED for the
/// house's factor (`.agents/scratch/bestofboth/bob-factor.md`).
const MECHANISM: f64 = 1e-9;

/// One rigid part of an owner.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Part {
    /// Its element: the structure's `part_of` for each of its chunks.
    pub element: usize,
    pub chunks: Vec<usize>,
    /// Centred at its centre of mass; fixed when it holds buried matter.
    pub node: Node,
    /// kg.
    pub mass: f64,
    /// About its centre: equal-volume chunk spheres with parallel-axis terms, the geometric
    /// assessment stand-in distinct from the occupied inertia physics uses.
    pub inertia: DMat3,
}

/// The live joints between two parts, or from part `a` to the ground (`b` is `None`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Interface {
    pub a: usize,
    pub b: Option<usize>,
    /// Indices of its intact bonded edges in [`Graph::edges`].
    pub bonded: Vec<usize>,
    /// Indices of its push-only edges.
    pub push: Vec<usize>,
}

/// A push-only joint sliding at Coulomb friction, fixed at its slip's start (ASSUMED: a slip
/// keeps its direction and never re-sticks). Its friction is `μ N` times these, `N` its own
/// compression in the solved field ([`bondsolve::Friction`](super::bondsolve::Friction)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Slip {
    /// cos φ t: `t` the unit direction its chunk `a` moves relative to its far side, φ its
    /// face's slip angle between sliding and twisting.
    pub force: DVec3,
    /// sin φ s ρ n, m: `s` the twist's sense, ρ the patch's polar reach about its face's
    /// pressure centre, `n` its normal toward chunk `a`.
    pub twist: DVec3,
}

/// One owner's part graph.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Graph {
    pub parts: Vec<Part>,
    pub interfaces: Vec<Interface>,
    pub edges: Vec<Edge>,
    /// Per edge: its joint.
    pub joints: Vec<usize>,
    /// Per structure chunk: its part, `None` outside this owner or fallen.
    part: Vec<Option<usize>>,
    /// Unanchored components: each is balanced by its rigid acceleration and gauged at its first part.
    free: Vec<Vec<usize>>,
    /// The solver's nodes: the parts', with each free component's gauge part fixed. A gauge is
    /// a coordinate frame, never a support.
    nodes: Vec<Node>,
}

fn find(root: &mut [usize], mut c: usize) -> usize {
    while root[c] != c {
        root[c] = root[root[c]];
        c = root[c];
    }
    c
}

impl Graph {
    /// The part graph of `state`; a push-only interface holding a joint of `slipping` slips.
    pub(crate) fn of(
        structure: &Structure,
        state: &State,
        slipping: &BTreeMap<usize, Slip>,
    ) -> Self {
        let count = structure.chunks.len();
        let live: Vec<usize> = state
            .joints
            .iter()
            .copied()
            .filter(|&j| !state.separated[j] && state.live(&structure.joints[j]))
            .collect();
        let bonded = |j: usize| !state.cracked[j] && structure.joints[j].seam().is_some();
        let mut root: Vec<usize> = (0..count).collect();
        for &j in &live {
            let joint = &structure.joints[j];
            // The soil holds an element's buried chunks: they are a fixed part of their own, so
            // the section at grade is an interface, not the inside of an immovable part.
            if let Side::Chunk(b) = joint.b
                && bonded(j)
                && structure.part_of[joint.a] == structure.part_of[b]
                && structure.chunks[joint.a].buried == structure.chunks[b].buried
            {
                let (x, y) = (find(&mut root, joint.a), find(&mut root, b));
                root[x.max(y)] = x.min(y);
            }
        }
        let mut slot = vec![None; count];
        let mut part = vec![None; count];
        let mut parts: Vec<Part> = Vec::new();
        for &c in state.members.iter().filter(|&&c| state.standing(c)) {
            let r = find(&mut root, c);
            let p = *slot[r].get_or_insert_with(|| {
                parts.push(Part {
                    element: structure.part_of[c],
                    chunks: Vec::new(),
                    node: Node {
                        centre: DVec3::ZERO,
                        fixed: false,
                    },
                    mass: 0.0,
                    inertia: DMat3::ZERO,
                });
                parts.len() - 1
            });
            part[c] = Some(p);
            parts[p].chunks.push(c);
        }
        for p in &mut parts {
            let chunks = || p.chunks.iter().map(|&c| &structure.chunks[c]);
            p.mass = chunks().map(|c| f64::from(c.mass)).sum();
            p.node.centre = chunks()
                .map(|c| c.centroid.as_dvec3() * f64::from(c.mass))
                .sum::<DVec3>()
                / p.mass;
            p.node.fixed = chunks().any(|c| c.buried);
            p.inertia = chunks()
                .map(|c| {
                    let r = c.centroid.as_dvec3() - p.node.centre;
                    let m = f64::from(c.mass);
                    DMat3::from_diagonal(DVec3::splat(
                        c.intrinsic_inertia() + m * r.length_squared(),
                    )) - DMat3::from_cols(r * r.x, r * r.y, r * r.z) * m
                })
                .fold(DMat3::ZERO, |sum, i| sum + i);
        }
        let mut edges = Vec::new();
        let mut joints = Vec::new();
        let mut interfaces: Vec<Interface> = Vec::new();
        let mut by_pair = BTreeMap::new();
        for &j in &live {
            let joint = &structure.joints[j];
            let a = part[joint.a].expect("a live joint's chunk stands in its owner");
            let b = match joint.b {
                Side::Chunk(b) => {
                    Some(part[b].expect("a live joint's far chunk stands in its owner"))
                }
                Side::Ground => None,
            };
            if b == Some(a) {
                continue; // Inside one part: rigid.
            }
            let push = !bonded(j);
            let key = match b {
                Some(b) => (a.min(b), Some(a.max(b))),
                None => (a, None),
            };
            let i = *by_pair.entry(key).or_insert_with(|| {
                interfaces.push(Interface {
                    a: key.0,
                    b: key.1,
                    ..Interface::default()
                });
                interfaces.len() - 1
            });
            let list = if push {
                &mut interfaces[i].push
            } else {
                &mut interfaces[i].bonded
            };
            list.push(edges.len());
            let mut edge = joint_edge(structure, j, a, b, push);
            if let Some(slip) = slipping.get(&j).filter(|_| push) {
                let mu = edge.push.expect("a push edge states its friction");
                edge.shear_stiffness = 0.0;
                edge.friction = Some(super::bondsolve::Friction {
                    force: slip.force * mu,
                    twist: slip.twist * mu,
                });
            } // A bond or a sticking rest holds through its stiffness.
            edges.push(edge);
            joints.push(j);
        }
        let mut neighbours = vec![Vec::new(); parts.len()];
        let mut anchored: Vec<_> = parts.iter().map(|p| p.node.fixed).collect();
        for e in &edges {
            match e.b {
                Some(b) => {
                    neighbours[e.a].push(b);
                    neighbours[b].push(e.a);
                }
                None => anchored[e.a] = true,
            }
        }
        let mut free = Vec::new();
        let mut seen = vec![false; parts.len()];
        for start in 0..parts.len() {
            if seen[start] {
                continue; // Already in a component.
            }
            seen[start] = true;
            let mut stack = vec![start];
            let mut group = Vec::new();
            let mut held = false;
            while let Some(p) = stack.pop() {
                held |= anchored[p];
                group.push(p);
                for &n in &neighbours[p] {
                    if !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
            if !held {
                group.sort_unstable();
                free.push(group);
            }
        }
        let mut nodes: Vec<_> = parts.iter().map(|p| p.node).collect();
        for group in &free {
            nodes[group[0]].fixed = true;
        }
        Self {
            parts,
            interfaces,
            edges,
            joints,
            part,
            free,
            nodes,
        }
    }

    /// The slipping joints that leave `node` a mechanism: those touching the parts `node`
    /// reaches through interfaces with a stiff joint, short of a fixed node.
    fn mechanism(&self, node: usize) -> Vec<usize> {
        let mut seen = vec![false; self.parts.len()];
        let mut stack = vec![node];
        seen[node] = true;
        let mut joints = Vec::new();
        while let Some(p) = stack.pop() {
            for interface in self
                .interfaces
                .iter()
                .filter(|i| i.a == p || i.b == Some(p))
            {
                let slips = |e: &&usize| self.edges[**e].shear_stiffness == 0.0;
                joints.extend(interface.push.iter().filter(slips).map(|&e| self.joints[e]));
                if interface.bonded.is_empty() && interface.push.iter().all(|e| slips(&e)) {
                    continue; // A slipping face holds nothing tangentially.
                }
                let far = if interface.a == p {
                    interface.b
                } else {
                    Some(interface.a)
                };
                if let Some(far) = far
                    && !seen[far]
                    && !self.nodes[far].fixed
                {
                    seen[far] = true;
                    stack.push(far);
                } // The ground and fixed parts hold; they spread nothing.
            }
        }
        joints.sort_unstable();
        joints.dedup();
        joints
    }
}

fn coordinates(w: Wrench) -> [f64; 6] {
    [
        w.linear.x,
        w.linear.y,
        w.linear.z,
        w.angular.x,
        w.angular.y,
        w.angular.z,
    ]
}

fn wrench(v: &[f64]) -> Wrench {
    Wrench {
        linear: DVec3::new(v[0], v[1], v[2]),
        angular: DVec3::new(v[3], v[4], v[5]),
    }
}

/// An edge's carried wrench as it loads `node`, about that node's centre.
fn at_node(e: &Edge, nodes: &[Node], carried: Wrench, node: usize) -> Wrench {
    let sign = if node == e.a { 1.0 } else { -1.0 };
    Wrench {
        linear: carried.linear * sign,
        angular: (carried.angular + (e.centre - nodes[node].centre).cross(carried.linear)) * sign,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Column {
    rows: Vec<usize>,
    /// The pivot, then `L` below it across `rows`.
    values: Vec<f64>,
    /// The pivot, then this row of `U` across `rows`.
    upper: Vec<f64>,
}

/// The sparse LU factor of a node graph's stiffness (parts, or one part's chunks), ordered by
/// exact minimum degree on node blocks on its symmetric pattern, without pivoting; fixed nodes
/// are eliminated. Bonded and sticking stiffness is symmetric positive definite, where this is
/// `L D Lᵀ`; a slipping patch removes its tangential stiffness and adds its friction's
/// non-symmetric term (ASSUMED: a perturbation the order's pivots absorb). No regularization or
/// invented support stands in for what slip removed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Factor {
    columns: Vec<Column>,
    map: Vec<Option<usize>>,
    /// Scalar operations of its numeric factorization, a DERIVED loop count.
    pub ops: u64,
}

/// A node graph's stiffness that leaves a motion free within [`MECHANISM`].
#[derive(Clone, Debug, PartialEq)]
struct Mechanism {
    /// The node whose pivot fell.
    node: usize,
    /// Per node, the free motion: that pivot's coordinate moved by one, every node ordered
    /// after it held, and the nodes before it in equilibrium. A fixed node does not move.
    mode: Vec<Wrench>,
}

impl Mechanism {
    /// The edges that carry its motion: those whose two sides move apart, over their patch, by
    /// more than `√MECHANISM` of the most any edge does (Resolution: the mode moves a stiff
    /// edge by about [`MECHANISM`] of a free one, so the geometric mean parts the two).
    fn carriers(&self, nodes: &[Node], edges: &[Edge]) -> Vec<usize> {
        let motion = |e: &Edge| {
            let at = |n: usize| {
                let w = self.mode[n];
                w.linear + w.angular.cross(e.centre - nodes[n].centre)
            };
            let (mut shift, mut turn) = (at(e.a), self.mode[e.a].angular);
            if let Some(b) = e.b {
                shift -= at(b);
                turn -= self.mode[b].angular;
            }
            // The patch's radius of gyration turns a rotation into the motion over its area.
            shift.length() + turn.length() * e.patch.polar.sqrt()
        };
        let motions: Vec<f64> = edges.iter().map(motion).collect();
        let most = motions.iter().copied().fold(0.0, f64::max);
        assert!(
            most > 0.0,
            "a mechanism's mode moves its pivot's node against an anchor or a held node"
        );
        (0..edges.len())
            .filter(|&e| motions[e] > MECHANISM.sqrt() * most)
            .collect()
    }
}

impl Factor {
    /// The factor, or the mechanism whose pivot fell below [`MECHANISM`] of its elastic diagonal.
    fn new(nodes: &[Node], edges: &[Edge]) -> Result<Self, Mechanism> {
        let mut graph = vec![std::collections::BTreeSet::new(); nodes.len()];
        for e in edges {
            if let Some(b) = e.b
                && !nodes[e.a].fixed
                && !nodes[b].fixed
            {
                graph[e.a].insert(b);
                graph[b].insert(e.a);
            }
        }
        let mut live: std::collections::BTreeSet<_> =
            (0..nodes.len()).filter(|&i| !nodes[i].fixed).collect();
        let mut fronts = Vec::new();
        let mut order = Vec::new();
        let mut map = vec![None; nodes.len()];
        while let Some(&a) = live.iter().min_by_key(|&&i| (graph[i].len(), i)) {
            order.push(a);
            let front: Vec<_> = graph[a].iter().copied().collect();
            for (offset, &b) in front.iter().enumerate() {
                graph[b].remove(&a);
                for &c in &front[offset + 1..] {
                    if graph[b].insert(c) {
                        graph[c].insert(b);
                    }
                }
            }
            live.remove(&a);
            map[a] = Some(6 * fronts.len());
            fronts.push(front);
        }
        let n = 6 * fronts.len();
        // Per column (a unit displacement): each row's force, `K[row][column]`.
        let mut matrix = vec![BTreeMap::<usize, f64>::new(); n];
        // Per coordinate: its stiffness's diagonal without friction, the scale a pivot is judged
        // against. Friction's non-symmetric term can lower a diagonal to zero or below, where
        // a relative test on it would pass a vanishing pivot.
        let mut elastic = vec![0.0; n];
        let mut input = vec![Wrench::default(); nodes.len()];
        for e in edges {
            let bare = e.friction.map(|_| Edge {
                friction: None,
                ..e.clone()
            });
            let ends: Vec<_> = [Some(e.a), e.b]
                .into_iter()
                .flatten()
                .filter(|&a| !nodes[a].fixed)
                .collect();
            for &a in &ends {
                for axis in 0..6 {
                    let mut unit = [0.0; 6];
                    unit[axis] = 1.0;
                    input[a] = wrench(&unit);
                    let carried = e.resultant(nodes, &input);
                    let stiff = bare
                        .as_ref()
                        .map_or(carried, |b| b.resultant(nodes, &input));
                    input[a] = Wrench::default();
                    let column = map[a].expect("a free node is ordered") + axis;
                    elastic[column] += coordinates(at_node(e, nodes, stiff, a))[axis];
                    for &b in &ends {
                        let base = map[b].expect("a free node is ordered");
                        for (dof, value) in coordinates(at_node(e, nodes, carried, b))
                            .into_iter()
                            .enumerate()
                        {
                            *matrix[column].entry(base + dof).or_default() += value;
                        }
                    }
                }
            }
        }
        // An entry absent from the sparse assembly is the matrix's mathematical zero.
        let entry = |row: usize, column: usize| matrix[column].get(&row).copied().unwrap_or(0.0);
        let mut columns = Vec::with_capacity(n);
        for (block, front) in fronts.iter().enumerate() {
            for axis in 0..6 {
                let j = 6 * block + axis;
                let mut rows: Vec<_> = (j..6 * (block + 1)).collect();
                for &b in front {
                    let base = map[b].expect("a front node is ordered");
                    rows.extend(base..base + 6);
                }
                rows.sort_unstable();
                columns.push(Column {
                    values: vec![0.0; rows.len()],
                    upper: vec![0.0; rows.len()],
                    rows,
                });
            }
        }
        let mut prior = vec![Vec::new(); n];
        for (j, c) in columns.iter().enumerate() {
            for (pos, &i) in c.rows.iter().enumerate().skip(1) {
                prior[i].push((j, pos));
            }
        }
        let (mut lower, mut upper) = (vec![0.0; n], vec![0.0; n]);
        let mut ops = 0;
        for j in 0..n {
            for &i in &columns[j].rows {
                lower[i] = entry(i, j);
                upper[i] = entry(j, i);
            }
            for &(k, pos) in &prior[j] {
                let c = &columns[k];
                let (ljk, ukj) = (c.values[pos], c.upper[pos]);
                for ((&i, &lik), &uki) in c.rows[pos..]
                    .iter()
                    .zip(&c.values[pos..])
                    .zip(&c.upper[pos..])
                {
                    lower[i] -= lik * ukj;
                    upper[i] -= ljk * uki;
                    ops += 4;
                }
            }
            assert!(
                lower[j].is_finite(),
                "a finite stiffness whose pivots each passed MECHANISM of their elastic diagonal \
                 has finite pivots"
            );
            if lower[j] <= MECHANISM * elastic[j] {
                // DERIVED: rows before `j` of `K x = 0`, with `x_j = 1` and every later
                // coordinate held, are `U₁₁ x₁ = −U₁₂ e_j`: the factor's own back-substitution.
                let mut x = vec![0.0; n];
                x[j] = 1.0;
                for c in (0..j).rev() {
                    let column = &columns[c];
                    let sum: f64 = column
                        .rows
                        .iter()
                        .zip(&column.upper)
                        .skip(1)
                        .map(|(&i, &u)| u * x[i])
                        .sum();
                    x[c] = -sum / column.upper[0];
                }
                return Err(Mechanism {
                    node: order[j / 6],
                    mode: map
                        .iter()
                        .map(|&base| base.map_or_else(Wrench::default, |b| wrench(&x[b..b + 6])))
                        .collect(),
                });
            }
            let pivot = lower[j];
            let c = &mut columns[j];
            c.values[0] = pivot;
            c.upper[0] = pivot;
            for ((&i, value), up) in c.rows.iter().zip(&mut c.values).zip(&mut c.upper).skip(1) {
                *value = lower[i] / pivot;
                *up = upper[i];
            }
            ops += c.rows.len() as u64;
        }
        Ok(Self { columns, map, ops })
    }

    /// Unknown coordinates: six per free part.
    pub(crate) fn unknowns(&self) -> usize {
        self.columns.len()
    }

    /// Stored entries of `L` and of `U` each, the pivots counted once in each.
    pub(crate) fn entries(&self) -> usize {
        self.columns.iter().map(|c| c.rows.len()).sum()
    }

    /// Scalar operations of one load column's two triangular solves, a DERIVED count.
    pub(crate) fn solve_ops(&self) -> u64 {
        (4 * (self.entries() - self.unknowns()) + self.unknowns()) as u64
    }

    fn solve(&self, loads: &[Wrench]) -> Vec<Wrench> {
        let mut x = vec![0.0; self.columns.len()];
        for (&base, &load) in self.map.iter().zip(loads) {
            if let Some(base) = base {
                x[base..base + 6].copy_from_slice(&coordinates(load));
            } // A fixed node is buried matter or a free component's gauge.
        }
        for (j, c) in self.columns.iter().enumerate() {
            for (&i, &l) in c.rows.iter().zip(&c.values).skip(1) {
                x[i] -= l * x[j];
            }
        }
        for (j, c) in self.columns.iter().enumerate().rev() {
            for (&i, &u) in c.rows.iter().zip(&c.upper).skip(1) {
                x[j] -= u * x[i];
            }
            x[j] /= c.upper[0];
        }
        self.map
            .iter()
            .map(|&base| base.map_or_else(Wrench::default, |base| wrench(&x[base..base + 6])))
            .collect()
    }
}

/// The joints an owner's topology reads: its standing members, then each live joint and
/// whether it is cracked.
#[derive(Clone, Debug, Default, PartialEq)]
struct Signature {
    members: Vec<usize>,
    joints: Vec<(usize, bool)>,
}

impl Signature {
    fn of(structure: &Structure, state: &State) -> Self {
        Self {
            members: state
                .members
                .iter()
                .copied()
                .filter(|&c| state.standing(c))
                .collect(),
            joints: state
                .joints
                .iter()
                .copied()
                .filter(|&j| !state.separated[j] && state.live(&structure.joints[j]))
                .map(|j| (j, state.cracked[j]))
                .collect(),
        }
    }
}

/// One owner's part graph, factor and last solved field. Source addresses stay in the ledger.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Assessment {
    pub(crate) graph: Graph,
    pub(crate) factor: Factor,
    signature: Signature,
    /// The solved loads per part: sustained, then sustained plus this tick's pulse.
    loads: Vec<Wrench>,
    answer: Vec<Wrench>,
    /// The owner's weight, N, and that weight times its radius of gyration, N·m.
    scale: (f64, f64),
    topology_changed: bool,
    initialized: bool,
    last_spin: Vec3,
    impulsive: bool,
    /// Gravity overloads compiled at load await the first authoritative failure stage.
    initial_verdict: Vec<Failure>,
    /// Every push-only joint sliding at Coulomb friction. It slips until its face lifts or tips,
    /// or its part is released.
    pub(crate) slipping: BTreeMap<usize, Slip>,
    /// Per element: the chunk-scale kernel of its unburied chunks, factored once per design at
    /// load and shared by its instances; the event entry for an intact element.
    pub(crate) kernels: std::sync::Arc<Vec<Kernel>>,
    /// Chunks removed since the last hole solve: their neighbours' parts await it.
    removed: Vec<usize>,
    pub(crate) support: Vec<Vec<usize>>,
    /// Scalar operations of the last tick's factorization and solves, a DERIVED count.
    pub(crate) work: u64,
}

impl Assessment {
    #[cfg(test)]
    pub(crate) fn applied_force(&self) -> DVec3 {
        self.loads[self.graph.parts.len()..]
            .iter()
            .map(|l| l.linear)
            .sum()
    }

    pub(crate) fn needs_tick(&self) -> bool {
        !self.initialized
            || !self.initial_verdict.is_empty()
            || self.topology_changed
            || self.impulsive
    }

    pub(crate) fn spin_changed(&self, spin: Vec3) -> bool {
        self.last_spin != spin
    }

    /// Factor and settle initial gravity once per construction asset, slips included; instances
    /// share it until their own first change. Overloads are committed by the authority, never
    /// during loading.
    pub(crate) fn at_rest(structure: &Structure) -> Self {
        let pristine = State::new(structure);
        let mut state = pristine.clone();
        let mut assessment = Self::default();
        let mut verdict = assessment.rebuild(structure, &mut state);
        if verdict.is_empty() {
            assessment.loads = assessment.loads_of(&[], Vec3::ZERO, 1.0);
            verdict = assessment
                .settle(structure, &mut state, &[], Vec3::ZERO, 1.0)
                .0;
        }
        // The authority's first tick commits them on the instance's own state.
        assessment.initial_verdict = verdict;
        let mut elements = vec![Vec::new(); structure.part_of.iter().max().map_or(0, |&e| e + 1)];
        for (c, chunk) in structure.chunks.iter().enumerate() {
            if !chunk.buried {
                elements[structure.part_of[c]].push(c);
            } // Buried chunks are the soil's: a kernel's anchors.
        }
        assessment.kernels = std::sync::Arc::new(
            elements
                .iter()
                .map(|chunks| Kernel::of(structure, &pristine, chunks))
                .collect(),
        );
        assessment.support = super::loose_groups(structure, &pristine);
        // A disconnected initial component is released even when nothing loads it.
        assessment.topology_changed = !assessment.support.is_empty();
        assessment
    }

    /// Rebuild the part graph and factor it. A mechanism separates the slipping rests that leave
    /// it without a tangential hold or, where it reaches none, the joints its free motion moves
    /// ([`Mechanism::carriers`]); returns those failures.
    fn rebuild(&mut self, structure: &Structure, state: &mut State) -> Vec<Failure> {
        let mut lost = Vec::new();
        // A slip ends with its joint: once it opens or its chunks fall.
        self.slipping
            .retain(|&j, _| !state.separated[j] && state.live(&structure.joints[j]));
        loop {
            self.graph = Graph::of(structure, state, &self.slipping);
            match Factor::new(&self.graph.nodes, &self.graph.edges) {
                Ok(factor) => {
                    self.factor = factor;
                    break;
                }
                Err(mechanism) => {
                    let mut joints = self.graph.mechanism(mechanism.node);
                    if joints.is_empty() {
                        // No slip made it: the joints its free motion moves are too slight to
                        // hold it (a remnant of removed or cracked matter), and they part.
                        joints = mechanism
                            .carriers(&self.graph.nodes, &self.graph.edges)
                            .into_iter()
                            .map(|e| self.graph.joints[e])
                            .collect();
                    }
                    for j in joints {
                        state.cracked[j] = true;
                        state.separated[j] = true;
                        self.slipping.remove(&j);
                        lost.push(Failure {
                            joint: j,
                            cause: Cause::Slide,
                            demand: 0.0,
                            capacity: 0.0,
                            work: 0.0,
                        });
                    }
                }
            }
        }
        self.work += self.factor.ops;
        self.signature = Signature::of(structure, state);
        let parts = &self.graph.parts;
        let mass: f64 = parts.iter().map(|p| p.mass).sum();
        // An owner whose matter all turned to dust has no part, no load and nothing to judge.
        self.scale = if parts.is_empty() {
            (0.0, 0.0)
        } else {
            let centre = parts.iter().map(|p| p.node.centre * p.mass).sum::<DVec3>() / mass;
            let gyration = parts
                .iter()
                .map(|p| {
                    let i = p.inertia;
                    (i.x_axis.x + i.y_axis.y + i.z_axis.z) / 2.0
                        + p.mass * (p.node.centre - centre).length_squared()
                })
                .sum::<f64>()
                / mass;
            let weight = mass * f64::from(crate::physical_laws::gravity_mps2());
            (weight, weight * gyration.sqrt())
        };
        self.loads.clear();
        self.topology_changed = false;
        self.initialized = true;
        lost
    }

    /// Per part: its sustained wrench (gravity and presses), then that plus each blow as its
    /// mean force over the step, capped by its stated peak. A free component's rigid
    /// acceleration is removed, its spin adding centrifugal and gyroscopic loading.
    fn loads_of(&self, sources: &[Load], spin: Vec3, step: f32) -> Vec<Wrench> {
        let parts = &self.graph.parts;
        let mut steady = vec![Wrench::default(); parts.len()];
        let mut pulse = vec![Wrench::default(); parts.len()];
        for source in sources {
            let Some(p) = self.graph.part[source.chunk] else {
                continue; // Removal consumed this carrier.
            };
            let mut force = source.blow.as_dvec3() / f64::from(step);
            if let Some(peak) = source.peak {
                let magnitude = force.length();
                if magnitude > f64::from(peak.length()) {
                    force *= f64::from(peak.length()) / magnitude;
                }
            }
            let arm = source.at.as_dvec3() - parts[p].node.centre;
            steady[p].add(Wrench::at(source.press.as_dvec3(), arm), 1.0);
            pulse[p].add(Wrench::at(force, arm), 1.0);
        }
        let mut free = vec![false; parts.len()];
        for group in &self.graph.free {
            let nodes: Vec<_> = group.iter().map(|&p| parts[p].node).collect();
            let masses: Vec<_> = group.iter().map(|&p| parts[p].mass).collect();
            let own: Vec<_> = group.iter().map(|&p| parts[p].inertia).collect();
            let mut framed: Vec<_> = group.iter().map(|&p| steady[p]).collect();
            let mut impulses: Vec<_> = group.iter().map(|&p| pulse[p]).collect();
            accelerating_frame(&nodes, &masses, &own, spin.as_dvec3(), &mut framed);
            accelerating_frame(&nodes, &masses, &own, DVec3::ZERO, &mut impulses);
            for ((&p, load), impulse) in group.iter().zip(framed).zip(impulses) {
                steady[p] = load;
                pulse[p] = impulse;
                free[p] = true;
            }
        }
        let g = f64::from(crate::physical_laws::gravity_mps2());
        for ((load, part), free) in steady.iter_mut().zip(parts).zip(free) {
            if !free {
                load.linear -= DVec3::Y * (g * part.mass);
            } // Uniform gravity cancels from a free component's internal demand.
        }
        let total: Vec<_> = steady
            .iter()
            .zip(pulse)
            .map(|(&s, p)| {
                let mut sum = s;
                sum.add(p, 1.0);
                sum
            })
            .collect();
        steady.into_iter().chain(total).collect()
    }

    fn within_tolerance(&self, loads: &[Wrench]) -> bool {
        let (force, moment) = self.scale;
        loads.len() == self.loads.len()
            && loads.iter().zip(&self.loads).all(|(a, b)| {
                (a.linear - b.linear).length() <= LOAD_TOLERANCE * force
                    && (a.angular - b.angular).length() <= LOAD_TOLERANCE * moment
            })
    }

    /// Solve both load columns; returns how many were solved. An owner whose parts are all
    /// fixed (buried, or one gauged free part) has no unknown and no internal demand to solve.
    fn solve(&mut self) -> usize {
        let n = self.graph.parts.len();
        if self.factor.unknowns() == 0 {
            self.answer = vec![Wrench::default(); 2 * n];
            return 0;
        }
        let mut answer = self.factor.solve(&self.loads[..n]);
        answer.extend(self.factor.solve(&self.loads[n..]));
        self.answer = answer;
        self.work += 2 * self.factor.solve_ops();
        2
    }

    /// One tick: rebuild and refactor on changed topology, and when a part's loads moved past
    /// [`LOAD_TOLERANCE`], settle the contacts and judge the bonds ([`Self::settle`]), then solve
    /// the holes removal opened. A crack changes topology for the next tick. Returns the failures
    /// and the number of load columns solved.
    pub(crate) fn advance(
        &mut self,
        structure: &Structure,
        state: &mut State,
        sources: &[Load],
        spin: Vec3,
        step: f32,
        changed: bool,
    ) -> (Vec<Failure>, usize) {
        assert!(
            step.is_finite() && step > 0.0,
            "the part solve has a positive numerical tick"
        );
        let pending = !std::mem::take(&mut self.initial_verdict).is_empty();
        self.last_spin = spin;
        self.work = 0;
        let topology = !self.initialized
            || self.topology_changed
            || changed
            || Signature::of(structure, state) != self.signature;
        let lost = if topology {
            let before = std::mem::take(&mut self.signature.members);
            self.removed
                .extend(before.into_iter().filter(|&c| !state.standing(c)));
            self.rebuild(structure, state)
        } else {
            Vec::new()
        };
        let loads = self.loads_of(sources, spin, step);
        self.impulsive = sources
            .iter()
            .any(|s| s.blow != Vec3::ZERO && self.graph.part[s.chunk].is_some());
        let (failures, columns) = if !lost.is_empty() {
            (lost, 0) // A mechanism's rests left; the next tick solves what stands.
        } else if !topology && !pending && self.within_tolerance(&loads) {
            return (Vec::new(), 0);
        } else {
            self.loads = loads;
            let (mut failures, columns) = self.settle(structure, state, sources, spin, step);
            if failures.is_empty() && !self.removed.is_empty() {
                let removed = std::mem::take(&mut self.removed);
                failures = self.holes(structure, state, &removed, sources);
            } // A failure changes topology first: the holes wait for the field that follows.
            (failures, columns)
        };
        self.topology_changed = !failures.is_empty();
        if topology || self.topology_changed {
            self.support = super::loose_groups(structure, state);
        }
        (failures, columns)
    }

    /// Settle the contact active set, then judge the bonds on the final field. Each pass solves,
    /// opens every push-only face that lifts or tips and starts every face whose friction wrench
    /// passes its Coulomb capacity slipping; a face only goes stick to slip to open, so the passes
    /// end within one per face. A part a slip leaves a mechanism opens its slipping rests. A
    /// cohesive failure commits only on the final field. Returns the failures and the load columns
    /// solved.
    fn settle(
        &mut self,
        structure: &Structure,
        state: &mut State,
        sources: &[Load],
        spin: Vec3,
        step: f32,
    ) -> (Vec<Failure>, usize) {
        let mut columns = 0;
        let mut failures = Vec::new();
        loop {
            columns += self.solve();
            let contact = self.contact();
            if contact.opened.is_empty() && contact.slips.is_empty() {
                break;
            }
            for failure in &contact.opened {
                state.cracked[failure.joint] = true;
                state.separated[failure.joint] = true;
            }
            failures.extend(contact.opened);
            self.slipping.extend(contact.slips);
            failures.extend(self.rebuild(structure, state));
            self.loads = self.loads_of(sources, spin, step);
        }
        let (cracks, separated) = self.bonds(structure);
        for failure in &cracks {
            state.cracked[failure.joint] = true;
        }
        for &j in &separated {
            state.separated[j] = true;
        }
        failures.extend(cracks);
        (failures, columns)
    }

    /// Holes: each unburied part beside a chunk in `removed` is solved at chunk scale ([`Kernel`],
    /// factored again for its remaining chunks) for gravity, the presses of `sources` on its
    /// chunks and the sustained forces its other interfaces hand it, friction included. Its joints
    /// that cannot hold crack or separate, and it is solved again on what is left until it holds;
    /// each pass changes a joint for good, so it ends. The part law judges the pieces from the
    /// next tick.
    fn holes(
        &mut self,
        structure: &Structure,
        state: &mut State,
        removed: &[usize],
        sources: &[Load],
    ) -> Vec<Failure> {
        let mut beside = std::collections::BTreeSet::new();
        for &r in removed {
            beside.extend(
                state.topology.neighbours[r]
                    .iter()
                    .filter_map(|&(b, _)| self.graph.part[b]),
            ); // A neighbour outside this owner or fallen has no part.
        }
        let mut failures = Vec::new();
        for p in beside {
            if self.graph.parts[p].node.fixed {
                continue; // The soil holds buried matter.
            }
            let chunks = self.graph.parts[p].chunks.clone();
            let loads = self.boundary(structure, p, &chunks, sources);
            loop {
                let kernel = Kernel::of(structure, state, &chunks);
                let hold = kernel.hold(structure, &loads);
                self.work += kernel.factor.ops + hold.work;
                if hold.failures.is_empty() {
                    break;
                }
                for failure in &hold.failures {
                    state.cracked[failure.joint] = true;
                }
                for &j in &hold.separated {
                    state.separated[j] = true;
                }
                failures.extend(hold.failures);
            }
        }
        failures
    }

    /// Per chunk of part `p`: the presses of `sources` on it, and the sustained forces the part
    /// graph's edges to other unburied parts hand it (friction included), about its centroid. The
    /// ground and buried parts are the kernel's anchors, not loads.
    fn boundary(
        &self,
        structure: &Structure,
        p: usize,
        chunks: &[usize],
        sources: &[Load],
    ) -> Vec<Wrench> {
        let steady = &self.answer[..self.graph.parts.len()];
        let index: BTreeMap<usize, usize> =
            chunks.iter().enumerate().map(|(i, &c)| (c, i)).collect();
        let centroid = |c: usize| structure.chunks[c].centroid.as_dvec3();
        let mut loads = vec![Wrench::default(); chunks.len()];
        for source in sources {
            if let Some(&i) = index.get(&source.chunk) {
                let arm = source.at.as_dvec3() - centroid(source.chunk);
                loads[i].add(Wrench::at(source.press.as_dvec3(), arm), 1.0);
            } // A press on another part reaches this one through its interfaces.
        }
        for (e, edge) in self.graph.edges.iter().enumerate() {
            let (far, sign) = match edge.b {
                Some(b) if edge.a == p => (b, -1.0),
                Some(b) if b == p => (edge.a, 1.0),
                _ => continue, // Not this part's, or the ground: an anchor.
            };
            if self.graph.parts[far].node.fixed {
                continue; // Buried matter is an anchor.
            }
            let joint = &structure.joints[self.graph.joints[e]];
            let chunk = match joint.b {
                Side::Chunk(b) if sign > 0.0 => b,
                _ => joint.a,
            };
            let carried = edge.resultant(&self.graph.nodes, steady);
            let force = carried.linear * sign;
            loads[index[&chunk]].add(
                Wrench {
                    linear: force,
                    angular: carried.angular * sign + (edge.centre - centroid(chunk)).cross(force),
                },
                1.0,
            );
        }
        loads
    }

    /// Every bonded interface's verdict on the settled field: its cracks, and the joints that
    /// separate. A failed bond whose interface the field pulls apart separates in the same tick.
    fn bonds(&self, structure: &Structure) -> (Vec<Failure>, Vec<usize>) {
        let n = self.graph.parts.len();
        let (steady, total) = self.answer.split_at(n);
        let (mut failures, mut separated) = (Vec::new(), Vec::new());
        for interface in &self.graph.interfaces {
            let (cracks, field) = self.bonded_verdict(structure, interface, steady, total);
            if cracks.is_empty() {
                continue; // Every bond holds.
            }
            let all: Vec<_> = interface
                .bonded
                .iter()
                .chain(&interface.push)
                .copied()
                .collect();
            if self.face(interface, &all, field).compression < -IDLE * self.scale.0 {
                separated.extend(all.iter().map(|&e| self.graph.joints[e]));
                failures.extend(interface.push.iter().map(|&e| Failure {
                    joint: self.graph.joints[e],
                    cause: Cause::LiftForce,
                    demand: 0.0,
                    capacity: 0.0,
                    work: 0.0,
                }));
            }
            failures.extend(cracks);
        }
        (failures, separated)
    }
    /// The interface's bonded joints crack together once one reaches its strength: a sustained
    /// field (gravity and presses) over its tensile strength or its cohesion plus friction on its
    /// compression; or the total field over either strength while the pulse adds at least the
    /// whole interface's opening or sliding work, `Σ G A`. A force-controlled driver pays its own
    /// separation work. Returns the cracks and the field that decided them.
    fn bonded_verdict<'a>(
        &self,
        structure: &Structure,
        interface: &Interface,
        steady: &'a [Wrench],
        total: &'a [Wrench],
    ) -> (Vec<Failure>, &'a [Wrench]) {
        let nodes = &self.graph.nodes;
        struct Row {
            joint: usize,
            sustained: [(f64, f64); 2],
            pulse: [(f64, f64); 2],
            work: [f64; 2],
        }
        let mut rows = Vec::new();
        let (mut added, mut cost) = ([0.0; 2], [0.0; 2]);
        for &e in &interface.bonded {
            let edge = &self.graph.edges[e];
            let joint = &structure.joints[self.graph.joints[e]];
            let seam = joint.seam().expect("a bonded edge holds by a seam");
            let strength = super::bondsolve::JointStrength::of(structure, joint, false);
            let read = |field: &[Wrench]| {
                let carried = edge.resultant(nodes, field);
                let (tension, shear) = edge.stress(carried, edge.normal);
                let compression = (-carried.linear.dot(edge.normal) / edge.area).max(0.0);
                let [tensile, sliding] = strength.limits(compression);
                [(tension, tensile), (shear, sliding)]
            };
            let (opening, sliding) = edge.energy(nodes, total);
            let (held_opening, held_sliding) = edge.energy(nodes, steady);
            added[0] += opening - held_opening;
            added[1] += sliding - held_sliding;
            let work = [
                edge.area * f64::from(seam.fracture_energy),
                edge.area * f64::from(seam.shear.energy),
            ];
            cost[0] += work[0];
            cost[1] += work[1];
            rows.push(Row {
                joint: self.graph.joints[e],
                sustained: read(steady),
                pulse: read(total),
                work,
            });
        }
        let over = |reading: &[(f64, f64); 2], mode: usize| reading[mode].0 > reading[mode].1;
        let verdict = (0..2)
            .find(|&mode| rows.iter().any(|r| over(&r.sustained, mode)))
            .map(|mode| (mode, false))
            .or_else(|| {
                (0..2)
                    .find(|&mode| {
                        rows.iter().any(|r| over(&r.pulse, mode)) && added[mode] >= cost[mode]
                    })
                    .map(|mode| (mode, true))
            });
        let Some((mode, paid)) = verdict else {
            return (Vec::new(), steady); // Every bonded joint holds.
        };
        let cracks = rows
            .iter()
            .map(|r| {
                let reading = if paid { r.pulse } else { r.sustained };
                Failure {
                    joint: r.joint,
                    cause: [Cause::Tension, Cause::Shear][mode],
                    demand: reading[mode].0 as f32,
                    capacity: reading[mode].1 as f32,
                    work: if paid { r.work[mode] as f32 } else { 0.0 },
                }
            })
            .collect();
        (cracks, if paid { total } else { steady })
    }

    /// The resultant `edges` of `interface` carry in `field`, read from the interface's `a` side
    /// as one face (ASSUMED for a face of several joints): their area-weighted normal, and the
    /// hull of their corners projected on it.
    fn face(&self, interface: &Interface, edges: &[usize], field: &[Wrench]) -> Face {
        let nodes = &self.graph.nodes;
        let area: f64 = edges.iter().map(|&e| self.graph.edges[e].area).sum();
        let centre = edges
            .iter()
            .map(|&e| self.graph.edges[e].centre * self.graph.edges[e].area)
            .sum::<DVec3>()
            / area;
        let (mut normal, mut force, mut moment) = (DVec3::ZERO, DVec3::ZERO, DVec3::ZERO);
        for &e in edges {
            let edge = &self.graph.edges[e];
            // A carried wrench and its normal flip with the side they are read from.
            let sign = if edge.a == interface.a { 1.0 } else { -1.0 };
            let carried = edge.resultant(nodes, field);
            normal += edge.normal * (sign * edge.area);
            force += carried.linear * sign;
            moment += carried.angular * sign + (edge.centre - centre).cross(carried.linear * sign);
        }
        let normal = normal.normalize();
        let compression = -force.dot(normal);
        Face {
            centre,
            normal,
            compression,
            shear: force + normal * compression,
            bend: moment - normal * moment.dot(normal),
            twist: moment.dot(normal),
            corners: edges
                .iter()
                .flat_map(|&e| self.graph.edges[e].corners())
                .map(|p| p - centre)
                .collect(),
        }
    }

    /// Every push-only face's contact verdict on the solved total field, whatever bonds share its
    /// interface: a rest bears on its own and never borrows a bond's tension. A face is an
    /// interface's push-only joints whose normals, read from its `a` side, lean the same way as
    /// the face's first joint (a positive dot; ASSUMED), so a part wedged between two faces of
    /// another is judged on each.
    fn contact(&self) -> Contact {
        let total = &self.answer[self.graph.parts.len()..];
        let edges = &self.graph.edges;
        let mut contact = Contact::default();
        for interface in &self.graph.interfaces {
            let toward_a =
                |e: usize| edges[e].normal * if edges[e].a == interface.a { 1.0 } else { -1.0 };
            let mut faces: Vec<Vec<usize>> = Vec::new();
            for &e in &interface.push {
                match faces
                    .iter_mut()
                    .find(|face| toward_a(face[0]).dot(toward_a(e)) > 0.0)
                {
                    Some(face) => face.push(e),
                    None => faces.push(vec![e]),
                }
            }
            for face in &faces {
                self.face_verdict(interface, face, total, &mut contact);
            }
        }
        contact
    }

    /// One face bears as a whole (ASSUMED): it opens when its resultant pulls (lift) or its
    /// pressure centre leaves the hull of its corners (tip). Coulomb friction bounds its whole
    /// friction wrench, about its pressure centre `p`: each patch `i` presses with `Nᵢ` (its own
    /// normal force) at friction `μᵢ`, so the face holds a sliding force up to `F = Σ μᵢ Nᵢ`
    /// and a twist up to `T = Σ μᵢ Nᵢ ρᵢ`, where `ρᵢ² = Qᵢ + |cᵢ − p|²` is the patch's polar
    /// radius of gyration (`Qᵢ`, from its geometry) moved to `p` (DERIVED for uniform pressure as
    /// the root-mean-square reach, which bounds the mean reach by Cauchy–Schwarz). It slips past
    /// the elliptic limit `(f / F)² + (τ / T)² = 1` (ASSUMED: the elliptic approximation of the
    /// limit surface), `f` its sliding and `τ` its twisting demand. A slip keeps the direction
    /// `(cos φ t, sin φ s)` the demand had at its start, `tan φ = (τ / T) / (f / F)`. A face
    /// carrying less than [`IDLE`] is left.
    fn face_verdict(
        &self,
        interface: &Interface,
        joints: &[usize],
        total: &[Wrench],
        contact: &mut Contact,
    ) {
        let edges = &self.graph.edges;
        let face = self.face(interface, joints, total);
        let (idle, idle_moment) = (IDLE * self.scale.0, IDLE * self.scale.1);
        let sign = |e: usize| if edges[e].a == interface.a { 1.0 } else { -1.0 };
        let (cause, offset) = if face.compression < -idle {
            (Some(Cause::LiftForce), DVec3::ZERO)
        } else if face.compression <= idle {
            // No compression holds it: a bending moment it carries tips it.
            let cause = (face.bend.length() > idle_moment).then_some(Cause::Tip);
            (cause, DVec3::ZERO)
        } else {
            // DERIVED: a normal force N at offset r about the centre has the in-plane moment
            // r × (−N n), so r = (M⊥ × n) / N.
            let offset = face.bend.cross(face.normal) / face.compression;
            let cause = (!inside(&face.corners, face.normal, offset)).then_some(Cause::Tip);
            (cause, offset)
        };
        if let Some(cause) = cause {
            contact.opened.extend(joints.iter().map(|&e| Failure {
                joint: self.graph.joints[e],
                cause,
                demand: 0.0,
                capacity: 0.0,
                work: 0.0,
            }));
            return;
        }
        let under_way = joints.iter().find_map(|&e| {
            let slip = self.slipping.get(&self.graph.joints[e])?;
            Some((slip.force * sign(e), slip.twist * sign(e)))
        });
        if let Some((force, twist)) = under_way {
            // A rest that joins a slipping face slips with it (ASSUMED: at the reach of the
            // rest it joined).
            for &e in joints {
                let joint = self.graph.joints[e];
                if !self.slipping.contains_key(&joint) {
                    contact.slips.push((
                        joint,
                        Slip {
                            force: force * sign(e),
                            twist: twist * sign(e),
                        },
                    ));
                }
            }
            return;
        }
        let pressure = face.centre + offset;
        let flat = |r: DVec3| r - face.normal * r.dot(face.normal);
        let mut reach = Vec::with_capacity(joints.len());
        let (mut sliding, mut twisting) = (0.0, 0.0);
        for &e in joints {
            let edge = &edges[e];
            let pressing = -edge
                .resultant(&self.graph.nodes, total)
                .linear
                .dot(edge.normal);
            let rho = (edge.patch.polar + flat(edge.centre - pressure).length_squared()).sqrt();
            let held = edge.push.expect("a push edge states its friction") * pressing.max(0.0);
            sliding += held;
            twisting += held * rho;
            reach.push(rho);
        }
        let f = face.shear.length();
        // The twist about the pressure centre: the face's own, less its shear's moment there.
        let tau = face.twist - flat(offset).cross(face.shear).dot(face.normal);
        let lever = if sliding > idle {
            twisting / sliding
        } else {
            reach.iter().sum::<f64>() / reach.len() as f64
        };
        let holds = if sliding > idle {
            (f / sliding).powi(2) + (tau / twisting).powi(2) <= 1.0
        } else {
            f <= idle && tau.abs() <= idle_moment
        };
        if holds {
            return; // Friction holds it.
        }
        let demand = DVec3::new(f, tau.abs() / lever, 0.0).normalize();
        let along = if f > 0.0 { face.shear / f } else { DVec3::ZERO };
        for (&e, rho) in joints.iter().zip(reach) {
            contact.slips.push((
                self.graph.joints[e],
                Slip {
                    force: along * (demand.x * sign(e)),
                    twist: face.normal * (demand.y * tau.signum() * rho * sign(e)),
                },
            ));
        }
    }
}

/// One contact pass's changes.
#[derive(Default)]
struct Contact {
    /// Faces that lift or tip: their joints separate.
    opened: Vec<Failure>,
    /// Joints that start slipping.
    slips: Vec<(usize, Slip)>,
}

/// What a set of an interface's joints carries, as one face.
struct Face {
    /// Its area-weighted centre.
    centre: DVec3,
    /// Area-weighted unit normal, toward the interface's `a` part.
    normal: DVec3,
    /// N, positive pressing.
    compression: f64,
    /// The force in its plane, N, from its `a` part's side: along that part's slip.
    shear: DVec3,
    /// The moment about its centre in its plane, N·m.
    bend: DVec3,
    /// The moment about its normal through its centre, N·m.
    twist: f64,
    /// Its joints' corners, relative to its centre.
    corners: Vec<DVec3>,
}

/// Whether `point`, projected on the plane normal to `normal`, lies within the convex hull of
/// `corners` projected likewise. Fewer than three distinct corners bound no area.
fn inside(corners: &[DVec3], normal: DVec3, point: DVec3) -> bool {
    let u = normal.any_orthonormal_vector();
    let v = normal.cross(u);
    let flat = |p: DVec3| (p.dot(u), p.dot(v));
    let mut points: Vec<(f64, f64)> = corners.iter().map(|&p| flat(p)).collect();
    points.sort_by(|a, b| a.partial_cmp(b).expect("corners are finite"));
    points.dedup();
    if points.len() < 3 {
        return false;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    // Andrew's monotone chain: a counter-clockwise hull.
    let mut hull: Vec<(f64, f64)> = Vec::new();
    for pass in [false, true] {
        let start = hull.len();
        let ordered: Box<dyn Iterator<Item = &(f64, f64)>> = if pass {
            Box::new(points.iter().rev())
        } else {
            Box::new(points.iter())
        };
        for &p in ordered {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0
            {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    let p = flat(point);
    hull.len() >= 3
        && hull
            .iter()
            .zip(hull.iter().cycle().skip(1))
            .all(|(&a, &b)| cross(a, b, p) >= 0.0)
}
