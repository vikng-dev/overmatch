//! The wall prototype's model, about the game's environment destruction (game state only): a
//! structure, what a deposit of energy does to it, and the candidate crack laws.
//!
//! A structure is a chunk table ([`Chunk`]: volume, centroid, mass, material) and a joint graph
//! ([`Joint`]: the pair, contact area, centroid and normal; a [`Side::Ground`] joint is a ground
//! anchor). Removal, every crack law and support read only those two; a generator alone knows the
//! pattern of its cut ([`running_bond`], [`jittered`]), and its [`Block`]s serve the view, the
//! colliders and the geometry probes — the back face ([`back_distance`]) and a blast's rays
//! ([`blast_rays`]) — never a law.
//!
//! * **Removal.** A chunk's pool is its crush energy, `crush × volume` (J); a deposit wears it,
//!   and a chunk whose pool is spent is turned to dust. A line deposit (a round's energy loss in a
//!   chunk) wears that chunk ([`wear`]); a point deposit crushes the ball of material its energy
//!   pays for ([`crush_point`]).
//! * **Cracks.** A [`CrackLaw`] compares each live joint's load from the deposit's [`Source`]s with
//!   a threshold; a joint over it is cracked loose ([`crack`]). Sources superpose; a joint does not
//!   remember a load below its threshold.
//! * **Structure.** A push on a group that one section alone holds to the ground fails the
//!   section when its angular impulse would spend the section's fracture energy ([`structural`]).
//! * **Support.** A chunk group with no path of intact joints to a ground anchor falls as one
//!   body ([`loose_groups`]).
//!
//! The README beside the captures (`.agents/scratch/destruction-2026-10-30-buildings/
//! wall-prototype/README.md`) owns each law's derivation, its values' sources and its verdict.

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

    /// The Griffith strength of a joint of `area` m², Pa: `√(E G_f / (π a))` with the flaw
    /// half-length `a = √area / 2`, the whole joint taken as the flaw.
    pub fn griffith_strength(&self, area: f32) -> f32 {
        let a = area.sqrt() / 2.0;
        (self.modulus * self.joint_toughness / (PI * a)).sqrt()
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

/// An axis-aligned box, world metres: a generator's chunk shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Block {
    pub min: Vec3,
    pub max: Vec3,
}

impl Block {
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) / 2.0
    }

    /// Where a ray from `origin` along unit `dir` enters this box: its distance, `None` when it
    /// misses or the box lies behind. A ray starting inside enters at 0.
    pub fn entry(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        let (mut near, mut far) = (0.0_f32, f32::INFINITY);
        for axis in 0..3 {
            if dir[axis].abs() < 1e-9 {
                if origin[axis] < self.min[axis] || origin[axis] > self.max[axis] {
                    return None;
                }
            } else {
                let a = (self.min[axis] - origin[axis]) / dir[axis];
                let b = (self.max[axis] - origin[axis]) / dir[axis];
                near = near.max(a.min(b));
                far = far.min(a.max(b));
            }
        }
        (near <= far).then_some(near)
    }
}

/// A cut: the structure the laws read, and per chunk and joint what the view draws.
#[derive(Clone, Debug)]
pub(crate) struct Cut {
    pub structure: Structure,
    /// Each chunk's box, index-aligned with the table.
    pub blocks: Vec<Block>,
    /// Each joint's contact rectangle, corner by corner, index-aligned with the joints.
    pub faces: Vec<[Vec3; 4]>,
}

/// The cut generators the sandbox offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CutKind {
    RunningBond,
    Jittered,
}

impl CutKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::RunningBond => "running bond",
            Self::Jittered => "jittered",
        }
    }
}

/// Contact tolerance, m: faces closer than this touch.
const TOUCH: f32 = 1e-4;

