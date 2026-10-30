//! The wall prototype's model, about the game's environment destruction (game state only): a
//! structure, what a deposit of energy does to it, and the crack law.
//!
//! A structure is a chunk table ([`Chunk`]: volume, centroid, mass, material) and a joint graph
//! ([`Joint`]: the pair, contact area, centroid and normal; a [`Side::Ground`] joint is a ground
//! anchor). Removal, the crack law and support read only those two; the bake's convex hulls
//! ([`Block`]) serve the view, the colliders and the geometry probes — a blast's rays
//! ([`blast_rays`]) — never a law.
//!
//! * **Removal.** A chunk's pool is its crush energy, `crush × volume` (J); a deposit wears it,
//!   and a chunk whose pool is spent is turned to dust. A line deposit (a round's energy loss in a
//!   chunk) wears that chunk ([`wear`]).
//! * **Cracks.** Each live joint's load from the deposit's [`Source`]s is compared with the
//!   joint's tensile strength; a joint over it is cracked loose ([`crack`]). The load is the
//!   elastic stress about the cavity a source crushed. Sources superpose; a joint does not
//!   remember a load below its threshold. Griffith's strength for the joint, `√(E_m G_f / (π a))`
//!   with the joint's own size as the flaw, gives the same reach: 0.41 m against this law's
//!   0.36 m for a lone 278 kJ deposit (DERIVED).
//! * **Structure.** A push on a group that one section alone holds to the ground fails the
//!   section when its angular impulse would spend the section's fracture energy ([`structural`]).
//!   The check ignores the wall's own mass: a known gap.
//! * **Support.** A chunk group with no path of intact joints to a ground anchor falls as one
//!   body ([`loose_groups`]).
//!
//! The README beside the captures (`.agents/scratch/destruction-2026-10-30-buildings/
//! wall-prototype/README.md`) owns the derivations, the values' sources and the verdicts.

use std::f32::consts::PI;

use bevy::math::Vec3;

/// One material's values, as the laws read them. Every value's source is the README's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Masonry {
    /// kg/m³.
    pub density: f32,
    /// The unit's compressive strength, Pa.
    pub compressive: f32,
    /// The energy that turns one cubic metre to dust, J/m³.
    pub crush: f32,
    /// The joints' tensile strength, Pa.
    pub joint_tensile: f32,
    /// The joints' mode-I fracture energy, J/m².
    pub joint_toughness: f32,
    /// The masonry's Young's modulus, Pa.
    pub modulus: f32,
}

impl Masonry {
    /// The elastic wave speed `√(E/ρ)`, m/s.
    pub fn wave_speed(&self) -> f32 {
        (self.modulus / self.density).sqrt()
    }
}

/// One chunk of the table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Chunk {
    /// m³.
    pub volume: f32,
    pub centroid: Vec3,
    /// kg.
    pub mass: f32,
    pub material: Masonry,
}

/// The far side of a joint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Chunk(usize),
    /// A ground anchor.
    Ground,
}

/// One joint of the graph: chunk `a` against `b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Joint {
    pub a: usize,
    pub b: Side,
    /// m².
    pub area: f32,
    pub centroid: Vec3,
    /// Unit normal of the contact plane; its sign carries nothing.
    pub normal: Vec3,
}

/// A chunk table and its joint graph.
#[derive(Clone, Debug, Default)]
pub(crate) struct Structure {
    pub chunks: Vec<Chunk>,
    pub joints: Vec<Joint>,
}

/// A convex chunk shape, world metres: the bake's hull corners, the planes bounding it (outward
/// unit normal `n` and offset `o`, a point `x` inside when `n · x ≤ o`), and its bounding box.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Block {
    pub points: Vec<Vec3>,
    pub planes: Vec<(Vec3, f32)>,
    pub min: Vec3,
    pub max: Vec3,
}

/// How far a point may sit off a plane and still lie on it, m.
const ON_PLANE: f32 = 1e-4;

