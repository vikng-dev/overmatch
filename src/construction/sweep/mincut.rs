//! Directed minimum cut for the game's environment destruction model, game state only.
//! Goldberg–Tarjan push-relabel with global distance updates; capacities are forces in N.
//! <https://doi.org/10.1145/48014.61051>. Design "Bodies in contact" owns the work bound.

use std::collections::VecDeque;

/// Operation counts, not elapsed time. Scans include residual arcs in all searches and relabels.
#[derive(Default, Debug, Clone, Copy)]
pub(crate) struct Work {
    pub(crate) nodes: usize,
    pub(crate) edges: usize,
    pub(crate) global_relabels: usize,
    pub(crate) relabels: usize,
    pub(crate) scans: usize,
    pub(crate) pushes: usize,
}

impl std::ops::AddAssign for Work {
    fn add_assign(&mut self, rhs: Self) {
        self.nodes += rhs.nodes;
        self.edges += rhs.edges;
        self.global_relabels += rhs.global_relabels;
        self.relabels += rhs.relabels;
        self.scans += rhs.scans;
        self.pushes += rhs.pushes;
    }
}

struct Arc {
    to: usize,
    reverse: usize,
    residual: f64,
}

/// An exact releasing cut, or a certificate that the demanded flow can be carried.
#[derive(Debug)]
pub(super) enum Cut {
    Held,
    Released { inside: Vec<bool>, capacity: f64 },
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

    fn reachable(&mut self, source: usize) -> Vec<bool> {
        let mut seen = vec![false; self.arcs.len()];
        let mut queue = VecDeque::from([source]);
        seen[source] = true;
        while let Some(a) = queue.pop_front() {
            for arc in &self.arcs[a] {
                self.work.scans += 1;
                if arc.residual > 0.0 && !seen[arc.to] {
                    seen[arc.to] = true;
                    queue.push_back(arc.to);
                }
            }
        }
        seen
    }

    /// Exact reverse-residual distances to the sink. Unreachable vertices return surplus
    /// to the synthetic source; labels never decrease, preserving push-relabel's progress bound.
    fn global_relabel(
        &mut self,
        source: usize,
        sink: usize,
        height: &mut [usize],
        cursor: &mut [usize],
    ) {
        let n = self.arcs.len();
        let mut distance = vec![n + 1; n];
        let mut queue = VecDeque::from([sink]);
        distance[sink] = 0;
        while let Some(a) = queue.pop_front() {
            for arc in &self.arcs[a] {
                self.work.scans += 1;
                if arc.to != source
                    && distance[arc.to] == n + 1
                    && self.arcs[arc.to][arc.reverse].residual > 0.0
                {
                    distance[arc.to] = distance[a] + 1;
                    queue.push_back(arc.to);
                }
            }
        }
        for (h, d) in height.iter_mut().zip(distance) {
            *h = (*h).max(d);
        }
        height[source] = n;
        cursor.fill(0);
        self.work.global_relabels += 1;
    }

    fn push(&mut self, a: usize, index: usize, flow: f64) -> usize {
        let arc = &mut self.arcs[a][index];
        arc.residual -= flow;
        let (to, reverse) = (arc.to, arc.reverse);
        self.arcs[to][reverse].residual += flow;
        self.work.pushes += 1;
        to
    }