/// A running bond: courses of `chunk` m, each offset half a chunk from the one below, every
/// chunk through the full thickness `size.z`, the courses' end pieces clipped at the wall's ends.
pub(crate) fn running_bond(min: Vec3, size: Vec3, chunk: f32, material: Masonry) -> Cut {
    let courses = (size.y / chunk).round().max(1.0) as usize;
    let course = size.y / courses as f32;
    let mut blocks = Vec::new();
    for k in 0..courses {
        let y = min.y + k as f32 * course;
        let mut x = min.x;
        let mut width = if k % 2 == 1 { chunk / 2.0 } else { chunk };
        while x < min.x + size.x - TOUCH {
            let end = (x + width).min(min.x + size.x);
            blocks.push(Block {
                min: Vec3::new(x, y, min.z),
                max: Vec3::new(end, y + course, min.z + size.z),
            });
            x = end;
            width = chunk;
        }
    }
    of_blocks(blocks, min.y, material)
}

/// A jittered cut: courses of height 0.7–1.3 × `chunk`, pieces of width 0.6–1.4 × `chunk`, seams
/// drawn per course from `seed`; a remainder under 0.3 × `chunk` joins its neighbour. Not a
/// pattern anything should rely on: it shows the laws run unchanged on a cut that is no grid.
pub(crate) fn jittered(min: Vec3, size: Vec3, chunk: f32, seed: u64, material: Masonry) -> Cut {
    let mut rng = SplitMix(seed);
    let rows = split(size.y, chunk, 0.7, 1.3, &mut rng);
    let mut blocks = Vec::new();
    let mut y = min.y;
    for height in rows {
        let mut x = min.x;
        for width in split(size.x, chunk, 0.6, 1.4, &mut rng) {
            blocks.push(Block {
                min: Vec3::new(x, y, min.z),
                max: Vec3::new(x + width, y + height, min.z + size.z),
            });
            x += width;
        }
        y += height;
    }
    of_blocks(blocks, min.y, material)
}

/// `length` cut into pieces of `lo..hi` × `chunk`, the last piece taking the remainder and a
/// remainder under 0.3 × `chunk` joining the piece before it.
fn split(length: f32, chunk: f32, lo: f32, hi: f32, rng: &mut SplitMix) -> Vec<f32> {
    let mut pieces = Vec::new();
    let mut left = length;
    while left > TOUCH {
        let piece = chunk * (lo + (hi - lo) * rng.unit());
        if left - piece < 0.3 * chunk {
            pieces.push(left);
            break;
        }
        pieces.push(piece);
        left -= piece;
    }
    pieces
}

/// A small deterministic generator for the jittered cut (SplitMix64).
struct SplitMix(u64);

