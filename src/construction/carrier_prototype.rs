//! Test-only carrier experiment for the game's environment destruction model, game state only.
//! The verdict and measurement protocol live in `construction-carrier-prototype.md`.

use super::*;
use bevy::math::DMat3;

#[derive(Clone, Copy, Debug, Default)]
struct Wrench {
    force: DVec3,
    moment: DVec3,
}

impl Wrench {
    fn point(at: Vec3, force: Vec3) -> Self {
        let force = force.as_dvec3();
        Self {
            force,
            moment: at.as_dvec3().cross(force),
        }
    }

    fn add(&mut self, other: Self) {
        self.force += other.force;
        self.moment += other.moment;
    }

    fn at(self, at: Vec3) -> (Vec3, Vec3) {
        (
            self.force.as_vec3(),
            (self.moment - at.as_dvec3().cross(self.force)).as_vec3(),
        )
    }
}

// Impulses have three distinct strength responses in Load::felt. A body's nonlinear pair-work
// share remains per source at the hinge; these buckets never replace that law.
const IMPULSIVE: usize = 0;
const YIELDING: usize = 1;
const PEAKED: usize = 2;

fn bucket(load: &Load) -> usize {
    match (load.peak, load.origin) {
        (Some(_), _) => PEAKED,
        (None, Origin::Field | Origin::Inertia { step: None }) => IMPULSIVE,
        _ => YIELDING,
    }
}

#[derive(Clone, Copy, Debug)]
struct Totals {
    mass: f64,
    first: DVec3,
    inertia: DMat3,
    gravity: Wrench,
    press: Wrench,
    peak: Wrench,
    mean: Wrench,
    blows: [Wrench; 3],
    inertial_press: DVec3,
    framed: bool,
}

impl Default for Totals {
    fn default() -> Self {
        Self {
            mass: 0.0,
            first: DVec3::ZERO,
            inertia: DMat3::ZERO,
            gravity: Wrench::default(),
            press: Wrench::default(),
            peak: Wrench::default(),
            mean: Wrench::default(),
            blows: [Wrench::default(); 3],
            inertial_press: DVec3::ZERO,
            framed: false,
        }
    }
}

fn tensor(r: DVec3, mass: f64, own: f64) -> DMat3 {
    DMat3::from_diagonal(DVec3::splat(own + mass * r.length_squared()))
        - DMat3::from_cols(r * r.x, r * r.y, r * r.z) * mass
}

fn own(chunk: &Chunk) -> f32 {
    let radius = (3.0 * chunk.volume / (4.0 * PI)).cbrt();
    0.4 * chunk.mass * radius * radius
}

impl Totals {
    fn chunk(chunk: &Chunk) -> Self {
        let mass = f64::from(chunk.mass);
        let at = chunk.centroid.as_dvec3();
        Self {
            mass,
            first: at * mass,
            inertia: tensor(at, mass, f64::from(own(chunk))),
            gravity: Wrench::point(chunk.centroid, Vec3::NEG_Y * (gravity_mps2() * chunk.mass)),
            ..Self::default()
        }
    }

    fn add(&mut self, other: Self) {
        self.mass += other.mass;
        self.first += other.first;
        self.inertia += other.inertia;
        self.gravity.add(other.gravity);
        self.press.add(other.press);
        self.peak.add(other.peak);
        self.mean.add(other.mean);
        for (a, b) in self.blows.iter_mut().zip(other.blows) {
            a.add(b);
        }
        self.inertial_press += other.inertial_press;
        self.framed |= other.framed;
    }

    fn load(&mut self, load: &Load) {
        self.press.add(Wrench::point(load.at, load.press));
        if let Some(force) = load.peak {
            self.peak.add(Wrench::point(load.at, force));
        }
        if let Some(force) = load.mean_force() {
            self.mean.add(Wrench::point(load.at, force));
        }
        self.blows[bucket(load)].add(Wrench::point(load.at, load.blow));
    }

    fn field(self) -> (Vec3, f32) {
        if !self.framed {
            return (Vec3::Y, gravity_mps2());
        }
        let force = ((self.gravity.force + self.inertial_press) / self.mass).as_vec3();
        let g = force.length();
        if g > 0.0 {
            (-force / g, g)
        } else {
            (Vec3::ZERO, 0.0)
        }
    }
}

