//! The chunk-scale bonded solve inside one part, in the game's environment destruction model
//! (game state only).
//!
//! [`Kernel`] takes a part's standing chunks as nodes. Its intact bonds are edges at their seam
//! stiffness; its cracked seams and rests are push-only edges at bulk stiffness that stick
//! (ASSUMED: once cracks split the part, the part law judges the pieces' sliding with Coulomb
//! friction). Joints to the ground or to buried chunks are its anchors. Joints to chunks outside
//! its set are no edge: their force is the caller's load. A node set nothing anchors is balanced
//! by its rigid acceleration and gauged at its first node. It is factored once per element design
//! at load ([`super::Assessment::kernels`]) and again only for a part whose chunks or bonds
//! changed; [`Kernel::hold`] is its one event entry. The construction design's "Inside a standing
//! part" owns the model.

use bevy::math::{DMat3, DVec3};

use super::super::bondsolve::{Edge, Node, Wrench, accelerating_frame, joint_edge, pair_friction};
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
    pub(crate) factor: Factor,
}

/// One event's verdict at chunk scale.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Hold {
    /// Bonds past their strength crack (`Tension`, `Shear`); push-only joints the field pulls
    /// apart separate (`LiftForce`), and so does a bond that cracks while pulled.
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
        let mut node = vec![None; structure.chunks.len()];
        for (i, &c) in chunks.iter().enumerate() {
            node[c] = Some(i);
        }
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
        let mut anchored: Vec<bool> = nodes.iter().map(|n| n.fixed).collect();
        for &j in &state.joints {
            let joint = &structure.joints[j];
            if state.separated[j] || !state.live(joint) {
                continue; // Open or gone: it carries nothing.
            }
            let push = state.cracked[j] || joint.seam().is_none();
            let far = match joint.b {
                Side::Chunk(b) => Some(b),
                Side::Ground => None,
            };
            let edge = match (node[joint.a], far.map(|b| (b, node[b]))) {
                (Some(a), Some((_, Some(b)))) => Some(joint_edge(structure, j, a, Some(b), push)),
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
                if edge.b.is_none() {
                    anchored[edge.a] = true;
                }
                edges.push(edge);
                joints.push(j);
            }
        }
        let mut neighbours = vec![Vec::new(); nodes.len()];
        for e in &edges {
            if let Some(b) = e.b {
                neighbours[e.a].push(b);
                neighbours[b].push(e.a);
            } // An anchor joins no node.
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
        let factor = Factor::new(&nodes, &edges)
            .expect("an anchored or gauged chunk set with no slipping face is positive definite");
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
            factor,
        }
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
                Some(seam) => {
                    let (tension, shear) = edge.stress(carried, edge.normal);
                    let compression = (-pulled / edge.area).max(0.0);
                    let holds = f64::from(seam.shear.cohesion)
                        + pair_friction(structure, joint) * compression;
                    if tension > f64::from(seam.tensile) {
                        Some((Cause::Tension, tension, f64::from(seam.tensile)))
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