impl SplitMix {
    fn unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// The structure of `blocks`, whatever their pattern: a joint wherever two boxes share a face of
/// positive area, its area that overlap; a ground anchor under every box standing on `ground`.
pub(crate) fn of_blocks(blocks: Vec<Block>, ground: f32, material: Masonry) -> Cut {
    let chunks = blocks
        .iter()
        .map(|block| {
            let volume = block.size().x * block.size().y * block.size().z;
            Chunk {
                volume,
                centroid: block.center(),
                mass: material.density * volume,
                material,
            }
        })
        .collect();
    let mut joints = Vec::new();
    let mut faces = Vec::new();
    for (i, a) in blocks.iter().enumerate() {
        for (j, b) in blocks.iter().enumerate().skip(i + 1) {
            for axis in 0..3 {
                let plane = if (a.max[axis] - b.min[axis]).abs() < TOUCH {
                    a.max[axis]
                } else if (b.max[axis] - a.min[axis]).abs() < TOUCH {
                    a.min[axis]
                } else {
                    continue;
                };
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                let (u0, u1) = (a.min[u].max(b.min[u]), a.max[u].min(b.max[u]));
                let (v0, v1) = (a.min[v].max(b.min[v]), a.max[v].min(b.max[v]));
                if u1 - u0 <= TOUCH || v1 - v0 <= TOUCH {
                    continue;
                }
                let corner = |cu: f32, cv: f32| {
                    let mut p = Vec3::ZERO;
                    p[axis] = plane;
                    p[u] = cu;
                    p[v] = cv;
                    p
                };
                let mut normal = Vec3::ZERO;
                normal[axis] = 1.0;
                joints.push(Joint {
                    a: i,
                    b: Side::Chunk(j),
                    area: (u1 - u0) * (v1 - v0),
                    centroid: corner((u0 + u1) / 2.0, (v0 + v1) / 2.0),
                    normal,
                });
                faces.push([
                    corner(u0, v0),
                    corner(u1, v0),
                    corner(u1, v1),
                    corner(u0, v1),
                ]);
            }
        }
        if (a.min.y - ground).abs() < TOUCH {
            let size = a.size();
            joints.push(Joint {
                a: i,
                b: Side::Ground,
                area: size.x * size.z,
                centroid: Vec3::new(a.center().x, ground, a.center().z),
                normal: Vec3::Y,
            });
            faces.push([
                Vec3::new(a.min.x, ground, a.min.z),
                Vec3::new(a.max.x, ground, a.min.z),
                Vec3::new(a.max.x, ground, a.max.z),
                Vec3::new(a.min.x, ground, a.max.z),
            ]);
        }
    }
    Cut {
        structure: Structure { chunks, joints },
        blocks,
        faces,
    }
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

/// A point deposit's crush: the ball's radius, the energy spent and each chunk's share.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PointCrush {
    pub radius: f32,
    pub spent: f32,
    pub doses: Vec<(usize, f32)>,
}

/// The crush of `energy` J deposited at `at`: the ball about `at` holding `energy / crush` m³ of
/// crushable material is crushed, each standing chunk paying its overlap with the ball times its
/// crush energy. A chunk enters as the ball of its own volume about its centroid, a worn chunk's
/// overlap scaled by what its pool still holds; a deposit larger than the wall leaves its excess
/// unspent. Reads the chunk table only.
pub(crate) fn crush_point(
    structure: &Structure,
    state: &WallState,
    at: Vec3,
    energy: f32,
) -> PointCrush {
    // (chunk, centre distance, equivalent radius, J per m³ of overlap: crush × pool fraction left)
    let pieces: Vec<(usize, f32, f32, f32)> = structure
        .chunks
        .iter()
        .enumerate()
        .filter(|(i, _)| state.standing(*i))
        .map(|(i, chunk)| {
            let radius = (3.0 * chunk.volume / (4.0 * PI)).cbrt();
            let rate = chunk.material.crush * state.pool[i] / full_pool(chunk);
            (i, chunk.centroid.distance(at), radius, rate)
        })
        .collect();
    let crushed = |r: f32| -> f32 {
        pieces
            .iter()
            .map(|&(_, d, radius, rate)| lens(r, radius, d) * rate)
            .sum()
    };
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    while crushed(hi) < energy && hi < 1.0e3 {
        hi *= 2.0;
    }
    for _ in 0..48 {
        let mid = (lo + hi) / 2.0;
        if crushed(mid) < energy {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let doses: Vec<(usize, f32)> = pieces
        .iter()
        .map(|&(i, d, radius, rate)| (i, lens(hi, radius, d) * rate))
        .filter(|(_, dose)| *dose > 0.0)
        .collect();
    let total: f32 = doses.iter().map(|(_, dose)| dose).sum();
    let scale = if total > energy { energy / total } else { 1.0 };
    PointCrush {
        radius: hi,
        spent: total * scale,
        doses: doses
            .into_iter()
            .map(|(i, dose)| (i, dose * scale))
            .collect(),
    }
}

/// The volume two balls of radii `r` and `big` share, centres `d` apart, m³.
fn lens(r: f32, big: f32, d: f32) -> f32 {
    if d >= r + big {
        0.0
    } else if d <= (r - big).abs() {
        4.0 / 3.0 * PI * r.min(big).powi(3)
    } else {
        PI * (r + big - d).powi(2) * (d * d + 2.0 * d * (r + big) - 3.0 * (r - big).powi(2))
            / (12.0 * d)
    }
}

/// The candidate crack laws (README). Each states its formula on screen ([`Self::formula`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CrackLaw {
    /// A: the energy flux reaching a joint against its fracture energy.
    Flux,
    /// B: the elastic stress about the crushed cavity against the joint's tensile strength.
    Stress,
    /// C: A, with [`CRACK_SHARE`] of the flux going to cracks.
    Share,
    /// D: B, plus the push reflected as tension off the back face.
    ThinWall,
    /// E: B's stress against the Griffith strength the joint's toughness and size give.
    Griffith,
}

/// Law C's share of the deposit that goes to cracking. ASSUMED: about 1 % of the energy fed to a
/// brittle solid ends as new surface is the figure comminution texts commonly give (CLAIMED, not
/// read here); no handbook states it for masonry.
pub(crate) const CRACK_SHARE: f32 = 0.01;

impl CrackLaw {
    pub const ALL: [Self; 5] = [
        Self::Flux,
        Self::Stress,
        Self::Share,
        Self::ThinWall,
        Self::Griffith,
    ];

    pub fn letter(self) -> char {
        match self {
            Self::Flux => 'A',
            Self::Stress => 'B',
            Self::Share => 'C',
            Self::ThinWall => 'D',
            Self::Griffith => 'E',
        }
    }

    pub fn of_letter(letter: char) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|law| law.letter() == letter.to_ascii_uppercase())
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Flux => "energy flux",
            Self::Stress => "stress",
            Self::Share => "energy share",
            Self::ThinWall => "thin wall",
            Self::Griffith => "Griffith",
        }
    }

    /// The law as the screen states it, ASCII.
    pub fn formula(self) -> &'static str {
        match self {
            Self::Flux => "crack if  E/(4 pi d^2) >= Gf",
            Self::Stress => {
                "crack if  3E/(8 pi d^3) (1 - 3 cos^2) >= ft   (Lame, mean over the joint)"
            }
            Self::Share => "crack if  eta E/(4 pi d^2) >= Gf   (eta = 0.01 ASSUMED)",
            Self::ThinWall => {
                "crack if  B + 3E/(4 pi d'^3) cos'^2 >= ft   (d' from the back-face image)"
            }
            Self::Griffith => "crack if  B stress >= sqrt(Em Gf / (pi a)),  a = sqrt(area)/2",
        }
    }

    /// The distance at which a lone point deposit of `energy` J cracks a joint, m, where the law
    /// gives one in closed form: B's and E's for a joint facing straight across the radius
    /// (`cos = 0`), E's for a joint of `area` m²; none for D, whose reach depends on the back face.
    pub fn point_reach(self, energy: f32, material: &Masonry, area: f32) -> Option<f32> {
        let flux = |share: f32| (share * energy / (4.0 * PI * material.joint_toughness)).sqrt();
        let stress = |strength: f32| (3.0 * energy / (8.0 * PI * strength)).cbrt();
        match self {
            Self::Flux => Some(flux(1.0)),
            Self::Share => Some(flux(CRACK_SHARE)),
            Self::Stress => Some(stress(material.joint_tensile)),
            Self::Griffith => Some(stress(material.griffith_strength(area))),
            Self::ThinWall => None,
        }
    }
}