/// Each free owner's rigid response is formed once per strength-response class, then applied
/// once per chunk. The six sphere couples integrate to torque -I_own * alpha (DERIVED).
fn intake(
    structure: &Structure,
    state: &State,
    sources: &[Load],
    spin: Vec3,
) -> (Vec<Totals>, Vec<bool>) {
    let mut totals: Vec<_> = structure.chunks.iter().map(Totals::chunk).collect();
    let mut scores = vec![0.0; totals.len()];
    for load in sources {
        totals[load.chunk].load(load);
        let step = match load.origin {
            Origin::Body { step, .. } | Origin::Contact { step, .. } => step,
            Origin::Field => 0.0,
            Origin::Inertia { .. } => unreachable!("intake accepts external sources only"),
        };
        scores[load.chunk] += load.blow.length() + load.press.length() * step;
    }
    let mut roots = vec![false; totals.len()];
    for group in loose_groups(structure, state) {
        let reference = *group
            .iter()
            .max_by(|&&a, &&b| {
                let (a_at, b_at) = (structure.chunks[a].centroid, structure.chunks[b].centroid);
                scores[a].total_cmp(&scores[b]).then_with(|| {
                    b_at.x
                        .total_cmp(&a_at.x)
                        .then(b_at.y.total_cmp(&a_at.y))
                        .then(b_at.z.total_cmp(&a_at.z))
                })
            })
            .expect("a loose group has members");
        roots[reference] = true;
        let mass: f32 = group.iter().map(|&c| structure.chunks[c].mass).sum();
        let centre = group
            .iter()
            .map(|&c| structure.chunks[c].centroid * structure.chunks[c].mass)
            .sum::<Vec3>()
            / mass;
        let inertia = group.iter().fold(Mat3::ZERO, |sum, &c| {
            let chunk = &structure.chunks[c];
            let r = chunk.centroid - centre;
            sum + Mat3::from_diagonal(Vec3::splat(own(chunk) + chunk.mass * r.length_squared()))
                - Mat3::from_cols(r * r.x, r * r.y, r * r.z) * chunk.mass
        });
        let inverse = inertia.inverse();
        assert!(inverse.is_finite(), "positive chunks give finite inertia");
        let gyroscopic = -(inverse * spin.cross(inertia * spin));
        let mut sum = Totals::default();
        for &c in &group {
            sum.add(totals[c]);
        }
        let response = |w: Wrench| {
            let (force, moment) = w.at(centre);
            (force / mass, inverse * moment)
        };
        let press = response(sum.press);
        let peak = response(sum.peak);
        let mean = response(sum.mean);
        let blows = sum.blows.map(response);
        for &c in &group {
            let chunk = &structure.chunks[c];
            let r = chunk.centroid - centre;
            let response_at = |(translation, alpha): (Vec3, Vec3)| {
                let force = -chunk.mass * (translation + alpha.cross(r));
                let mut wrench = Wrench::point(chunk.centroid, force);
                wrench.moment += (-own(chunk) * alpha).as_dvec3();
                wrench
            };
            let mut base = Wrench::point(
                chunk.centroid,
                Vec3::Y * (gravity_mps2() * chunk.mass)
                    - chunk.mass * (spin.cross(spin.cross(r)) + gyroscopic.cross(r)),
            );
            base.moment += (-own(chunk) * gyroscopic).as_dvec3();
            let t = &mut totals[c];
            t.framed = true;
            t.inertial_press = base.force;
            t.inertial_press += response_at(press).force;
            t.press.add(base);
            t.press.add(response_at(press));
            t.peak.add(response_at(peak));
            t.mean.add(response_at(mean));
            for (b, response) in t.blows.iter_mut().zip(blows) {
                b.add(response_at(response));
            }
        }
    }
    (totals, roots)
}

struct Carrier {
    adjacency: Vec<Vec<(usize, usize)>>,
    parent: Vec<Option<(usize, usize)>>,
    order: Vec<usize>,
    totals: Vec<Totals>,
    bridge_child: Vec<Option<usize>>,
}

impl Carrier {
    fn new(structure: &Structure, state: &State, sources: &[Load], spin: Vec3) -> Self {
        let (mut totals, roots) = intake(structure, state, sources, spin);
        let root = totals.len();
        totals.push(Totals::default());
        let mut adjacency = vec![Vec::new(); totals.len()];
        let mut edge = |a: usize, b: usize, j: usize| {
            adjacency[a].push((b, j));
            adjacency[b].push((a, j));
        };
        for &j in &state.joints {
            let joint = &structure.joints[j];
            if !state.cracked[j] && state.live(joint) && joint.seam().is_some() {
                edge(
                    joint.a,
                    match joint.b {
                        Side::Chunk(b) => b,
                        Side::Ground => root,
                    },
                    j,
                );
            }
        }
        for &c in &state.members {
            if state.standing(c) && (structure.chunks[c].buried || roots[c]) {
                edge(c, root, structure.joints.len() + c);
            }
        }
        let mut parent = vec![None; totals.len()];
        let mut seen = vec![false; totals.len()];
        let mut order = vec![root];
        seen[root] = true;
        let mut k = 0;
        while k < order.len() {
            let a = order[k];
            k += 1;
            for &(b, j) in &adjacency[a] {
                if !seen[b] {
                    seen[b] = true;
                    parent[b] = Some((a, j));
                    order.push(b);
                }
            }
        }
        for &c in order.iter().rev() {
            if let Some((p, _)) = parent[c] {
                let t = totals[c];
                totals[p].add(t);
            }
        }
        // Low links use edge identity, including parallel anchors and implicit soil/reference
        // edges. A non-bridge can never be a single holder, regardless of BFS carrier order.
        let mut discovery = vec![usize::MAX; totals.len()];
        let mut low = discovery.clone();
        let mut dfs_parent = vec![None; totals.len()];
        let mut bridge = vec![false; structure.joints.len() + root];
        discovery[root] = 0;
        low[root] = 0;
        let mut clock = 1;
        let mut stack = vec![(root, 0)];
        while let Some((a, next)) = stack.last_mut() {
            let a = *a;
            if *next < adjacency[a].len() {
                let (b, j) = adjacency[a][*next];
                *next += 1;
                if dfs_parent[a].is_some_and(|(_, incoming)| incoming == j) {
                    continue;
                }
                if discovery[b] == usize::MAX {
                    dfs_parent[b] = Some((a, j));
                    discovery[b] = clock;
                    low[b] = clock;
                    clock += 1;
                    stack.push((b, 0));
                } else {
                    low[a] = low[a].min(discovery[b]);
                }
            } else {
                stack.pop();
                if let Some((p, j)) = dfs_parent[a] {
                    bridge[j] = low[a] > discovery[p];
                    low[p] = low[p].min(low[a]);
                }
            }
        }
        let mut bridge_child = vec![None; structure.joints.len()];
        for &c in &order {
            if let Some((_, j)) = parent[c]
                && j < bridge_child.len()
                && bridge[j]
            {
                bridge_child[j] = Some(c);
            }
        }
        Self {
            adjacency,
            parent,
            order,
            totals,
            bridge_child,
        }
    }

    /// A changed press changes no geometry, topology or reference. A moving owner also changes
    /// its rigid acceleration and potentially its reference; that is a full intake, not this path.
    fn press_delta(&mut self, mut chunk: usize, delta: Wrench) -> usize {
        let mut visited = 0;
        loop {
            self.totals[chunk].press.add(delta);
            visited += 1;
            match self.parent[chunk] {
                Some((p, _)) => chunk = p,
                None => break,
            }
        }
        visited
    }