    pub(super) fn cut(mut self, source: usize, sink: usize, demand: f64) -> (Cut, Work) {
        assert!(
            source != sink && demand >= 0.0,
            "distinct endpoints and nonnegative demand"
        );
        if demand == 0.0 {
            return (Cut::Held, self.work);
        }
        // A finite supply avoids infinite preflows on forced-membership arcs. For an exact-cut
        // diagnostic, the sum of finite directed capacities bounds every finite cut from above.
        let supply = if demand.is_finite() {
            demand
        } else {
            let mut bound = 0.0_f64;
            for arcs in &self.arcs {
                for arc in arcs {
                    self.work.scans += 1;
                    if arc.residual.is_finite() && arc.residual > 0.0 {
                        // Resolution: outward rounding keeps the inlet above the true sum,
                        // even when a small parallel capacity is below a large one's ULP.
                        bound = (bound + arc.residual).next_up();
                    }
                }
            }
            bound
        };
        assert!(
            supply.is_finite(),
            "an admitted construction has a finite capacity sum"
        );
        let inlet = self.arcs.len();
        self.arcs.push(Vec::new());
        self.work.nodes += 1;
        self.edge(inlet, source, supply, 0.0);
        let n = self.arcs.len();
        let mut excess = vec![0.0; n];
        let mut height = vec![0; n];
        let mut cursor = vec![0; n];
        let mut active = VecDeque::new();
        if supply > 0.0 {
            self.push(inlet, 0, supply);
            excess[source] = supply;
            active.push_back(source);
        }
        self.global_relabel(inlet, sink, &mut height, &mut cursor);
        // Resolution: refresh after one graph-sized amount of discharge work, amortising
        // distance updates against local scans. Two residual arcs belong to each edge pair.
        let interval = n + 2 * self.work.edges;
        let mut refresh = self.work.scans + interval;
        while let Some(a) = active.pop_front() {
            while excess[a] > 0.0 {
                if cursor[a] == self.arcs[a].len() {
                    let mut next = usize::MAX;
                    for arc in &self.arcs[a] {
                        self.work.scans += 1;
                        if arc.residual > 0.0 {
                            next = next.min(height[arc.to] + 1);
                        }
                    }
                    assert!(
                        next != usize::MAX && next > height[a],
                        "an active vertex has a reverse flow arc and strictly relabels"
                    );
                    height[a] = next;
                    cursor[a] = 0;
                    self.work.relabels += 1;
                    continue;
                }
                self.work.scans += 1;
                let arc = &self.arcs[a][cursor[a]];
                if arc.residual > 0.0 && height[a] == height[arc.to] + 1 {
                    let flow = excess[a].min(arc.residual);
                    let to = self.push(a, cursor[a], flow);
                    if excess[to] == 0.0 && to != inlet && to != sink {
                        active.push_back(to);
                    }
                    excess[a] -= flow;
                    excess[to] += flow;
                    // Sink inflow in a preflow certifies a feasible flow of at least that value:
                    // all remaining surplus can return to the inlet along reverse flow arcs.
                    if excess[sink] >= demand {
                        return (Cut::Held, self.work);
                    }
                } else {
                    cursor[a] += 1;
                }
            }
            if self.work.scans >= refresh {
                self.global_relabel(inlet, sink, &mut height, &mut cursor);
                refresh = self.work.scans + interval;
            }
        }
        // Discharging surplus back to the inlet leaves a feasible maximum flow. Forward
        // reachability selects the smallest source-side minimum cut, excluding unrelated matter.
        let mut inside = self.reachable(source);
        assert!(
            !inside[sink],
            "exhausted preflow reaches an exact finite construction cut"
        );
        inside.truncate(inlet);
        (
            Cut::Released {
                inside,
                capacity: excess[sink],
            },
            self.work,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's environment destruction: a globally cheapest plug can require cancelling
    /// earlier flow. A path-only greedy cut returns 2 N here; source capacities bound the
    /// DERIVED optimum at 1.5 N, reached by routing through the reverse middle arc.
    #[test]
    fn residual_flow_can_be_rerouted_through_a_reverse_arc() {
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
        let (
            Cut::Released {
                inside,
                capacity: flow,
            },
            _,
        ) = graph.cut(0, 5, f64::INFINITY)
        else {
            panic!("an unlimited demand exhausts this finite cut");
        };
        let capacity: f64 = edges
            .iter()
            .filter(|&&(a, b, _)| inside[a] && !inside[b])
            .map(|&(_, _, capacity)| capacity)
            .sum();
        assert_eq!(capacity, 1.5);
        assert_eq!(flow, capacity);
    }

    /// The game's environment destruction: asymmetric joint capacities and residual cancellation
    /// must select the globally cheapest plug. Exhaustive subsets independently certify small
    /// cyclic graphs, including zero-capacity arcs; a greedy expansion is insufficient.
    #[test]
    fn directed_cuts_match_exhaustive_subsets() {
        // Fixture: four chunk nodes between a forced patch and fixed surroundings.
        for seed in 0..64 {
            let mut edges = Vec::new();
            for a in 0..4 {
                for b in a + 1..4 {
                    let forward = ((seed + a * 7 + b * 3) % 11) as f64;
                    let backward = ((seed * 3 + a * 5 + b) % 13) as f64;
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
            // Fixture demand margin: quarter-unit excess, with equality and an exact-cut probe.
            for demand in [0.0, expected, expected + 0.25, f64::INFINITY] {
                let mut graph = Network::new(6);
                graph.edge(4, 0, f64::INFINITY, 0.0);
                graph.edge(3, 5, f64::INFINITY, 0.0);
                for &(a, b, forward, backward) in &edges {
                    graph.edge(a, b, forward, backward);
                }
                match graph.cut(4, 5, demand).0 {
                    Cut::Held => assert!(demand <= expected, "seed={seed}, demand={demand}"),
                    Cut::Released {
                        inside,
                        capacity: flow,
                    } => {
                        assert!(demand > expected, "seed={seed}, demand={demand}");
                        assert!(inside[0] && !inside[3]);
                        assert_eq!(capacity(&inside), expected, "seed={seed}");
                        assert_eq!(flow, expected, "seed={seed}");
                    }
                }
            }
        }
    }

    /// The game's environment destruction: a minimum-capacity tie must not add an unrelated
    /// component to the occupied plug. Hole selection relies on the smallest source side.
    #[test]
    fn an_unrelated_component_is_not_added_to_a_releasing_plug() {
        // Fixture: one occupied chunk with unit support and one disconnected, untouched chunk.
        let mut graph = Network::new(4);
        graph.edge(0, 1, f64::INFINITY, 0.0);
        graph.edge(1, 3, 1.0, 0.0);
        let Cut::Released { inside, capacity } = graph.cut(0, 3, 2.0).0 else {
            panic!("demand exceeds the unit support");
        };
        assert_eq!(capacity, 1.0);
        assert_eq!(inside, [true, true, false, false]);
    }

    /// The game's environment destruction: an unlimited diagnostic must exhaust every finite
    /// support even when a small parallel capacity disappears in a rounded inlet sum.
    #[test]
    fn an_unlimited_demand_cannot_be_limited_by_a_rounded_capacity_sum() {
        // Fixture: parallel capacities differing by more than f64's significand width.
        let large = 2.0_f64.powi(54);
        assert_eq!(large + 1.0, large);
        let mut graph = Network::new(2);
        graph.edge(0, 1, large, 0.0);
        graph.edge(0, 1, 1.0, 0.0);
        let Cut::Released { inside, capacity } = graph.cut(0, 1, f64::INFINITY).0 else {
            panic!("the diagnostic exhausts a finite cut");
        };
        assert_eq!(inside, [true, false]);
        // DERIVED: the cut sum rounds to the large capacity at f64 storage precision.
        assert_eq!(capacity, large + 1.0);
    }

    /// The game's environment destruction: a light press needs only a holding certificate,
    /// not the full capacity of a compression chain with successively farther ground supports.
    #[test]
    fn a_light_press_stops_at_its_first_holding_certificate() {
        // Fixture: 128 exposed chunks, each grounded at 1 N, with 1000 N bearing between them.
        let chunks = 128;
        let (source, sink) = (chunks, chunks + 1);
        let mut graph = Network::new(chunks + 2);
        for c in 0..chunks {
            graph.edge(c, sink, 1.0, 0.0);
            if c + 1 < chunks {
                graph.edge(c, c + 1, 1000.0, 0.0);
            }
        }
        graph.edge(source, 0, f64::INFINITY, 0.0);
        let (cut, work) = graph.cut(source, sink, 0.25);
        assert!(matches!(cut, Cut::Held));
        // DERIVED: one reverse-distance pass and the direct two-arc path suffice; the
        // allowance includes the synthetic inlet. Far supports need no individual routing.
        assert!(work.scans <= 2 * (work.nodes + 2 * work.edges), "{work:?}");
        assert_eq!(work.global_relabels, 1);
        assert!(work.pushes <= 3);
        println!("MEASURED light-press flow work: {work:?}");
    }
    /// The game's environment destruction: exhausting a compression chain must not repeat
    /// a whole-graph search for each unit ground capacity. Large impacts rely on this bound.
    #[test]
    fn a_large_demand_exhausts_a_grounded_chain_with_linear_visits() {
        // Fixture: unit ground capacities, bearing above their sum, and demand above the cut.
        for chunks in [512, 4096, 32768] {
            let (source, sink) = (chunks, chunks + 1);
            let mut graph = Network::new(chunks + 2);
            for c in 0..chunks {
                graph.edge(c, sink, 1.0, 0.0);
                if c + 1 < chunks {
                    graph.edge(c, c + 1, (chunks + 2) as f64, 0.0);
                }
            }
            graph.edge(source, 0, f64::INFINITY, 0.0);
            let (cut, work) = graph.cut(source, sink, (chunks + 1) as f64);
            let Cut::Released { inside, capacity } = cut else {
                panic!("demand exceeds the sum of all ground supports");
            };
            assert_eq!(capacity, chunks as f64);
            assert!(inside[..chunks].iter().all(|&v| v));
            println!("MEASURED large-demand chain N={chunks}: {work:?}");
            // DERIVED: forward routing, surplus return and graph-sized distance refreshes
            // each take linear visits on this chain. The allowance covers their arc rescans;
            // successive full searches for every support exceed it already at the first size.
            assert!(work.scans <= 64 * (work.nodes + work.edges), "{work:?}");
        }
    }

    /// The game's environment destruction: demand equal to capacity holds; a larger demand
    /// must return the exact cut, so early termination cannot manufacture a releasing plug.
    #[test]
    fn equality_holds_and_excess_demand_returns_the_exact_cut() {
        // Fixture: one joint with unit capacity in either direction.
        for demand in [0.0, 1.0, 2.0] {
            let mut graph = Network::new(2);
            graph.edge(0, 1, 1.0, 1.0);
            match graph.cut(0, 1, demand).0 {
                Cut::Held => assert!(demand <= 1.0),
                Cut::Released { inside, capacity } => {
                    assert_eq!(demand, 2.0);
                    assert_eq!(inside, [true, false]);
                    assert_eq!(capacity, 1.0);
                }
            }
        }
    }
}
