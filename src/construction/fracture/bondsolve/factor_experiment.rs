//! Instrument for the game's environment destruction model, game state only.
//! The lane report owns the experiment's design and operation accounting.

use super::*;
use std::collections::{BTreeMap, BTreeSet};

// Resolution: bound relative residual and reference-field disagreement; never a strength margin.
const ACCURACY: f64 = 1e-6;

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

fn wrench(v: [f64; 6]) -> Wrench {
    Wrench {
        linear: DVec3::new(v[0], v[1], v[2]),
        angular: DVec3::new(v[3], v[4], v[5]),
    }
}

fn basis(axis: usize) -> Wrench {
    let mut v = [0.0; 6];
    v[axis] = 1.0;
    wrench(v)
}

#[derive(Debug, PartialEq, Eq)]
struct Component {
    members: Vec<usize>,
    anchored: bool,
}

/// Connectivity precedes every numeric update. A newly free piece is a split result,
/// not a small pivot to regularize. Fixed nodes here are authored buried matter only.
fn components(nodes: &[Node], edges: &[Edge], removed: &BTreeSet<usize>) -> Vec<Component> {
    let mut adjacent = vec![Vec::new(); nodes.len()];
    let mut anchored: Vec<_> = nodes.iter().map(|n| n.fixed).collect();
    for (id, e) in edges
        .iter()
        .enumerate()
        .filter(|(id, _)| !removed.contains(id))
    {
        if let Some(b) = e.b {
            adjacent[e.a].push(b);
            adjacent[b].push(e.a);
        } else {
            anchored[e.a] = true;
        }
        assert!(
            e.contact.is_none(),
            "bonded connectivity excludes resting contacts: {id}"
        );
    }
    let mut seen = vec![false; nodes.len()];
    let mut result = Vec::new();
    for start in 0..nodes.len() {
        if seen[start] {
            continue; // This node already belongs to a connected component.
        }
        let mut group = Component {
            members: Vec::new(),
            anchored: false,
        };
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(a) = stack.pop() {
            group.members.push(a);
            group.anchored |= anchored[a];
            for &b in &adjacent[a] {
                if !seen[b] {
                    seen[b] = true;
                    stack.push(b);
                }
            }
        }
        group.members.sort_unstable();
        result.push(group);
    }
    result
}

/// A free body's caller balances acceleration before solving. One chunk supplies its
/// coordinate gauge; it is not a support and must not appear in physical reaction totals.
fn gauged(nodes: &[Node], groups: &[Component]) -> Vec<Node> {
    let mut result = nodes.to_vec();
    for group in groups.iter().filter(|g| !g.anchored) {
        result[group.members[0]].fixed = true;
    }
    result
}

#[derive(Clone)]
struct Column {
    rows: Vec<usize>,
    values: Vec<f64>,
}

#[derive(Clone)]
struct Factor {
    columns: Vec<Column>,
    map: Vec<Option<usize>>,
    factor_ops: u64,
    assembly_ops: u64,
    matrix_nnz: usize,
    ordering_fill_edges: usize,
}

/// Exact minimum degree on chunk blocks. Each eliminated chunk completes its live
/// neighbour clique; retaining full blocks also retains cancellation zeros for downdates.
fn ordering(nodes: &[Node], edges: &[Edge]) -> (Vec<usize>, Vec<Vec<usize>>, usize) {
    let mut graph = vec![BTreeSet::new(); nodes.len()];
    for e in edges {
        if let Some(b) = e.b
            && !nodes[e.a].fixed
            && !nodes[b].fixed
        {
            graph[e.a].insert(b);
            graph[b].insert(e.a);
        }
    }
    let mut live: BTreeSet<_> = (0..nodes.len()).filter(|&i| !nodes[i].fixed).collect();
    let mut order = Vec::new();
    let mut fronts = Vec::new();
    let mut fill = 0;
    while let Some(&a) = live.iter().min_by_key(|&&i| (graph[i].len(), i)) {
        let front: Vec<_> = graph[a].iter().copied().collect();
        for (offset, &b) in front.iter().enumerate() {
            graph[b].remove(&a);
            for &c in &front[offset + 1..] {
                if graph[b].insert(c) {
                    graph[c].insert(b);
                    fill += 1;
                }
            }
        }
        live.remove(&a);
        order.push(a);
        fronts.push(front);
    }
    (order, fronts, fill)
}