/// One point of a deposit: `energy` J at `at`, pushing along unit `push`, with the free back face
/// `back` m ahead along it (`None` where the push meets no back face).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Source {
    pub energy: f32,
    pub at: Vec3,
    pub push: Vec3,
    pub back: Option<f32>,
}

impl Source {
    /// The radius of the ball this source alone crushes, m: where every law's field is clamped,
    /// since nothing inside it is left to load.
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

    /// Energy flux through a sphere about the source at `x`, J/m².
    fn flux(&self, x: Vec3, material: &Masonry) -> f32 {
        let (d, _) = self.ray(self.at, x, material);
        self.energy / (4.0 * PI * d * d)
    }

    /// The normal stress on a plane of normal `n` at `x`, Pa, tension positive: the Lamé field of a
    /// spherical cavity the deposit crushed, at pressure the crush energy (`P r_c³ = 3E/4π`),
    /// radial `−3E/(4π d³)` and hoop `+3E/(8π d³)`.
    fn lame(&self, x: Vec3, n: Vec3, material: &Masonry) -> f32 {
        let (d, dir) = self.ray(self.at, x, material);
        let cos = n.dot(dir);
        3.0 * self.energy / (8.0 * PI * d.powi(3)) * (1.0 - 3.0 * cos * cos)
    }