    /// Exact components after a multi-joint plane cut. This deliberately exposes the unresolved
    /// all-planes cost; a tree subtree is not an answer for a cut across multiple support routes.
    fn plane_groups(&self, cut: &[usize], local: &[Totals]) -> (Vec<(Vec<usize>, Totals)>, usize) {
        let root = self.adjacency.len() - 1;
        let mut removed = vec![
            false;
            self.adjacency
                .iter()
                .flatten()
                .map(|&(_, j)| j)
                .max()
                .map_or(0, |j| j + 1)
        ];
        for &j in cut {
            removed[j] = true;
        }
        let mut seen = vec![false; self.adjacency.len()];
        let mut result = Vec::new();
        let mut visits = 0;
        // Ground first: every remaining component is held by this cut alone.
        for start in std::iter::once(root).chain(self.order.iter().copied().filter(|&c| c != root))
        {
            if seen[start] {
                continue;
            }
            let mut queue = vec![start];
            seen[start] = true;
            let mut k = 0;
            let mut total = Totals::default();
            while k < queue.len() {
                let a = queue[k];
                k += 1;
                visits += 1;
                if a != root {
                    total.add(local[a]);
                }
                for &(b, j) in &self.adjacency[a] {
                    visits += 1;
                    if !removed[j] && !seen[b] {
                        seen[b] = true;
                        queue.push(b);
                    }
                }
            }
            if start != root {
                queue.sort_unstable();
                result.push((queue, total));
            }
        }
        (result, visits)
    }
}

fn pulse(navier: &Navier, structure: &Structure, total: Totals) -> Reading {
    let (gf, gm) = total.gravity.at(navier.centre);
    let (pf, pm) = total.press.at(navier.centre);
    let (peak, peak_m) = total.peak.at(navier.centre);
    let (mean, mean_m) = total.mean.at(navier.centre);
    let (demand, capacity) = navier.peak(&structure.joints, gf + pf + mean, gm + pm + mean_m);
    Reading {
        falling: plastic_demand(strength_ratio(|s| {
            let (demand, capacity) =
                navier.peak(&structure.joints, gf + pf + peak * s, gm + pm + peak_m * s);
            demand >= capacity
        })),
        yields: demand >= capacity,
    }
}

fn weight(navier: &Navier, structure: &Structure, total: Totals) -> Failure {
    let mut wrench = total.gravity;
    wrench.add(total.press);
    let (force, moment) = wrench.at(navier.centre);
    let (demand, capacity) = navier.peak(&structure.joints, force, moment);
    Failure {
        joints: navier
            .stresses(&structure.joints, force, moment)
            .filter(|&(_, d, c)| d >= c)
            .map(|(j, ..)| j)
            .collect(),
        cause: Cause::Weight,
        demand,
        capacity,
    }
}

/// The existing hinge law, with mass moments and impulse wrenches read from the carrier totals.
/// Body-source pair work still reads each body's lever. The experiment does not linearize it.
fn push(
    structure: &Structure,
    section: &[usize],
    group: &[usize],
    sources: &[Load],
    total: Totals,
    reading: Reading,
) -> Option<Failure> {
    let joints: Vec<_> = section.iter().map(|&j| &structure.joints[j]).collect();
    let origin = joints[0].patch().0;
    let area: f32 = joints.iter().map(|j| j.area).sum();
    let centre = joints
        .iter()
        .map(|j| (j.patch().0 - origin) * j.area)
        .sum::<Vec3>()
        / area;
    let com = (total.first / total.mass).as_vec3() - origin;
    let normal_sum = joints
        .iter()
        .map(|j| {
            j.normal
                * j.area
                * if j.normal.dot(com - (j.patch().0 - origin)) >= 0.0 {
                    1.0
                } else {
                    -1.0
                }
        })
        .sum::<Vec3>();
    let n = normal_sum.try_normalize()?;
    let factors = [
        1.0,
        if reading.yields { 1.0 } else { 0.0 },
        reading.falling.sqrt(),
    ];
    let wrench_at = |at: Vec3| {
        total
            .blows
            .iter()
            .zip(factors)
            .fold(Wrench::default(), |mut sum, (w, factor)| {
                sum.force += w.force * f64::from(factor);
                sum.moment += w.moment * f64::from(factor);
                sum
            })
            .at(at)
    };
    let (_, moment) = wrench_at(origin + centre);
    let bending = moment - n * moment.dot(n);
    // Resolution: exact zero supplies no hinge. Near-zero reassociation is reported by the
    // comparison rather than concealed by a new force or strength tolerance.
    let axis = bending.try_normalize()?;
    let across = n.cross(axis);
    let back = joints
        .iter()
        .map(|j| across.dot((j.patch().0 - origin) - centre) - (j.patch().1 / PI).sqrt())
        .fold(f32::INFINITY, f32::min);
    let hinge = origin + centre + across * back;
    let h = hinge.as_dvec3();
    let a = axis.as_dvec3();
    // I_h = I_origin - I_com(origin) + I_com(hinge), parallel-axis theorem (DERIVED).
    let com_world = total.first / total.mass;
    let shifted =
        total.inertia - tensor(com_world, total.mass, 0.0) + tensor(com_world - h, total.mass, 0.0);
    let inertia = a.dot(shifted * a) as f32;
    let (_, moment) = wrench_at(hinge);
    let mut demand = moment.dot(axis);
    for load in sources
        .iter()
        .filter(|load| group.binary_search(&load.chunk).is_ok())
    {
        if matches!(load.origin, Origin::Body { .. }) {
            let moment = (load.at - hinge).cross(load.blow).dot(axis);
            if moment != 0.0 {
                let lever = moment / load.blow.length();
                demand += moment
                    * (load.origin.work_share(lever * lever / inertia) - 1.0)
                    * load.felt(reading);
            }
        }
    }
    if demand <= 0.0 {
        return None;
    }
    let (up, g) = total.field();
    let r = (com + origin - hinge) - axis * (com + origin - hinge).dot(axis);
    let (height, rising) = (r.dot(up), axis.cross(r).dot(up));
    let lift = if n.dot(up) > 0.0 && rising > 0.0 {
        total.mass as f32 * g * (height.hypot(rising) - height)
    } else {
        0.0
    };
    let cohesion: f32 = joints
        .iter()
        .filter_map(|j| Some(j.seam()?.fracture_energy * j.area))
        .sum();
    Some(Failure {
        joints: section.to_vec(),
        cause: Cause::Push,
        demand,
        capacity: (2.0 * inertia * (cohesion + lift)).sqrt(),
    })
}

