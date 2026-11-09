//! The chunk-scale bonded solve inside one part, in the game's environment destruction model
//! (game state only).
//!
//! [`Kernel`] takes a part's standing chunks as nodes. Its intact bonds are edges at their seam
//! stiffness; its cracked seams and rests are push-only edges at bulk stiffness that stick
//! (ASSUMED: once cracks split the part, the part law judges the pieces' sliding with Coulomb
//! friction). Joints to the ground or to buried chunks are its anchors. Joints to chunks outside
//! its set are no edge: their force is the caller's load. A node set nothing anchors is balanced
//! by its rigid acceleration and gauged at its first node. A heavily cracked set can be a
//! mechanism, its stiffness leaving some chunks' motion free within the factor's resolution: the
//! joints that motion moves part (`Slide`), and the rest is factored again. It is factored once per element
//! design at load ([`super::Assessment::kernels`]) and again only for a part whose chunks or bonds
//! changed; [`Kernel::hold`] is its one event entry. The construction design's "Inside a standing
//! part" owns the model.

use bevy::math::{DMat3, DVec3};

use super::super::bondsolve::{Edge, Node, Wrench, accelerating_frame, joint_edge};
use super::super::{Cause, Failure, Side, State, Structure};
use super::{Factor, IDLE};

/// One part's chunk graph and its factor.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Kernel {
    /// Per node: its chunk.
    pub chunks: Vec<usize>,
    nodes: Vec<Node>,
    masses: Vec<f64>,
    inertia: Vec<DMat3>,
    edges: Vec<Edge>,
    /// Per edge: its joint.
    joints: Vec<usize>,
    /// Unanchored node sets, each gauged at its first node.
    free: Vec<Vec<usize>>,
    /// The joints a mechanism's free motion moved, ascending: they part, the rest is factored
    /// without them, and every event reports them parted ([`Self::hold`]).
    loose: Vec<usize>,
    pub(crate) factor: Factor,
    #[cfg(test)]
    pub(super) joint_visits: usize,
}

/// One event's verdict at chunk scale.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Hold {
    /// Bonds past their strength crack (`Tension`, `Shear`); push-only joints the field pulls
    /// apart separate (`LiftForce`), and so does a bond that cracks while pulled; a loose chunk's
    /// joints separate (`Slide`).
    pub failures: Vec<Failure>,
    /// The joints that separate.
    pub separated: Vec<usize>,
    /// Per edge joint: the wrench it carries from its chunk `a`'s side, about its centre.
    pub carried: Vec<(usize, Wrench)>,
    /// Scalar operations of its solve, a DERIVED count.
    pub work: u64,
}

