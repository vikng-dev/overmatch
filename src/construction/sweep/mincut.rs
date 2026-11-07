//! Directed minimum cut for the game's environment destruction model, game state only.
//! Goldberg–Tarjan push-relabel with global distance updates; capacities are forces in N.
//! <https://doi.org/10.1145/48014.61051>. Design "Bodies in contact" owns the work bound.

use std::collections::VecDeque;

/// Operation counts, not elapsed time. Scans include capacity preparation and residual visits.
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
    capacity: f64,
    residual: u128,
}

/// A releasing cut within the capacity-grid resolution, or a conservative holding certificate.
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
            capacity: forward,
            residual: 0,
        });
        self.arcs[b].push(Arc {
            to: a,
            reverse: ar,
            capacity: backward,
            residual: 0,
        });
        self.work.edges += 1;
    }

    /// Resolution: a 120-bit total leaves room in u128 for reverse capacities and supply.
    /// Power-of-two scaling is exact in f64; truncation only lowers a finite capacity.
    /// Design "Bodies in contact" defines the resulting force tolerance.
    fn quantum(&self) -> f64 {
        const CAPACITY_BITS: i32 = 120;
        let (count, maximum) = self
            .arcs
            .iter()
            .flatten()
            .map(|arc| arc.capacity)
            .filter(|c| c.is_finite() && *c > 0.0)
            .fold((0_usize, 0.0_f64), |(count, maximum), c| {
                (count + 1, maximum.max(c))
            });
        if count == 0 {
            return f64::from_bits(1);
        }
        let bits = maximum.to_bits();
        let exponent = if maximum.is_normal() {
            (bits >> 52) as i32 - 1023
        } else {
            63 - bits.leading_zeros() as i32 - 1074
        };
        // DERIVED: each capacity is below 2^(exponent+1); rounding count upward to a
        // power of two bounds their integer sum below 2^CAPACITY_BITS.
        let exponent =
            (exponent + 1 + count.next_power_of_two().ilog2() as i32 - CAPACITY_BITS).max(-1074);
        if exponent >= -1022 {
            f64::from_bits(((exponent + 1023) as u64) << 52)
        } else {
            f64::from_bits(1 << (exponent + 1074))
        }
    }

    fn reachable(&mut self, source: usize) -> Vec<bool> {
        let mut seen = vec![false; self.arcs.len()];
        let mut queue = VecDeque::from([source]);
        seen[source] = true;
        while let Some(a) = queue.pop_front() {
            for arc in &self.arcs[a] {
                self.work.scans += 1;
                if arc.residual > 0 && !seen[arc.to] {
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
                    && self.arcs[arc.to][arc.reverse].residual > 0
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

    fn push(&mut self, a: usize, index: usize, flow: u128) -> usize {
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
        let quantum = self.quantum();
        self.work.scans += 2 * self.work.edges;
        let mut total = 0_u128;
        for arc in self.arcs.iter_mut().flatten() {
            self.work.scans += 1;
            if arc.capacity.is_finite() {
                arc.residual = (arc.capacity / quantum) as u128;
                total += arc.residual;
            }
        }
        // A finite cut costs at most total units; one more makes every infinite arc
        // uncuttable. The same bound caps a larger demand's supply without overflowing a preflow.
        let bound = total + 1;
        for arc in self.arcs.iter_mut().flatten() {
            self.work.scans += 1;
            if arc.capacity.is_infinite() {
                arc.residual = bound;
            }
        }
        // Positive demand below the grid (including scaling underflow) still needs one unit, and
        // an unlimited one saturates to the bound. Rounding up makes every holding certificate
        // conservative in force units.
        let supply = ((demand / quantum).ceil() as u128).clamp(1, bound);
        let inlet = self.arcs.len();
        self.arcs.push(Vec::new());
        self.work.nodes += 1;
        self.edge(inlet, source, 0.0, 0.0);
        self.arcs[inlet][0].residual = supply;
        let n = self.arcs.len();
        let mut excess = vec![0_u128; n];
        let mut height = vec![0; n];
        let mut cursor = vec![0; n];
        let mut active = VecDeque::new();
        self.push(inlet, 0, supply);
        excess[source] = supply;
        active.push_back(source);
        self.global_relabel(inlet, sink, &mut height, &mut cursor);
        // Resolution: refresh after one graph-sized amount of discharge work, amortising
        // distance updates against local scans. Two residual arcs belong to each edge pair.
        let interval = n + 2 * self.work.edges;
        let mut refresh = self.work.scans + interval;
        while let Some(a) = active.pop_front() {
            while excess[a] > 0 {
                if cursor[a] == self.arcs[a].len() {
                    let next = self.arcs[a]
                        .iter()
                        .filter_map(|arc| {
                            self.work.scans += 1;
                            (arc.residual > 0).then_some(height[arc.to] + 1)
                        })
                        .min()
                        .expect(
                            "integer excess retains a positive reverse flow arc to its supplier",
                        );
                    assert!(
                        next > height[a],
                        "valid residual labels strictly increase on relabel"
                    );
                    height[a] = next;
                    cursor[a] = 0;
                    self.work.relabels += 1;
                    continue;
                }
                self.work.scans += 1;
                let arc = &self.arcs[a][cursor[a]];
                if arc.residual > 0 && height[a] == height[arc.to] + 1 {
                    let flow = excess[a].min(arc.residual);
                    let to = self.push(a, cursor[a], flow);
                    if excess[to] == 0 && to != inlet && to != sink {
                        active.push_back(to);
                    }
                    excess[a] -= flow;
                    excess[to] += flow;
                    // Sink inflow in a preflow certifies a feasible flow of at least that value:
                    // all remaining surplus can return to the inlet along reverse flow arcs.
                    if excess[sink] >= supply {
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
        // reachability selects the smallest grid cut's source side, excluding unrelated matter.
        let mut inside = self.reachable(source);
        assert!(
            !inside[sink],
            "exact integer discharge exhausts every source-to-sink residual path"
        );
        let capacity: f64 = self.arcs[..inlet]
            .iter()
            .enumerate()
            .filter(|(a, _)| inside[*a])
            .flat_map(|(_, arcs)| arcs)
            .filter(|arc| !inside[arc.to])
            .map(|arc| arc.capacity)
            .sum();
        assert!(
            capacity.is_finite(),
            "an admitted construction has a finite cut"
        );
        inside.truncate(inlet);
        (Cut::Released { inside, capacity }, self.work)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's environment destruction: fractional flow splitting must certify a held
    /// contact without relying on rounded sink accumulation equalling its inlet supply.
    #[test]
    fn fractional_parallel_supports_hold_without_roundoff_panic() {
        // Fixture: forced seed 1, three supported chunks, source 0 and ground 5.
        let mut graph = Network::new(6);
        graph.edge(0, 1, f64::INFINITY, 0.0);
        for (chunk, capacity) in [(2, 29.9), (3, 39.8), (4, 93.4)] {
            graph.edge(1, chunk, capacity, capacity);
            graph.edge(chunk, 5, 200.0, 200.0);
        }
        // DERIVED: all three supports in parallel carry 163.1 N, above the 100 N demand.
        assert!(matches!(graph.cut(0, 5, 100.0).0, Cut::Held));
    }

    /// The game's environment destruction: rounding must never turn an insufficient support
    /// into a holding certificate, even when the deficit is a single representable step.
    #[test]
    fn a_one_ulp_capacity_deficit_releases() {
        // Fixture: subnormal, normal-boundary, unit, large and maximum finite capacities.
        for capacity in [f64::from_bits(1), f64::MIN_POSITIVE, 1.0, 1e100, f64::MAX] {
            let mut graph = Network::new(2);
            graph.edge(0, 1, capacity, capacity);
            assert!(matches!(
                graph.cut(0, 1, capacity.next_up()).0,
                Cut::Released { .. }
            ));
        }
    }

    /// The game's environment destruction: contact verdicts and returned plugs must agree
    /// with exact integer-tenths cuts, including fractional cyclic and asymmetric networks.
    #[test]
    fn fractional_cuts_match_randomised_exact_reference() {
        // Fixture: deterministic LCG, small enough to enumerate every source-side subset.
        let mut random = 0x0005_eed6_u64;
        let mut next = || {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            random
        };
        for case in 0..2048 {
            let nodes = 3 + (next() % 6) as usize;
            let sink = nodes - 1;
            let scale = 2.0_f64.powi([-80, 0, 80][case % 3]);
            let force = |units: u64| units as f64 / 10.0 * scale;
            let mut edges = Vec::new();
            for a in 0..nodes {
                for b in a + 1..nodes {
                    if next() % 4 != 0 {
                        edges.push((a, b, next() % 2001, next() % 2001));
                    }
                }
            }
            let cut_units = |inside: &[bool]| {
                edges
                    .iter()
                    .map(|&(a, b, forward, backward)| match (inside[a], inside[b]) {
                        (true, false) => forward,
                        (false, true) => backward,
                        _ => 0,
                    })
                    .sum::<u64>()
            };
            // Source 0 forces seed 1; sink stays outside. Integer sums are exact.
            let expected = (0..1 << nodes)
                .filter(|bits| bits & 3 == 3 && bits & (1 << sink) == 0)
                .map(|bits| {
                    cut_units(&(0..nodes).map(|n| bits & (1 << n) != 0).collect::<Vec<_>>())
                })
                .min()
                .unwrap();
            for demand in [
                0.0,
                force(expected / 2),
                force(expected),
                force(expected + 1),
                f64::INFINITY,
            ] {
                let mut graph = Network::new(nodes);
                graph.edge(0, 1, f64::INFINITY, 0.0);
                for &(a, b, forward, backward) in &edges {
                    graph.edge(a, b, force(forward), force(backward));
                }
                let finite: Vec<_> = graph
                    .arcs
                    .iter()
                    .flatten()
                    .map(|arc| arc.capacity)
                    .filter(|c| c.is_finite() && *c > 0.0)
                    .collect();
                // DERIVED: grid loss plus input/output summation error, as stated in design.
                let k = finite.len() as f64 + 1.0;
                let gamma = k * f64::EPSILON / (1.0 - k * f64::EPSILON);
                let tolerance = k * graph.quantum() + gamma * finite.iter().sum::<f64>();
                match graph.cut(0, sink, demand).0 {
                    Cut::Held => assert!(
                        demand <= force(expected) + tolerance,
                        "case={case}, demand={demand}, expected={expected} tenths"
                    ),
                    Cut::Released { inside, capacity } => {
                        assert!(inside[0] && inside[1] && !inside[sink]);
                        assert!(demand + tolerance >= force(expected), "case={case}");
                        assert!(
                            (capacity - force(expected)).abs() <= tolerance,
                            "case={case}, capacity={capacity}, expected={}, tolerance={tolerance}",
                            force(expected)
                        );
                        assert!(
                            (capacity - force(cut_units(&inside))).abs() <= tolerance,
                            "case={case}: returned capacity must describe the returned plug"
                        );
                    }
                }
            }
        }
    }

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
        // allowance includes the inlet and three grid passes. Far supports need no routing.
        assert!(work.scans <= 5 * (work.nodes + 2 * work.edges), "{work:?}");
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