fn shipped() -> (String, Arc<Structure>) {
    let root = crate::map::tests::shipped_assets();
    let mut ids: Vec<_> = std::fs::read_dir(root.join("constructions"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter_map(|dir| {
            let id = dir.file_name()?.to_str()?.to_owned();
            dir.join(format!("{id}.construction.ron"))
                .is_file()
                .then_some(id)
        })
        .collect();
    ids.sort();
    let library =
        crate::construction::ConstructionLibrary::load(&root, ids.iter().map(String::as_str));
    let id = ids
        .into_iter()
        .max_by_key(|id| library.get(id).unwrap().structure(1.0).chunks.len())
        .expect("shipped construction data exists");
    let structure = library.get(&id).unwrap().structure(1.0);
    (id, structure)
}

/// A load delta must reach every ancestor exactly once. This invariant is what lets a standing
/// owner reuse its mass sums between contact updates; the particular carrier path is irrelevant.
#[test]
fn a_press_delta_reaches_only_its_carrier_ancestors() {
    let (_, structure) = shipped();
    let state = State::new(&structure);
    let mut carrier = Carrier::new(&structure, &state, &[], Vec3::ZERO);
    let chunk = *carrier
        .order
        .iter()
        .find(|&&c| carrier.parent[c].is_some())
        .unwrap();
    let before = carrier.totals.clone();
    // Fixture: a point press; its size has no role in the graph invariant.
    let delta = Wrench::point(structure.chunks[chunk].centroid, Vec3::new(2.0, -3.0, 5.0));
    let mut path = vec![chunk];
    while let Some((p, _)) = carrier.parent[*path.last().unwrap()] {
        path.push(p);
    }
    assert_eq!(carrier.press_delta(chunk, delta), path.len());
    for (c, (before, after)) in before.iter().zip(&carrier.totals).enumerate() {
        let expected = if path.contains(&c) {
            delta.force
        } else {
            DVec3::ZERO
        };
        assert_eq!(after.press.force - before.press.force, expected);
        assert_eq!(after.mass, before.mass);
    }
}

/// The game's shared support must never be assigned to one arbitrary carrier. A cycle and two
/// parallel anchors have no single holder; removing either route leaves the other intact.
#[test]
fn low_links_refuse_shared_routes_and_parallel_anchors() {
    let (_, source) = shipped();
    // Fixture: two anchored piers and a span, represented by three equal positive chunks.
    let mut chunk = source.chunks[0];
    chunk.buried = false;
    let mut structure = Structure {
        chunks: vec![chunk; 3],
        joints: Vec::new(),
    };
    let mut joint = source
        .joints
        .iter()
        .copied()
        .find(|j| j.seam().is_some())
        .unwrap();
    for (a, b) in [
        (0, Side::Ground),
        (1, Side::Ground),
        (2, Side::Chunk(0)),
        (2, Side::Chunk(1)),
    ] {
        joint.a = a;
        joint.b = b;
        structure.joints.push(joint);
    }
    let state = State::new(&structure);
    let carrier = Carrier::new(&structure, &state, &[], Vec3::ZERO);
    assert!(carrier.bridge_child.iter().all(Option::is_none));
    let mut reversed = state.clone();
    reversed.joints.reverse();
    let alternative = Carrier::new(&structure, &reversed, &[], Vec3::ZERO);
    assert_eq!(carrier.bridge_child, alternative.bridge_child);
    let mut state = state;
    state.cracked[3] = true;
    let carrier = Carrier::new(&structure, &state, &[], Vec3::ZERO);
    assert!(carrier.bridge_child[2].is_some());
    structure.joints.push(structure.joints[0]);
    let carrier = Carrier::new(&structure, &State::new(&structure), &[], Vec3::ZERO);
    assert!(carrier.bridge_child.iter().all(Option::is_none));
}

// Temporary one-time census adapters. They exercise the shipped failure ordering while the
// independent topology census checks which groups a carrier is permitted to assess.
struct Context {
    carrier: Carrier,
    local: Vec<Totals>,
    sources: Vec<Load>,
}
thread_local! {
    static ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
    static COUNTERS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

pub(super) fn prepare(structure: &Structure, state: &State, inputs: GroupLoads<'_>) {
    if ACTIVE.get() {
        let (local, _) = intake(structure, state, inputs.sources, inputs.spin);
        CONTEXT.with_borrow_mut(|slot| {
            *slot = Some(Context {
                carrier: Carrier::new(structure, state, inputs.sources, inputs.spin),
                local,
                sources: inputs.sources.to_vec(),
            })
        });
    }
}

fn with_total<T>(
    section: &[usize],
    group: &[usize],
    f: impl FnOnce(Totals, &[Load]) -> T,
) -> Option<T> {
    if !ACTIVE.get() {
        return None;
    }
    CONTEXT.with_borrow(|slot| {
        let ctx = slot
            .as_ref()
            .expect("an active census prepares the current fracture stage");
        let single = if section.len() == 1 {
            ctx.carrier.bridge_child[section[0]]
        } else {
            None
        };
        let total = match single {
            Some(c) => {
                COUNTERS.set((COUNTERS.get().0 + 1, COUNTERS.get().1));
                ctx.carrier.totals[c]
            }
            None => {
                COUNTERS.set((COUNTERS.get().0, COUNTERS.get().1 + 1));
                group.iter().fold(Totals::default(), |mut total, &c| {
                    total.add(ctx.local[c]);
                    total
                })
            }
        };
        Some(f(total, &ctx.sources))
    })
}

pub(super) fn override_weight(
    structure: &Structure,
    section: &[usize],
    group: &[usize],
) -> Option<Option<Failure>> {
    with_total(section, group, |total, _| {
        Navier::of(structure, section, group, &mut 0).map(|n| weight(&n, structure, total))
    })
}
pub(super) fn override_pulse(
    structure: &Structure,
    section: &[usize],
    group: &[usize],
) -> Option<Reading> {
    with_total(section, group, |total, _| {
        match Navier::of(structure, section, group, &mut 0) {
            Some(n) => pulse(&n, structure, total),
            None => Reading::UNRESISTED,
        }
    })
}
pub(super) fn override_push(
    structure: &Structure,
    section: &[usize],
    group: &[usize],
    reading: Reading,
) -> Option<Option<Failure>> {
    with_total(section, group, |total, sources| {
        push(structure, section, group, sources, total, reading)
    })
}

#[derive(Default, Debug)]
struct Census {
    evaluations: usize,
    identical: usize,
    weight_identical: usize,
    structural_identical: usize,
    single_groups: usize,
    single_mismatch: usize,
    plane_groups: usize,
    plane_mismatch: usize,
    weight_verdicts: usize,
    weight_different: usize,
    push_verdicts: usize,
    push_different: usize,
}

fn signatures(failures: &[Failure]) -> Vec<(Vec<usize>, Cause)> {
    failures
        .iter()
        .map(|f| (f.joints.clone(), f.cause))
        .collect()
}

fn verdict(failure: &Option<Failure>) -> Option<(Vec<usize>, Cause, bool)> {
    failure
        .as_ref()
        .map(|f| (f.joints.clone(), f.cause, f.demand >= f.capacity))
}

fn compare(
    label: &str,
    structure: &Structure,
    state: &State,
    sources: &[Load],
    spin: Vec3,
    census: &mut Census,
) -> Option<Failure> {
    compare_changed(label, structure, state, sources, spin, (&[], &[]), census).0
}

fn compare_changed(
    label: &str,
    structure: &Structure,
    state: &State,
    sources: &[Load],
    spin: Vec3,
    changed: (&[usize], &[usize]),
    census: &mut Census,
) -> (Option<Failure>, State) {
    let inputs = GroupLoads::new(sources, spin);
    ACTIVE.set(false);
    let mut old_state = state.clone();
    let old_structural = structural(
        structure,
        &mut old_state,
        inputs,
        &mut SectionCache::default(),
    );
    let old_weigh = weigh(
        structure,
        &mut old_state,
        changed.0,
        changed.1,
        inputs,
        &[],
        &mut SectionCache::default(),
    );
    ACTIVE.set(true);
    let mut new_state = state.clone();
    let new_structural = structural(
        structure,
        &mut new_state,
        inputs,
        &mut SectionCache::default(),
    );
    let new_weigh = weigh(
        structure,
        &mut new_state,
        changed.0,
        changed.1,
        inputs,
        &[],
        &mut SectionCache::default(),
    );
    ACTIVE.set(false);
    CONTEXT.with_borrow_mut(|ctx| *ctx = None);
    let push_equal = signatures(&old_structural) == signatures(&new_structural);
    let weight_equal = signatures(&old_weigh.failures) == signatures(&new_weigh.failures)
        && old_weigh.unassessed == new_weigh.unassessed;
    let equal = push_equal && weight_equal && old_state == new_state;
    census.evaluations += 1;
    census.identical += usize::from(equal);
    census.structural_identical += usize::from(push_equal);
    census.weight_identical += usize::from(weight_equal);
    println!(
        "EVAL {label}: identical={equal} push={} weight={} unassessed={} steps={}",
        old_structural.len(),
        old_weigh.failures.len(),
        old_weigh.unassessed.len(),
        old_weigh.steps
    );
    if !equal {
        println!(
            "DIFFERENCE {label}: old_push={:?} new_push={:?} old_weight={:?} new_weight={:?} old_unassessed={:?} new_unassessed={:?}",
            old_structural,
            new_structural,
            old_weigh.failures,
            new_weigh.failures,
            old_weigh.unassessed,
            new_weigh.unassessed
        );
    }
    // Independently compare every singleton, including joints the load did not touch. Low links
    // must agree with direct held() walks for every rooted endpoint, not only a chosen route.
    let framed = inputs.framed(structure, state);
    let graph = loaded_graph(structure, state, &framed);
    let carrier = Carrier::new(structure, state, sources, spin);
    let (local, _) = intake(structure, state, sources, spin);
    let reachable: HashSet<_> = carrier.order.iter().copied().collect();
    for &j in &state.joints {
        let joint = &structure.joints[j];
        if state.cracked[j] || !state.live(joint) || joint.seam().is_none() {
            continue;
        }
        let mut old_groups = Vec::new();
        for c in [
            Some(joint.a),
            match joint.b {
                Side::Chunk(b) => Some(b),
                Side::Ground => None,
            },
        ]
        .into_iter()
        .flatten()
        {
            if reachable.contains(&c)
                && let Some(group) = held(c, &[j], &graph, &mut 0)
            {
                old_groups.push(group);
            }
        }
        old_groups.sort();
        old_groups.dedup();
        let mut new_groups = Vec::new();
        if let Some(child) = carrier.bridge_child[j] {
            let mut group = Vec::new();
            let mut queue = vec![child];
            while let Some(c) = queue.pop() {
                group.push(c);
                for &(b, edge) in &carrier.adjacency[c] {
                    if carrier.parent[b] == Some((c, edge)) {
                        queue.push(b);
                    }
                }
            }
            group.sort_unstable();
            new_groups.push(group);
        }
        census.single_groups += 1;
        if old_groups != new_groups {
            census.single_mismatch += 1;
            println!(
                "TOPOLOGY DIFFERENCE {label} single={j} old={old_groups:?} new={new_groups:?}"
            );
        }
        for group in old_groups {
            let total = carrier.totals[carrier.bridge_child[j]
                .expect("the topology census identifies each held singleton")];
            compare_section(
                label,
                structure,
                &[j],
                &group,
                &framed,
                sources,
                total,
                census,
            );
        }
    }
    for section in sections(structure, state)
        .into_iter()
        .filter(|s| s.len() > 1)
    {
        let (new_groups, _) = carrier.plane_groups(&section, &local);
        let mut old_groups = Vec::new();
        for &j in &section {
            let joint = &structure.joints[j];
            for c in [
                Some(joint.a),
                match joint.b {
                    Side::Chunk(b) => Some(b),
                    Side::Ground => None,
                },
            ]
            .into_iter()
            .flatten()
            {
                if reachable.contains(&c)
                    && !old_groups
                        .iter()
                        .any(|g: &Vec<usize>| g.binary_search(&c).is_ok())
                    && let Some(group) = held(c, &section, &graph, &mut 0)
                {
                    old_groups.push(group);
                }
            }
        }
        old_groups.sort();
        let mut actual: Vec<_> = new_groups.iter().map(|(group, _)| group.clone()).collect();
        actual.sort();
        census.plane_groups += 1;
        if old_groups != actual {
            census.plane_mismatch += 1;
            println!("PLANE DIFFERENCE {label} section={section:?}");
        }
        for (group, total) in new_groups {
            compare_section(
                label, structure, &section, &group, &framed, sources, total, census,
            );
        }
    }
    (
        old_structural
            .first()
            .or(old_weigh.failures.first())
            .cloned(),
        old_state,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "one-time census compares the explicit law inputs"
)]
fn compare_section(
    label: &str,
    structure: &Structure,
    section: &[usize],
    group: &[usize],
    framed: &[Load],
    sources: &[Load],
    total: Totals,
    census: &mut Census,
) {
    let old_weight = weight_failure(
        structure,
        section,
        group,
        framed,
        Vec3::NEG_Y * gravity_mps2(),
        &mut 0,
    );
    let new_weight =
        Navier::of(structure, section, group, &mut 0).map(|n| weight(&n, structure, total));
    census.weight_verdicts += 1;
    if verdict(&old_weight) != verdict(&new_weight) {
        census.weight_different += 1;
        println!(
            "WEIGHT DIFFERENCE {label} section={section:?} old={old_weight:?} new={new_weight:?}"
        );
    }
    let old_reading = section_pulse(structure, section, group, framed, &mut 0);
    let new_reading = match Navier::of(structure, section, group, &mut 0) {
        Some(n) => pulse(&n, structure, total),
        None => Reading::UNRESISTED,
    };
    let old_push = section_failure(structure, section, group, framed, old_reading);
    let new_push = push(structure, section, group, sources, total, new_reading);
    census.push_verdicts += 1;
    // None and a subthreshold hinge are both held; a different hinge matters only if it opens.
    let opens = |f: &Option<Failure>| f.as_ref().is_some_and(|f| f.demand >= f.capacity);
    if opens(&old_push) != opens(&new_push) {
        census.push_different += 1;
        println!(
            "PUSH DIFFERENCE {label} section={section:?} old={old_push:?} new={new_push:?} old_reading={old_reading:?} new_reading={new_reading:?}"
        );
    }
}

fn doubled(structure: &Structure) -> Structure {
    let n = structure.chunks.len();
    let lo = structure
        .chunks
        .iter()
        .map(|c| c.centroid)
        .fold(Vec3::INFINITY, Vec3::min);
    let hi = structure
        .chunks
        .iter()
        .map(|c| c.centroid)
        .fold(Vec3::NEG_INFINITY, Vec3::max);
    // Fixture: disjoint translated copy in every axis, so the double also doubles plane offsets.
    let offset = (hi - lo + Vec3::ONE) * Vec3::new(2.3, 1.7, 3.1);
    let mut double = structure.clone();
    double.chunks.extend(structure.chunks.iter().map(|c| Chunk {
        centroid: c.centroid + offset,
        ..*c
    }));
    double.joints.extend(structure.joints.iter().map(|j| Joint {
        a: j.a + n,
        b: match j.b {
            Side::Chunk(b) => Side::Chunk(b + n),
            Side::Ground => Side::Ground,
        },
        centroid: j.centroid + offset,
        bond: match j.bond {
            Bond::AreaShare {
                seam,
                footprint_area,
                footprint_centroid,
            } => Bond::AreaShare {
                seam,
                footprint_area,
                footprint_centroid: footprint_centroid + offset,
            },
            bond => bond,
        },
        ..*j
    }));
    double
}

fn timed(mut f: impl FnMut(), iterations: usize) -> (f64, f64, f64) {
    // Fixture: warm-up and sample counts are measurement protocol, never gameplay constants.
    for _ in 0..5 {
        f();
    }
    let mut times = Vec::new();
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        f();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    (times[0], times[times.len() / 2], *times.last().unwrap())
}

fn timings(structure: &Structure, sources: &[Load]) {
    let state = State::new(structure);
    let carrier = Carrier::new(structure, &state, sources, Vec3::ZERO);
    let full = timed(
        || {
            std::hint::black_box(Carrier::new(structure, &state, sources, Vec3::ZERO));
        },
        101,
    );
    println!(
        "TIMING carrier_full_ms min/median/max={full:?} chunks={} joints={}",
        structure.chunks.len(),
        structure.joints.len()
    );
    press_timing(carrier, structure);
    for (label, structure) in [
        ("source", structure.clone()),
        ("double", doubled(structure)),
    ] {
        let state = State::new(&structure);
        let carrier = Carrier::new(&structure, &state, sources, Vec3::ZERO);
        let (local, _) = intake(&structure, &state, sources, Vec3::ZERO);
        let planes: Vec<_> = sections(&structure, &state)
            .into_iter()
            .filter(|s| s.len() > 1)
            .collect();
        let mut visits = 0;
        for section in &planes {
            visits += carrier.plane_groups(section, &local).1;
        }
        let time = timed(
            || {
                for section in &planes {
                    let (groups, _) = carrier.plane_groups(section, &local);
                    for (group, total) in groups {
                        if let Some(n) = Navier::of(&structure, section, &group, &mut 0) {
                            std::hint::black_box(weight(&n, &structure, total));
                        }
                    }
                }
            },
            31,
        );
        println!(
            "TIMING planes_{label}_ms={time:?} chunks={} joints={} planes={} visits={visits}",
            structure.chunks.len(),
            structure.joints.len(),
            planes.len()
        );
    }
}

/// One-time observation bed; no assertion pins a shipped construction's emergent verdicts or
/// timing. The branch report records the comparison, and this bed is removed after the verdict.
#[test]
#[ignore = "one-time carrier census and timings under the machine lock"]
fn carrier_census() {
    let (id, structure) = shipped();
    println!(
        "DATA {id} chunks={} joints={}",
        structure.chunks.len(),
        structure.joints.len()
    );
    let target = structure
        .chunks
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.buried)
        .max_by(|(_, a), (_, b)| a.centroid.y.total_cmp(&b.centroid.y))
        .unwrap()
        .0;
    let force_scale: f32 = structure
        .joints
        .iter()
        .filter(|j| j.a == target || j.b == Side::Chunk(target))
        .filter_map(|j| Some(j.area * j.seam()?.tensile))
        .sum();
    let mass: f32 = structure.chunks.iter().map(|c| c.mass).sum();
    let impulse_scale = (2.0
        * mass
        * structure
            .joints
            .iter()
            .filter_map(|j| Some(j.area * j.seam()?.fracture_energy))
            .sum::<f32>())
    .sqrt();
    let make_loads = |k: usize| {
        // Fixture: directions, amplitudes and lever offsets sweep strength and energy limits.
        let direction = [Vec3::X, Vec3::Y, Vec3::NEG_Y][k % 3];
        let scale = [0.01, 0.1, 1.0, 10.0][k / 3];
        let origin = match k % 3 {
            0 => Origin::Contact {
                other: Entity::PLACEHOLDER,
                step: 1.0 / 64.0,
            },
            1 => Origin::Body {
                body: Entity::PLACEHOLDER,
                mass,
                speed: 1.0,
                step: 1.0 / 64.0,
            },
            _ => Origin::Field,
        };
        vec![Load {
            chunk: target,
            at: structure.chunks[target].centroid + Vec3::new(0.03, 0.02, 0.01),
            energy: 0.0,
            crushed: 0.0,
            press: direction * (force_scale * scale),
            blow: direction * (impulse_scale * scale),
            peak: (k % 3 == 2).then_some(direction * (force_scale * scale)),
            origin,
        }]
    };
    let mut census = Census::default();
    for k in 0..12 {
        let sources = make_loads(k);
        compare(
            &format!("grounded-{k}"),
            &structure,
            &State::new(&structure),
            &sources,
            Vec3::ZERO,
            &mut census,
        );
    }
    let mut moving = State::new(&structure);
    for (c, chunk) in structure.chunks.iter().enumerate() {
        if chunk.buried {
            moving.fate[c] = Fate::Dust;
        }
    }
    for (j, joint) in structure.joints.iter().enumerate() {
        if joint.b == Side::Ground {
            moving.cracked[j] = true;
        }
    }
    for k in 0..12 {
        let sources = make_loads(k);
        let spin = if k % 2 == 0 {
            Vec3::ZERO
        } else {
            Vec3::new(0.3, 0.7, 0.2)
        };
        compare(
            &format!("moving-{k}"),
            &structure,
            &moving,
            &sources,
            spin,
            &mut census,
        );
    }
    let mut breaking = State::new(&structure);
    for k in 0..12 {
        let sources = make_loads(9 + k % 3);
        let first = compare(
            &format!("first-break-{k}"),
            &structure,
            &breaking,
            &sources,
            Vec3::ZERO,
            &mut census,
        );
        match first {
            Some(first) => {
                println!(
                    "FIRST BREAK {k} {:?} joints={:?}",
                    first.cause, first.joints
                );
                for &j in &first.joints {
                    breaking.cracked[j] = true;
                }
            }
            None => println!("FIRST BREAK {k} chain settled"),
        }
    }
    println!("CENSUS {census:?} aggregate_reads={:?}", COUNTERS.get());
    let sources = make_loads(0);
    timings(&structure, &sources);
    failure_timing(&structure, &breaking, &sources);
}

impl Totals {
    fn subtract(&mut self, other: Self) {
        self.mass -= other.mass;
        self.first -= other.first;
        self.inertia -= other.inertia;
        self.gravity.force -= other.gravity.force;
        self.gravity.moment -= other.gravity.moment;
        self.press.force -= other.press.force;
        self.press.moment -= other.press.moment;
        self.peak.force -= other.peak.force;
        self.peak.moment -= other.peak.moment;
        self.mean.force -= other.mean.force;
        self.mean.moment -= other.mean.moment;
        for (a, b) in self.blows.iter_mut().zip(other.blows) {
            a.force -= b.force;
            a.moment -= b.moment;
        }
        self.inertial_press -= other.inertial_press;
    }
}

fn failure_timing(structure: &Structure, state: &State, sources: &[Load]) {
    let carrier = Carrier::new(structure, state, sources, Vec3::ZERO);
    let child = carrier
        .bridge_child
        .iter()
        .flatten()
        .copied()
        .max_by(|&a, &b| carrier.totals[a].mass.total_cmp(&carrier.totals[b].mass));
    let Some(child) = child else {
        println!("TIMING failure: no single holder in this snapshot");
        return;
    };
    let mut samples = Vec::new();
    let mut size = 0;
    let mut path = 0;
    // Fixture: clone outside the measured interval; the owner already retains this cache.
    for _ in 0..101 {
        let mut parent = carrier.parent.clone();
        let mut totals = carrier.totals.clone();
        let start = std::time::Instant::now();
        let held = totals[child];
        let mut queue = vec![child];
        let mut k = 0;
        while k < queue.len() {
            let c = queue[k];
            k += 1;
            for &(b, edge) in &carrier.adjacency[c] {
                if parent[b] == Some((c, edge)) {
                    queue.push(b);
                }
            }
        }
        let mut next = parent[child];
        parent[child] = None;
        let mut ancestors = 0;
        while let Some((p, _)) = next {
            totals[p].subtract(held);
            ancestors += 1;
            next = parent[p];
        }
        std::hint::black_box((&queue, &parent, &totals));
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        size = queue.len();
        path = ancestors;
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "TIMING bridge_failure_ms min/median/max={:?} hanging_chunks={size} ancestors={path}",
        (
            samples[0],
            samples[samples.len() / 2],
            samples.last().unwrap()
        )
    );
}

/// Complete positive singleton coverage and the instrument's real initial support-removal input.
/// Observations only: this does not pin a construction's emergent result or a timing.
#[test]
#[ignore = "one-time positive-singleton and scheduled-break census"]
fn carrier_positive_and_scheduled_census() {
    let (_, structure) = shipped();
    let intact = State::new(&structure);
    let carrier = Carrier::new(&structure, &intact, &[], Vec3::ZERO);
    let root = structure.chunks.len();
    let mut memberships: Vec<Vec<usize>> = (0..=root).map(|c| vec![c]).collect();
    for &c in carrier.order.iter().rev() {
        if let Some((p, _)) = carrier.parent[c] {
            let children = memberships[c].clone();
            memberships[p].extend(children);
        }
    }
    let chosen = carrier
        .order
        .iter()
        .copied()
        .filter(|&c| {
            c != root
                && carrier.parent[c].is_some_and(|(_, j)| j < structure.joints.len())
                && memberships[c].iter().all(|&c| !structure.chunks[c].buried)
        })
        .max_by_key(|&c| memberships[c].len())
        .unwrap();
    let held: HashSet<_> = memberships[chosen].iter().copied().collect();
    let keep = carrier.parent[chosen].unwrap().1;
    let mut single = intact;
    for (j, joint) in structure.joints.iter().enumerate() {
        let outside = match joint.b {
            Side::Chunk(b) => held.contains(&b) != held.contains(&joint.a),
            Side::Ground => held.contains(&joint.a),
        };
        if outside && j != keep {
            single.cracked[j] = true;
        }
    }
    let chunk = *held
        .iter()
        .max_by(|&&a, &&b| {
            structure.chunks[a]
                .centroid
                .y
                .total_cmp(&structure.chunks[b].centroid.y)
                .then(a.cmp(&b))
        })
        .unwrap();
    let seam = structure.joints[keep].seam().unwrap();
    let force = seam.tensile * structure.joints[keep].area;
    let mass: f32 = memberships[chosen]
        .iter()
        .map(|&c| structure.chunks[c].mass)
        .sum();
    let impulse = (2.0 * mass * seam.fracture_energy * structure.joints[keep].area).sqrt();
    let mut census = Census::default();
    println!(
        "POSITIVE SINGLETON held={} remaining_joint={keep} target={chunk}",
        held.len()
    );
    for k in 0..12 {
        // Fixture: force/energy ratios and directions straddle section limits independently of
        // the selected construction. Every mass, patch and material value comes from its data.
        let direction = [Vec3::X, Vec3::Y, Vec3::NEG_Y][k % 3];
        let scale = [0.01, 0.1, 1.0, 10.0][k / 3];
        let origin = match k % 3 {
            0 => Origin::Contact {
                other: Entity::PLACEHOLDER,
                step: 1.0 / 64.0,
            },
            1 => Origin::Body {
                body: Entity::PLACEHOLDER,
                mass,
                speed: 1.0,
                step: 1.0 / 64.0,
            },
            _ => Origin::Field,
        };
        let sources = vec![Load {
            chunk,
            at: structure.chunks[chunk].centroid + Vec3::new(0.03, 0.02, 0.01),
            energy: 0.0,
            crushed: 0.0,
            press: direction * (force * scale),
            blow: direction * (impulse * scale),
            peak: (k % 3 == 2).then_some(direction * (force * scale)),
            origin,
        }];
        compare(
            &format!("singleton-{k}"),
            &structure,
            &single,
            &sources,
            Vec3::ZERO,
            &mut census,
        );
    }
    failure_timing(&structure, &single, &[]);
    // Same support selector and input formula as net::loadtest::drive: every ground anchor's
    // chunk and every buried chunk, sorted; energy 2 * V * crush at the chunk centroid.
    let supports: std::collections::BTreeSet<_> = structure
        .chunks
        .iter()
        .enumerate()
        .filter(|(c, chunk)| {
            chunk.buried
                || structure
                    .joints
                    .iter()
                    .any(|j| j.a == *c && j.b == Side::Ground)
        })
        .map(|(c, _)| c)
        .collect();
    let sources: Vec<_> = supports
        .iter()
        .flat_map(|&c| {
            let chunk = &structure.chunks[c];
            Load::line(
                c,
                &chunk.material,
                chunk.centroid,
                chunk.centroid,
                2.0 * chunk.volume * chunk.material.crush,
                Vec3::ZERO,
            )
        })
        .collect();
    let mut broken = State::new(&structure);
    let struck = strike(&structure, &mut broken, &sources);
    println!(
        "SCHEDULED INITIAL supports={} dusted={} cracked={}",
        supports.len(),
        struck.dusted.len(),
        struck.cracked.len()
    );
    compare_changed(
        "scheduled-first-break",
        &structure,
        &broken,
        &sources,
        Vec3::ZERO,
        (&struck.dusted, &struck.cracked),
        &mut census,
    );
    println!(
        "POSITIVE CENSUS {census:?} aggregate_reads={:?}",
        COUNTERS.get()
    );
}

fn press_timing(carrier: Carrier, structure: &Structure) {
    let mut carrier = carrier;
    let mut depths = vec![0; carrier.parent.len()];
    for &c in &carrier.order {
        if let Some((p, _)) = carrier.parent[c] {
            depths[c] = depths[p] + 1;
        }
    }
    let c = (0..structure.chunks.len())
        .max_by_key(|&c| depths[c])
        .unwrap();
    let delta = Wrench::point(structure.chunks[c].centroid, Vec3::X);
    let minus = Wrench {
        force: -delta.force,
        moment: -delta.moment,
    };
    let mut forward = true;
    let changed = timed(
        || {
            let change = if forward { delta } else { minus };
            std::hint::black_box(carrier.press_delta(c, change));
            std::hint::black_box(&carrier.totals);
            forward = !forward;
        },
        1001,
    );
    println!(
        "TIMING changed_press_ms={changed:?} path_nodes={}",
        depths[c] + 1
    );
}

#[test]
#[ignore = "one-time observable press-update timing"]
fn carrier_press_probe() {
    let (_, structure) = shipped();
    let carrier = Carrier::new(&structure, &State::new(&structure), &[], Vec3::ZERO);
    press_timing(carrier, &structure);
}