impl Kernel {
    /// The chunk graph of `chunks`, standing chunks of one owner, under `state`.
    pub(crate) fn of(structure: &Structure, state: &State, chunks: &[usize]) -> Self {
        let node: std::collections::HashMap<_, _> =
            chunks.iter().enumerate().map(|(i, &c)| (c, i)).collect();
        let mut nodes: Vec<Node> = chunks
            .iter()
            .map(|&c| Node {
                centre: structure.chunks[c].centroid.as_dvec3(),
                fixed: structure.chunks[c].buried,
            })
            .collect();
        let soil = |c: usize| structure.chunks[c].buried;
        let mut edges = Vec::new();
        let mut joints = Vec::new();
        #[cfg(test)]
        let mut joint_visits = 0;
        for &c in chunks {
            for &j in &state.topology.incident[c] {
                #[cfg(test)]
                {
                    joint_visits += 1;
                }
                let joint = &structure.joints[j];
                if joint.a != c && node.contains_key(&joint.a) {
                    continue; // An internal joint is assembled once, from its a endpoint.
                }
                if state.separated[j] || !state.live(joint) {
                    continue; // Open or gone: it carries nothing.
                }
                let push = state.cracked[j] || joint.seam().is_none();
                let far = match joint.b {
                    Side::Chunk(b) => Some(b),
                    Side::Ground => None,
                };
                let edge = match (
                    node.get(&joint.a).copied(),
                    far.map(|b| (b, node.get(&b).copied())),
                ) {
                    (Some(a), Some((_, Some(b)))) => {
                        Some(joint_edge(structure, j, a, Some(b), push))
                    }
                    (Some(a), None) => Some(joint_edge(structure, j, a, None, push)),
                    (Some(a), Some((b, None))) => {
                        soil(b).then(|| joint_edge(structure, j, a, None, push))
                    }
                    (None, Some((_, Some(b)))) if soil(joint.a) => {
                        // The soil side is the joint's `a`: the edge reads from the set's chunk.
                        let mut edge = joint_edge(structure, j, b, None, push);
                        edge.normal = -edge.normal;
                        Some(edge)
                    }
                    _ => None, // A boundary to a chunk outside the set: the caller's load.
                };
                if let Some(edge) = edge {
                    edges.push(edge);
                    joints.push(j);
                }
            }
        }
        let buried: Vec<bool> = nodes.iter().map(|n| n.fixed).collect();
        let mut loose = Vec::new();
        let (factor, free) = loop {
            let mut anchored = buried.clone();
            let mut neighbours = vec![Vec::new(); nodes.len()];
            for e in &edges {
                match e.b {
                    Some(b) => {
                        neighbours[e.a].push(b);
                        neighbours[b].push(e.a);
                    }
                    None => anchored[e.a] = true,
                }
            }
            for (node, &fixed) in nodes.iter_mut().zip(&buried) {
                node.fixed = fixed;
            }
            let mut free = Vec::new();
            let mut seen = vec![false; nodes.len()];
            for start in 0..nodes.len() {
                if seen[start] {
                    continue; // Already in a set.
                }
                seen[start] = true;
                let (mut stack, mut group, mut held) = (vec![start], Vec::new(), false);
                while let Some(n) = stack.pop() {
                    held |= anchored[n];
                    group.push(n);
                    for &m in &neighbours[n] {
                        if !seen[m] {
                            seen[m] = true;
                            stack.push(m);
                        }
                    }
                }
                if !held {
                    group.sort_unstable();
                    nodes[group[0]].fixed = true;
                    free.push(group);
                }
            }
            match Factor::new(&nodes, &edges) {
                Ok(factor) => break (factor, free),
                Err(mechanism) => {
                    // Loose chunks: the joints their free motion moves part, and the rest is
                    // factored again. Each pass parts at least one joint, so it ends.
                    for e in mechanism.carriers(&nodes, &edges).into_iter().rev() {
                        edges.remove(e);
                        loose.push(joints.remove(e));
                    }
                }
            }
        };
        loose.sort_unstable();
        Self {
            masses: chunks
                .iter()
                .map(|&c| f64::from(structure.chunks[c].mass))
                .collect(),
            inertia: chunks
                .iter()
                .map(|&c| {
                    DMat3::from_diagonal(DVec3::splat(structure.chunks[c].intrinsic_inertia()))
                })
                .collect(),
            chunks: chunks.to_vec(),
            nodes,
            edges,
            joints,
            free,
            loose,
            factor,
            #[cfg(test)]
            joint_visits,
        }
    }

    /// Whether this kernel still states `chunks`' stiffness under `state`: the same chunks, every
    /// joint it reads still bearing as it was factored, and none it parted for being loose.
    /// Joints only ever leave or crack, so no new edge can have appeared.
    pub(crate) fn current(&self, structure: &Structure, state: &State, chunks: &[usize]) -> bool {
        self.chunks == chunks
            && self.loose.is_empty()
            && self.edges.iter().zip(&self.joints).all(|(edge, &j)| {
                let joint = &structure.joints[j];
                !state.separated[j]
                    && state.live(joint)
                    && edge.push.is_some() == (state.cracked[j] || joint.seam().is_none())
            })
    }

