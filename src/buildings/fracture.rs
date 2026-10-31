//! The buildings' fracture laws, about the game's environment destruction (game state only): a
//! structure, what a deposit of energy does to it, the structural check, support and the blast
//! field. One home for the laws the armour sandbox's wall prototype adopted (`sandbox::wall` calls
//! these).
//!
//! A structure is a chunk table ([`Chunk`]: volume, centroid, mass, material) and a joint graph
//! ([`Joint`]: the pair, contact area, centroid and normal; a [`Side::Ground`] joint is a ground
//! anchor). The laws read only those two; a chunk's convex hull ([`Block`]) serves the colliders,
//! the views and the geometry probes that say which chunk a deposit landed in, never a law.
//!
//! * **Removal** ([`wear`]). A chunk's pool is its crush energy, `crush × volume` (J); a deposit
//!   wears it, and a chunk whose pool is spent is turned to dust.
//! * **Cracks: the push rule** ([`crack`]). Each live joint's load from the deposit's [`Source`]s
//!   is compared with the seam's tensile strength; a joint at or over it is cracked loose. The
//!   load is the elastic stress about the cavity a source crushed. Sources superpose; a joint does
//!   not remember a load below its threshold.
//! * **Structure** ([`structural`]). A push on a group that one section alone holds to the ground
//!   cracks the section when its angular impulse would spend the section's fracture energy.
//! * **Weight** ([`weigh`]). After a wall changes, each group that one section alone holds and
//!   whose support the change moved cracks the section when its weight, as a direct pull and a
//!   bending moment across it, reaches the seam's tensile strength. A part held through several
//!   sections at once (a span between two piers) is left standing and counted: the design's step 3
//!   (`.agents/docs/design/wall-destruction.md`, "Structural calculation").
//! * **Support** ([`loose_groups`]). A chunk group with no path of intact joints to a ground anchor
//!   or to a buried chunk ([`Chunk::buried`]: the soil holds it) falls as one body.
//! * **The blast field** ([`blast_rays`]). A charge's blast energy leaves it uniformly over the
//!   sphere; each ray stops on the first standing chunk it meets and pushes the face it meets with
//!   the blast's reflected impulse over the patch it stands for ([`Blast::push`]). The wall design
//!   (`.agents/docs/design/wall-destruction.md`, "Standoff blast") owns the model.
//!
//! A deposit becomes point sources ([`Loading`]): a round's line in each chunk is split every
//! [`LINE_STEP`]; a field's rays give one source per chunk they stop on, at the energy-weighted mean
//! of their stops. Gaps these laws leave open (the wall prototype's README and the 2026-10-30 design
//! review): a chunk partly crushed stays whole, the share of a round's lost momentum the crushed
//! brick carries away is unknown (the struck chunks keep all of it), and a blast is an impulse, not
//! a pressure over time. The README beside the prototype's captures
//! (`.agents/scratch/destruction-2026-10-30-buildings/wall-prototype/README.md`) owns the
//! derivations, the values' sources and the verdicts.

use std::f32::consts::PI;

use bevy::math::Vec3;

use crate::substances::{REFERENCE_FACTOR, SubstanceRegistry};

/// Spacing of the point sources a line deposit is split into along its chord, m: a quarter of a
/// 20 cm chunk (Resolution).
pub(crate) const LINE_STEP: f32 = 0.05;

/// One material's values, as the laws read them, all from its substance row
/// ([`Masonry::of`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Masonry {
    /// kg/m³.
    pub density: f32,
    /// The energy that turns one cubic metre to dust, J/m³.
    pub crush: f32,
    /// The tensile strength across a seam, Pa.
    pub tensile: f32,
    /// The seam's mode-I fracture energy, J/m².
    pub fracture_energy: f32,
}

impl Masonry {
    /// Substance `name`'s values, or `None` where its row states no seam (an element of it never
    /// breaks). The density and the seam's strength and fracture energy are the row's bulk facts
    /// (the registry refuses a row stating one of the seam's two without the other); the crush
    /// energy is the static term of the walk's own force law at the row's factor (`factor / 1000 ×
    /// a`, `a` the reference's cavity strength): the energy per volume the walk spends opening a
    /// cavity in it. Refuses a name the registry does not hold as hard matter.
    pub(crate) fn of(registry: &SubstanceRegistry, name: &str) -> Result<Option<Self>, String> {
        let hard = registry.hard(name).map_err(|err| err.to_string())?;
        let Some((bulk, tensile, fracture_energy)) = hard.bulk.and_then(|bulk| {
            bulk.tensile
                .zip(bulk.fracture_energy)
                .map(|(tensile, energy)| (bulk, tensile, energy))
        }) else {
            return Ok(None);
        };
        let share = f64::from(hard.factor() / REFERENCE_FACTOR);
        Ok(Some(Self {
            density: bulk.density as f32,
            crush: (share * registry.reference_facts().cavity_strength()) as f32,
            tensile: tensile as f32,
            fracture_energy: fracture_energy as f32,
        }))
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
    /// Held by the soil: its hull lies wholly below grade, the plane its building stands on. The
    /// soil bears it as an anchor does and no crack frees it, so a group still joined to it stands
    /// and no body is ever released inside the ground.
    pub buried: bool,
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

/// A convex chunk shape: the bake's hull corners, the planes bounding it (outward unit normal `n`
/// and offset `o`, a point `x` inside when `n · x ≤ o`), and its bounding box.
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
        self.span(origin, dir).map(|(near, _)| near)
    }

    /// Where a ray from `origin` along unit `dir` enters this shape, and the outward unit normal
    /// of the face it enters through; `None` when it misses or the shape lies behind. A ray
    /// starting inside enters at 0 through no face, and reads as meeting one square on (`-dir`).
    pub fn entry_face(&self, origin: Vec3, dir: Vec3) -> Option<(f32, Vec3)> {
        self.cut(origin, dir).map(|(near, _, face)| (near, face))
    }

    /// The stretch of the ray from `origin` along unit `dir` inside this shape, `(enter, leave)`
    /// distances, `None` when it misses or the shape lies behind. A ray starting inside enters at
    /// 0.
    pub fn span(&self, origin: Vec3, dir: Vec3) -> Option<(f32, f32)> {
        self.cut(origin, dir).map(|(near, far, _)| (near, far))
    }

