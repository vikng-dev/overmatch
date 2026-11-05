//! Parts decide what stands, in the game's environment destruction model (game state only).
//!
//! A part ([`Part`]) is the standing chunks of one baked element that intact bonds inside it
//! join, its buried chunks a fixed part of their own: one rigid node with six degrees. An
//! interface ([`Interface`]) is every live joint crossing between two parts, or from a part to
//! the ground; its stiffness and capacity are its joints', so damage inside a part reaches an
//! interface only through the joint area that remains.
//!
//! [`Assessment`] holds one owner's part graph, its sparse Cholesky factor and the loads it last
//! solved. Bonded joints and push-only joints (rests and cracked seams) share one exact linear
//! solve; push-only interfaces are judged once per tick on that trial. An interface whose bonded
//! joints reach their strength cracks whole and bears on as push-only; a push-only interface that
//! lifts, tips or slides separates; connectivity ([`super::loose_groups`]) then releases what lost
//! its support, and the physics engine owns contact between separate bodies. Unchanged loads
//! within [`LOAD_TOLERANCE`] on unchanged topology cost nothing. The construction design's
//! "Parts decide what stands" owns the model.

use bevy::math::{DMat3, DVec3, Vec3};
use std::collections::BTreeMap;

use super::bondsolve::{Edge, Node, Wrench, accelerating_frame, joint_edge, pair_friction};
use super::{Cause, Failure, Load, Side, State, Structure};

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
    pub(crate) fn of(structure: &Structure, state: &State) -> Self {
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
            edges.push(joint_edge(structure, j, a, b, push));
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
    values: Vec<f64>,
}

/// The sparse Cholesky factor of a part graph's stiffness, ordered by exact minimum degree on
/// part blocks; fixed nodes are eliminated. A nonpositive pivot is a bug: anchored or gauged
/// stiffness is positive definite, and no regularization or invented support stands in.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Factor {
    columns: Vec<Column>,
    map: Vec<Option<usize>>,
    /// Scalar operations of its numeric factorization, a DERIVED loop count.
    pub ops: u64,
}