impl Block {
    /// The convex hull of `points`: its bounding planes are every plane through three corners
    /// that has all the corners on one side.
    pub fn hull(points: Vec<Vec3>) -> Self {
        let mut corners: Vec<Vec3> = Vec::new();
        for point in points {
            if corners.iter().all(|c| c.distance(point) > ON_PLANE) {
                corners.push(point);
            }
        }
        let mut planes: Vec<(Vec3, f32)> = Vec::new();
        let count = corners.len();
        for i in 0..count {
            for j in i + 1..count {
                for k in j + 1..count {
                    let normal = (corners[j] - corners[i]).cross(corners[k] - corners[i]);
                    if normal.length() < 1e-9 {
                        continue;
                    }
                    let normal = normal.normalize();
                    let offset = normal.dot(corners[i]);
                    let (mut above, mut below) = (false, false);
                    for corner in &corners {
                        let side = normal.dot(*corner) - offset;
                        above |= side > ON_PLANE;
                        below |= side < -ON_PLANE;
                    }
                    let plane = match (above, below) {
                        (false, true) => (normal, offset),
                        (true, false) => (-normal, -offset),
                        _ => continue,
                    };
                    if planes
                        .iter()
                        .all(|(n, o)| n.dot(plane.0) < 0.9999 || (o - plane.1).abs() > ON_PLANE)
                    {
                        planes.push(plane);
                    }
                }
            }
        }
        let min = corners
            .iter()
            .fold(Vec3::splat(f32::INFINITY), |a, b| a.min(*b));
        let max = corners
            .iter()
            .fold(Vec3::splat(f32::NEG_INFINITY), |a, b| a.max(*b));
        Self {
            points: corners,
            planes,
            min,
            max,
        }
    }

    /// The axis-aligned box `min..max`.
    pub fn cuboid(min: Vec3, max: Vec3) -> Self {
        let corner = |x: bool, y: bool, z: bool| {
            Vec3::new(
                if x { max.x } else { min.x },
                if y { max.y } else { min.y },
                if z { max.z } else { min.z },
            )
        };
        let mut points = Vec::new();
        for x in [false, true] {
            for y in [false, true] {
                for z in [false, true] {
                    points.push(corner(x, y, z));
                }
            }
        }
        Self::hull(points)
    }

    /// The bounding box's centre.
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) / 2.0
    }

    /// The distance from the centroid of the corners to the nearest bounding plane, m.
    pub fn inradius(&self) -> f32 {
        let centre = self.points.iter().sum::<Vec3>() / self.points.len().max(1) as f32;
        self.planes
            .iter()
            .map(|(n, o)| o - n.dot(centre))
            .fold(f32::INFINITY, f32::min)
    }

    /// Where a ray from `origin` along unit `dir` enters this shape: its distance, `None` when it
    /// misses or the shape lies behind. A ray starting inside enters at 0.
    pub fn entry(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        let (mut near, mut far) = (0.0_f32, f32::INFINITY);
        for &(n, o) in &self.planes {
            let rate = n.dot(dir);
            let gap = o - n.dot(origin);
            if rate.abs() < 1e-9 {
                if gap < 0.0 {
                    return None;
                }
            } else if rate < 0.0 {
                near = near.max(gap / rate);
            } else {
                far = far.min(gap / rate);
            }
        }
        (near <= far).then_some(near)
    }

    /// The corners on the plane through `at` of unit normal `normal`, ordered counter-clockwise
    /// about `normal`: the face the shape shows there. Empty when fewer than three lie on it.
    pub fn face_on(&self, at: Vec3, normal: Vec3) -> Vec<Vec3> {
        let on: Vec<Vec3> = self
            .points
            .iter()
            .copied()
            .filter(|p| normal.dot(*p - at).abs() < ON_PLANE * 10.0)
            .collect();
        if on.len() < 3 {
            return Vec::new();
        }
        let centre = on.iter().sum::<Vec3>() / on.len() as f32;
        let (u, v) = plane_axes(normal);
        let mut on = on;
        on.sort_by(|a, b| {
            let angle = |p: &Vec3| (*p - centre).dot(v).atan2((*p - centre).dot(u));
            angle(a).total_cmp(&angle(b))
        });
        on
    }
}