    /// The push reflected off the back face as tension on a plane of normal `n` at `x`, Pa: the
    /// radial stress magnitude `3E/(4π d'³)` of the source's image across the back face, along the
    /// image's radius. Zero without a back face.
    fn reflected(&self, x: Vec3, n: Vec3, material: &Masonry) -> f32 {
        let Some(back) = self.back else {
            return 0.0;
        };
        let image = self.at + self.push * (2.0 * back);
        let (d, dir) = self.ray(image, x, material);
        let cos = n.dot(dir);
        3.0 * self.energy / (4.0 * PI * d.powi(3)) * cos * cos
    }
}

/// Seven points of a disc of `joint`'s area in its plane, with weights summing to 1: the centre at
/// ¼ and six on the ring at `√(2/3)` of its radius at ⅛ each.
fn disc(joint: &Joint) -> [(Vec3, f32); 7] {
    let n = joint.normal;
    let t = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let u = n.cross(t).normalize();
    let v = n.cross(u);
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

/// The load `law` puts on `joint` from `sources`, and the threshold it is compared to, in the
/// law's units (J/m² for A and C, Pa for the rest).
pub(crate) fn joint_load(
    law: CrackLaw,
    joint: &Joint,
    material: &Masonry,
    sources: &[Source],
) -> (f32, f32) {
    let mean = |stress: &dyn Fn(Vec3) -> f32| -> f32 {
        disc(joint)
            .iter()
            .map(|(x, weight)| weight * stress(*x))
            .sum()
    };
    let lame = |x: Vec3| -> f32 {
        sources
            .iter()
            .map(|s| s.lame(x, joint.normal, material))
            .sum()
    };
    match law {
        CrackLaw::Flux | CrackLaw::Share => {
            let share = if law == CrackLaw::Share {
                CRACK_SHARE
            } else {
                1.0
            };
            let flux: f32 = sources
                .iter()
                .map(|s| s.flux(joint.centroid, material))
                .sum();
            (share * flux, material.joint_toughness)
        }
        CrackLaw::Stress => (mean(&lame), material.joint_tensile),
        CrackLaw::ThinWall => {
            let reflected = |x: Vec3| -> f32 {
                sources
                    .iter()
                    .map(|s| s.reflected(x, joint.normal, material))
                    .sum()
            };
            (mean(&|x| lame(x) + reflected(x)), material.joint_tensile)
        }
        CrackLaw::Griffith => (mean(&lame), material.griffith_strength(joint.area)),
    }
}

/// Crack every live, intact joint whose load from `sources` under `law` reaches its threshold;
/// returns the joints cracked now. A joint's material is its chunk `a`'s.
pub(crate) fn crack(
    law: CrackLaw,
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
        let (load, threshold) = joint_load(law, joint, &material, sources);
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

/// The free back face along `push` from `at`, m: how far ahead the last standing block the push
/// crosses ends, found by a ray cast back from beyond the wall. `None` when that ray meets no
/// standing block before `at`. A probe of the cut's geometry, handed to a law as
/// [`Source::back`].
pub(crate) fn back_distance(
    blocks: &[Block],
    standing: impl Fn(usize) -> bool,
    at: Vec3,
    push: Vec3,
) -> Option<f32> {
    const FROM: f32 = 50.0;
    let origin = at + push * FROM;
    let hit = blocks
        .iter()
        .enumerate()
        .filter(|(i, _)| standing(*i))
        .filter_map(|(_, block)| block.entry(origin, -push))
        .fold(f32::INFINITY, f32::min);
    (hit < FROM - 1e-4).then_some(FROM - hit)
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