fn at_node(e: &Edge, nodes: &[Node], carried: Wrench, node: usize) -> Wrench {
    let sign = if node == e.a { 1.0 } else { -1.0 };
    Wrench {
        linear: carried.linear * sign,
        angular: (carried.angular + (e.centre - nodes[node].centre).cross(carried.linear)) * sign,
    }
}

impl Factor {
    fn new(nodes: &[Node], edges: &[Edge]) -> Self {
        assert!(
            edges
                .iter()
                .all(|e| e.contact.is_none() && e.slip.is_none())
        );
        let (order, fronts, ordering_fill_edges) = ordering(nodes, edges);
        let mut map = vec![None; nodes.len()];
        for (i, &node) in order.iter().enumerate() {
            map[node] = Some(6 * i);
        }
        let n = 6 * order.len();
        let mut matrix = vec![BTreeMap::<usize, f64>::new(); n];
        let mut input = vec![Wrench::default(); nodes.len()];
        let mut assembly_ops = 0;
        for e in edges {
            let ends: Vec<_> = [Some(e.a), e.b]
                .into_iter()
                .flatten()
                .filter(|&a| !nodes[a].fixed)
                .collect();
            for &a in &ends {
                for axis in 0..6 {
                    input[a] = basis(axis);
                    let carried = e.resultant(nodes, &input);
                    // DERIVED from Edge::resultant and at_node: scalar arithmetic, including
                    // arithmetic on zero basis entries, with one operation per division/sqrt.
                    assembly_ops += if e.b.is_some() { 89 } else { 68 };
                    input[a] = Wrench::default();
                    let column = map[a].unwrap() + axis;
                    for &b in &ends {
                        assembly_ops += 21;
                        for (dof, value) in coordinates(at_node(e, nodes, carried, b))
                            .into_iter()
                            .enumerate()
                        {
                            let row = map[b].unwrap() + dof;
                            if row >= column {
                                *matrix[column].entry(row).or_default() += value;
                                assembly_ops += 1;
                            }
                        }
                    }
                }
            }
        }
        let matrix_nnz = matrix
            .iter()
            .map(|c| c.values().filter(|&&v| v != 0.0).count())
            .sum::<usize>()
            * 2
            - n;
        let mut columns = Vec::with_capacity(n);
        for (block, front) in fronts.iter().enumerate() {
            for axis in 0..6 {
                let j = 6 * block + axis;
                let mut rows: Vec<_> = (j..6 * (block + 1)).collect();
                for &b in front {
                    rows.extend(map[b].unwrap()..map[b].unwrap() + 6);
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
        let mut factor_ops = 0;
        for j in 0..n {
            for &i in &columns[j].rows {
                // A missing sparse entry is the mathematical zero of the assembled matrix.
                work[i] = matrix[j].get(&i).copied().unwrap_or(0.0);
            }
            for &(k, pos) in &prior[j] {
                let c = &columns[k];
                let ljk = c.values[pos];
                for (&i, &lik) in c.rows[pos..].iter().zip(&c.values[pos..]) {
                    work[i] -= ljk * lik;
                    factor_ops += 2;
                }
            }
            assert!(
                work[j].is_finite() && work[j] > 0.0,
                "anchored/gauged bonded stiffness must be SPD at {j}: {}",
                work[j]
            );
            let diagonal = work[j].sqrt();
            factor_ops += 1;
            let c = &mut columns[j];
            c.values[0] = diagonal;
            for (&i, value) in c.rows.iter().zip(&mut c.values).skip(1) {
                *value = work[i] / diagonal;
                factor_ops += 1;
            }
        }
        Self {
            columns,
            map,
            factor_ops,
            assembly_ops,
            matrix_nnz,
            ordering_fill_edges,
        }
    }

    fn nnz(&self) -> usize {
        self.columns.iter().map(|c| c.rows.len()).sum()
    }

    fn solve_ops(&self) -> u64 {
        (4 * (self.nnz() - self.columns.len()) + 2 * self.columns.len()) as u64
    }

    fn bytes(&self) -> usize {
        size_of::<Self>()
            + self
                .columns
                .iter()
                .map(|c| {
                    c.values.capacity() * size_of::<f64>() + c.rows.capacity() * size_of::<usize>()
                })
                .sum::<usize>()
            + self.columns.capacity() * size_of::<Column>()
            + self.map.capacity() * size_of::<Option<usize>>()
    }

    fn solve(&self, loads: &[Wrench]) -> Vec<Wrench> {
        let mut x = vec![0.0; self.columns.len()];
        for (&base, &load) in self.map.iter().zip(loads) {
            if let Some(base) = base {
                x[base..base + 6].copy_from_slice(&coordinates(load));
            } // Eliminated coordinates are authored anchors or the body's coordinate gauge.
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
            .map(|&base| match base {
                Some(base) => wrench(x[base..base + 6].try_into().unwrap()),
                None => Wrench::default(),
            })
            .collect()
    }

    /// Hyperbolic Cholesky downdate, A <- A - x x^T. A bond's endpoint clique
    /// is in the original symbolic pattern, so its reach cannot add factor entries.
    /// Each column depends on earlier updated columns; that dependency is serial.
    fn downdate(&mut self, mut x: Vec<f64>) -> u64 {
        let mut ops = 0;
        for (j, c) in self.columns.iter_mut().enumerate() {
            if x[j] == 0.0 {
                continue; // The update has not reached this elimination-tree branch.
            }
            let diagonal = c.values[0];
            let square = diagonal * diagonal - x[j] * x[j];
            assert!(
                square.is_finite() && square > 0.0,
                "connectivity must split free pieces before a downdate; nonpositive pivot at {j}: {square}"
            );
            let r = square.sqrt();
            let cosine = r / diagonal;
            let sine = x[j] / diagonal;
            c.values[0] = r;
            ops += 6;
            for (&i, l) in c.rows.iter().zip(&mut c.values).skip(1) {
                *l = (*l - sine * x[i]) / cosine;
                x[i] = cosine * x[i] - sine * *l;
                ops += 6;
            }
            x[j] = 0.0;
        }
        ops
    }

    fn remove_bond(&mut self, nodes: &[Node], e: &Edge) -> u64 {
        let modes = bond_modes(nodes, e, &self.map, self.columns.len());
        // DERIVED preparation: six force/couple columns and dense 6x6 Cholesky (409),
        // plus six B^T L products per nonfixed endpoint (378, counting sum's initial add).
        let ends = [Some(e.a), e.b]
            .into_iter()
            .flatten()
            .filter(|&i| self.map[i].is_some())
            .count();
        409 + 378 * ends as u64 + modes.into_iter().map(|x| self.downdate(x)).sum::<u64>()
    }
}

#[derive(Debug)]
enum Crack {
    Updated(u64),
    Split(Vec<Component>),
}

/// A split result leaves this factor untouched. The body's owner must create factors for
/// the returned pieces, subtract free-body acceleration, and spawn them through the split.
fn crack(
    factor: &mut Factor,
    nodes: &[Node],
    edges: &[Edge],
    removed: &mut BTreeSet<usize>,
    groups: &[Component],
    index: usize,
) -> Crack {
    assert!(!removed.contains(&index), "a bond can crack only once");
    let mut next = removed.clone();
    next.insert(index);
    let next_groups = components(nodes, edges, &next);
    if next_groups != groups {
        return Crack::Split(next_groups);
    }
    let ops = factor.remove_bond(nodes, &edges[index]);
    *removed = next;
    Crack::Updated(ops)
}

/// K_edge = B^T H B = U U^T; H acts on relative patch translation and rotation.
/// Its two positive 3x3 blocks give exactly six rank-one removal vectors.
fn bond_modes(nodes: &[Node], e: &Edge, map: &[Option<usize>], n: usize) -> Vec<Vec<f64>> {
    let mut h = [[0.0; 6]; 6];
    for j in 0..6 {
        let unit = basis(j);
        let column = coordinates(Wrench {
            linear: e.force(unit.linear),
            angular: e.couple(unit.angular),
        });
        for i in j..6 {
            let mut value = column[i];
            for (&lik, &ljk) in h[i][..j].iter().zip(&h[j][..j]) {
                value -= lik * ljk;
            }
            h[i][j] = if i == j {
                value.sqrt()
            } else {
                value / h[j][j]
            };
        }
    }
    let mut modes = vec![vec![0.0; n]; 6];
    for a in [Some(e.a), e.b].into_iter().flatten() {
        if let Some(base) = map[a] {
            let sign = if a == e.a { 1.0 } else { -1.0 };
            for axis in 0..6 {
                let unit = basis(axis);
                let b = coordinates(Wrench {
                    linear: unit.point(e.centre - nodes[a].centre) * sign,
                    angular: unit.angular * sign,
                });
                for j in 0..6 {
                    modes[j][base + axis] = (j..6).map(|i| b[i] * h[i][j]).sum();
                }
            }
        } // Authored anchors and coordinate gauges have no unknowns.
    }
    modes
}

fn residual(nodes: &[Node], edges: &[Edge], load: &[Wrench], answer: &[Wrench]) -> f64 {
    let mut product = vec![Wrench::default(); nodes.len()];
    Problem {
        nodes,
        edges,
        loads: load,
    }
    .apply(answer, &mut product);
    let mut error = 0.0;
    let mut norm = 0.0;
    for ((n, &f), p) in nodes.iter().zip(load).zip(product) {
        if !n.fixed {
            let mut r = f;
            r.add(p, -1.0);
            error += r.dot(r);
            norm += f.dot(f);
        }
    }
    relative_norm(error, norm)
}

fn relative_norm(error: f64, norm: f64) -> f64 {
    if norm == 0.0 {
        // An unloaded body or empty joint field has no relative scale. Only exact zero
        // error agrees with that field; an unexpected nonzero error must fail the gate.
        if error == 0.0 { 0.0 } else { f64::INFINITY }
    } else {
        (error / norm).sqrt()
    }
}

fn relative_field(nodes: &[Node], edges: &[Edge], a: &[Wrench], b: &[Wrench]) -> f64 {
    let mut error = 0.0;
    let mut norm = 0.0;
    for e in edges {
        let x = e.resultant(nodes, a);
        let y = e.resultant(nodes, b);
        let mut r = x;
        r.add(y, -1.0);
        error += r.dot(r);
        norm += x.dot(x);
    }
    relative_norm(error, norm)
}

/// The game's bonded solver must recover cantilever shear and moment from equilibrium.
/// An omitted patch couple, arm sign, or fixed-coordinate elimination breaks the check.
#[test]
fn cantilever_and_redundant_bond_downdate() {
    // Fixture: SI dimensions, load and stiffness; two identical bonded supports.
    let nodes = [Node {
        centre: DVec3::Y * 3.0,
        fixed: false,
    }];
    let e = Edge {
        a: 0,
        b: None,
        centre: DVec3::ZERO,
        normal: DVec3::Y,
        area: 0.25,
        patch: Patch::circle(DVec3::ZERO, DVec3::Y, 0.25),
        normal_stiffness: 8e9,
        shear_stiffness: 3e9,
        contact: None,
        bearing: None,
        slip: None,
    };
    let load = [Wrench::at(DVec3::X * 100.0, DVec3::ZERO)];
    let edges = [e.clone(), e.clone()];
    let mut factor = Factor::new(&nodes, &edges);
    let answer = factor.solve(&load);
    let reaction = e.resultant(&nodes, &answer);
    assert!((reaction.linear.x / 50.0 - 1.0).abs() < ACCURACY);
    assert!((reaction.angular.z / -150.0 - 1.0).abs() < ACCURACY);
    let groups = components(&nodes, &edges, &BTreeSet::new());
    let mut removed = BTreeSet::new();
    assert!(matches!(
        crack(&mut factor, &nodes, &edges, &mut removed, &groups, 0),
        Crack::Updated(_)
    ));
    let answer = factor.solve(&load);
    let reaction = e.resultant(&nodes, &answer);
    assert!((reaction.linear.x / 100.0 - 1.0).abs() < ACCURACY);
    assert!((reaction.angular.z / -300.0 - 1.0).abs() < ACCURACY);
    assert!(residual(&nodes, std::slice::from_ref(&e), &load, &answer) < ACCURACY);
    let Crack::Split(after) = crack(&mut factor, &nodes, &edges, &mut removed, &groups, 1) else {
        panic!("the split must receive the free piece before any singular downdate");
    };
    assert!(!after[0].anchored);
    assert_eq!(
        factor.solve(&load),
        answer,
        "a split cannot mutate the old factor"
    );
}

/// Free pieces need internal joint forces independent of the chosen coordinate gauge.
/// Mistaking that gauge for a physical anchor changes the force or loses the split.
#[test]
fn free_body_gauge_and_disconnecting_crack() {
    // Fixture: a balanced axial load on two bonded chunks, SI units.
    let nodes = [
        Node {
            centre: DVec3::ZERO,
            fixed: false,
        },
        Node {
            centre: DVec3::X,
            fixed: false,
        },
    ];
    let centre = DVec3::X / 2.0;
    let edges = [Edge {
        a: 0,
        b: Some(1),
        centre,
        normal: DVec3::X,
        area: 0.25,
        patch: Patch::circle(centre, DVec3::X, 0.25),
        normal_stiffness: 8e9,
        shear_stiffness: 3e9,
        contact: None,
        bearing: None,
        slip: None,
    }];
    let groups = components(&nodes, &edges, &BTreeSet::new());
    let loads = [
        Wrench::at(DVec3::X * 100.0, DVec3::ZERO),
        Wrench::at(-DVec3::X * 100.0, DVec3::ZERO),
    ];
    for gauge in 0..2 {
        let mut solve_nodes = nodes;
        solve_nodes[gauge].fixed = true;
        let mut factor = Factor::new(&solve_nodes, &edges);
        let answer = factor.solve(&loads);
        let carried = edges[0].resultant(&nodes, &answer);
        assert!((carried.linear.x / 100.0 - 1.0).abs() < ACCURACY);
        assert!(carried.angular.length() < ACCURACY);
        let mut removed = BTreeSet::new();
        let Crack::Split(pieces) = crack(&mut factor, &nodes, &edges, &mut removed, &groups, 0)
        else {
            panic!("a disconnected free piece must reach the split before a singular solve");
        };
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(|p| !p.anchored));
        assert!(removed.is_empty());
        assert_eq!(factor.solve(&loads), answer);
    }
    let isolated = [nodes[0]];
    let groups = components(&isolated, &[], &BTreeSet::new());
    let solve_nodes = gauged(&isolated, &groups);
    let factor = Factor::new(&solve_nodes, &[]);
    let answer = factor.solve(&loads[..1]);
    assert!(
        factor.columns.is_empty(),
        "an isolated piece has no internal solve"
    );
    assert_eq!(residual(&solve_nodes, &[], &loads[..1], &answer), 0.0);
    assert_eq!(relative_field(&solve_nodes, &[], &answer, &answer), 0.0);
}

fn census(id: &str, structure: &Structure) {
    let mut assessment = Assessment::default();
    assessment.rebuild(structure, &State::new(structure));
    let nodes: Vec<_> = structure
        .chunks
        .iter()
        .map(|c| Node {
            centre: c.centroid.as_dvec3(),
            fixed: c.buried,
        })
        .collect();
    let edges: Vec<_> = assessment
        .edges
        .into_iter()
        .filter(|e| e.contact.is_none())
        .map(|e| Edge { bearing: None, ..e })
        .collect();
    let groups = components(&nodes, &edges, &BTreeSet::new());
    let solve_nodes = gauged(&nodes, &groups);
    let factor = Factor::new(&solve_nodes, &edges);
    println!(
        "FACTOR {id} chunks={} bonds={} resting={} groups={} free={} unknowns={} matrix_nnz={} factor_nnz={} bytes={} factor_ops={} assembly_ops={} solve_ops_1={} ordering_fill_edges={}",
        nodes.len(),
        edges.len(),
        structure.joints.len() - edges.len(),
        groups.len(),
        groups.iter().filter(|g| !g.anchored).count(),
        factor.columns.len(),
        factor.matrix_nnz,
        factor.nnz(),
        factor.bytes(),
        factor.factor_ops,
        factor.assembly_ops,
        factor.solve_ops(),
        factor.ordering_fill_edges
    );
    // Fixture: nonuniform applied wrenches exercise all modes; free groups have their
    // aggregate acceleration subtracted using the existing game's body-frame law.
    let mut loads: Vec<_> = structure
        .chunks
        .iter()
        .enumerate()
        .map(|(i, c)| Wrench {
            linear: DVec3::new(
                (i as f64).sin(),
                -f64::from(c.mass) * f64::from(crate::physical_laws::gravity_mps2()),
                (i as f64).cos(),
            ),
            angular: DVec3::new((i as f64).cos(), (i as f64).sin(), 1.0),
        })
        .collect();
    for group in groups.iter().filter(|g| !g.anchored) {
        let ns: Vec<_> = group.members.iter().map(|&i| nodes[i]).collect();
        let ms: Vec<_> = group
            .members
            .iter()
            .map(|&i| f64::from(structure.chunks[i].mass))
            .collect();
        let inertia: Vec<_> = group
            .members
            .iter()
            .map(|&i| DMat3::from_diagonal(DVec3::splat(structure.chunks[i].intrinsic_inertia())))
            .collect();
        let mut fs: Vec<_> = group.members.iter().map(|&i| loads[i]).collect();
        accelerating_frame(&ns, &ms, &inertia, DVec3::ZERO, &mut fs);
        for (&i, f) in group.members.iter().zip(fs) {
            loads[i] = f;
        }
    }
    let answer = factor.solve(&loads);
    let res = residual(&solve_nodes, &edges, &loads, &answer);
    let mut cg = Krylov::default();
    let problem = Problem {
        nodes: &solve_nodes,
        edges: &edges,
        loads: &loads,
    };
    cg.advance(&problem, 8 * factor.columns.len().max(1));
    let comparison = relative_field(&solve_nodes, &edges, &answer, &cg.answer);
    println!(
        "ACCURACY {id} relative_residual={res:e} cg_progress={:?} cg_joint_visits={} joint_field_relative_error={comparison:e}",
        cg.progress, cg.joint_visits
    );
    assert_eq!(cg.progress, Progress::Equilibrated);
    assert!(res < ACCURACY && comparison < ACCURACY);
    crack_census(id, &nodes, &solve_nodes, &edges, &groups, &factor, &loads);
}

fn crack_census(
    id: &str,
    nodes: &[Node],
    solve_nodes: &[Node],
    edges: &[Edge],
    groups: &[Component],
    base: &Factor,
    loads: &[Wrench],
) {
    let mut factor = base.clone();
    let mut removed = BTreeSet::new();
    let mut operations = 0;
    let mut maximum = 0;
    let mut splits = 0;
    let mut fixed_bonds = 0;
    for (index, e) in edges.iter().enumerate() {
        if base.map[e.a].is_none() && e.b.is_none_or(|b| base.map[b].is_none()) {
            fixed_bonds += 1;
            continue; // A bond entirely within fixed matter has no stiffness unknowns.
        }
        let ops = match crack(&mut factor, nodes, edges, &mut removed, groups, index) {
            Crack::Updated(ops) => ops,
            Crack::Split(_) => {
                splits += 1;
                continue;
            }
        };
        operations += ops;
        maximum = maximum.max(ops);
        if [1, 10, 50, 200].contains(&removed.len()) {
            let live: Vec<_> = edges
                .iter()
                .enumerate()
                .filter(|(i, _)| !removed.contains(i))
                .map(|(_, e)| e.clone())
                .collect();
            let answer = factor.solve(loads);
            let res = residual(solve_nodes, &live, loads, &answer);
            let fresh = Factor::new(solve_nodes, &live);
            let difference = relative_field(solve_nodes, &live, &answer, &fresh.solve(loads));
            println!(
                "CRACK {id} k={} cumulative_update_ops={operations} last_update_ops={ops} max_update_ops={maximum} solve_ops_2={} relative_residual={res:e} fresh_field_relative_error={difference:e} fresh_factor_ops={} split_candidates={splits}",
                removed.len(),
                2 * factor.solve_ops(),
                fresh.factor_ops
            );
            assert!(res < ACCURACY && difference < ACCURACY);
        }
        if removed.len() == 200 {
            break;
        }
    }
    println!(
        "CRACK_END {id} removed={} split_candidates={splits} fixed_bonds={fixed_bonds}",
        removed.len()
    );
    let live: Vec<_> = edges
        .iter()
        .enumerate()
        .filter(|(i, _)| !removed.contains(i))
        .map(|(_, e)| e.clone())
        .collect();
    let answer = factor.solve(loads);
    let res = residual(solve_nodes, &live, loads, &answer);
    let fresh = Factor::new(solve_nodes, &live);
    let difference = relative_field(solve_nodes, &live, &answer, &fresh.solve(loads));
    println!(
        "CRACK_FINAL {id} k={} relative_residual={res:e} fresh_field_relative_error={difference:e}",
        removed.len()
    );
    assert!(res < ACCURACY && difference < ACCURACY);
}

/// Observations of shipped construction data, not a golden output or a runtime plugin claim.
#[test]
#[ignore = "design factor and crack operation census; run explicitly with --nocapture"]
fn shipped_design_census() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let mut ids: Vec<_> = std::fs::read_dir(root.join("constructions"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|id| {
            root.join(format!("constructions/{id}/{id}.construction.ron"))
                .exists()
        })
        .collect();
    ids.sort();
    let library =
        crate::construction::ConstructionLibrary::load(&root, ids.iter().map(String::as_str));
    for id in ids {
        let kind = library.get(&id).unwrap();
        census(&id, &kind.layout.structure(&kind.asset));
    }
}
