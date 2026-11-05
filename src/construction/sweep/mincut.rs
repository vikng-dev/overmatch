//! Directed minimum cut for the game's environment destruction model, game state only.
//! Dinic's level graph and blocking flow (1970), with residual reachability selecting the
//! source side: <https://www.mathnet.ru/eng/dan35701>. Capacities are forces in N.

use std::collections::VecDeque;

/// Operation counts, not elapsed time. Scans include residual arcs in level and path searches.
#[derive(Default, Debug, Clone, Copy)]
pub(crate) struct Work {
    pub(crate) nodes: usize,
    pub(crate) edges: usize,
    pub(crate) phases: usize,
    pub(crate) scans: usize,
    pub(crate) augmentations: usize,
}

impl std::ops::AddAssign for Work {
    fn add_assign(&mut self, rhs: Self) {
        self.nodes += rhs.nodes;
        self.edges += rhs.edges;
        self.phases += rhs.phases;
        self.scans += rhs.scans;
        self.augmentations += rhs.augmentations;
    }
}

struct Arc {
    to: usize,
    reverse: usize,
    residual: f64,
}

pub(super) struct Network {
    arcs: Vec<Vec<Arc>>,
    work: Work,
}

impl Network {
    pub(super) fn new(nodes: usize) -> Self {
        Self {
            arcs: (0..nodes).map(|_| Vec::new()).collect(),
            work: Work {
                nodes,
                ..Work::default()
            },
        }
    }

    /// Opposite directed capacities share a residual pair; either may be zero. Infinite arcs
    /// impose source membership or an anchor. There must be a finite source-to-sink cut.
    pub(super) fn edge(&mut self, a: usize, b: usize, forward: f64, backward: f64) {
        assert!(
            a != b && forward >= 0.0 && backward >= 0.0,
            "distinct endpoints and nonnegative capacities"
        );
        let (ar, br) = (self.arcs[a].len(), self.arcs[b].len());
        self.arcs[a].push(Arc {
            to: b,
            reverse: br,
            residual: forward,
        });
        self.arcs[b].push(Arc {
            to: a,
            reverse: ar,
            residual: backward,
        });
        self.work.edges += 1;
    }

    fn levels(&mut self, source: usize) -> Vec<usize> {
        let mut level = vec![usize::MAX; self.arcs.len()];
        let mut queue = VecDeque::from([source]);
        level[source] = 0;
        while let Some(a) = queue.pop_front() {
            for arc in &self.arcs[a] {
                self.work.scans += 1;
                if arc.residual > 0.0 && level[arc.to] == usize::MAX {
                    level[arc.to] = level[a] + 1;
                    queue.push_back(arc.to);
                }
            }
        }
        level
    }

    pub(super) fn cut(mut self, source: usize, sink: usize) -> (Vec<bool>, Work) {
        loop {
            let level = self.levels(source);
            if level[sink] == usize::MAX {
                return (
                    level.into_iter().map(|l| l != usize::MAX).collect(),
                    self.work,
                );
            }
            self.work.phases += 1;
            let mut cursor = vec![0; self.arcs.len()];
            // Explicit path storage bounds stack use even on a construction-length chain.
            let mut path = vec![source];
            while let Some(&a) = path.last() {
                if a == sink {
                    let flow = path[..path.len() - 1]
                        .iter()
                        .map(|&node| self.arcs[node][cursor[node]].residual)
                        .fold(f64::INFINITY, f64::min);
                    assert!(
                        flow.is_finite() && flow > 0.0,
                        "the construction has a finite cut"
                    );
                    for &node in &path[..path.len() - 1] {
                        let arc = &mut self.arcs[node][cursor[node]];
                        arc.residual -= flow;
                        let (to, reverse) = (arc.to, arc.reverse);
                        self.arcs[to][reverse].residual += flow;
                    }
                    self.work.augmentations += 1;
                    path.truncate(1);
                    continue;
                }
                while cursor[a] < self.arcs[a].len() {
                    self.work.scans += 1;
                    let arc = &self.arcs[a][cursor[a]];
                    if arc.residual > 0.0 && level[arc.to] == level[a] + 1 {
                        break;
                    }
                    cursor[a] += 1;
                }
                if cursor[a] < self.arcs[a].len() {
                    path.push(self.arcs[a][cursor[a]].to);
                } else {
                    path.pop();
                    if let Some(&parent) = path.last() {
                        cursor[parent] += 1;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's environment destruction: a globally cheapest plug can require cancelling
    /// earlier flow. A path-only greedy cut returns 2 N here; source capacities bound the
    /// DERIVED optimum at 1.5 N, reached by routing through the reverse middle arc.
    #[test]
    fn a_blocking_flow_can_be_rerouted_through_a_reverse_arc() {
        // Fixture: source 0, sink 5; insertion order sends the first unit through 1 -> 3.
        let edges = [
            (0, 1, 1.0),
            (0, 2, 0.5),
            (1, 3, 1.0),
            (1, 4, 1.0),
            (2, 3, 0.5),
            (3, 5, 1.0),
            (4, 5, 1.0),
        ];
        let mut graph = Network::new(6);
        for &(a, b, capacity) in &edges {
            graph.edge(a, b, capacity, 0.0);
        }
        let (inside, _) = graph.cut(0, 5);
        let capacity: f64 = edges
            .iter()
            .filter(|&&(a, b, _)| inside[a] && !inside[b])
            .map(|&(_, _, capacity)| capacity)
            .sum();
        assert_eq!(capacity, 1.5);
    }

    /// The game's environment destruction: asymmetric joint capacities and residual cancellation
    /// must select the globally cheapest plug. Exhaustive subsets independently certify small
    /// cyclic graphs, including zero-capacity arcs; a greedy expansion is insufficient.
    #[test]
    fn directed_cuts_match_exhaustive_subsets() {
        // Fixture: four chunk nodes between a forced patch and fixed surroundings.
        for seed in 0..64 {
            let mut graph = Network::new(6);
            graph.edge(4, 0, f64::INFINITY, 0.0);
            graph.edge(3, 5, f64::INFINITY, 0.0);
            let mut edges = Vec::new();
            for a in 0..4 {
                for b in a + 1..4 {
                    let forward = ((seed + a * 7 + b * 3) % 11) as f64;
                    let backward = ((seed * 3 + a * 5 + b) % 13) as f64;
                    graph.edge(a, b, forward, backward);
                    edges.push((a, b, forward, backward));
                }
            }
            let capacity = |inside: &[bool]| {
                edges
                    .iter()
                    .map(|&(a, b, f, r)| match (inside[a], inside[b]) {
                        (true, false) => f,
                        (false, true) => r,
                        _ => 0.0,
                    })
                    .sum::<f64>()
            };
            let expected = (0..4)
                .map(|bits| capacity(&[true, bits & 1 != 0, bits & 2 != 0, false]))
                .fold(f64::INFINITY, f64::min);
            let (inside, _) = graph.cut(4, 5);
            assert!(inside[0] && !inside[3]);
            assert_eq!(capacity(&inside), expected, "seed={seed}");
        }
    }
}