    #[cfg(test)]
    pub(super) fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// One event: the field of `loads` (per node, about its chunk's centroid: the forces of the
    /// joints to chunks outside the set, contacts and presses) with each chunk's weight added on
    /// anchored sets, and every joint's verdict on it. An unanchored set's rigid acceleration is
    /// removed, so uniform gravity cancels from it. A bond cracks past its tensile strength, or
    /// past its cohesion plus friction on its compression; a push-only joint the field pulls
    /// apart separates.
    pub(crate) fn hold(&self, structure: &Structure, loads: &[Wrench]) -> Hold {
        assert_eq!(loads.len(), self.nodes.len(), "one load per node");
        let mut loads = loads.to_vec();
        let g = f64::from(crate::physical_laws::gravity_mps2());
        let mut free = vec![false; self.nodes.len()];
        for group in &self.free {
            let nodes: Vec<_> = group.iter().map(|&n| self.nodes[n]).collect();
            let masses: Vec<_> = group.iter().map(|&n| self.masses[n]).collect();
            let own: Vec<_> = group.iter().map(|&n| self.inertia[n]).collect();
            let mut framed: Vec<_> = group.iter().map(|&n| loads[n]).collect();
            accelerating_frame(&nodes, &masses, &own, DVec3::ZERO, &mut framed);
            for (&n, load) in group.iter().zip(framed) {
                loads[n] = load;
                free[n] = true;
            }
        }
        for ((load, &mass), free) in loads.iter_mut().zip(&self.masses).zip(free) {
            if !free {
                load.linear -= DVec3::Y * (g * mass);
            } // Uniform gravity cancels from an unanchored set's internal demand.
        }
        let field = if self.factor.unknowns() == 0 {
            vec![Wrench::default(); self.nodes.len()]
        } else {
            self.factor.solve(&loads)
        };
        let idle = IDLE * g * self.masses.iter().sum::<f64>();
        let mut hold = Hold {
            work: if self.factor.unknowns() == 0 {
                0
            } else {
                self.factor.solve_ops()
            },
            // A loose chunk is a mechanism, as a part nothing holds is in the part law: it parts.
            failures: self
                .loose
                .iter()
                .map(|&joint| Failure {
                    joint,
                    cause: Cause::Slide,
                    demand: 0.0,
                    capacity: 0.0,
                    work: 0.0,
                })
                .collect(),
            separated: self.loose.clone(),
            ..Hold::default()
        };
        for (edge, &j) in self.edges.iter().zip(&self.joints) {
            let joint = &structure.joints[j];
            let carried = edge.resultant(&self.nodes, &field);
            hold.carried.push((j, carried));
            let pulled = carried.linear.dot(edge.normal);
            // Per verdict: its cause, demand and capacity (Pa for a bond).
            let verdict = match joint.seam().filter(|_| edge.push.is_none()) {
                None => (pulled > idle).then_some((Cause::LiftForce, 0.0, 0.0)),
                Some(_) => {
                    let (tension, shear) = edge.stress(carried, edge.normal);
                    let compression = (-pulled / edge.area).max(0.0);
                    let [tensile, holds] =
                        super::super::bondsolve::JointStrength::of(structure, joint, false)
                            .limits(compression);
                    if tension > tensile {
                        Some((Cause::Tension, tension, tensile))
                    } else {
                        (shear > holds).then_some((Cause::Shear, shear, holds))
                    }
                }
            };
            if let Some((cause, demand, capacity)) = verdict {
                if cause == Cause::LiftForce || pulled > idle {
                    hold.separated.push(j);
                }
                hold.failures.push(Failure {
                    joint: j,
                    cause,
                    demand: demand as f32,
                    capacity: capacity as f32,
                    work: 0.0,
                });
            }
        }
        hold
    }
}