/// Two unit axes spanning the plane of unit `normal`, right-handed with it.
pub(crate) fn plane_axes(normal: Vec3) -> (Vec3, Vec3) {
    let t = if normal.x.abs() < 0.9 {
        Vec3::X
    } else {
        Vec3::Y
    };
    let u = normal.cross(t).normalize();
    (u, normal.cross(u))
}

/// The convex polygon both `a` and `b` (corners counter-clockwise about `normal`, in one plane)
/// cover: `a` clipped by each edge of `b`.
pub(crate) fn clip_polygon(a: &[Vec3], b: &[Vec3], normal: Vec3) -> Vec<Vec3> {
    let mut out = a.to_vec();
    for k in 0..b.len() {
        let (from, to) = (b[k], b[(k + 1) % b.len()]);
        // Inside is the left of the edge, `normal × edge` pointing inward.
        let inward = normal.cross(to - from);
        let side = |p: Vec3| inward.dot(p - from);
        let input = std::mem::take(&mut out);
        for i in 0..input.len() {
            let (p, q) = (input[i], input[(i + 1) % input.len()]);
            let (sp, sq) = (side(p), side(q));
            if sp >= 0.0 {
                out.push(p);
            }
            if (sp >= 0.0) != (sq >= 0.0) {
                out.push(p + (q - p) * (sp / (sp - sq)));
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

/// A cut: the structure the laws read, and per chunk and joint what the view draws.
#[derive(Clone, Debug)]
pub(crate) struct Cut {
    pub structure: Structure,
    /// Each chunk's shape, index-aligned with the table.
    pub blocks: Vec<Block>,
    /// Each joint's contact polygon, index-aligned with the joints.
    pub faces: Vec<Vec<Vec3>>,
}

/// What became of a chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fate {
    Standing,
    /// Turned to dust: its pool was spent.
    Dust,
    /// Fell with a loose group.
    Fallen,
}

/// A structure's live state: each chunk's pool (J) and fate, each joint's crack and whether the
/// structural check ([`structural`]) cracked it rather than a crack law.
#[derive(Clone, Debug)]
pub(crate) struct WallState {
    pub pool: Vec<f32>,
    pub fate: Vec<Fate>,
    pub cracked: Vec<bool>,
    pub structural: Vec<bool>,
}

impl WallState {
    /// Every chunk standing with a full pool, every joint intact.
    pub fn new(structure: &Structure) -> Self {
        Self {
            pool: structure.chunks.iter().map(full_pool).collect(),
            fate: vec![Fate::Standing; structure.chunks.len()],
            cracked: vec![false; structure.joints.len()],
            structural: vec![false; structure.joints.len()],
        }
    }

    pub fn standing(&self, chunk: usize) -> bool {
        self.fate[chunk] == Fate::Standing
    }

    /// Whether `joint` still joins standing matter: its chunk and, unless it is an anchor, the
    /// chunk across it.
    pub fn live(&self, joint: &Joint) -> bool {
        self.standing(joint.a)
            && match joint.b {
                Side::Chunk(b) => self.standing(b),
                Side::Ground => true,
            }
    }
}

/// A chunk's full pool, J: its crush energy over its volume.
pub(crate) fn full_pool(chunk: &Chunk) -> f32 {
    chunk.material.crush * chunk.volume
}

/// Wear `chunk`'s pool by `energy` J; a pool spent to a millionth of full turns it to dust.
/// Returns whether it did now.
pub(crate) fn wear(
    structure: &Structure,
    state: &mut WallState,
    chunk: usize,
    energy: f32,
) -> bool {
    if !state.standing(chunk) {
        return false;
    }
    state.pool[chunk] = (state.pool[chunk] - energy).max(0.0);
    let dust = state.pool[chunk] <= 1e-6 * full_pool(&structure.chunks[chunk]);
    if dust {
        state.fate[chunk] = Fate::Dust;
    }
    dust
}

/// One point of a deposit: `energy` J at `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Source {
    pub energy: f32,
    pub at: Vec3,
}

impl Source {
    /// The radius of the ball this source alone crushes, m: where the field is clamped, since
    /// nothing inside it is left to load.
    fn crushed(&self, material: &Masonry) -> f32 {
        (3.0 * self.energy / (4.0 * PI * material.crush)).cbrt()
    }

    /// Distance and unit direction from `from` to `x`, the distance clamped at the crushed
    /// radius.
    fn ray(&self, from: Vec3, x: Vec3, material: &Masonry) -> (f32, Vec3) {
        let offset = x - from;
        let length = offset.length();
        let dir = if length > 1e-9 {
            offset / length
        } else {
            Vec3::ZERO
        };
        (length.max(self.crushed(material)), dir)
    }

    /// The normal stress on a plane of normal `n` at `x`, Pa, tension positive: the Lamé field of a
    /// spherical cavity the deposit crushed, at pressure the crush energy (`P r_c³ = 3E/4π`),
    /// radial `−3E/(4π d³)` and hoop `+3E/(8π d³)`.
    fn lame(&self, x: Vec3, n: Vec3, material: &Masonry) -> f32 {
        let (d, dir) = self.ray(self.at, x, material);
        let cos = n.dot(dir);
        3.0 * self.energy / (8.0 * PI * d.powi(3)) * (1.0 - 3.0 * cos * cos)
    }
}

/// Seven points of a disc of `joint`'s area in its plane, with weights summing to 1: the centre at
/// ¼ and six on the ring at `√(2/3)` of its radius at ⅛ each.
fn disc(joint: &Joint) -> [(Vec3, f32); 7] {
    let (u, v) = plane_axes(joint.normal);
    let ring = (joint.area / PI).sqrt() * (2.0_f32 / 3.0).sqrt();
    let mut points = [(joint.centroid, 0.25); 7];
    for (k, point) in points.iter_mut().skip(1).enumerate() {
        let angle = k as f32 * PI / 3.0;
        *point = (
            joint.centroid + (u * angle.cos() + v * angle.sin()) * ring,
            0.125,
        );
    }
    points
}

/// The load the crack law puts on `joint` from `sources` (Pa, the mean of the Lame normal stress
/// over the joint's disc), and the threshold it is compared to: the joint's tensile strength.
pub(crate) fn joint_load(joint: &Joint, material: &Masonry, sources: &[Source]) -> (f32, f32) {
    let load = disc(joint)
        .iter()
        .map(|(x, weight)| {
            weight
                * sources
                    .iter()
                    .map(|s| s.lame(*x, joint.normal, material))
                    .sum::<f32>()
        })
        .sum();
    (load, material.joint_tensile)
}

/// Crack every live, intact joint whose load from `sources` reaches its threshold; returns the
/// joints cracked now. A joint's material is its chunk `a`'s.
pub(crate) fn crack(
    structure: &Structure,
    state: &mut WallState,
    sources: &[Source],
) -> Vec<usize> {
    if sources.is_empty() {
        return Vec::new();
    }
    let mut cracked = Vec::new();
    for (index, joint) in structure.joints.iter().enumerate() {
        if state.cracked[index] || !state.live(joint) {
            continue;
        }
        let material = structure.chunks[joint.a].material;
        let (load, threshold) = joint_load(joint, &material, sources);
        if load >= threshold {
            state.cracked[index] = true;
            cracked.push(index);
        }
    }
    cracked
}

/// The groups of standing chunks with no path of intact joints to a ground anchor, each a list of
/// chunk indices, ascending, the groups ordered by their first chunk. Reads the joint graph only.
pub(crate) fn loose_groups(structure: &Structure, state: &WallState) -> Vec<Vec<usize>> {
    let count = structure.chunks.len();
    let mut neighbours = vec![Vec::new(); count];
    let mut grounded = vec![false; count];
    for (index, joint) in structure.joints.iter().enumerate() {
        if state.cracked[index] || !state.live(joint) {
            continue;
        }
        match joint.b {
            Side::Chunk(b) => {
                neighbours[joint.a].push(b);
                neighbours[b].push(joint.a);
            }
            Side::Ground => grounded[joint.a] = true,
        }
    }
    let mut seen = vec![false; count];
    let flood = |start: Vec<usize>, seen: &mut Vec<bool>| -> Vec<usize> {
        let mut stack = start;
        let mut reached = Vec::new();
        while let Some(chunk) = stack.pop() {
            if seen[chunk] {
                continue;
            }
            seen[chunk] = true;
            reached.push(chunk);
            stack.extend(neighbours[chunk].iter().copied().filter(|n| !seen[*n]));
        }
        reached.sort_unstable();
        reached
    };
    flood(
        (0..count)
            .filter(|i| grounded[*i] && state.standing(*i))
            .collect(),
        &mut seen,
    );
    let mut groups = Vec::new();
    for chunk in 0..count {
        if !seen[chunk] && state.standing(chunk) {
            groups.push(flood(vec![chunk], &mut seen));
        }
    }
    groups
}

/// The first standing block a ray from `origin` along unit `dir` enters: its index and distance.
pub(crate) fn first_hit(
    blocks: &[Block],
    standing: impl Fn(usize) -> bool,
    origin: Vec3,
    dir: Vec3,
) -> Option<(usize, f32)> {
    blocks
        .iter()
        .enumerate()
        .filter(|(i, _)| standing(*i))
        .filter_map(|(i, block)| block.entry(origin, dir).map(|t| (i, t)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// One push on a chunk: the impulse (N·s) delivered at `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Push {
    pub chunk: usize,
    pub at: Vec3,
    pub impulse: Vec3,
}

/// One blast ray's arrival: the chunk it stopped on, where, its energy (J) and impulse (N·s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RayHit {
    pub chunk: usize,
    pub at: Vec3,
    pub energy: f32,
    pub impulse: Vec3,
}

/// A blast field's rays from `origin`: each of `directions` (unit, with its share of the field;
/// the shares sum to 1) carries that share of `energy` (J) and of `momentum` (N·s), and stops on
/// the first standing block it meets, which shadows everything behind it. A ray that meets none
/// leaves. Coupling is the geometry's: the share of directions the blocks intercept.
pub(crate) fn blast_rays(
    blocks: &[Block],
    standing: impl Fn(usize) -> bool,
    origin: Vec3,
    energy: f32,
    momentum: f32,
    directions: &[(Vec3, f32)],
) -> Vec<RayHit> {
    directions
        .iter()
        .filter_map(|&(dir, share)| {
            first_hit(blocks, &standing, origin, dir).map(|(chunk, t)| RayHit {
                chunk,
                at: origin + dir * t,
                energy: energy * share,
                impulse: dir * (momentum * share),
            })
        })
        .collect()
}

/// One structural failure: the section's joints, the angular impulse bending it (N·s·m) and the
/// capacity it was compared with.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Failure {
    pub joints: Vec<usize>,
    pub demand: f32,
    pub capacity: f32,
}

/// Which standing chunks reach a ground anchor through intact live joints, `excluded` taken as
/// cracked.
fn grounded(structure: &Structure, state: &WallState, excluded: &[usize]) -> Vec<bool> {
    let count = structure.chunks.len();
    let mut neighbours = vec![Vec::new(); count];
    let mut stack = Vec::new();
    for (index, joint) in structure.joints.iter().enumerate() {
        if state.cracked[index] || !state.live(joint) || excluded.contains(&index) {
            continue;
        }
        match joint.b {
            Side::Chunk(b) => {
                neighbours[joint.a].push(b);
                neighbours[b].push(joint.a);
            }
            Side::Ground => stack.push(joint.a),
        }
    }
    let mut reached = vec![false; count];
    while let Some(chunk) = stack.pop() {
        if reached[chunk] {
            continue;
        }
        reached[chunk] = true;
        stack.extend(neighbours[chunk].iter().copied().filter(|n| !reached[*n]));
    }
    reached
}

/// The sections a structural check tries: every live intact joint alone, and every set of two or
/// more live intact joints lying in one plane (normals parallel, offsets within a millimetre).
/// Read from the joint graph alone: a cut with no common planes offers single joints only.
fn sections(structure: &Structure, state: &WallState) -> Vec<Vec<usize>> {
    let mut planes: Vec<(Vec3, f32, Vec<usize>)> = Vec::new();
    let mut singles = Vec::new();
    for (index, joint) in structure.joints.iter().enumerate() {
        if state.cracked[index] || !state.live(joint) {
            continue;
        }
        singles.push(vec![index]);
        let n = joint.normal;
        let n = if n.max_element().abs() >= n.min_element().abs() {
            n
        } else {
            -n
        };
        let offset = n.dot(joint.centroid);
        match planes
            .iter_mut()
            .find(|(m, o, _)| m.dot(n) > 0.999 && (o - offset).abs() < 1e-3)
        {
            Some((.., members)) => members.push(index),
            None => planes.push((n, offset, vec![index])),
        }
    }
    singles.extend(
        planes
            .into_iter()
            .filter(|(.., members)| members.len() > 1)
            .map(|(.., members)| members),
    );
    singles
}

/// The structural check: a push bends a section about its centroid, and the section fails when
/// the angular impulse in its plane, `|Σ (x − c) × J|` over the pushes on the group it alone holds
/// to the ground, reaches the impulse that would spend its fracture energy on opening it,
/// `√(2 I G_f A)`: `I` the group's moment of inertia about the bending axis through the section's
/// centroid (each chunk a ball of its volume), `A` the section's area, `G_f` its chunks' joint
/// toughness. The worst failing section cracks first, and the check repeats until none fails, so a
/// failure can expose the next. Reads the chunk table and the joint graph only.
pub(crate) fn structural(
    structure: &Structure,
    state: &mut WallState,
    pushes: &[Push],
) -> Vec<Failure> {
    let mut failures = Vec::new();
    if pushes.is_empty() {
        return failures;
    }
    for _ in 0..64 {
        let before = grounded(structure, state, &[]);
        let mut worst: Option<Failure> = None;
        for section in sections(structure, state) {
            let after = grounded(structure, state, &section);
            let held: Vec<usize> = (0..structure.chunks.len())
                .filter(|&i| before[i] && !after[i])
                .collect();
            if held.is_empty() {
                continue;
            }
            let area: f32 = section.iter().map(|&j| structure.joints[j].area).sum();
            let centre = section
                .iter()
                .map(|&j| structure.joints[j].centroid * structure.joints[j].area)
                .sum::<Vec3>()
                / area;
            let n = structure.joints[section[0]].normal;
            let moment: Vec3 = pushes
                .iter()
                .filter(|push| held.binary_search(&push.chunk).is_ok())
                .map(|push| (push.at - centre).cross(push.impulse))
                .sum();
            let bending = moment - n * moment.dot(n);
            let demand = bending.length();
            if demand <= 0.0 {
                continue;
            }
            let axis = bending / demand;
            let inertia: f32 = held
                .iter()
                .map(|&i| {
                    let chunk = &structure.chunks[i];
                    let r = chunk.centroid - centre;
                    let own = (3.0 * chunk.volume / (4.0 * PI)).cbrt();
                    chunk.mass * (r.length_squared() - r.dot(axis).powi(2) + 0.4 * own * own)
                })
                .sum();
            let toughness = structure.chunks[structure.joints[section[0]].a]
                .material
                .joint_toughness;
            let capacity = (2.0 * inertia * toughness * area).sqrt();
            let ratio = demand / capacity;
            if ratio >= 1.0 && worst.as_ref().is_none_or(|w| ratio > w.demand / w.capacity) {
                worst = Some(Failure {
                    joints: section,
                    demand,
                    capacity,
                });
            }
        }
        let Some(failure) = worst else {
            break;
        };
        for &j in &failure.joints {
            state.cracked[j] = true;
            state.structural[j] = true;
        }
        failures.push(failure);
    }
    failures
}