    /// [`Self::span`], with the outward normal of the plane the ray enters through (`-dir` when it
    /// starts inside).
    fn cut(&self, origin: Vec3, dir: Vec3) -> Option<(f32, f32, Vec3)> {
        let (mut near, mut far, mut face) = (0.0_f32, f32::INFINITY, -dir);
        for &(n, o) in &self.planes {
            let rate = n.dot(dir);
            let gap = o - n.dot(origin);
            if rate.abs() < 1e-9 {
                if gap < 0.0 {
                    return None;
                }
            } else if rate < 0.0 {
                if gap / rate > near {
                    near = gap / rate;
                    face = n;
                }
            } else {
                far = far.min(gap / rate);
            }
        }
        (near <= far).then_some((near, far, face))
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

    /// Every face, as its outward unit normal and its corners counter-clockwise seen from outside.
    pub fn faces(&self) -> Vec<(Vec3, Vec<Vec3>)> {
        self.planes
            .iter()
            .map(|&(normal, offset)| (normal, self.face_on(normal * offset, normal)))
            .filter(|(_, corners)| corners.len() >= 3)
            .collect()
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

/// What became of a chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fate {
    Standing,
    /// Turned to dust: its pool was spent.
    Dust,
    /// Fell with a loose group.
    Fallen,
}

/// A structure's live state: each chunk's pool (J) and fate, and each joint's crack.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WallState {
    pub pool: Vec<f32>,
    pub fate: Vec<Fate>,
    pub cracked: Vec<bool>,
}

impl WallState {
    /// Every chunk standing with a full pool, every joint intact.
    pub fn new(structure: &Structure) -> Self {
        Self {
            pool: structure.chunks.iter().map(full_pool).collect(),
            fate: vec![Fate::Standing; structure.chunks.len()],
            cracked: vec![false; structure.joints.len()],
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
    /// The radius of the ball this source alone crushes in `material`, m: where the field is
    /// clamped, since nothing inside it is left to load.
    fn crushed(&self, material: &Masonry) -> f32 {
        (3.0 * self.energy / (4.0 * PI * material.crush)).cbrt()
    }

    /// The normal stress on a plane of normal `n` at `x`, Pa, tension positive: the Lamé field of a
    /// spherical cavity the deposit crushed, at pressure the crush energy (`P r_c³ = 3E/4π`),
    /// radial `−3E/(4π d³)` and hoop `+3E/(8π d³)`, the distance clamped at `crushed`, this
    /// source's crushed radius in the joint's material ([`Source::crushed`]).
    fn lame(&self, x: Vec3, n: Vec3, crushed: f32) -> f32 {
        let offset = x - self.at;
        let length = offset.length();
        let dir = if length > 1e-9 {
            offset / length
        } else {
            Vec3::ZERO
        };
        let d = length.max(crushed);
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

/// Each source's crushed radius in `material`, in source order ([`Source::crushed`]).
fn crushed_radii(sources: &[Source], material: &Masonry) -> Vec<f32> {
    sources.iter().map(|s| s.crushed(material)).collect()
}

/// The load the push rule puts on `joint` from `sources` (Pa, the mean of the Lamé normal stress
/// over the joint's disc), each source clamped at its radius in `crushed` (its crushed radius in
/// the joint's `material`, [`crushed_radii`]), and the threshold it is compared to: the seam's
/// tensile strength.
fn joint_load(
    joint: &Joint,
    material: &Masonry,
    sources: &[Source],
    crushed: &[f32],
) -> (f32, f32) {
    let load = disc(joint)
        .iter()
        .map(|(x, weight)| {
            weight
                * sources
                    .iter()
                    .zip(crushed)
                    .map(|(s, &radius)| s.lame(*x, joint.normal, radius))
                    .sum::<f32>()
        })
        .sum();
    (load, material.tensile)
}

/// Crack every live, intact joint whose load from `sources` reaches its threshold
/// ([`joint_load`]); returns the joints cracked now, ascending. A joint's material is its chunk
/// `a`'s. The sources' crushed radii are taken once per crush energy the joints meet, keyed on its
/// bits, not at each disc point of each joint: the same expression of the same values.
pub(crate) fn crack(
    structure: &Structure,
    state: &mut WallState,
    sources: &[Source],
) -> Vec<usize> {
    if sources.is_empty() {
        return Vec::new();
    }
    let mut radii: Vec<(u32, Vec<f32>)> = Vec::new();
    let mut cracked = Vec::new();
    for (index, joint) in structure.joints.iter().enumerate() {
        if state.cracked[index] || !state.live(joint) {
            continue;
        }
        let material = structure.chunks[joint.a].material;
        let key = material.crush.to_bits();
        let at = match radii.iter().position(|(crush, _)| *crush == key) {
            Some(at) => at,
            None => {
                radii.push((key, crushed_radii(sources, &material)));
                radii.len() - 1
            }
        };
        let (load, threshold) = joint_load(joint, &material, sources, &radii[at].1);
        if load >= threshold {
            state.cracked[index] = true;
            cracked.push(index);
        }
    }
    cracked
}

/// The groups of standing chunks with no path of intact joints to a ground anchor or a buried
/// chunk, each a list of chunk indices, ascending, the groups ordered by their first chunk. Reads
/// the joint graph and the burial only: no float enters, so every peer that holds the same fates
/// and cracks finds the same groups.
pub(crate) fn loose_groups(structure: &Structure, state: &WallState) -> Vec<Vec<usize>> {
    let count = structure.chunks.len();
    let mut neighbours = vec![Vec::new(); count];
    let mut grounded: Vec<bool> = structure.chunks.iter().map(|chunk| chunk.buried).collect();
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

/// The first standing block a ray from `origin` along unit `dir` enters: its index, distance and
/// the outward normal of the face it enters through ([`Block::entry_face`]).
pub(crate) fn first_hit(
    blocks: &[Block],
    standing: impl Fn(usize) -> bool,
    origin: Vec3,
    dir: Vec3,
) -> Option<(usize, f32, Vec3)> {
    blocks
        .iter()
        .enumerate()
        .filter(|(i, _)| standing(*i))
        .filter_map(|(i, block)| block.entry_face(origin, dir).map(|(t, n)| (i, t, n)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// One deposit as the laws read it: its point sources, and the energy (J) each chunk took.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Loading {
    pub sources: Vec<Source>,
    pub energies: Vec<(usize, f32)>,
}

impl Loading {
    /// A round's lines, each `(chunk, entry, end, energy J)`: each chunk's line split into point
    /// sources [`LINE_STEP`] apart, sharing its energy evenly.
    pub(crate) fn lines(lines: &[(usize, Vec3, Vec3, f32)]) -> Self {
        let mut sources = Vec::new();
        for &(_, entry, end, energy) in lines {
            let chord = end - entry;
            let n = (chord.length() / LINE_STEP).ceil().max(1.0) as usize;
            for k in 0..n {
                sources.push(Source {
                    energy: energy / n as f32,
                    at: entry + chord * ((k as f32 + 0.5) / n as f32),
                });
            }
        }
        Self {
            sources,
            energies: lines.iter().map(|&(i, _, _, e)| (i, e)).collect(),
        }
    }

    /// A field's rays, each `(chunk, where it stopped, energy J)`, summed per chunk: each chunk a
    /// source at the energy-weighted mean of its stops, in chunk order.
    pub(crate) fn field(hits: &[(usize, Vec3, f32)]) -> Self {
        let mut per: std::collections::BTreeMap<usize, (f32, Vec3)> = Default::default();
        for &(chunk, at, energy) in hits {
            let entry = per.entry(chunk).or_default();
            entry.0 += energy;
            entry.1 += at * energy;
        }
        let mut loading = Self::default();
        for (chunk, (energy, weighted)) in per {
            if energy <= 0.0 {
                continue;
            }
            loading.sources.push(Source {
                energy,
                at: weighted / energy,
            });
            loading.energies.push((chunk, energy));
        }
        loading
    }

    /// Both deposits as one: their sources superpose.
    pub(crate) fn and(mut self, other: Self) -> Self {
        self.sources.extend(other.sources);
        self.energies.extend(other.energies);
        self
    }
}

/// What one deposit did: the chunks it wore without emptying, the chunks it turned to dust and
/// the joints it cracked, each ascending and without repeats.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Struck {
    pub worn: Vec<usize>,
    pub dusted: Vec<usize>,
    pub cracked: Vec<usize>,
}

/// Run `loading` through the removal and the push rule, in that order: every chunk's energy wears
/// its pool, then every source loads the joints still live. Support is the caller's next step
/// ([`loose_groups`], [`fall`]), so a check that must see the cracks before support (the
/// sandbox's structural one) runs between.
pub(crate) fn strike(structure: &Structure, state: &mut WallState, loading: &Loading) -> Struck {
    let mut struck = Struck::default();
    for &(chunk, energy) in &loading.energies {
        if wear(structure, state, chunk, energy) {
            struck.dusted.push(chunk);
        } else if state.standing(chunk) {
            struck.worn.push(chunk);
        }
    }
    struck.worn.sort_unstable();
    struck.worn.dedup();
    struck.worn.retain(|chunk| state.standing(*chunk));
    struck.dusted.sort_unstable();
    struck.cracked = crack(structure, state, &loading.sources);
    struck
}

/// Every loose group falls: its chunks' fate becomes [`Fate::Fallen`]. Returns the groups.
pub(crate) fn fall(structure: &Structure, state: &mut WallState) -> Vec<Vec<usize>> {
    let groups = loose_groups(structure, state);
    for &chunk in groups.iter().flatten() {
        state.fate[chunk] = Fate::Fallen;
    }
    groups
}

/// One push on a chunk: the impulse (N·s) delivered at `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Push {
    pub chunk: usize,
    pub at: Vec3,
    pub impulse: Vec3,
}

/// What loaded a failed section, and so the units of its demand and capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Load {
    /// A push ([`structural`]): the angular impulse bending the section against the impulse that
    /// would spend its fracture energy, N·s·m.
    Push,
    /// The held group's weight ([`weigh`]): the peak tension across the section against its seam's
    /// tensile strength, Pa.
    Weight,
}

/// One structural failure: the section's joints, what loaded it, and the demand on it with the
/// capacity it was compared with, in its load's units.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Failure {
    pub joints: Vec<usize>,
    pub load: Load,
    pub demand: f32,
    pub capacity: f32,
}

/// The live, intact joints' graph: each chunk's neighbours with the joint between, each chunk's
/// live anchors, and whether it is buried (held by the soil, never by a joint a section can cut).
type Graph = (Vec<Vec<(usize, usize)>>, Vec<Vec<usize>>, Vec<bool>);

fn graph(structure: &Structure, state: &WallState) -> Graph {
    let count = structure.chunks.len();
    let mut neighbours = vec![Vec::new(); count];
    let mut anchors = vec![Vec::new(); count];
    for (index, joint) in structure.joints.iter().enumerate() {
        if state.cracked[index] || !state.live(joint) {
            continue;
        }
        match joint.b {
            Side::Chunk(b) => {
                neighbours[joint.a].push((b, index));
                neighbours[b].push((joint.a, index));
            }
            Side::Ground => anchors[joint.a].push(index),
        }
    }
    let buried = structure.chunks.iter().map(|chunk| chunk.buried).collect();
    (neighbours, anchors, buried)
}

/// The chunks reachable from `start` through `graph` with the joints of `cut` (ascending) removed,
/// when none of them is buried or holds an anchor outside `cut`: the group `cut` alone holds to
/// the ground. `None` when a path to the ground remains. Breadth first, stopping at the first
/// grounded chunk, so a section that holds nothing costs a walk to the nearest anchor. Adds the
/// chunks it expands to `steps`.
fn held(
    start: usize,
    cut: &[usize],
    (neighbours, anchors, buried): &Graph,
    steps: &mut usize,
) -> Option<Vec<usize>> {
    let outside = |joint: &usize| cut.binary_search(joint).is_err();
    let mut seen = vec![false; neighbours.len()];
    seen[start] = true;
    // The queue too: the chunks in the order they were found, `next` the first not yet expanded.
    let mut reached = vec![start];
    let mut next = 0;
    while let Some(&chunk) = reached.get(next) {
        next += 1;
        *steps += 1;
        if buried[chunk] || anchors[chunk].iter().any(outside) {
            return None;
        }
        for &(neighbour, joint) in &neighbours[chunk] {
            if outside(&joint) && !seen[neighbour] {
                seen[neighbour] = true;
                reached.push(neighbour);
            }
        }
    }
    reached.sort_unstable();
    Some(reached)
}

/// One shortest path from `start` to the ground through `graph`: its joints and the anchor that
/// ends it (none when it ends on a buried chunk), ascending; `None` when `start` reaches neither.
/// A section that cuts none of these joints leaves the path standing, so it holds nothing of
/// `start` ([`held`] answers `None`): the structural and weight checks search only the sections
/// that cut a push's or a touched chunk's path. Adds the chunks it expands to `steps`.
fn ground_path(
    start: usize,
    (neighbours, anchors, buried): &Graph,
    steps: &mut usize,
) -> Option<Vec<usize>> {
    // Per chunk reached: the chunk it was reached from and the joint between; `start`'s is unset.
    let mut via: Vec<Option<(usize, usize)>> = vec![None; neighbours.len()];
    let mut seen = vec![false; neighbours.len()];
    seen[start] = true;
    let mut queue = std::collections::VecDeque::from([start]);
    while let Some(chunk) = queue.pop_front() {
        *steps += 1;
        let anchor = anchors[chunk].first();
        if buried[chunk] || anchor.is_some() {
            let mut joints: Vec<usize> = anchor.into_iter().copied().collect();
            let mut at = chunk;
            while let Some((from, joint)) = via[at] {
                joints.push(joint);
                at = from;
            }
            joints.sort_unstable();
            return Some(joints);
        }
        for &(next, joint) in &neighbours[chunk] {
            if !seen[next] {
                seen[next] = true;
                via[next] = Some((chunk, joint));
                queue.push_back(next);
            }
        }
    }
    None
}

/// The sections a structural check tries: every live intact joint alone, and every set of two or
/// more live intact joints lying in one plane (normals parallel, offsets within a millimetre),
/// each ascending, the singles first in joint order, then the planes in creation order. Read from
/// the structure's joints and the state's cracks and fates alone.
fn sections(structure: &Structure, state: &WallState) -> Vec<Vec<usize>> {
    let mut planes: Vec<(Vec3, f32, Vec<usize>)> = Vec::new();
    // The planes by their offset's bucket ([`plane_bucket`]), each bucket in creation order. A
    // joint's plane is the first plane, in creation order, within its offset's tolerance: one
    // whose offset differs by under a millimetre lies in its bucket or the two either side.
    let mut buckets: std::collections::HashMap<i64, Vec<usize>> = Default::default();
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
        let bucket = plane_bucket(offset);
        let found = (-2..=2)
            .filter_map(|step| buckets.get(&bucket.saturating_add(step)))
            .flatten()
            .copied()
            .filter(|&p| {
                let (m, o, _) = &planes[p];
                m.dot(n) > 0.999 && (o - offset).abs() < 1e-3
            })
            .min();
        match found {
            Some(p) => planes[p].2.push(index),
            None => {
                buckets.entry(bucket).or_default().push(planes.len());
                planes.push((n, offset, vec![index]));
            }
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

/// One structure's [`sections`], kept from one structural check to the next. They are a function
/// of the structure and of the state's cracks and fates alone, so the cache answers with the ones
/// it holds exactly while the state's cracks and fates equal those it read them from, and reads
/// them afresh otherwise: a crack, a chunk turned to dust or fallen, or a state replaced whole
/// invalidates it, whoever made the change. It holds no structure: one cache serves the one
/// structure its holder keeps beside it (a building's [`super::breakage::Live`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct SectionCache {
    /// The cracks and fates `sections` were read from. Empty at first: the state of a structure
    /// with no chunks and no joints, which has no sections.
    cracked: Vec<bool>,
    fate: Vec<Fate>,
    sections: Vec<Vec<usize>>,
}

impl SectionCache {
    /// The sections of `structure` in `state`.
    fn read(&mut self, structure: &Structure, state: &WallState) -> &[Vec<usize>] {
        if self.cracked != state.cracked || self.fate != state.fate {
            self.sections = sections(structure, state);
            self.cracked.clone_from(&state.cracked);
            self.fate.clone_from(&state.fate);
        }
        &self.sections
    }
}

/// A plane offset's bucket, its millimetres floored. Two offsets the section rule joins lie at
/// most two buckets apart, DERIVED from IEEE 754 round-to-nearest alone: the f64 product is exact
/// (an f32's 24-bit significand times 1000's 7 odd bits fits f64's 53), so the bucket is `⌊1000
/// x⌋` of the f32 `x` itself; the rule's `|o − x| < 1e-3`, taken in f32, holds only when the exact
/// difference is under the f32 `1e-3` (1.00000005e-3), since rounding is monotone and that bound is
/// representable; and two reals under 1.00000005 apart floor at most two apart. The second bucket
/// covers only the f32 `1e-3` lying above a millimetre.
fn plane_bucket(offset: f32) -> i64 {
    (f64::from(offset) * 1e3).floor() as i64
}

/// The structural check: a push bends a section about its centroid, and the section fails when
/// the angular impulse in its plane, `|Σ (x − c) × J|` over the pushes on the group it alone holds
/// to the ground, reaches the impulse that would spend its fracture energy on opening it,
/// `√(2 I G_f A)`: `I` the group's moment of inertia about the bending axis through the section's
/// centroid (each chunk a ball of its volume), `A` the section's area, `G_f` its chunks' seam
/// fracture energy. The worst failing section cracks first, and the check repeats until none
/// fails, so a failure can expose the next. A seam cracks on that cohesive work alone: whether the
/// freed group then rocks back or topples is the physics' to decide, with the group's weight,
/// contact and friction ([`super::breakage`] launches it with the momentum pushed into it).
///
/// The group a section holds is the part of the pushed chunks' component that loses its last path
/// to the ground with the section's joints removed ([`held`]): a group no push reaches takes no
/// demand, and a push on a chunk with no path to the ground left (the push rule freed it) bends no
/// section, its group leaving as a body with that push ([`super::breakage`]). Only the sections
/// that cut a push's path to the ground are searched for it ([`ground_path`]). Reads the chunk
/// table and the joint graph only; `sections` is `structure`'s cache of them ([`SectionCache`]).
pub(crate) fn structural(
    structure: &Structure,
    state: &mut WallState,
    pushes: &[Push],
    sections: &mut SectionCache,
) -> Vec<Failure> {
    let mut failures = Vec::new();
    if pushes.is_empty() {
        return failures;
    }
    let mut steps = 0;
    for _ in 0..64 {
        let graph = graph(structure, state);
        let paths: Vec<Option<Vec<usize>>> = pushes
            .iter()
            .map(|push| {
                state
                    .standing(push.chunk)
                    .then(|| ground_path(push.chunk, &graph, &mut steps))
                    .flatten()
            })
            .collect();
        let crossing = crossing(structure.joints.len(), &paths);
        let mut worst: Option<(f32, Failure)> = None;
        for section in sections.read(structure, state) {
            // The pushes whose path the section cuts, in push order: the only pushes the search
            // reads for it.
            let mut cut: Vec<usize> = section
                .iter()
                .flat_map(|&joint| crossing[joint].iter().copied())
                .collect();
            if cut.is_empty() {
                continue;
            }
            cut.sort_unstable();
            cut.dedup();
            let mut group: Vec<usize> = Vec::new();
            for push in cut.into_iter().map(|k| &pushes[k]) {
                if group.binary_search(&push.chunk).is_ok() {
                    continue;
                }
                if let Some(reached) = held(push.chunk, section, &graph, &mut steps) {
                    group.extend(reached);
                    group.sort_unstable();
                    group.dedup();
                }
            }
            if group.is_empty() {
                continue;
            }
            if let Some(failure) = section_failure(structure, section, &group, pushes) {
                let ratio = failure.demand / failure.capacity;
                if ratio >= 1.0 && worst.as_ref().is_none_or(|(w, _)| ratio > *w) {
                    worst = Some((ratio, failure));
                }
            }
        }
        let Some((_, failure)) = worst else {
            break;
        };
        for &j in &failure.joints {
            state.cracked[j] = true;
        }
        failures.push(failure);
    }
    failures
}

/// The demand and capacity of `section` holding `group` under `pushes` ([`structural`]), `None`
/// when the pushes bend it about no axis.
fn section_failure(
    structure: &Structure,
    section: &[usize],
    group: &[usize],
    pushes: &[Push],
) -> Option<Failure> {
    let joints: Vec<&Joint> = section.iter().map(|&j| &structure.joints[j]).collect();
    let area: f32 = joints.iter().map(|joint| joint.area).sum();
    let centre = joints
        .iter()
        .map(|joint| joint.centroid * joint.area)
        .sum::<Vec3>()
        / area;
    let n = joints[0].normal;
    let moment: Vec3 = pushes
        .iter()
        .filter(|push| group.binary_search(&push.chunk).is_ok())
        .map(|push| (push.at - centre).cross(push.impulse))
        .sum();
    let bending = moment - n * moment.dot(n);
    let demand = bending.length();
    if demand <= 0.0 {
        return None;
    }
    let axis = bending / demand;
    let inertia: f32 = group
        .iter()
        .map(|&i| {
            let chunk = &structure.chunks[i];
            let r = chunk.centroid - centre;
            let own = (3.0 * chunk.volume / (4.0 * PI)).cbrt();
            chunk.mass * (r.length_squared() - r.dot(axis).powi(2) + 0.4 * own * own)
        })
        .sum();
    let toughness = structure.chunks[joints[0].a].material.fracture_energy;
    Some(Failure {
        joints: section.to_vec(),
        load: Load::Push,
        demand,
        capacity: (2.0 * inertia * toughness * area).sqrt(),
    })
}

/// Per joint, the starts whose path to the ground (`paths`, one per start, [`ground_path`]) crosses
/// it, ascending: a section cuts start `k`'s path exactly when one of its joints lists `k`.
fn crossing(joints: usize, paths: &[Option<Vec<usize>>]) -> Vec<Vec<usize>> {
    let mut crossing = vec![Vec::new(); joints];
    for (k, path) in paths.iter().enumerate() {
        for &joint in path.iter().flatten() {
            crossing[joint].push(k);
        }
    }
    crossing
}

/// The pull of gravity, m/s²: avian's default `Gravity`, the one the game's physics world runs
/// on, so the weight a section holds is the weight that moves the group once it falls.
pub(crate) const GRAVITY: f32 = 9.81;

/// What one weight check did ([`weigh`]): the sections it cracked, in order; the touched chunks
/// it left standing unassessed, ascending; and its operation count, the joints its section scans
/// and its stress sums read plus the chunks its walks expanded and its stress sums read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Weighed {
    pub failures: Vec<Failure>,
    pub unassessed: Vec<usize>,
    pub steps: usize,
}

/// The weight check, run after a wall changed: the chunks whose support the change moved
/// ([`touched`]: each side of a joint in `cracked`, each chunk joined to one in `dusted`, ascending)
/// are the only ones it starts from, so a wall nothing changed costs nothing and a building the
/// bake made too weak to stand stays up until a change reaches the weak part. For each section
/// that cuts a touched chunk's path to the ground ([`ground_path`]), the group it alone holds
/// with that chunk ([`held`]) hangs from the section's joints that border it, its one cut set:
/// statics fixes the force and moment through it exactly, and [`weight_failure`] reads them as a
/// stress. The worst section at or over its strength cracks, the chunks on either side of it are
/// touched in turn, and the check repeats until none fails (64 sections at most, as the push
/// check), so a failure can expose the next. A freed group falls by support ([`fall`]) and the
/// physics.
///
/// A touched chunk that no section alone holds with a group (its weight reaching the ground through
/// several sections at once, as a span between two piers, or through a cut set no single plane
/// holds) stands unassessed and is counted ([`Weighed::unassessed`]): a static solve's to decide.
/// A group inside a larger one a section holds (that span under the wall above it) is assessed
/// only as part of the larger. A fall adds no touched chunk: a loose group has no intact joint to a
/// grounded one, so it neither held nor hung on anything that stands. Reads the chunk table and
/// the joint graph only; `sections` is `structure`'s cache of them ([`SectionCache`]).
pub(crate) fn weigh(
    structure: &Structure,
    state: &mut WallState,
    dusted: &[usize],
    cracked: &[usize],
    sections: &mut SectionCache,
) -> Weighed {
    let mut weighed = Weighed::default();
    if dusted.is_empty() && cracked.is_empty() {
        return weighed;
    }
    let mut touched = touched(structure, state, dusted, cracked, &mut weighed.steps);
    for _ in 0..64 {
        touched.retain(|&c| state.standing(c));
        if touched.is_empty() {
            break;
        }
        let graph = graph(structure, state);
        let paths: Vec<Option<Vec<usize>>> = touched
            .iter()
            .map(|&c| ground_path(c, &graph, &mut weighed.steps))
            .collect();
        let crossing = crossing(structure.joints.len(), &paths);
        let mut assessed = vec![false; touched.len()];
        let mut worst: Option<(f32, Failure)> = None;
        for section in sections.read(structure, state) {
            weighed.steps += section.len();
            let mut cut: Vec<usize> = section
                .iter()
                .flat_map(|&joint| crossing[joint].iter().copied())
                .collect();
            cut.sort_unstable();
            cut.dedup();
            // The chunks of the groups this section was already weighed with.
            let mut weighed_with: Vec<usize> = Vec::new();
            for &k in &cut {
                if weighed_with.binary_search(&touched[k]).is_ok() {
                    continue;
                }
                let Some(group) = held(touched[k], section, &graph, &mut weighed.steps) else {
                    continue;
                };
                let failure = weight_failure(structure, section, &group, &mut weighed.steps);
                if let Some(failure) = failure {
                    for &other in &cut {
                        assessed[other] |= group.binary_search(&touched[other]).is_ok();
                    }
                    let ratio = failure.demand / failure.capacity;
                    if ratio >= 1.0 && worst.as_ref().is_none_or(|(w, _)| ratio > *w) {
                        worst = Some((ratio, failure));
                    }
                }
                weighed_with.extend(group);
                weighed_with.sort_unstable();
                weighed_with.dedup();
            }
        }
        let Some((_, failure)) = worst else {
            // A chunk with no path is loose and falls; one whose path is empty is buried, the
            // soil's to hold.
            weighed.unassessed = touched
                .iter()
                .zip(&paths)
                .zip(&assessed)
                .filter(|((_, path), assessed)| {
                    path.as_ref().is_some_and(|path| !path.is_empty()) && !**assessed
                })
                .map(|((&c, _), _)| c)
                .collect();
            break;
        };
        for &j in &failure.joints {
            state.cracked[j] = true;
            let joint = &structure.joints[j];
            touched.push(joint.a);
            if let Side::Chunk(b) = joint.b {
                touched.push(b);
            }
        }
        touched.sort_unstable();
        touched.dedup();
        weighed.failures.push(failure);
    }
    weighed
}

/// The standing chunks whose support a change moved ([`weigh`]), ascending: each side of a joint
/// in `cracked`, and each chunk joined by an uncracked joint to one in `dusted` (ascending). Adds
/// the joints it reads to `steps`.
fn touched(
    structure: &Structure,
    state: &WallState,
    dusted: &[usize],
    cracked: &[usize],
    steps: &mut usize,
) -> Vec<usize> {
    let ends = |joint: &Joint| {
        [
            Some(joint.a),
            match joint.b {
                Side::Chunk(b) => Some(b),
                Side::Ground => None,
            },
        ]
    };
    let mut touched: Vec<usize> = cracked
        .iter()
        .flat_map(|&j| ends(&structure.joints[j]))
        .flatten()
        .collect();
    *steps += cracked.len();
    if !dusted.is_empty() {
        *steps += structure.joints.len();
        for (index, joint) in structure.joints.iter().enumerate() {
            let ends = ends(joint);
            if !state.cracked[index]
                && ends
                    .iter()
                    .flatten()
                    .any(|c| dusted.binary_search(c).is_ok())
            {
                touched.extend(ends.into_iter().flatten());
            }
        }
    }
    touched.retain(|&c| state.standing(c));
    touched.sort_unstable();
    touched.dedup();
    touched
}

/// The peak tension across the joints of `section` that border `group` (ascending), the group the
/// section alone holds ([`weigh`]), against their seam's tensile strength: `None` when the group
/// lies on both sides of the section's plane. The joints carry the group's weight `W` and its
/// moment about their area centroid `c`, `M = Σ (x − c) × m g`; read as a stress linear over the
/// plane (Navier: plane sections stay plane), the direct pull is `−W·n / A` (`n` the plane's normal
/// toward the group, tension positive, so a group hanging below its section pulls and one standing
/// on it presses), and the bending part `M_b` of the moment (in the plane) sets the stress's
/// gradient `g` from `J g = n × M_b`, `J` the joints' second moment of area about `c`. Each joint
/// is a disc of its area (the push rule's reading of a joint, [`disc`]), so it adds its own
/// `A²/4π` to `J`, and its peak is the gradient's reach to its rim. Each joint compares with its
/// own seam (chunk `a`'s material's tensile strength, as [`crack`] reads it); the section's
/// demand and capacity are the joint's whose ratio is highest. ASSUMED out of the check: the
/// shear and the twist about `n` the joints also carry (no shear cohesion is in the material data),
/// and crushing (self-weight compression is about 1/230 of brick's crushing strength, DERIVED in
/// the collapse survey, `.agents/scratch/destruction-2026-10-31-collapse/collapse-survey.md`).
/// Adds the chunks and joints it reads to `steps`.
fn weight_failure(
    structure: &Structure,
    section: &[usize],
    group: &[usize],
    steps: &mut usize,
) -> Option<Failure> {
    let inside = |c: usize| group.binary_search(&c).is_ok();
    // Per bordering joint: its index and its normal turned toward the group.
    let border: Vec<(usize, Vec3)> = section
        .iter()
        .filter_map(|&j| {
            let joint = &structure.joints[j];
            let held = match joint.b {
                Side::Chunk(b) if inside(b) != inside(joint.a) => {
                    if inside(b) {
                        b
                    } else {
                        joint.a
                    }
                }
                Side::Ground if inside(joint.a) => joint.a,
                _ => return None,
            };
            let toward = structure.chunks[held].centroid - joint.centroid;
            let n = joint.normal;
            Some((j, if toward.dot(n) >= 0.0 { n } else { -n }))
        })
        .collect();
    *steps += section.len() + group.len() + border.len();
    let &(_, normal) = border.first()?;
    if border.iter().any(|(_, n)| n.dot(normal) <= 0.0) {
        return None;
    }
    let joints = &structure.joints;
    let area: f32 = border.iter().map(|&(j, _)| joints[j].area).sum();
    let centre = border
        .iter()
        .map(|&(j, _)| joints[j].centroid * joints[j].area)
        .sum::<Vec3>()
        / area;
    let (mass, moment) = group.iter().fold((0.0, Vec3::ZERO), |(mass, moment), &i| {
        let chunk = &structure.chunks[i];
        let weight = Vec3::NEG_Y * (GRAVITY * chunk.mass);
        (
            mass + chunk.mass,
            moment + (chunk.centroid - centre).cross(weight),
        )
    });
    let direct = -mass * GRAVITY * normal.y / area;
    let bending = moment - normal * moment.dot(normal);
    let (u, v) = plane_axes(normal);
    let offset = |j: usize| {
        let d = joints[j].centroid - centre;
        (d.dot(u), d.dot(v))
    };
    let (mut juu, mut juv, mut jvv) = (0.0, 0.0, 0.0);
    for &(j, _) in &border {
        let a = joints[j].area;
        let own = a * a / (4.0 * PI);
        let (du, dv) = offset(j);
        juu += own + a * du * du;
        juv += a * du * dv;
        jvv += own + a * dv * dv;
    }
    let lever = normal.cross(bending);
    let (lu, lv) = (lever.dot(u), lever.dot(v));
    let det = juu * jvv - juv * juv;
    let (gu, gv) = ((jvv * lu - juv * lv) / det, (juu * lv - juv * lu) / det);
    let reach = gu.hypot(gv);
    let (demand, capacity) = border
        .iter()
        .map(|&(j, _)| {
            let (du, dv) = offset(j);
            let rim = (joints[j].area / PI).sqrt();
            let peak = direct + gu * du + gv * dv + reach * rim;
            (peak, structure.chunks[joints[j].a].material.tensile)
        })
        .max_by(|a, b| (a.0 / a.1).total_cmp(&(b.0 / b.1)))?;
    Some(Failure {
        joints: border.into_iter().map(|(j, _)| j).collect(),
        load: Load::Weight,
        demand,
        capacity,
    })
}

/// A blast field's directions with their shares (summing to 1): the field machinery's ball at
/// ratio 0, uniform over the sphere ([`crate::ballistics::spall_cone`]), [`BLAST_RAYS`] of them.
pub(crate) fn blast_directions() -> &'static [(Vec3, f32)] {
    use crate::ballistics::spall_cone::{AxisRule, ConeShape, FieldDescriptor, FieldLaw};
    static DIRECTIONS: std::sync::LazyLock<Vec<(Vec3, f32)>> = std::sync::LazyLock::new(|| {
        let spread = ConeShape::new(AxisRule::ExitDirection, BLAST_RAYS)
            .and_then(|shape| {
                shape.spread(FieldDescriptor {
                    law: FieldLaw::Ball,
                    ratio: 0.0,
                })
            })
            .expect("the uniform ball is inside the field law at the blast's ray count");
        let directions: Vec<(Vec3, f32)> = spread
            .directions(bevy::math::Dir3::Y)
            .map(|(dir, weight)| (Vec3::from(dir), weight))
            .collect();
        let total: f32 = directions.iter().map(|(_, w)| w).sum();
        directions
            .into_iter()
            .map(|(dir, w)| (dir, w / total))
            .collect()
    });
    &DIRECTIONS
}

/// The blast field's ray count (Resolution): 0.0031 sr a ray, so a 25 cm chunk face 3 m away holds
/// about 2.3 of them square on (DERIVED).
pub(crate) const BLAST_RAYS: usize = 4096;

/// TNT's energy, J/kg: the TNT-equivalent convention (CITED: the ton of TNT is defined as
/// 4.184 GJ), which the burst cores' rows state as their chemical energy.
const TNT_ENERGY: f32 = 4.184e6;

/// The charge factor between a surface burst and a free-air one: a hemispherical surface burst of
/// `W` loads like a free-air burst of `1.8 W` (CLAIMED: UFC 3-340-02's reflection factor for a
/// surface burst, the ground's crater taking the rest of the doubling). The fits are the surface
/// burst's; a blast in the air reads them at `W / 1.8`.
const SURFACE_BURST_FACTOR: f32 = 1.8;

/// The least cosine of incidence a ray's patch is reckoned at (ASSUMED, Resolution): past 84° one
/// ray's patch, `4π share d² / cos θ`, outgrows the chunk face it stops on and would load the face
/// with impulse that lands beyond it.
const GRAZING_COS: f32 = 0.1;

/// One of Swisdak's fits of a surface burst of TNT (Swisdak 1994, "Simplified Kingery Airblast
/// Calculations", 26th DoD Explosives Safety Seminar; its metric table, which restates Kingery &
/// Bulmash 1984, ARBRL-TR-02555, within 1 %): scaled impulse `i / W^⅓` (kPa·ms per kg^⅓, which is
/// Pa·s per kg^⅓) at scaled distance `Z = R / W^⅓` (m/kg^⅓) as `exp(Σ cₖ (ln Z)ᵏ)`, each branch
/// good up to its `Z`. Below `from` it holds the edge's value (the contact regime is the wall
/// design's "Contact and near-contact blast", not this law); past the last branch it falls as
/// `1/Z` (ASSUMED: the acoustic far field). CITED through the `kingery-bulmash` Python package's
/// transcription (fcento100/kingery-bulmash); the primary was not opened. Its metric reflected
/// row agrees with its imperial row, transcribed apart, to 0.05 % (DERIVED; the module's tests
/// hold it to 0.1 %).
struct ImpulseFit {
    from: f32,
    branches: &'static [(f32, [f32; 5])],
}

impl ImpulseFit {
    /// The scaled impulse at scaled distance `z`, Pa·s per kg^⅓.
    fn at(&self, z: f32) -> f32 {
        let (edge, _) = self.branches[self.branches.len() - 1];
        let held = z.max(self.from).min(edge);
        let (_, c) = self
            .branches
            .iter()
            .find(|(to, _)| held <= *to)
            .expect("held is within the last branch's edge");
        let l = held.ln();
        let scaled = c.iter().rev().fold(0.0, |sum, ck| sum * l + ck).exp();
        scaled * (held / z.max(held))
    }
}

/// Swisdak's normally reflected impulse, Z 0.06–40 m/kg^⅓.
const REFLECTED_IMPULSE: ImpulseFit = ImpulseFit {
    from: 0.06,
    branches: &[(40.0, [6.7853, -1.3466, 0.101, -0.01123, 0.0])],
};

/// Swisdak's incident (side-on) impulse, Z 0.2–158.7 m/kg^⅓.
const INCIDENT_IMPULSE: ImpulseFit = ImpulseFit {
    from: 0.2,
    branches: &[
        (0.96, [5.522, 1.117, 0.6, -0.292, -0.087]),
        (2.38, [5.465, -0.308, -1.464, 1.362, -0.432]),
        (33.7, [5.2749, -0.4677, -0.2499, 0.0588, -0.00554]),
        (158.7, [5.9825, -1.062, 0.0, 0.0, 0.0]),
    ],
};

/// A blast: its energy (J) and its charge `W` (kg of TNT), the TNT that carries that energy
/// (ASSUMED: impulse equivalence is energy equivalence; the wall design notes they need not agree).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Blast {
    pub energy: f32,
    pub charge: f32,
}

impl Blast {
    /// A blast of `energy` J in the air.
    pub(crate) fn of(energy: f32) -> Self {
        let energy = energy.max(0.0);
        Self {
            energy,
            charge: energy / TNT_ENERGY,
        }
    }