impl Factor {
    fn new(nodes: &[Node], edges: &[Edge]) -> Self {
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
        let mut map = vec![None; nodes.len()];
        while let Some(&a) = live.iter().min_by_key(|&&i| (graph[i].len(), i)) {
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
        let mut matrix = vec![BTreeMap::<usize, f64>::new(); n];
        let mut input = vec![Wrench::default(); nodes.len()];
        for e in edges {
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
                    input[a] = Wrench::default();
                    let column = map[a].expect("a free node is ordered") + axis;
                    for &b in &ends {
                        let base = map[b].expect("a free node is ordered");
                        for (dof, value) in coordinates(at_node(e, nodes, carried, b))
                            .into_iter()
                            .enumerate()
                        {
                            if base + dof >= column {
                                *matrix[column].entry(base + dof).or_default() += value;
                            }
                        }
                    }
                }
            }
        }
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
        let mut work = vec![0.0; n];
        let mut ops = 0;
        for j in 0..n {
            for &i in &columns[j].rows {
                // An entry absent from the sparse assembly is the matrix's mathematical zero.
                work[i] = matrix[j].get(&i).copied().unwrap_or(0.0);
            }
            for &(k, pos) in &prior[j] {
                let c = &columns[k];
                let ljk = c.values[pos];
                for (&i, &lik) in c.rows[pos..].iter().zip(&c.values[pos..]) {
                    work[i] -= ljk * lik;
                    ops += 2;
                }
            }
            assert!(
                work[j].is_finite() && work[j] > 0.0,
                "anchored or gauged part stiffness is positive definite at {j}: {}",
                work[j]
            );
            let diagonal = work[j].sqrt();
            let c = &mut columns[j];
            c.values[0] = diagonal;
            for (&i, value) in c.rows.iter().zip(&mut c.values).skip(1) {
                *value = work[i] / diagonal;
            }
            ops += c.rows.len() as u64;
        }
        Self { columns, map, ops }
    }

    /// Unknown coordinates: six per free part.
    pub(crate) fn unknowns(&self) -> usize {
        self.columns.len()
    }

    /// Stored lower-triangle entries.
    pub(crate) fn entries(&self) -> usize {
        self.columns.iter().map(|c| c.rows.len()).sum()
    }

    /// Scalar operations of one load column's two triangular solves, a DERIVED count.
    pub(crate) fn solve_ops(&self) -> u64 {
        (4 * (self.entries() - self.unknowns()) + 2 * self.unknowns()) as u64
    }

    fn solve(&self, loads: &[Wrench]) -> Vec<Wrench> {
        let mut x = vec![0.0; self.columns.len()];
        for (&base, &load) in self.map.iter().zip(loads) {
            if let Some(base) = base {
                x[base..base + 6].copy_from_slice(&coordinates(load));
            } // A fixed node is buried matter or a free component's gauge.
        }
        for (j, c) in self.columns.iter().enumerate() {
            x[j] /= c.values[0];
            for (&i, &l) in c.rows.iter().zip(&c.values).skip(1) {
                x[i] -= l * x[j];
            }
        }
        for (j, c) in self.columns.iter().enumerate().rev() {
            for (&i, &l) in c.rows.iter().zip(&c.values).skip(1) {
                x[j] -= l * x[i];
            }
            x[j] /= c.values[0];
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

    /// Factor and solve initial gravity once per construction asset; instances share it until
    /// their own first change. Overloads are committed by the authority, never during loading.
    pub(crate) fn at_rest(structure: &Structure) -> Self {
        let state = State::new(structure);
        let mut assessment = Self::default();
        assessment.rebuild(structure, &state);
        assessment.loads = assessment.loads_of(&[], Vec3::ZERO, 1.0);
        assessment.solve();
        assessment.initial_verdict = assessment.judge(structure).0;
        assessment.support = super::loose_groups(structure, &state);
        // A disconnected initial component is released even when nothing loads it.
        assessment.topology_changed = !assessment.support.is_empty();
        assessment
    }

    fn rebuild(&mut self, structure: &Structure, state: &State) {
        self.graph = Graph::of(structure, state);
        self.factor = Factor::new(&self.graph.nodes, &self.graph.edges);
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

    /// One tick: rebuild and refactor on changed topology, solve when a part's loads moved
    /// past [`LOAD_TOLERANCE`], judge every interface once and record its failures. A failure
    /// changes topology for the next tick; no failure starts another solve in its own tick.
    /// Returns the failures and the number of load columns solved.
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
        if topology {
            self.rebuild(structure, state);
        }
        let loads = self.loads_of(sources, spin, step);
        self.impulsive = sources
            .iter()
            .any(|s| s.blow != Vec3::ZERO && self.graph.part[s.chunk].is_some());
        if !topology && !pending && self.within_tolerance(&loads) {
            return (Vec::new(), 0);
        }
        self.loads = loads;
        let columns = self.solve();
        let (failures, separated) = self.judge(structure);
        for failure in &failures {
            state.cracked[failure.joint] = true;
        }
        for &j in &separated {
            state.separated[j] = true;
        }
        self.topology_changed = !failures.is_empty();
        if topology || self.topology_changed {
            self.support = super::loose_groups(structure, state);
        }
        (failures, columns)
    }

    /// Every interface's verdict on the solved field: its failures, and the joints that
    /// separate. A failed bond whose interface the field pulls apart separates in the same tick.
    fn judge(&self, structure: &Structure) -> (Vec<Failure>, Vec<usize>) {
        let n = self.graph.parts.len();
        let (steady, total) = self.answer.split_at(n);
        let mut failures = Vec::new();
        let mut separated = Vec::new();
        for interface in &self.graph.interfaces {
            let (cracks, field) = self.bonded_verdict(structure, interface, steady, total);
            if !cracks.is_empty() {
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
            let moved = self.push_verdict(interface, total);
            separated.extend(moved.iter().map(|f| f.joint));
            failures.extend(moved);
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
            let friction = pair_friction(structure, joint);
            let read = |field: &[Wrench]| {
                let carried = edge.resultant(nodes, field);
                let (tension, shear) = edge.stress(carried, edge.normal);
                let compression = (-carried.linear.dot(edge.normal) / edge.area).max(0.0);
                [
                    (tension, f64::from(seam.tensile)),
                    (
                        shear,
                        f64::from(seam.shear.cohesion) + friction * compression,
                    ),
                ]
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
            normal,
            compression,
            tangential: (force + normal * compression).length(),
            bend: moment - normal * moment.dot(normal),
            corners: edges
                .iter()
                .flat_map(|&e| self.graph.edges[e].corners())
                .map(|p| p - centre)
                .collect(),
        }
    }

    /// A push-only interface (no intact bond left) bears as one face (ASSUMED): the area-weighted
    /// normal, and the hull of its corners projected on it. Its joints separate together when
    /// the face's resultant pulls (lift), its pressure centre leaves the hull (tip) or its
    /// tangential force passes friction on its compression (slide). A face carrying less than
    /// [`IDLE`] is left. While an interface keeps a bond, its bonds decide it and its push-only
    /// joints bear beside them in the linear field (ASSUMED: bilateral until the bond fails).
    fn push_verdict(&self, interface: &Interface, total: &[Wrench]) -> Vec<Failure> {
        if interface.push.is_empty() || !interface.bonded.is_empty() {
            return Vec::new();
        }
        let face = self.face(interface, &interface.push, total);
        let area: f64 = interface
            .push
            .iter()
            .map(|&e| self.graph.edges[e].area)
            .sum();
        let friction: f64 = interface
            .push
            .iter()
            .map(|&e| {
                let edge = &self.graph.edges[e];
                edge.push.expect("a push edge states its friction") * edge.area / area
            })
            .sum();
        let (idle, idle_moment) = (IDLE * self.scale.0, IDLE * self.scale.1);
        let cause = if face.compression < -idle {
            Some(Cause::LiftForce)
        } else if face.compression <= idle {
            // No compression holds it: a shear or a bending moment it carries moves it.
            if face.bend.length() > idle_moment {
                Some(Cause::Tip)
            } else if face.tangential > idle {
                Some(Cause::Slide)
            } else {
                None // It carries nothing.
            }
        } else {
            // DERIVED: a normal force N at offset r about the centre has the in-plane moment
            // r × (−N n), so r = (M⊥ × n) / N.
            let offset = face.bend.cross(face.normal) / face.compression;
            if !inside(&face.corners, face.normal, offset) {
                Some(Cause::Tip)
            } else if face.tangential > friction * face.compression {
                Some(Cause::Slide)
            } else {
                None
            }
        };
        let Some(cause) = cause else {
            return Vec::new(); // It bears.
        };
        interface
            .push
            .iter()
            .map(|&e| Failure {
                joint: self.graph.joints[e],
                cause,
                demand: 0.0,
                capacity: 0.0,
                work: 0.0,
            })
            .collect()
    }
}

/// What a set of an interface's joints carries, as one face.
struct Face {
    /// Area-weighted unit normal, toward the interface's `a` part.
    normal: DVec3,
    /// N, positive pressing.
    compression: f64,
    /// N.
    tangential: f64,
    /// The moment about the face's centre in its plane, N·m.
    bend: DVec3,
    /// Its joints' corners, relative to its area-weighted centre.
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