    /// The specific impulse (Pa·s) the blast loads a face with `distance` m from the charge, at
    /// incidence cosine `cos`: the normally reflected impulse `i_r` square on, the incident `i_s`
    /// grazing, between them `i_r cos²θ + i_s (1 + cos²θ − 2 cos θ)` (CLAIMED: Randers-Pehrson &
    /// Bannister 1997, ARL-TR-1310, the ConWep blend, which states it for pressure; read here on
    /// impulse, ASSUMED).
    pub(crate) fn specific_impulse(&self, distance: f32, cos: f32) -> f32 {
        let scale = (self.charge / SURFACE_BURST_FACTOR).cbrt();
        if scale <= 0.0 {
            return 0.0;
        }
        let z = distance / scale;
        let cos = cos.clamp(0.0, 1.0);
        scale
            * (REFLECTED_IMPULSE.at(z) * cos * cos
                + INCIDENT_IMPULSE.at(z) * (1.0 - cos) * (1.0 - cos))
    }

    /// The impulse (N·s) a ray of `share` of the field delivers where it stops `distance` m out,
    /// travelling along unit `dir`, on a face of outward unit normal `normal`: the specific impulse
    /// there over the patch of the face the ray stands for, its solid angle `4π share` × `d²` /
    /// cos θ ([`GRAZING_COS`] the least), pushed into the face (a pressure acts along the normal).
    pub(crate) fn push(&self, share: f32, distance: f32, dir: Vec3, normal: Vec3) -> Vec3 {
        let cos = -dir.dot(normal);
        let patch = 4.0 * PI * share * distance * distance / cos.max(GRAZING_COS);
        -normal * (self.specific_impulse(distance, cos) * patch)
    }
}

/// One blast ray's arrival: the chunk it stopped on, where, its energy (J) and impulse (N·s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RayHit {
    pub chunk: usize,
    pub at: Vec3,
    pub energy: f32,
    pub impulse: Vec3,
}

/// A blast's rays from `origin`: each of `directions` (unit, with its share of the field; the
/// shares sum to 1) carries that share of the blast's energy, stops on the first standing block it
/// meets, which shadows everything behind it, and pushes the face it meets ([`Blast::push`]). A ray
/// that meets none leaves. Coupling is the geometry's: the share of directions the blocks
/// intercept.
pub(crate) fn blast_rays(
    blocks: &[Block],
    standing: impl Fn(usize) -> bool,
    origin: Vec3,
    blast: Blast,
    directions: &[(Vec3, f32)],
) -> Vec<RayHit> {
    directions
        .iter()
        .filter_map(|&(dir, share)| {
            first_hit(blocks, &standing, origin, dir).map(|(chunk, t, normal)| RayHit {
                chunk,
                at: origin + dir * t,
                energy: blast.energy * share,
                impulse: blast.push(share, t, dir, normal),
            })
        })
        .collect()
}

impl Loading {
    /// A blast's ray hits as a field deposit ([`Loading::field`]), and their pushes: per chunk the
    /// summed impulse at the energy-weighted mean of its hits.
    pub(crate) fn blast(hits: &[RayHit]) -> (Self, Vec<Push>) {
        let loading = Self::field(
            &hits
                .iter()
                .map(|hit| (hit.chunk, hit.at, hit.energy))
                .collect::<Vec<_>>(),
        );
        let mut pushes: Vec<Push> = Vec::new();
        for hit in hits {
            match pushes.iter_mut().find(|push| push.chunk == hit.chunk) {
                Some(push) => push.impulse += hit.impulse,
                None => pushes.push(Push {
                    chunk: hit.chunk,
                    at: Vec3::ZERO,
                    impulse: hit.impulse,
                }),
            }
        }
        for push in &mut pushes {
            if let Some((source, _)) = loading
                .sources
                .iter()
                .zip(&loading.energies)
                .find(|(_, (chunk, _))| *chunk == push.chunk)
            {
                push.at = source.at;
            }
        }
        pushes.retain(|push| loading.energies.iter().any(|(c, _)| *c == push.chunk));
        pushes.sort_by_key(|push| push.chunk);
        (loading, pushes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row of `n` fixture chunks 0.2 m apart along x, each joined to the next and the first
    /// anchored: fixture geometry, the shipped brick's values.
    fn row(n: usize) -> Structure {
        let material = brick();
        let chunks = (0..n)
            .map(|i| Chunk {
                volume: 0.01,
                centroid: Vec3::new(0.2 * i as f32, 0.1, 0.0),
                mass: material.density * 0.01,
                material,
                buried: false,
            })
            .collect();
        let mut joints: Vec<Joint> = (1..n)
            .map(|i| Joint {
                a: i - 1,
                b: Side::Chunk(i),
                area: 0.04,
                centroid: Vec3::new(0.2 * i as f32 - 0.1, 0.1, 0.0),
                normal: Vec3::X,
            })
            .collect();
        joints.push(Joint {
            a: 0,
            b: Side::Ground,
            area: 0.04,
            centroid: Vec3::ZERO,
            normal: Vec3::Y,
        });
        Structure { chunks, joints }
    }

    /// A fixture wall `w` chunks wide and `h` high of 0.25 m cubes in the xy plane, each joined to
    /// its right and upper neighbour, the bottom row anchored: fixture geometry, the shipped
    /// brick's values. Chunk `row × w + col`.
    fn grid(w: usize, h: usize) -> Structure {
        let material = brick();
        let size = 0.25_f32;
        let volume = size.powi(3);
        let at = |col: f32, row: f32| Vec3::new(col * size, row * size, 0.0);
        let chunks = (0..w * h)
            .map(|i| Chunk {
                volume,
                centroid: at((i % w) as f32 + 0.5, (i / w) as f32 + 0.5),
                mass: material.density * volume,
                material,
                buried: false,
            })
            .collect();
        let mut joints = Vec::new();
        for row in 0..h {
            for col in 0..w {
                let i = row * w + col;
                let (c, r) = (col as f32, row as f32);
                if col + 1 < w {
                    joints.push(Joint {
                        a: i,
                        b: Side::Chunk(i + 1),
                        area: size * size,
                        centroid: at(c + 1.0, r + 0.5),
                        normal: Vec3::X,
                    });
                }
                if row + 1 < h {
                    joints.push(Joint {
                        a: i,
                        b: Side::Chunk(i + w),
                        area: size * size,
                        centroid: at(c + 0.5, r + 1.0),
                        normal: Vec3::Y,
                    });
                }
                if row == 0 {
                    joints.push(Joint {
                        a: i,
                        b: Side::Ground,
                        area: size * size,
                        centroid: at(c + 0.5, 0.0),
                        normal: Vec3::Y,
                    });
                }
            }
        }
        Structure { chunks, joints }
    }

    /// The structural check as specified, every section searched with [`held`] for every push
    /// whose chunk still reaches the ground: the reference the pruned check answers to.
    fn structural_by_full_search(
        structure: &Structure,
        state: &mut WallState,
        pushes: &[Push],
    ) -> Vec<Failure> {
        let mut failures = Vec::new();
        for _ in 0..64 {
            let graph = graph(structure, state);
            let mut worst: Option<(f32, Failure)> = None;
            for section in sections(structure, state) {
                let mut group: Vec<usize> = Vec::new();
                for push in pushes {
                    if !state.standing(push.chunk)
                        || group.binary_search(&push.chunk).is_ok()
                        || held(push.chunk, &[], &graph, &mut 0).is_some()
                    {
                        continue;
                    }
                    if let Some(reached) = held(push.chunk, &section, &graph, &mut 0) {
                        group.extend(reached);
                        group.sort_unstable();
                        group.dedup();
                    }
                }
                if group.is_empty() {
                    continue;
                }
                if let Some(failure) = section_failure(structure, &section, &group, pushes) {
                    let ratio = failure.demand / failure.capacity;
                    if ratio >= 1.0 && worst.as_ref().is_none_or(|(w, _)| ratio > *w) {
                        worst = Some((ratio, failure));
                    }
                }
            }
            let Some((_, failure)) = worst else {
                break;
            };
            for &j in &failure.joints {
                state.cracked[j] = true;
            }
            failures.push(failure);
        }
        failures
    }

    /// The section rule as specified, each joint's plane found by a scan of every plane in
    /// creation order: the reference the bucketed search answers to.
    fn sections_by_scan(structure: &Structure, state: &WallState) -> Vec<Vec<usize>> {
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

    /// THE STRUCTURAL CHECK'S SECTIONS ARE THE SCAN'S: on the shipped house (fixture data) at two
    /// instance scales, whole and with a fixture pattern of cracked joints and spent chunks, the
    /// bucketed plane search gives every section the scan gives, members and order alike. A bucket
    /// too narrow for the millimetre tolerance, or a search that took a later plane than the first
    /// in creation order, fails here; the structural check, and with it every crack it records,
    /// relies on the sections being the rule's.
    #[test]
    fn the_bucketed_sections_are_the_scan() {
        let kind = crate::buildings::tests::shipped_house();
        for scale in [1.0, 1.137] {
            let structure = kind.layout.structure(&kind.asset, scale);
            let mut state = WallState::new(&structure);
            assert_eq!(
                sections(&structure, &state),
                sections_by_scan(&structure, &state),
                "whole, scale {scale}"
            );
            for (j, cracked) in state.cracked.iter_mut().enumerate() {
                *cracked = j % 7 == 3;
            }
            for (c, fate) in state.fate.iter_mut().enumerate() {
                if c % 11 == 5 {
                    *fate = Fate::Dust;
                }
            }
            assert_eq!(
                sections(&structure, &state),
                sections_by_scan(&structure, &state),
                "broken, scale {scale}"
            );
        }
    }

    /// THE STRUCTURAL CHECK SEARCHES ONLY THE SECTIONS THAT CUT A PUSH'S PATH TO THE GROUND, AND
    /// ANSWERS AS THE FULL SEARCH DOES: on a fixture wall, an out-of-plane push on its top middle
    /// chunk (its path to the ground the longest), pushes on two chunks, and pushes on a chunk
    /// already cut loose and on a grounded one each give the full search's failures, in its order,
    /// and its cracks, one [`SectionCache`] carried from case to case as a building carries its
    /// own from deposit to deposit. A pruning that skipped a section cutting a push's path fails
    /// here; the server relies on the pruning to keep a burst beside a house within its tick.
    ///
    /// AND A PUSH ON A LOOSE CHUNK BENDS NO SECTION: alone, it fails nothing. Were every section
    /// to hold the loose chunk (each cut leaves it as loose as before), its demand over capacity
    /// would not fall with the section's distance, and one push on a freed chunk would crack 64
    /// joints anywhere in the structure.
    #[test]
    fn the_pruned_structural_check_is_the_full_search() {
        let (w, h) = (8, 6);
        let structure = grid(w, h);
        let push = |chunk: usize, impulse: Vec3| Push {
            chunk,
            at: structure.chunks[chunk].centroid + Vec3::Z * 0.125,
            impulse,
        };
        let top = (h - 1) * w + w / 2;
        let loose = 2 * w + 1;
        let cases: [(Vec<usize>, Vec<Push>); 3] = [
            (Vec::new(), vec![push(top, Vec3::Z * 400.0)]),
            (
                Vec::new(),
                vec![
                    push(top, Vec3::new(0.0, 0.0, -150.0)),
                    push(w + 6, Vec3::new(20.0, 0.0, 300.0)),
                ],
            ),
            (
                (0..structure.joints.len())
                    .filter(|&j| {
                        let joint = &structure.joints[j];
                        joint.a == loose || joint.b == Side::Chunk(loose)
                    })
                    .collect(),
                vec![push(loose, Vec3::Z * 500.0), push(top, Vec3::Z * 200.0)],
            ),
        ];
        let mut cache = SectionCache::default();
        for (index, (cracked, pushes)) in cases.into_iter().enumerate() {
            let mut full = WallState::new(&structure);
            for &j in &cracked {
                full.cracked[j] = true;
            }
            let mut pruned = full.clone();
            let expected = structural_by_full_search(&structure, &mut full, &pushes);
            assert!(!expected.is_empty(), "case {index} fails a section");
            assert_eq!(
                structural(&structure, &mut pruned, &pushes, &mut cache),
                expected,
                "case {index}"
            );
            assert_eq!(pruned, full, "case {index}");
        }
        let mut state = WallState::new(&structure);
        for (j, joint) in structure.joints.iter().enumerate() {
            state.cracked[j] = joint.a == loose || joint.b == Side::Chunk(loose);
        }
        let pushes = [push(loose, Vec3::Z * 500.0)];
        assert!(structural(&structure, &mut state, &pushes, &mut cache).is_empty());
    }

    /// THE SECTIONS ARE READ AFRESH ONCE THE CRACKS OR FATES CHANGE: on the shipped house (fixture
    /// data), one [`SectionCache`] read whole, then after a crack, after a chunk turned to dust,
    /// after a chunk fell and after the state was replaced whole (a replica deriving from a new
    /// record), answers each time as [`sections`] does on that state, and each change moves the
    /// sections. A cache that kept its sections past a change, or keyed on the cracks alone (dust
    /// ends a chunk's joints without cracking one), fails here; every crack the structural check
    /// records relies on its sections being the state's.
    #[test]
    fn a_change_to_the_cracks_or_fates_invalidates_the_sections() {
        let kind = crate::buildings::tests::shipped_house();
        let structure = kind.layout.structure(&kind.asset, 1.0);
        let mut state = WallState::new(&structure);
        let mut cache = SectionCache::default();
        let mut last = sections(&structure, &state);
        assert_eq!(cache.read(&structure, &state), &last[..], "whole");
        assert_eq!(
            cache.read(&structure, &state),
            &last[..],
            "whole, read again"
        );
        // A chunk one of whose joints is still live, so its fate moves the sections.
        let live_chunk = |state: &WallState| {
            let index = (0..structure.joints.len())
                .find(|&j| !state.cracked[j] && state.live(&structure.joints[j]))
                .expect("the house keeps a live joint");
            structure.joints[index].a
        };
        let changes: [(&str, &dyn Fn(&mut WallState)); 4] = [
            ("a crack", &|state| {
                let index = (0..structure.joints.len())
                    .find(|&j| matches!(structure.joints[j].b, Side::Chunk(_)))
                    .expect("the house joins two chunks");
                state.cracked[index] = true;
            }),
            ("dust", &|state| {
                let chunk = live_chunk(state);
                state.fate[chunk] = Fate::Dust;
            }),
            ("a fall", &|state| {
                let chunk = live_chunk(state);
                state.fate[chunk] = Fate::Fallen;
            }),
            ("a state replaced whole", &|state| {
                *state = WallState::new(&structure)
            }),
        ];
        for (change, apply) in changes {
            apply(&mut state);
            let fresh = sections(&structure, &state);
            assert_ne!(fresh, last, "{change} moves the sections");
            assert_eq!(cache.read(&structure, &state), &fresh[..], "after {change}");
            last = fresh;
        }
    }

    fn brick() -> Masonry {
        Masonry::of(SubstanceRegistry::shipped_static(), "Brick")
            .expect("the shipped registry holds Brick")
            .expect("Brick states a seam strength")
    }

    /// THE LAWS READ THE MATERIAL FROM DATA: the shipped Brick's seam strength is its row's
    /// `tensile`, its crush energy the walk's own static resistance at its factor (135.5 MJ/m³,
    /// DERIVED in the prototype's README from 36.5 ref-mm/m), and a substance whose row states no
    /// seam strength (the shipped Wood) has no values, so its elements never break. A law reading a
    /// constant in place of the row, or breaking every substance, fails here.
    #[test]
    fn the_material_is_the_substance_row() {
        let registry = SubstanceRegistry::shipped_static();
        let brick = brick();
        let bulk = registry.hard("Brick").unwrap().bulk.unwrap();
        assert_eq!(f64::from(brick.tensile), bulk.tensile.unwrap());
        assert_eq!(f64::from(brick.density), bulk.density);
        assert!(
            (brick.crush / 135.5e6 - 1.0).abs() < 0.01,
            "crush {}",
            brick.crush
        );
        assert_eq!(Masonry::of(registry, "Wood"), Ok(None));
        assert!(Masonry::of(registry, "Adobe").is_err());
    }

    /// A POOL WEARS, THEN EMPTIES: energy under a chunk's pool leaves it standing and worn; the
    /// rest of its pool turns it to dust, and a dusted chunk takes no more and joins nothing, so a
    /// chunk beyond it with no other path to the ground falls. A mutant that dusts on the first
    /// deposit, or that leaves a dusted chunk holding its joints, fails here.
    #[test]
    fn a_spent_pool_turns_a_chunk_to_dust_and_drops_what_it_held() {
        let structure = row(3);
        let mut state = WallState::new(&structure);
        let pool = full_pool(&structure.chunks[1]);
        let half = Loading {
            sources: Vec::new(),
            energies: vec![(1, pool / 2.0)],
        };
        let struck = strike(&structure, &mut state, &half);
        assert_eq!(struck.worn, vec![1]);
        assert!(struck.dusted.is_empty());
        assert!(fall(&structure, &mut state).is_empty());
        let struck = strike(&structure, &mut state, &half);
        assert_eq!(struck.dusted, vec![1]);
        assert!(!wear(&structure, &mut state, 1, pool), "dust takes no more");
        assert_eq!(fall(&structure, &mut state), vec![vec![2]]);
        assert_eq!(state.fate, vec![Fate::Standing, Fate::Dust, Fate::Fallen]);
    }

    /// THE SOIL HOLDS A BURIED CHUNK: on a fixture wall whose bottom row lies below grade, every
    /// anchor cracked frees nothing, and the anchors as a section hold nothing of the rows above
    /// (the structural check's search); cracking the bed joints above the buried row frees the
    /// rows above it as one group. A support law that grounded only through anchors would release
    /// the buried row as a body inside the ground, where the solver throws it out.
    #[test]
    fn a_buried_chunk_is_held_by_the_soil() {
        let (w, h) = (4, 3);
        let mut structure = grid(w, h);
        for chunk in &mut structure.chunks[..w] {
            chunk.buried = true;
        }
        let anchors: Vec<usize> = (0..structure.joints.len())
            .filter(|&j| structure.joints[j].b == Side::Ground)
            .collect();
        let state = WallState::new(&structure);
        assert_eq!(
            held(w + 1, &anchors, &graph(&structure, &state), &mut 0),
            None
        );
        let mut state = state;
        for &j in &anchors {
            state.cracked[j] = true;
        }
        assert!(loose_groups(&structure, &state).is_empty());
        for (j, joint) in structure.joints.iter().enumerate() {
            if joint.a < w && joint.b == Side::Chunk(joint.a + w) {
                state.cracked[j] = true;
            }
        }
        assert_eq!(
            loose_groups(&structure, &state),
            vec![(w..w * h).collect::<Vec<_>>()]
        );
    }

    /// THE PUSH RULE CRACKS A SEAM AT ITS STRENGTH: a lone source on a joint's line, at a distance
    /// where its load is just over the seam's tensile strength, cracks it; at the distance where
    /// it is just under, nothing cracks. The load is `3E/(8π d³) (1 − 3cos²θ)` averaged over the
    /// joint's disc, so the fixture takes a source in the joint's plane (θ = 90°) and finds the
    /// distance by bisection on [`joint_load`] itself.
    #[test]
    fn a_seam_cracks_where_the_push_reaches_its_strength() {
        let structure = row(2);
        let joint = structure.joints[0];
        let material = structure.chunks[0].material;
        let source = |d: f32| Source {
            energy: 1.0e5,
            at: joint.centroid + Vec3::Y * d,
        };
        let (mut near, mut far) = (0.0_f32, 5.0_f32);
        for _ in 0..60 {
            let mid = 0.5 * (near + far);
            let sources = [source(mid)];
            let crushed = crushed_radii(&sources, &material);
            if joint_load(&joint, &material, &sources, &crushed).0 >= material.tensile {
                near = mid;
            } else {
                far = mid;
            }
        }
        for (d, cracks) in [(near * 0.99, true), (far * 1.01, false)] {
            let mut state = WallState::new(&structure);
            let cracked = crack(&structure, &mut state, &[source(d)]);
            assert_eq!(cracked.contains(&0), cracks, "at {d} m");
        }
    }

    /// THE PUSH RULE CLAMPS EACH SOURCE AT ITS CRUSHED RADIUS IN THE JOINT'S OWN MATERIAL: on a
    /// fixture wall whose right half is a fixture masonry of an eighth of brick's crush energy (its
    /// crushed radii twice brick's), two sources, one in each half, crack at every energy of a
    /// sweep exactly the joints whose [`joint_load`] reaches its threshold, joint by joint. Some
    /// energy of the sweep cracks other joints when every joint takes brick's radii, so a [`crack`]
    /// that kept the radii of the first material it met fails here; a building of more than one
    /// substance relies on each seam reading its own.
    #[test]
    fn each_joint_clamps_the_sources_at_its_own_materials_radius() {
        let (w, h) = (6, 4);
        let mut structure = grid(w, h);
        let brick = brick();
        let soft = Masonry {
            crush: brick.crush / 8.0,
            ..brick
        };
        for (i, chunk) in structure.chunks.iter_mut().enumerate() {
            if i % w >= w / 2 {
                chunk.material = soft;
            }
        }
        let (left, right) = (
            structure.chunks[w + w / 2 - 1].centroid,
            structure.chunks[w + w / 2].centroid,
        );
        let mut told_apart = false;
        for step in 0..40 {
            let energy = 1.0e4 * 10.0_f32.powf(step as f32 / 10.0);
            let sources = [
                Source { energy, at: right },
                Source {
                    energy: energy / 2.0,
                    at: left,
                },
            ];
            let rule = |material_of: &dyn Fn(&Joint) -> Masonry| -> Vec<usize> {
                (0..structure.joints.len())
                    .filter(|&j| {
                        let joint = &structure.joints[j];
                        let material = material_of(joint);
                        let crushed = crushed_radii(&sources, &material);
                        let (load, threshold) = joint_load(joint, &material, &sources, &crushed);
                        load >= threshold
                    })
                    .collect()
            };
            let expected = rule(&|joint| structure.chunks[joint.a].material);
            told_apart |= rule(&|_| brick) != expected;
            let mut state = WallState::new(&structure);
            assert_eq!(
                crack(&structure, &mut state, &sources),
                expected,
                "{energy} J"
            );
        }
        assert!(told_apart, "the sweep tells the materials' radii apart");
    }

    /// THE BLAST'S IMPULSES ARE SWISDAK'S FITS: at Z = 1 and 10 m/kg^⅓ the reflected fit reads
    /// 884.7 and 59.33 Pa·s per kg^⅓ and the incident 236.3 and 31.04 (DERIVED from the cited
    /// rows), and the reflected agrees within 0.1 % with the same table's imperial row (psi·ms per
    /// lb^⅓ at ft/lb^⅓, transcribed apart) converted to metric. A coefficient mistyped, or a branch
    /// taken at the wrong Z, fails here; every blast push reads these values.
    #[test]
    fn the_blast_impulses_are_swisdaks_fits() {
        const LB_PER_KG: f32 = 2.204_623;
        const FT_PER_M: f32 = 3.280_84;
        const KPA_PER_PSI: f32 = 6.894_757;
        let imperial = |z_metric: f32| -> f32 {
            let l = (z_metric * FT_PER_M / LB_PER_KG.cbrt()).ln();
            let scaled = (5.9313 - 1.5622 * l + 0.1322 * l * l - 0.01123 * l * l * l).exp();
            scaled * KPA_PER_PSI * LB_PER_KG.cbrt()
        };
        for (z, reflected, incident) in [(1.0, 884.7, 236.3), (10.0, 59.33, 31.04)] {
            let r = REFLECTED_IMPULSE.at(z);
            let i = INCIDENT_IMPULSE.at(z);
            assert!(
                (r / reflected - 1.0).abs() < 1e-3,
                "reflected at Z {z}: {r}"
            );
            assert!(
                (r / imperial(z) - 1.0).abs() < 1e-3,
                "imperial at Z {z}: {r}"
            );
            assert!((i / incident - 1.0).abs() < 1e-3, "incident at Z {z}: {i}");
        }
    }

    /// A BLAST RAY PUSHES THE FACE IT MEETS ALONG THE FACE'S NORMAL, WITH THE SPECIFIC IMPULSE
    /// OVER THE PATCH IT STANDS FOR: square on, the reflected impulse over `4π share d²`; at 60°
    /// the blend's quarter of the reflected and quarter of the incident, over twice the patch;
    /// grazing, the patch stops growing at [`GRAZING_COS`]. A push along the ray, a patch without
    /// its `1 / cos θ`, or the blend's weights swapped fails here; the structural check and the
    /// launch speed of every group a blast frees read these pushes.
    #[test]
    fn a_blast_ray_pushes_its_face_along_the_normal() {
        // W / 1.8 = 1 kg, so the scaled distance is the distance (fixture data).
        let blast = Blast::of(TNT_ENERGY * SURFACE_BURST_FACTOR);
        let share = 1.0 / BLAST_RAYS as f32;
        let d = 2.0;
        let patch = 4.0 * PI * share * d * d;
        let (reflected, incident) = (REFLECTED_IMPULSE.at(d), INCIDENT_IMPULSE.at(d));
        let close = |got: Vec3, want: Vec3| (got - want).length() <= 1e-4 * want.length();

        let square = blast.push(share, d, Vec3::NEG_Z, Vec3::Z);
        assert!(close(square, Vec3::NEG_Z * reflected * patch), "{square}");

        let (sin, cos) = 60.0_f32.to_radians().sin_cos();
        let oblique = blast.push(share, d, Vec3::new(sin, 0.0, -cos), Vec3::Z);
        let blend = 0.25 * reflected + 0.25 * incident;
        assert!(
            close(oblique, Vec3::NEG_Z * blend * patch * 2.0),
            "{oblique}"
        );

        let grazing = 0.01_f32;
        let along = Vec3::new((1.0 - grazing * grazing).sqrt(), 0.0, -grazing);
        let skim = blast.push(share, d, along, Vec3::Z);
        let skim_impulse = blast.specific_impulse(d, grazing);
        assert!(
            close(skim, Vec3::NEG_Z * skim_impulse * patch / GRAZING_COS),
            "{skim}"
        );
    }

    /// The fixture cube's width, m, and so its faces' area, m².
    const CUBE: f32 = 0.25;

    /// A fixture of [`CUBE`]-wide cubes of the shipped brick (fixture geometry), each `mass` kg,
    /// chunk `i` at `cells[i]` (in cube widths): the first `buried` held by the soil, so no section
    /// cuts them; every two face to face joined by their face; each in `anchored` anchored by its
    /// bottom face.
    fn cubes(cells: &[[i32; 3]], buried: usize, anchored: &[usize], mass: f32) -> Structure {
        let material = brick();
        let at = |[x, y, z]: [i32; 3]| Vec3::new(x as f32, y as f32, z as f32) * CUBE;
        let chunks = cells
            .iter()
            .enumerate()
            .map(|(i, &cell)| Chunk {
                volume: CUBE.powi(3),
                centroid: at(cell),
                mass,
                material,
                buried: i < buried,
            })
            .collect();
        let mut joints = Vec::new();
        for (a, &p) in cells.iter().enumerate() {
            for (b, &q) in cells.iter().enumerate().skip(a + 1) {
                let d = Vec3::new(
                    (q[0] - p[0]) as f32,
                    (q[1] - p[1]) as f32,
                    (q[2] - p[2]) as f32,
                );
                if d.abs().element_sum() == 1.0 {
                    joints.push(Joint {
                        a,
                        b: Side::Chunk(b),
                        area: CUBE * CUBE,
                        centroid: (at(p) + at(q)) / 2.0,
                        normal: d.abs(),
                    });
                }
            }
        }
        joints.extend(anchored.iter().map(|&a| Joint {
            a,
            b: Side::Ground,
            area: CUBE * CUBE,
            centroid: at(cells[a]) - Vec3::Y * CUBE / 2.0,
            normal: Vec3::Y,
        }));
        Structure { chunks, joints }
    }

    /// Turn the chunks `props` (ascending) to dust, run the weight check on that change, then
    /// support: the state, the check's result and the groups that fell.
    fn dust_and_weigh(
        structure: &Structure,
        props: &[usize],
    ) -> (WallState, Weighed, Vec<Vec<usize>>) {
        let mut state = WallState::new(structure);
        let loading = Loading {
            sources: Vec::new(),
            energies: props
                .iter()
                .map(|&c| (c, full_pool(&structure.chunks[c])))
                .collect(),
        };
        let struck = strike(structure, &mut state, &loading);
        assert_eq!(struck.dusted, props, "the props turn to dust");
        let weighed = weigh(
            structure,
            &mut state,
            &struck.dusted,
            &struck.cracked,
            &mut SectionCache::default(),
        );
        let fell = fall(structure, &mut state);
        (state, weighed, fell)
    }

    /// Each failure's load and joints.
    fn cracks(weighed: &Weighed) -> Vec<(Load, Vec<usize>)> {
        weighed
            .failures
            .iter()
            .map(|failure| (failure.load, failure.joints.clone()))
            .collect()
    }

    /// The joint between chunks `a` and `b`.
    fn joint_between(structure: &Structure, a: usize, b: usize) -> usize {
        structure
            .joints
            .iter()
            .position(|joint| joint.a == a && joint.b == Side::Chunk(b))
            .expect("the fixture joins them")
    }

    /// A fixture arm of `n` cubes run out along +x from a support no section cuts (chunk 0), each
    /// arm cube standing on an anchored prop (chunks `n + 1 ..= 2n`), the arm chunks `1..=n`.
    fn propped_arm(n: i32) -> Vec<[i32; 3]> {
        std::iter::once([0, 1, 0])
            .chain((1..=n).map(|k| [k, 1, 0]))
            .chain((1..=n).map(|k| [k, 0, 0]))
            .collect()
    }

    /// The arm cube mass at which an arm of `n` cubes reaches the seam's strength at its root
    /// joint, kg (DERIVED from fixture geometry and the shipped brick's tensile strength `f_t`):
    /// the root, a disc of area `A = s²` and radius `r`, carries the moment `m g s n² / 2` in
    /// bending alone, whose peak `4 M / (π r³)` reaches `f_t` at `m* = π r³ f_t / (2 g s n²)`.
    fn cantilever_limit(n: i32) -> f32 {
        let r = (CUBE * CUBE / PI).sqrt();
        PI * r.powi(3) * brick().tensile / (2.0 * GRAVITY * CUBE * (n * n) as f32)
    }

    /// A CANTILEVER PAST ITS DERIVED LIMIT FALLS, AND ONE WITHIN IT STANDS: a fixture arm of four
    /// cubes on props, run out from a support, loses its props to dust and hangs from its root
    /// joint ([`cantilever_limit`]). At 2 % under the limit it stands with nothing cracked; at 2 %
    /// over, the weight check cracks the root joint alone and the arm falls as one group. A check
    /// that skipped the moment, read a joint's area for its section modulus, or never ran on the
    /// props' dust fails here; a wall's overhang falls under its own weight only by it.
    #[test]
    fn a_cantilever_past_its_derived_limit_falls_and_one_within_it_stands() {
        let n = 4;
        let cells = propped_arm(n);
        let arm: Vec<usize> = (1..=n as usize).collect();
        let props: Vec<usize> = (n as usize + 1..=2 * n as usize).collect();
        for (share, falls) in [(0.98, false), (1.02, true)] {
            let structure = cubes(&cells, 1, &props, cantilever_limit(n) * share);
            let (_, weighed, fell) = dust_and_weigh(&structure, &props);
            if falls {
                let root = joint_between(&structure, 0, 1);
                assert_eq!(cracks(&weighed), vec![(Load::Weight, vec![root])]);
                assert_eq!(fell, vec![arm.clone()]);
            } else {
                assert_eq!(cracks(&weighed), Vec::new(), "within the limit");
                assert!(fell.is_empty(), "within the limit");
            }
        }
    }

    /// A HANGING PANEL HOLDS UP TO ITS DERIVED LIMIT: a fixture panel two cubes wide and three
    /// deep, propped from below, hangs from two supports no section cuts once its props are dust.
    /// The plane of its two top joints (each of area `A`) carries its whole weight `6 m g` as a
    /// direct pull, centred, so it holds up to `m* = f_t A / (3 g)` (DERIVED, fixture geometry and
    /// the shipped brick's `f_t`). At 2 % under it hangs; at 2 % over, the check cracks that plane's
    /// two joints (neither alone holds the panel) and the panel falls as one group. A check that
    /// read the pull as a press, or weighed single joints only, fails here.
    #[test]
    fn a_hanging_panel_holds_up_to_its_derived_limit() {
        let deep = 3;
        let mut cells = vec![[0, deep + 1, 0], [1, deep + 1, 0]];
        cells.extend((1..=deep).flat_map(|y| [[0, y, 0], [1, y, 0]]));
        let panel: Vec<usize> = (2..cells.len()).collect();
        cells.extend([[0, 0, 0], [1, 0, 0]]);
        let props = vec![cells.len() - 2, cells.len() - 1];
        let limit = brick().tensile * CUBE * CUBE / (deep as f32 * GRAVITY);
        for (share, falls) in [(0.98, false), (1.02, true)] {
            let structure = cubes(&cells, 2, &props, limit * share);
            let (_, weighed, fell) = dust_and_weigh(&structure, &props);
            if falls {
                let top = vec![
                    joint_between(&structure, 0, panel[panel.len() - 2]),
                    joint_between(&structure, 1, panel[panel.len() - 1]),
                ];
                assert_eq!(cracks(&weighed), vec![(Load::Weight, top)]);
                assert_eq!(fell, vec![panel.clone()]);
            } else {
                assert_eq!(cracks(&weighed), Vec::new(), "within the limit");
                assert!(fell.is_empty(), "within the limit");
            }
        }
    }

    /// THE WEIGHT CHECK RUNS ONLY ON A CHANGE: a fixture arm built overhanging at three times its
    /// limit ([`cantilever_limit`]) stands while nothing changes: a check with no dust and no
    /// crack does no work and cracks nothing, and a deposit that only wears an arm cube changes
    /// nothing it reads. The first change that reaches the arm (its tip turned to dust, the shorter
    /// arm still past its limit) cracks its root. A check that weighed every group whatever changed
    /// fails here; a building the bake left too weak relies on it to stand until a hit reaches the
    /// weak part.
    #[test]
    fn a_wall_with_no_change_runs_no_check() {
        let n = 4;
        let cells: Vec<[i32; 3]> = propped_arm(n).into_iter().take(n as usize + 1).collect();
        let structure = cubes(&cells, 1, &[], 3.0 * cantilever_limit(n));
        let mut state = WallState::new(&structure);
        let mut sections = SectionCache::default();
        let idle = weigh(&structure, &mut state, &[], &[], &mut sections);
        assert_eq!(idle, Weighed::default(), "no change, no work");
        let worn = Loading {
            sources: Vec::new(),
            energies: vec![(2, full_pool(&structure.chunks[2]) / 2.0)],
        };
        let struck = strike(&structure, &mut state, &worn);
        assert_eq!(struck.worn, vec![2]);
        let idle = weigh(
            &structure,
            &mut state,
            &struck.dusted,
            &struck.cracked,
            &mut sections,
        );
        assert_eq!(idle, Weighed::default(), "a wear alone, no work");
        assert!(fall(&structure, &mut state).is_empty());

        let tip = n as usize;
        let dust = Loading {
            sources: Vec::new(),
            energies: vec![(tip, full_pool(&structure.chunks[tip]))],
        };
        let struck = strike(&structure, &mut state, &dust);
        let weighed = weigh(
            &structure,
            &mut state,
            &struck.dusted,
            &struck.cracked,
            &mut sections,
        );
        let root = joint_between(&structure, 0, 1);
        assert_eq!(cracks(&weighed), vec![(Load::Weight, vec![root])]);
        assert!(weighed.steps > 0);
    }

    /// A SPAN HELD AT BOTH ENDS STANDS UNASSESSED, AND IS COUNTED: a fixture beam of four cubes
    /// between two supports no section cuts, at ten times the cantilever limit of its length
    /// ([`cantilever_limit`]), loses its props to dust. No single section holds any of it, so the
    /// check cracks nothing and reports every beam chunk unassessed: the check is exact only where
    /// one cut set holds a group, and a static solve (the design's step 3) decides spans. A check
    /// that weighed a span on one end's joint fails here, and so does one that hid the spans it
    /// left.
    #[test]
    fn a_span_held_at_both_ends_stands_unassessed() {
        let n = 4;
        let mut cells = vec![[0, 1, 0], [n + 1, 1, 0]];
        cells.extend((1..=n).map(|k| [k, 1, 0]));
        let beam: Vec<usize> = (2..cells.len()).collect();
        cells.extend((1..=n).map(|k| [k, 0, 0]));
        let props: Vec<usize> = (beam.len() + 2..cells.len()).collect();
        let structure = cubes(&cells, 2, &props, 10.0 * cantilever_limit(n));
        let (_, weighed, fell) = dust_and_weigh(&structure, &props);
        assert_eq!(cracks(&weighed), Vec::new());
        assert!(fell.is_empty());
        assert_eq!(weighed.unassessed, beam);
    }
}
