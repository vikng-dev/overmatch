//! The buildings' fracture laws, about the game's environment destruction (game state only): a
//! structure, what a source loads it with, the structural check, support and the blast
//! field. One home for the laws the armour sandbox's wall prototype adopted (`sandbox::wall` calls
//! these).
//!
//! A structure is a chunk table ([`Chunk`]: volume, centroid, mass, material) and a joint graph
//! ([`Joint`]: the pair, contact area, centroid and normal; a [`Side::Ground`] joint is a ground
//! anchor). A chunk's convex hull ([`Block`]) also locates Law 2's actual rear surface; it serves
//! the colliders, views and geometry probes that say which chunk a deposit landed in.
//!
//! Every source enters the laws as loads ([`Load`]): the volume (m³) it crushes at one point of one
//! chunk and the kinetic cavity energy (J) the crack rule reads there, the momentum (N·s), its blow, it delivers
//! there, the force (N), its press, it holds there through the tick, and its origin ([`Origin`]): a
//! field, or a body in contact.
//!
//! * **Removal** ([`wear`]). A chunk's pool is its volume (m³); a load's crushed volume wears it,
//!   and a chunk whose pool is spent is turned to dust. A round's line and a fragment crush their
//!   energy over the material's crush energy; a blast crushes by Law 1, its rays' impulse through
//!   a compaction front wherever their peak pressure beats the material's dynamic compressive
//!   strength ([`Masonry::crushed_by`]), its energy never entering.
//! * **Kinetic cavity cracks** ([`crack`]). Each live joint's stress from kinetic loads' energies is
//!   compared with the seam's tensile strength; a joint at or over it is cracked loose. The stress
//!   is the elastic one about the cavity a load's energy crushed. Loads superpose; a joint does
//!   not remember a stress below its threshold. A blast supplies no cavity energy: its pressure
//!   and impulse act through the three blast laws alone.
//! * **Structure** ([`structural`]). The loads' blows on a group that one section alone holds to
//!   the ground crack the section when their angular impulse about the section's hinge would spend
//!   the section's fracture energy and, where the section bears the group, topple the group over
//!   it. A blast's blow counts by `√g` of the section's strength over its peak force
//!   ([`plastic_demand`], [`section_pulse`]): `J² g / 2m` must pay the energy, so a far burst
//!   whose peak the section holds turns nothing.
//! * **Strength** ([`weigh`]). From changed supports and pressed chunks, each group that one
//!   section alone holds is checked under gravity plus the loads' presses. Their resultant force
//!   and moment set the section's peak tensile stress. A part held through several sections at
//!   once stays unassessed and is counted; the wall design's "Structural calculation" owns that
//!   limit and the alternatives.
//! * **Rests** ([`unseat`], inside [`weigh`] and [`structural`]). A joint between two components
//!   the building's data marks resting ([`Bond::Resting`]: a beam laid on a wall, nothing fixing
//!   it) carries no tension: the resultant of gravity and presses must bear into the rests, its
//!   pressure centre inside their bearing hull, its tangent within their friction. A blow can
//!   also carry the part off, turned, lifted or slid, a blast's read as on a section by its
//!   mode's strength over its peak; then it is freed whole.
//! * **Support** ([`loose_groups`]). A chunk group with no path of intact joints to a ground anchor
//!   or to a buried chunk ([`Chunk::buried`]: the soil holds it) falls as one body.
//! * **The blast field** ([`blast_rays`]). A charge's blast energy leaves it uniformly over the
//!   sphere out to the fits' range ([`Blast::reach`]); each ray stops on the first standing chunk
//!   it meets and pushes the face it meets with the blast's reflected impulse over the patch it
//!   stands for ([`Blast::push`]), with the peak force of the blast's peak overpressure over the
//!   same patch ([`Blast::peak`], a load's `peak`, which the section laws read), the chunk taking only
//!   the share of the ray's cone its own silhouette intercepts ([`intercepted`]). The wall design
//!   (`.agents/docs/design/wall-destruction.md`, "Standoff blast") owns the model.
//!
//! A round's line in each chunk becomes loads [`LINE_STEP`] apart along its chord ([`Load::line`]);
//! a field's ray stops keep their own loads ([`Load::field`]); a blast gives one impulse-weighted
//! load per chunk face ([`Load::blast`]). Both preserve the pushes' angular impulse. Gaps these
//! laws leave open (the wall prototype's README and the 2026-10-30 design review): a chunk partly crushed stays whole, the
//! share of a round's lost momentum the crushed brick carries away is unknown (the struck chunks
//! keep all of it), and a blast's peak force acts only against the section and rest laws'
//! strength, never as a press.
//! The README beside the
//! prototype's captures
//! (`.agents/scratch/destruction-2026-10-30-buildings/wall-prototype/README.md`) owns the
//! derivations, the values' sources and the verdicts.

use std::f32::consts::PI;

use bevy::ecs::entity::Entity;
use bevy::math::Vec3;

use crate::substances::{BlastFacts, REFERENCE_FACTOR, SubstanceRegistry};

/// Spacing of the point sources a line deposit is split into along its chord, m: a quarter of a
/// 20 cm chunk (Resolution).
pub(crate) const LINE_STEP: f32 = 0.05;

/// One material's values, as the laws read them, all from its substance row
/// ([`Masonry::of`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Masonry {
    /// kg/m³.
    pub density: f32,
    /// The energy a round's line or a fragment spends turning one cubic metre to dust, J/m³: what
    /// converts their crushing work into the volume their loads take from a pool
    /// ([`Load::crushed`]). A blast's pulse crushes by [`Self::compaction`] instead.
    pub crush: f32,
    /// How a blast's pulse crushes it (Law 1, [`Compaction`]).
    pub compaction: Compaction,
    /// How a blast's pulse throws a rear scab off it (Law 2, [`Spall`]).
    pub spall: Spall,
    /// The seam between its chunks.
    pub seam: Seam,
    /// The friction coefficient its surface brings to a contact, dimensionless: one side of a
    /// rest's pair ([`crate::substances::pair_friction`]).
    pub friction: f32,
}

/// A bonded seam's strength.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Seam {
    /// The tensile strength across it, Pa.
    pub tensile: f32,
    /// Its mode-I fracture energy, J/m².
    pub fracture_energy: f32,
    /// How it answers sliding along it.
    pub shear: Shear,
}

/// How a seam answers sliding along it: what a punch ring reads of it ([`ring`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Shear {
    /// By its shear cohesion `c` without precompression (Pa) and its cohesive mode-II fracture
    /// energy `G_II` (J/m²): its material row's ([`Masonry::of`]).
    Cohesive { cohesion: f32, energy: f32 },
    /// By nothing the laws can read: the seam's data states its opening alone (an attachment kind
    /// stated by its tensile strength and opening work, as the nailed fixing is,
    /// `super::attachments`). It answers by its opening law alone: no punch ring forms across it,
    /// and the section, push and rest laws read it as before.
    OpeningOnly,
}

/// How a material crushes under a blast's pulse, about the game's environment destruction (game
/// state only): Law 1, a rigid–plastic–locking (snowplow) compaction front. A face patch taking
/// impulse `J` at peak pressure `P` crushes `Δm = J f(σ_c,d / P) / u_c` ([`crush_factor`]);
/// nothing where `P` does not beat `σ_c,d`, however long the pulse. The wall design ("Standoff
/// blast", Law 1) owns the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Compaction {
    /// The dynamic compressive strength `σ_c,d`, Pa: the row's quasi-static compressive strength
    /// times its stated dynamic increase factor at the row's reference rate. ASSUMED effective
    /// plateau: the law supplies no per-pulse strain rate; applicability is a validation limit
    /// recorded in the wall design's material-data section.
    pub strength: f32,
    /// The slowest compaction front's speed `u_c = √(σ_c,d ε_v / ρ)`, m/s, `ε_v = 1 − ρ / ρ_s` the
    /// volumetric strain to lock-up at the row's fully dense density `ρ_s` (DERIVED: the locking
    /// idealization compacts the pores shut and no further).
    pub front_speed: f32,
}

/// How a material throws a rear scab under a blast's pulse, about the game's environment
/// destruction (game state only): Law 2's material inputs ([`scab`]). The wall design ("Standoff
/// blast", Law 2) owns the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Spall {
    /// The bar wave speed `c = √(E / ρ)`, m/s, DERIVED from the row's Young's modulus and density:
    /// what turns the pulse's duration into its length `λ = c τ`.
    pub bar_speed: f32,
    /// The dynamic tensile strength `σ_t,d`, Pa: the row's static tensile strength (`bulk.tensile`)
    /// times its stated tensile dynamic increase factor, read as one effective plateau (ASSUMED,
    /// as [`Compaction::strength`] is).
    pub tensile: f32,
}

impl Spall {
    /// The spall inputs of a row of bulk `density` (kg/m³) and static `tensile` strength (Pa)
    /// stating `facts`. The registry refuses a row whose products are not positive and finite in
    /// f32.
    fn of(facts: &BlastFacts, density: f64, tensile: f64) -> Self {
        Self {
            bar_speed: (facts.youngs_modulus / density).sqrt() as f32,
            tensile: (tensile * facts.tensile_dif) as f32,
        }
    }
}

impl Compaction {
    /// The compaction of a row of bulk density `density` (kg/m³) stating `facts`. The registry
    /// refuses a row whose fully dense density does not exceed its bulk density, so `ε_v > 0`.
    fn of(facts: &BlastFacts, density: f64) -> Self {
        let strength = facts.compressive_strength * facts.compressive_dif;
        let lock_up = 1.0 - density / facts.solid_density;
        Self {
            strength: strength as f32,
            front_speed: (strength * lock_up / density).sqrt() as f32,
        }
    }
}

impl Masonry {
    /// The volume (m³) a face patch of this material taking impulse `impulse` (N·s, its
    /// magnitude) at peak pressure `pressure` (Pa) crushes: `Δm / ρ`, `Δm = J f(σ_c,d / P) / u_c`
    /// ([`Compaction`]). Linear in the impulse at a fixed pressure, so the volume a face takes is
    /// its patches' sum however the face is cut into patches; zero where the pressure does not
    /// beat the dynamic strength (a zero pressure's ratio is infinite).
    pub(crate) fn crushed_by(&self, pressure: f32, impulse: f32) -> f32 {
        let Compaction {
            strength,
            front_speed,
        } = self.compaction;
        impulse * crush_factor(strength / pressure) / (front_speed * self.density)
    }

    /// Substance `name`'s values, or `None` where its row states no bulk facts (an element of it
    /// never breaks). The density and the seam's strengths and fracture energies, opening and
    /// shear, are the row's bulk facts (the registry refuses a row stating some of the seam's
    /// four without the others); the
    /// crush energy is the static term of the walk's own force law at the row's factor
    /// (`factor / 1000 × a`, `a` the reference's cavity strength): the energy per volume the walk
    /// spends opening a cavity in it; the compaction is the row's blast facts' ([`Compaction`]);
    /// the friction is the row's. Refuses a name the registry does not hold as hard matter, and a
    /// row stating bulk facts but no seam (its chunks would have no law to part by), no friction
    /// (a part of it laid on another would have none to stand by) or no blast facts (a blast
    /// would have no law to crush it by; the registry refuses such a breakable row first).
    pub(crate) fn of(registry: &SubstanceRegistry, name: &str) -> Result<Option<Self>, String> {
        let hard = registry.hard(name).map_err(|err| err.to_string())?;
        let Some(bulk) = hard.bulk else {
            return Ok(None);
        };
        let (seam, static_tensile) = bulk
            .tensile
            .zip(bulk.fracture_energy)
            .zip(bulk.shear_cohesion.zip(bulk.shear_fracture_energy))
            .map(|((tensile, energy), (cohesion, shear))| {
                (
                    Seam {
                        tensile: tensile as f32,
                        fracture_energy: energy as f32,
                        shear: Shear::Cohesive {
                            cohesion: cohesion as f32,
                            energy: shear as f32,
                        },
                    },
                    tensile,
                )
            })
            .ok_or_else(|| {
                format!("substance {name:?} states bulk facts but no seam to part its chunks by")
            })?;
        let friction = hard.friction.ok_or_else(|| {
            format!("substance {name:?} states bulk facts but no friction for a rest on it")
        })?;
        let blast = bulk.blast.ok_or_else(|| {
            format!("substance {name:?} states a seam but no blast facts to crush it by")
        })?;
        let share = f64::from(hard.factor() / REFERENCE_FACTOR);
        Ok(Some(Self {
            density: bulk.density as f32,
            crush: (share * registry.reference_facts().cavity_strength()) as f32,
            compaction: Compaction::of(&blast, bulk.density),
            spall: Spall::of(&blast, bulk.density, static_tensile),
            seam,
            friction,
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

/// What holds a joint closed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Bond {
    /// A seam: it opens under tension at its strength, and a push spends its fracture energy.
    Bonded(Seam),
    /// One part laid on another, nothing fixing it: no pull strength and no fracture work. It
    /// holds only what bears on it from above, by weight and the friction of its two materials,
    /// so no tension check reads it (it lies in no section and on no path a section cuts) and the
    /// push rule never opens it; it opens when the part it carries leaves its rests ([`unseat`]).
    Resting,
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
    pub bond: Bond,
}

impl Joint {
    /// Its seam, `None` for a rest.
    pub(crate) fn seam(&self) -> Option<Seam> {
        match self.bond {
            Bond::Bonded(seam) => Some(seam),
            Bond::Resting => None,
        }
    }
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

    /// How far `at` lies outside this shape past its farthest bounding plane, m; 0 inside or on
    /// it. Exact over a face, short of the distance off an edge or a corner.
    pub fn outside(&self, at: Vec3) -> f32 {
        self.planes
            .iter()
            .map(|(n, o)| n.dot(at) - o)
            .fold(0.0, f32::max)
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

    /// How far a point on this shape's boundary travels along unit `dir` before it leaves, through
    /// the nearest plane `dir` points out of. Only those planes bound it: a boundary point one
    /// rounding outside a side plane parallel to `dir` (a face centroid at the face's edge) still
    /// exits through the back, and one past a plane `dir` leaves through has left (zero).
    pub fn exit(&self, from: Vec3, dir: Vec3) -> f32 {
        self.planes
            .iter()
            .filter(|(n, _)| n.dot(dir) > 0.0)
            .map(|&(n, o)| (o - n.dot(from)).max(0.0) / n.dot(dir))
            .min_by(f32::total_cmp)
            .expect("a bounded hull has a plane facing every direction")
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

    /// Every face as its outward unit normal and its area, m² (the shape's own units squared).
    pub fn face_areas(&self) -> Vec<(Vec3, f32)> {
        self.faces()
            .into_iter()
            .map(|(normal, corners)| {
                let twice: Vec3 = (1..corners.len() - 1)
                    .map(|k| (corners[k] - corners[0]).cross(corners[k + 1] - corners[0]))
                    .sum();
                (normal, 0.5 * twice.dot(normal).abs())
            })
            .collect()
    }
}

/// The area a convex shape of faces `faces` ([`Block::face_areas`]) shows a ray along unit `dir`:
/// its silhouette across the ray, the faces turned toward the ray each projected onto the plane
/// across it.
pub(crate) fn silhouette(faces: &[(Vec3, f32)], dir: Vec3) -> f32 {
    faces
        .iter()
        .map(|&(normal, area)| area * (-normal.dot(dir)).max(0.0))
        .sum()
}

/// Per ray stop `(target, section, silhouette)`, the share of its cone its target intercepts: a
/// ray stands for a cone, `section` its cross-section where it stops ([`Blast::section`]), and the
/// cones that stop on one target together cover at most the target's own silhouette across them,
/// their `section`-weighted mean `silhouette` ([`silhouette`]). Each stop on a target the cones
/// overfill keeps `silhouette / Σ section`, so the target takes the blast over its own area, and
/// a target the cones do not fill takes each whole (1). In stop order.
pub(crate) fn intercepted<K: Ord + Copy>(stops: &[(K, f32, f32)]) -> Vec<f32> {
    let mut per: std::collections::BTreeMap<K, (f32, f32)> = Default::default();
    for &(target, section, area) in stops {
        let sum = per.entry(target).or_default();
        sum.0 += section;
        sum.1 += section * area;
    }
    stops
        .iter()
        .map(|(target, ..)| {
            let (sections, weighted) = per[target];
            if sections <= 0.0 {
                return 1.0;
            }
            (weighted / sections / sections).min(1.0)
        })
        .collect()
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

/// A structure's live state: each chunk's pool (m³, the volume left to crush) and fate, and each
/// joint's crack.
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

    /// The contacts between standing matter: every seam, cracked or not (a crack opens a seam's
    /// tension, never its contact, so the chunks either side still touch), and every rest not
    /// yet opened. Each occupies area where a blast's rear boundary would otherwise be free; an
    /// opened rest or a departed neighbour occupies none.
    pub fn contacts<'a>(&'a self, structure: &'a Structure) -> impl Iterator<Item = Joint> + 'a {
        structure
            .joints
            .iter()
            .enumerate()
            .filter_map(|(j, joint)| {
                let touching = match joint.bond {
                    Bond::Bonded(_) => true,
                    Bond::Resting => !self.cracked[j],
                };
                (touching && self.live(joint)).then_some(*joint)
            })
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

/// A chunk's full pool, m³: its volume.
pub(crate) fn full_pool(chunk: &Chunk) -> f32 {
    chunk.volume
}

/// Wear `chunk`'s pool by `volume` m³ crushed; a pool spent to a millionth of full turns it to
/// dust. Returns whether it did now.
pub(crate) fn wear(
    structure: &Structure,
    state: &mut WallState,
    chunk: usize,
    volume: f32,
) -> bool {
    if !state.standing(chunk) {
        return false;
    }
    state.pool[chunk] = (state.pool[chunk] - volume).max(0.0);
    let dust = state.pool[chunk] <= 1e-6 * full_pool(&structure.chunks[chunk]);
    if dust {
        state.fate[chunk] = Fate::Dust;
    }
    dust
}

/// One load on a structure, about the game's environment destruction (game state only): what one
/// source puts through one point of one chunk in one deposit. Its crushed volume is what it takes
/// from the chunk's pool ([`strike`]); its kinetic cavity energy stresses the joints about it
/// ([`crack`]); its blow is the momentum it delivers there, which bends the
/// sections that hold the chunk and leaves with a group it frees ([`structural`]); its press is
/// the force it holds there through the tick, read with gravity by [`weigh`]. A field's blow is
/// its own law's: a round's line
/// ([`Load::line`]), a fragment's or spall ray's stop ([`Load::field`]), a blast ray's
/// ([`Load::blast`]), none of which presses. A body's contact (`super::breakage`) spends no energy,
/// crushes nothing, and its blow and press are what the solver moved through the point. The wall design owns the
/// model (`.agents/docs/design/wall-destruction.md`, "Bodies in contact").
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Load {
    /// The chunk it enters.
    pub chunk: usize,
    /// Where, in the structure's frame, m.
    pub at: Vec3,
    /// Kinetic cavity energy, J: a line's or fragment's source for [`crack`]. Zero for a blast,
    /// whose pressure and impulse act through the blast laws, and for a body contact.
    pub energy: f32,
    /// The volume it crushes, m³: a line's or a fragment's energy over its chunk's crush energy
    /// ([`Masonry::crush`]); a blast's, its rays' Law 1 volumes ([`Masonry::crushed_by`]), which
    /// its energy never enters.
    pub crushed: f32,
    /// N·s.
    pub blow: Vec3,
    /// N.
    pub press: Vec3,
    /// The force its blow peaks at (N), where its source states one: a blast's, the peak
    /// overpressure over the patches its rays load, into the faces ([`Blast::peak`]), its rays
    /// summed as if they arrive together (ASSUMED: one chunk's rays do). `None` where the source
    /// states none: a round's line (its blow is over before a section moves, ASSUMED), a fragment's
    /// stop (an impulsive blow) and a body's contact (its force is its press). The section and rest laws
    /// read it against their strength ([`Load::felt`]).
    pub peak: Option<Vec3>,
    pub origin: Origin,
}

/// Where a load comes from: what its blow shares momentum with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Origin {
    /// A field (a round's line, a fragment, a blast): its blow is its own law's, and it has no
    /// mass to share with what it frees.
    Field,
    /// A body in contact, by its entity: it shares its blow's momentum with whatever the blow
    /// frees. `mass` is the body's effective mass along the contact normal at the combined
    /// blow's centroid, kg, shared by that contact's points ([`crate::contact_transfer`]).
    Body {
        body: Entity,
        mass: f32,
        /// Closing speed along the normal at the solver's start, m/s; shared launch caps read it.
        speed: f32,
        /// The numerical physics step the solver delivered its blow within, s, not a material
        /// contact duration. `blow / step` bounds the contact's peak force from below
        /// ([`Load::mean_force`]); see the wall design's "Body contact duration" limitation.
        step: f32,
    },
}

/// What a section's or ring's strength leaves the blows on it ([`Load::felt`]): the plastic
/// demand factor a falling pulse's blows meet ([`plastic_demand`]), and whether the strength
/// yields to gravity, the presses and every body blow's least mean force together. The second
/// is the two-asymptote rule's force limit for a body's blow: its share pays the energy only
/// where its force beats the strength too.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reading {
    pub falling: f32,
    pub yields: bool,
}

impl Reading {
    /// Where no strength is read (a group on both sides of its section's plane): every blow keeps
    /// the energy limit alone.
    pub(crate) const UNRESISTED: Self = Self {
        falling: 1.0,
        yields: true,
    };
}

impl Origin {
    /// The plastic share against a receiver's inverse effective mass (kg⁻¹). DERIVED:
    /// `m_receiver / (m_source + m_receiver)`, written without an infinite hinge mass when
    /// a load acts on the hinge. A field is the zero-source-mass limit of this same exchange.
    pub(crate) fn share(self, inverse_mass: f32) -> f32 {
        let mass = match self {
            Self::Field => 0.0,
            Self::Body { mass, .. } => mass,
        };
        // A locked source has no response along the contact normal. Its plastic share is the
        // infinite-source-mass limit, including an inverse hinge mass rounded to zero.
        if mass.is_infinite() {
            return 0.0;
        }
        1.0 / (1.0 + mass * inverse_mass)
    }
}

impl Load {
    /// A round's line through `chunk`, of `material`, from `entry` to `end`, spending `energy` J
    /// and delivering `blow` N·s: one load at the middle of each of the chord's steps of at most
    /// [`LINE_STEP`], the steps sharing both evenly, each crushing its energy's volume.
    pub(crate) fn line(
        chunk: usize,
        material: &Masonry,
        entry: Vec3,
        end: Vec3,
        energy: f32,
        blow: Vec3,
    ) -> impl Iterator<Item = Self> {
        let chord = end - entry;
        let n = (chord.length() / LINE_STEP).ceil().max(1.0) as usize;
        let crush = material.crush;
        (0..n).map(move |k| Self {
            chunk,
            at: entry + chord * ((k as f32 + 0.5) / n as f32),
            energy: energy / n as f32,
            crushed: energy / n as f32 / crush,
            blow: blow / n as f32,
            press: Vec3::ZERO,
            peak: None,
            origin: Origin::Field,
        })
    }

    /// The game's environment destruction model: each field ray's `(chunk, stop, energy J,
    /// blow N·s)` becomes a load at its actual stop, crushing `energy / crush(chunk)` m³. Keeping
    /// individual points preserves angular impulse for nonparallel fragment pushes. A kinetic
    /// fragment states no peak force; its blow uses the same impulsive limit as a kinetic line.
    pub(crate) fn field(
        hits: &[(usize, Vec3, f32, Vec3)],
        crush: impl Fn(usize) -> f32,
    ) -> Vec<Self> {
        hits.iter()
            .map(|&(chunk, at, energy, blow)| Self {
                chunk,
                at,
                energy,
                crushed: energy / crush(chunk),
                blow,
                press: Vec3::ZERO,
                peak: None,
                origin: Origin::Field,
            })
            .collect()
    }

    /// One load per (chunk, face), at its rays' impulse-weighted centroid. The face normal is
    /// shared by its rays, so their pushes are parallel and this point preserves their angular
    /// impulse about every pivot. Opposing faces stay separate even if their blows cancel.
    /// Energy never gates the volume or the push: a pressure load needs neither chemical-energy
    /// wear nor a Lamé source to exist. Each face then meets Law 2 ([`scab`]) on the chunk
    /// `backing` names: a scab it throws takes its momentum off the face's blow, and its volume
    /// leaves the chunk's pool as one more load at the back point, with no blow. Returns the loads
    /// and the scabs.
    pub(crate) fn blast<'a>(
        hits: &[RayHit],
        backing: impl Fn(usize) -> Backing<'a>,
    ) -> (Vec<Self>, Vec<Scab>) {
        let mut faces = std::collections::BTreeMap::new();
        for hit in hits {
            let weight = hit.impulse.length();
            if weight == 0.0 {
                // A cone whose intercepted share is zero delivers nothing.
                assert!(
                    hit.crushed == 0.0 && hit.peak == Vec3::ZERO,
                    "a nonempty blast patch carries a positive impulse"
                );
                continue;
            }
            let key = (hit.chunk, hit.normal.to_array().map(f32::to_bits));
            let (load, total, rays) = faces.entry(key).or_insert_with(|| {
                (
                    Self {
                        chunk: hit.chunk,
                        at: Vec3::ZERO,
                        energy: 0.0,
                        crushed: 0.0,
                        blow: Vec3::ZERO,
                        press: Vec3::ZERO,
                        peak: Some(Vec3::ZERO),
                        origin: Origin::Field,
                    },
                    0.0,
                    Vec::new(),
                )
            });
            rays.push(hit);
            load.at += hit.at * weight;
            load.crushed += hit.crushed;
            load.blow += hit.impulse;
            *load
                .peak
                .as_mut()
                .expect("a blast load states its peak force") += hit.peak;
            *total += weight;
        }
        let (mut loads, mut removals, mut scabs) = (Vec::new(), Vec::new(), Vec::new());
        for (mut load, weight, rays) in faces.into_values() {
            load.at /= weight;
            let backing = backing(load.chunk);
            if let Some(thrown) = scab(&rays, &load, &backing) {
                load.blow -= thrown.momentum();
                removals.push(Self {
                    chunk: load.chunk,
                    at: thrown.at,
                    energy: 0.0,
                    crushed: thrown.mass / backing.material.density,
                    blow: Vec3::ZERO,
                    press: Vec3::ZERO,
                    peak: None,
                    origin: Origin::Field,
                });
                scabs.push(thrown);
            }
            loads.push(load);
        }
        loads.extend(removals);
        (loads, scabs)
    }

    /// The part of this load's blow a section's energy limit reads under `reading`: `√g` of a
    /// blow that peaks at a stated force ([`Load::peak`]), the strength resisting it while the
    /// pulse lasts, so the section absorbs `J² g / 2m` of it; a body's whole blow where the
    /// strength yields to its least mean force ([`Load::mean_force`]) and none of it where the
    /// strength holds that force (the two-asymptote rule); the whole of a round's line, read
    /// impulsively (ASSUMED: its blow is over before a section moves).
    fn felt(&self, reading: Reading) -> f32 {
        match (self.peak, self.origin) {
            (Some(_), _) => reading.falling.sqrt(),
            (None, Origin::Body { .. }) => {
                if reading.yields {
                    1.0
                } else {
                    0.0
                }
            }
            (None, Origin::Field) => 1.0,
        }
    }

    /// A lower bound on a body contact's peak force (N): its blow over the numerical solver
    /// step, not a resolved contact duration. Increasing the physics rate raises this bound for
    /// the same blow; the wall design records that approximation. `None` for a field, which
    /// states its pulse by its peak or not at all.
    fn mean_force(&self) -> Option<Vec3> {
        match self.origin {
            Origin::Body { step, .. } => Some(self.blow / step),
            Origin::Field => None,
        }
    }

    /// The radius of the ball this load's energy alone crushes in `material`, m: where its stress
    /// is clamped, since nothing inside it is left to load.
    fn crushed(&self, material: &Masonry) -> f32 {
        (3.0 * self.energy / (4.0 * PI * material.crush)).cbrt()
    }

    /// The normal stress on a plane of normal `n` at `x`, Pa, tension positive: the Lamé field of a
    /// spherical cavity the load's energy crushed, at pressure the crush energy (`P r_c³ = 3E/4π`),
    /// radial `−3E/(4π d³)` and hoop `+3E/(8π d³)`, the distance clamped at `crushed`, this
    /// load's crushed radius in the joint's material ([`Load::crushed`]).
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

/// Each source's crushed radius in `material`, in source order ([`Load::crushed`]).
fn crushed_radii(sources: &[Load], material: &Masonry) -> Vec<f32> {
    sources.iter().map(|s| s.crushed(material)).collect()
}

/// The stress the push rule puts on `joint` from the loads `sources` (Pa, the mean of the Lamé
/// normal stress over the joint's disc), each clamped at its radius in `crushed` (its crushed
/// radius in the joint's material, [`crushed_radii`]), and the threshold it is compared to:
/// `seam`'s tensile strength.
fn joint_load(joint: &Joint, seam: &Seam, sources: &[Load], crushed: &[f32]) -> (f32, f32) {
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
    (load, seam.tensile)
}

/// Crack every live, intact bonded joint whose stress from the energies of `loads` reaches its
/// seam's threshold ([`joint_load`]); returns the joints cracked now, ascending. A load that spends
/// no kinetic cavity energy sources no stress. A joint's material, whose crush energy clamps
/// the loads, is its chunk `a`'s; a rest is never cracked here ([`Bond::Resting`]). The loads'
/// crushed radii are taken once per crush energy the joints meet, keyed on its bits, not at each
/// disc point of each joint: the same expression of the same values.
pub(crate) fn crack(structure: &Structure, state: &mut WallState, loads: &[Load]) -> Vec<usize> {
    let sources: Vec<Load> = loads
        .iter()
        .filter(|load| load.energy > 0.0)
        .copied()
        .collect();
    if sources.is_empty() {
        return Vec::new();
    }
    let mut radii: Vec<(u32, Vec<f32>)> = Vec::new();
    let mut cracked = Vec::new();
    for (index, joint) in structure.joints.iter().enumerate() {
        let Some(seam) = joint.seam() else {
            continue;
        };
        if state.cracked[index] || !state.live(joint) {
            continue;
        }
        let material = structure.chunks[joint.a].material;
        let key = material.crush.to_bits();
        let at = match radii.iter().position(|(crush, _)| *crush == key) {
            Some(at) => at,
            None => {
                radii.push((key, crushed_radii(&sources, &material)));
                radii.len() - 1
            }
        };
        let (stress, threshold) = joint_load(joint, &seam, &sources, &radii[at].1);
        if stress >= threshold {
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

/// What one deposit did: the chunks it wore without emptying, the chunks it turned to dust and
/// the joints it cracked, each ascending and without repeats.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Struck {
    pub worn: Vec<usize>,
    pub dusted: Vec<usize>,
    pub cracked: Vec<usize>,
}

/// Run `loads` through the removal and the push rule, in that order: every load's crushed volume
/// wears its chunk's pool, then every load's energy stresses the joints still live ([`crack`]). A
/// load that crushes nothing wears nothing. Support is the caller's next step ([`loose_groups`],
/// [`fall`]), so a check that must see the cracks before support (the structural one) runs
/// between.
pub(crate) fn strike(structure: &Structure, state: &mut WallState, loads: &[Load]) -> Struck {
    let mut struck = Struck::default();
    for load in loads.iter().filter(|load| load.crushed > 0.0) {
        if wear(structure, state, load.chunk, load.crushed) {
            struck.dusted.push(load.chunk);
        } else if state.standing(load.chunk) {
            struck.worn.push(load.chunk);
        }
    }
    struck.worn.sort_unstable();
    struck.worn.dedup();
    struck.worn.retain(|chunk| state.standing(*chunk));
    struck.dusted.sort_unstable();
    struck.cracked = crack(structure, state, loads);
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

/// What loaded a failed section, and so the units of its demand and capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cause {
    /// A push turning a group about a section's hinge ([`structural`]), or a part over the hinge of
    /// the rests it stands on ([`unseat`]): the angular impulse about the hinge against the impulse
    /// that would pay for opening the section and lifting the group over it, N·s·m.
    Push,
    /// Gravity and presses on the held group ([`weigh`]): peak tension against its seam's
    /// tensile strength, Pa.
    Weight,
    /// A sustained resultant lifts a part off its bearing rests: force away from them, N.
    LiftForce,
    /// A part tipped off its rests by gravity and presses ([`unseat`]): how far its pressure
    /// centre reaches past the bearing hull, horizontally, against nothing, m
    /// (infinite when none bears). The section is every rest it stood on, as for the two below.
    Tip,
    /// A part slid by gravity and presses ([`unseat`]): tangent force over compression against
    /// the friction coefficient their pairs raise, dimensionless.
    Slide,
    /// A part standing on rests alone lifted off them by a push ([`unseat`]): the push's impulse
    /// along their normal away from them, against the impulse that raises the part's centre of
    /// mass by its height over them, N·s.
    Lift,
    /// A part standing on rests alone shoved off them by a push ([`unseat`]): the push's impulse
    /// along them net of the friction impulse it raises, against the impulse whose energy pays
    /// the friction work of sliding the part off, N·s.
    Shove,
    /// A group punched out through its ring of joints by blows ([`ring_failure`]): the blows'
    /// plastic share along its translation against the impulse whose energy opens the ring, N·s.
    Punch,
    /// A group punched out through its ring of joints by gravity and presses
    /// ([`ring_force_failure`]): their force along its translation against the ring's strength, N.
    PunchForce,
}

/// One structural failure: the section's joints, what loaded it, and the demand on it with the
/// capacity it was compared with, in its cause's units.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Failure {
    pub joints: Vec<usize>,
    pub cause: Cause,
    pub demand: f32,
    pub capacity: f32,
}

/// The live, intact bonded joints' graph: each chunk's neighbours with the joint between, each
/// chunk's live anchors, and whether it is buried (held by the soil, never by a joint a section can
/// cut). A rest is no edge: it carries no tension, so no section's group hangs on one, and a group
/// whose only way to the ground is over rests is the rest law's ([`unseat`]).
type Graph = (Vec<Vec<(usize, usize)>>, Vec<Vec<usize>>, Vec<bool>);

fn graph(structure: &Structure, state: &WallState) -> Graph {
    let count = structure.chunks.len();
    let mut neighbours = vec![Vec::new(); count];
    let mut anchors = vec![Vec::new(); count];
    for (index, joint) in structure.joints.iter().enumerate() {
        if state.cracked[index] || !state.live(joint) || joint.seam().is_none() {
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

/// The sections a structural check tries: every live intact bonded joint alone, and every set of
/// two or more live intact bonded joints lying in one plane (normals parallel, offsets within a
/// millimetre), each ascending, the singles first in joint order, then the planes in creation
/// order. Read from the structure's joints and the state's cracks and fates alone.
fn sections(structure: &Structure, state: &WallState) -> Vec<Vec<usize>> {
    let mut planes: Vec<(Vec3, f32, Vec<usize>)> = Vec::new();
    // The planes by their offset's bucket ([`plane_bucket`]), each bucket in creation order. A
    // joint's plane is the first plane, in creation order, within its offset's tolerance: one
    // whose offset differs by under a millimetre lies in its bucket or the two either side.
    let mut buckets: std::collections::HashMap<i64, Vec<usize>> = Default::default();
    let mut singles = Vec::new();
    for (index, joint) in structure.joints.iter().enumerate() {
        if state.cracked[index] || !state.live(joint) || joint.seam().is_none() {
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
    /// Per joint, the sections holding it, ascending: at most its single and its plane.
    of_joint: Vec<Vec<usize>>,
}

impl SectionCache {
    /// The sections of `structure` in `state`.
    fn read(&mut self, structure: &Structure, state: &WallState) -> &[Vec<usize>] {
        if self.cracked != state.cracked || self.fate != state.fate {
            self.sections = sections(structure, state);
            self.of_joint = vec![Vec::new(); structure.joints.len()];
            for (s, section) in self.sections.iter().enumerate() {
                for &j in section {
                    self.of_joint[j].push(s);
                }
            }
            self.cracked.clone_from(&state.cracked);
            self.fate.clone_from(&state.fate);
        }
        &self.sections
    }

    /// The sections of `structure` in `state`, and the indices, ascending, of those holding a joint
    /// of `paths` ([`ground_path`]): the only sections that can cut one, so the checks visit
    /// those alone, in the order they hold among all, and their work follows the paths' length,
    /// not the building's size.
    fn cutting(
        &mut self,
        structure: &Structure,
        state: &WallState,
        paths: &[Option<Vec<usize>>],
    ) -> (&[Vec<usize>], Vec<usize>) {
        self.read(structure, state);
        let mut cutting: Vec<usize> = paths
            .iter()
            .flatten()
            .flatten()
            .flat_map(|&j| self.of_joint[j].iter().copied())
            .collect();
        cutting.sort_unstable();
        cutting.dedup();
        (&self.sections, cutting)
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

/// The structural check: the loads' blows (a load with none pushes nothing) turn the group a
/// section alone holds to the ground about the section's hinge, the edge of the section that the
/// turn presses (the bending axis is that of the blows' angular impulse in the plane about the
/// section's centroid, `Σ (x − c) × J`), and the section fails when the angular impulse about the
/// hinge, with each body's plastic share against the hinge effective mass and each pulse's blow
/// through the demand the section's strength leaves it ([`section_pulse`]), reaches the
/// impulse whose rotational energy pays for opening the section and for carrying
/// the group's weight with it, `√(2 I (Σ G_f A + E_g))`: `I` the group's moment of inertia about
/// the hinge (each chunk a ball of its volume), `Σ G_f A` each joint's seam fracture energy over
/// its area, and `E_g` the work against gravity of raising the group's centre of mass over the
/// hinge where the section bears the group's weight (the section a blow opens carries its load: a
/// group standing on its section leaves only by toppling over it), 0 for a group beside or under
/// its section. The worst failing section cracks first, and the check repeats until none fails, so
/// a failure can expose the next. The freed group is the physics' from then on, with its weight,
/// contact and friction ([`super::breakage`] launches it with the momentum pushed into it). A
/// pushed part that stands on rests alone (no bonded path to the ground) is the rest law's
/// ([`unseat`]): every rest it stood on opens when the blows can carry it over them, turned about
/// their hinge as a section is with no seam to open, or slid along them against their friction.
/// Beside the sections, the pushed chunks' punch rings compete on the same ratio
/// ([`ring_groups`], [`ring_failure`]): a group translated out through every joint around it.
/// A body's blow meets both limits on each: its least mean force must beat the strength there,
/// and its share must pay the energy ([`Reading`]).
///
/// The group a section holds is the part of the pushed chunks' component that loses its last path
/// to the ground with the section's joints removed ([`held`]): a group no blow reaches takes no
/// demand, and a blow on a chunk with no path to the ground left (the push rule freed it) bends no
/// section, its group leaving as a body with that blow ([`super::breakage`]). Only the sections
/// that cut a pushed chunk's path to the ground are searched for it ([`ground_path`]). Reads the
/// chunk table and the joint graph only; `sections` is `structure`'s cache of them
/// ([`SectionCache`]).
pub(crate) fn structural(
    structure: &Structure,
    state: &mut WallState,
    loads: &[Load],
    sections: &mut SectionCache,
) -> Vec<Failure> {
    let mut failures = Vec::new();
    let pushed = pushed(structure, loads);
    if pushed.is_empty() {
        return failures;
    }
    let mut steps = 0;
    let incident = incident(structure);
    // Every pass cracks an intact joint (asserted where it cracks) and none closes again, so
    // the passes end.
    loop {
        let graph = graph(structure, state);
        let paths: Vec<Option<Vec<usize>>> = pushed
            .iter()
            .map(|&chunk| {
                state
                    .standing(chunk)
                    .then(|| ground_path(chunk, &graph, &mut steps))
                    .flatten()
            })
            .collect();
        let crossing = crossing(structure.joints.len(), &paths);
        let mut worst: Option<(f32, Failure)> = None;
        let (all, cutting) = sections.cutting(structure, state, &paths);
        for section in cutting.iter().map(|&s| &all[s]) {
            // The pushed chunks whose path the section cuts, in their order: the only ones the
            // search reads for it.
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
            for chunk in cut.into_iter().map(|k| pushed[k]) {
                if group.binary_search(&chunk).is_ok() {
                    continue;
                }
                if let Some(reached) = held(chunk, section, &graph, &mut steps) {
                    group.extend(reached);
                    group.sort_unstable();
                    group.dedup();
                }
            }
            if group.is_empty() {
                continue;
            }
            let pulse = section_pulse(structure, section, &group, loads, &mut steps);
            if let Some(failure) = section_failure(structure, section, &group, loads, pulse) {
                let ratio = failure.demand / failure.capacity;
                if ratio >= 1.0 && worst.as_ref().is_none_or(|(w, _)| ratio > *w) {
                    worst = Some((ratio, failure));
                }
            }
        }
        for group in ring_groups(state, &graph, &pushed, &mut steps) {
            if let Some(failure) = ring_failure(structure, state, &incident, &group, loads) {
                let ratio = failure.demand / failure.capacity;
                if ratio >= 1.0 && worst.as_ref().is_none_or(|(w, _)| ratio > *w) {
                    worst = Some((ratio, failure));
                }
            }
        }
        let Some((_, failure)) = worst else {
            break;
        };
        assert!(
            failure.joints.iter().any(|&j| !state.cracked[j]),
            "a failing section or ring holds an intact joint on a ground path, so each pass \
             opens one and the passes end"
        );
        for &j in &failure.joints {
            state.cracked[j] = true;
        }
        failures.push(failure);
    }
    // A pushed part that stands on rests alone (no bonded path to the ground) leaves with its
    // blows when the rest law opens every rest it stood on.
    let graph = graph(structure, state);
    let mut seen = vec![false; structure.chunks.len()];
    for &chunk in &pushed {
        if !state.standing(chunk) || seen[chunk] || ground_path(chunk, &graph, &mut steps).is_some()
        {
            continue;
        }
        let group = bonded_group(chunk, &graph, &mut seen);
        if let Some(failure) = unseat(
            structure,
            state,
            &group,
            loads,
            Vec3::NEG_Y * GRAVITY,
            &mut steps,
        ) {
            for &j in &failure.joints {
                state.cracked[j] = true;
            }
            failures.push(failure);
        }
    }
    failures
}

/// The chunks a blow of `loads` reaches, each once, in the order of its first such load.
fn pushed(structure: &Structure, loads: &[Load]) -> Vec<usize> {
    let mut seen = vec![false; structure.chunks.len()];
    let mut pushed = Vec::new();
    for load in loads {
        if load.blow != Vec3::ZERO && !seen[load.chunk] {
            seen[load.chunk] = true;
            pushed.push(load.chunk);
        }
    }
    pushed
}

/// The chunks joined to `start` through `graph`'s intact bonded joints, ascending, each marked in
/// `seen`.
fn bonded_group(start: usize, (neighbours, ..): &Graph, seen: &mut [bool]) -> Vec<usize> {
    seen[start] = true;
    let mut group = vec![start];
    let mut next = 0;
    while let Some(&chunk) = group.get(next) {
        next += 1;
        for &(neighbour, _) in &neighbours[chunk] {
            if !seen[neighbour] {
                seen[neighbour] = true;
                group.push(neighbour);
            }
        }
    }
    group.sort_unstable();
    group
}

/// The demand and capacity of `section` holding `group` under the blows of `loads`
/// ([`structural`]), `None` when they bend it about no axis. `pulse` is what the turn's strength
/// leaves the blows ([`Reading`], [`Load::felt`]), the caller's: a seam's Navier tension resists a
/// bonded section's turn ([`section_pulse`]), a seated part's tipping its turn over its rests
/// ([`unseat`]).
fn section_failure(
    structure: &Structure,
    section: &[usize],
    group: &[usize],
    loads: &[Load],
    pulse: Reading,
) -> Option<Failure> {
    let joints: Vec<&Joint> = section.iter().map(|&j| &structure.joints[j]).collect();
    let area: f32 = joints.iter().map(|joint| joint.area).sum();
    let centre = joints
        .iter()
        .map(|joint| joint.centroid * joint.area)
        .sum::<Vec3>()
        / area;
    let (mass, weighted) = group.iter().fold((0.0, Vec3::ZERO), |(mass, at), &i| {
        let chunk = &structure.chunks[i];
        (mass + chunk.mass, at + chunk.centroid * chunk.mass)
    });
    let com = weighted / mass;
    // The plane's normal toward the group, a group point moving along it leaving the section: the
    // joints' own, each turned toward the group, by area (one normal for a plane of joints).
    let n = joints
        .iter()
        .map(|joint| {
            let n = joint.normal;
            joint.area
                * if n.dot(com - joint.centroid) >= 0.0 {
                    n
                } else {
                    -n
                }
        })
        .sum::<Vec3>()
        .try_normalize()?;
    let on_group = || {
        loads
            .iter()
            .filter(|load| group.binary_search(&load.chunk).is_ok())
    };
    let moment: Vec3 = on_group()
        .map(|load| (load.at - centre).cross(load.blow * load.felt(pulse)))
        .sum();
    let bending = moment - n * moment.dot(n);
    if bending.length() <= 0.0 {
        return None;
    }
    let axis = bending.normalize();
    // The hinge: the section's edge farthest back along `n × axis`, where the turn presses.
    let across = n.cross(axis);
    let back = joints
        .iter()
        .map(|joint| across.dot(joint.centroid - centre) - (joint.area / PI).sqrt())
        .fold(f32::INFINITY, f32::min);
    let hinge = centre + across * back;
    let inertia: f32 = group
        .iter()
        .map(|&i| {
            let chunk = &structure.chunks[i];
            let r = chunk.centroid - hinge;
            let own = (3.0 * chunk.volume / (4.0 * PI)).cbrt();
            chunk.mass * (r.length_squared() - r.dot(axis).powi(2) + 0.4 * own * own)
        })
        .sum();
    let demand: f32 = on_group()
        .map(|load| {
            let moment = (load.at - hinge).cross(load.blow).dot(axis);
            // A blow with no moment about the hinge's axis (none at all, or one whose line meets
            // the axis) turns nothing whatever its source's mass, so an immovable source (a
            // locked body's contact, infinite mass) adds nothing rather than 0 × ∞.
            if moment == 0.0 {
                return 0.0;
            }
            let lever = moment / load.blow.length();
            moment * load.origin.share(lever * lever / inertia) * load.felt(pulse)
        })
        .sum();
    if demand <= 0.0 {
        return None;
    }
    // The work the turn does against the group's weight where the section bears it (`n` rising,
    // the weight pressing the plane): the opened group still stands on the plane and leaves only
    // past the hinge, its centre of mass raised to the top of its circle about it when the turn
    // starts it upward. A group beside or under its section has nothing under it once it opens.
    let r = (com - hinge) - axis * (com - hinge).dot(axis);
    let up = axis.cross(r).y;
    let lift = if n.y > 0.0 && up > 0.0 {
        mass * GRAVITY * (r.y.hypot(up) - r.y)
    } else {
        0.0
    };
    // A rest has no fracture work: only its seam's opens a bonded joint.
    let cohesion: f32 = joints
        .iter()
        .filter_map(|joint| Some(joint.seam()?.fracture_energy * joint.area))
        .sum();
    Some(Failure {
        joints: section.to_vec(),
        cause: Cause::Push,
        demand,
        capacity: (2.0 * inertia * (cohesion + lift)).sqrt(),
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

/// The strength check under gravity plus `loads`' presses. It starts from pressed chunks and
/// chunks whose support changed ([`touched`]: each side of a joint in `cracked`, each neighbour of
/// `dusted`, ascending), plus `unloaded` chunks whose former press disappeared. With none of
/// those seeds it does no work. For each section cutting a touched chunk's path to the ground
/// ([`ground_path`]), the group it alone holds
/// with that chunk ([`held`]) hangs from the section's joints that border it, its one cut set:
/// statics fixes the force and moment through it exactly, and [`weight_failure`] reads them as a
/// stress. The pressed chunks' punch rings compete on the same ratio, force over strength
/// ([`ring_groups`], [`ring_force_failure`]). The worst section or ring at or over its strength
/// cracks, the chunks on either side of it are
/// touched in turn, and the check repeats until none fails (each pass cracks at least one joint
/// and none closes, so the passes end), so a failure can expose the next. A freed group falls by support ([`fall`]) and the
/// physics.
///
/// A touched chunk that no section alone holds with a group (its loads reaching the ground through
/// several sections at once, as a span between two piers, or through a cut set no single plane
/// holds, or a group that also bears on a rest or a cracked seam from below,
/// [`bears_on_contact`]) stands unassessed and is counted ([`Weighed::unassessed`]): a static
/// solve's to decide.
/// A group inside a larger one a section holds (that span under the wall above it) is assessed
/// only as part of the larger. A fall adds no touched chunk: a loose group has no intact joint to a
/// grounded one, so it neither held nor hung on anything that stands. Reads the chunk table and
/// the joint graph only; `sections` is `structure`'s cache of them ([`SectionCache`]).
pub(crate) fn weigh(
    structure: &Structure,
    state: &mut WallState,
    dusted: &[usize],
    cracked: &[usize],
    loads: &[Load],
    unloaded: &[usize],
    sections: &mut SectionCache,
) -> Weighed {
    let mut weighed = Weighed::default();
    if dusted.is_empty()
        && cracked.is_empty()
        && unloaded.is_empty()
        && !loads.iter().any(|load| load.press != Vec3::ZERO)
    {
        return weighed;
    }
    let mut touched = touched(structure, state, dusted, cracked, &mut weighed.steps);
    touched.extend(
        loads
            .iter()
            .filter(|load| load.press != Vec3::ZERO)
            .map(|load| load.chunk),
    );
    touched.extend_from_slice(unloaded);
    touched.sort_unstable();
    touched.dedup();
    // Each chunk's joints, the anchors too: what a held group may bear on besides its section.
    let incident = incident(structure);
    weighed.steps += structure.joints.len();
    let pressed: Vec<usize> = loads
        .iter()
        .filter(|load| load.press != Vec3::ZERO)
        .map(|load| load.chunk)
        .collect();
    // Every pass cracks an intact joint (asserted where it cracks) and none closes again, so
    // the passes end.
    loop {
        touched.retain(|&c| state.standing(c));
        if touched.is_empty() {
            break;
        }
        let graph = graph(structure, state);
        // A disconnected group is about to leave the standing structure. Its contacts cannot
        // be credited as static props while the remaining structure is assessed.
        let mut grounded = vec![true; structure.chunks.len()];
        for chunk in loose_groups(structure, state).into_iter().flatten() {
            grounded[chunk] = false;
        }
        weighed.steps += structure.chunks.len() + structure.joints.len();
        let paths: Vec<Option<Vec<usize>>> = touched
            .iter()
            .map(|&c| ground_path(c, &graph, &mut weighed.steps))
            .collect();
        let crossing = crossing(structure.joints.len(), &paths);
        let mut assessed = vec![false; touched.len()];
        let mut worst: Option<(f32, Failure)> = None;
        let (all, cutting) = sections.cutting(structure, state, &paths);
        for section in cutting.iter().map(|&s| &all[s]) {
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
                let failure = (!bears_on_contact(structure, state, &incident, &grounded, &group))
                    .then(|| {
                        weight_failure(
                            structure,
                            section,
                            &group,
                            loads,
                            Vec3::NEG_Y * GRAVITY,
                            &mut weighed.steps,
                        )
                    })
                    .flatten();
                weighed.steps += group.len();
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
        for group in ring_groups(state, &graph, &pressed, &mut weighed.steps) {
            let failure = ring_force_failure(
                structure,
                state,
                &incident,
                &group,
                loads,
                Vec3::NEG_Y * GRAVITY,
            );
            if let Some(failure) = failure {
                let ratio = failure.demand / failure.capacity;
                if ratio >= 1.0 && worst.as_ref().is_none_or(|(w, _)| ratio > *w) {
                    worst = Some((ratio, failure));
                }
            }
        }
        let failure = match worst {
            Some((_, failure)) => failure,
            None => match unseated(
                structure,
                state,
                &graph,
                &touched,
                &paths,
                loads,
                &mut weighed.steps,
            ) {
                Some(failure) => failure,
                None => {
                    // A chunk with no path stands on rests (the rest check held it) or is loose
                    // and falls; one whose path is empty is buried, the soil's to hold.
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
                }
            },
        };
        assert!(
            failure.joints.iter().any(|&j| !state.cracked[j]),
            "a failing section, ring or rest holds an intact joint, so each pass opens one and \
             the passes end"
        );
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

/// Whether `group` (ascending) bears on a contact besides the section that holds it: a live joint
/// from one of its chunks (`incident`, each chunk's joints) to a standing chunk outside it, or to
/// the ground, that carries no tension but still carries compression, its normal toward the group
/// rising past [`BEARS`]. Such a contact is a rest not yet opened, or a seam a crack opened: a
/// crack opens a seam's tension, never its contact, and the chunks either side still touch.
/// The outside chunk must remain grounded: a disconnected group about to fall is no static prop.
/// A group that bears on one is held through it and its section at once, a static solve's to read
/// ([`weigh`]), as a span between two piers is.
fn bears_on_contact(
    structure: &Structure,
    state: &WallState,
    incident: &[Vec<usize>],
    grounded: &[bool],
    group: &[usize],
) -> bool {
    let inside = |c: usize| group.binary_search(&c).is_ok();
    group.iter().any(|&chunk| {
        incident[chunk].iter().any(|&j| {
            let joint = &structure.joints[j];
            let contact = match joint.bond {
                Bond::Resting => !state.cracked[j],
                Bond::Bonded(_) => state.cracked[j],
            };
            let outside = match joint.b {
                Side::Chunk(b) if inside(b) != inside(joint.a) => {
                    grounded[if inside(b) { joint.a } else { b }]
                }
                Side::Chunk(_) => false,
                Side::Ground => true,
            };
            if !contact || !outside || !state.live(joint) {
                return false;
            }
            let toward = structure.chunks[chunk].centroid - joint.centroid;
            let n = if toward.dot(joint.normal) >= 0.0 {
                joint.normal
            } else {
                -joint.normal
            };
            n.y > BEARS
        })
    })
}

/// How far a rest's normal, turned toward the part it carries, must rise to bear that part: above
/// the bake's rounding of a vertical face's normal (a millionth), far below any slope a part sits
/// on. A rest at or under it (a part's end butted against a wall, a part hanging under another)
/// bears nothing.
const BEARS: f32 = 1e-3;

/// The rest check, for the touched chunks with no bonded path to the ground (`paths`, one per
/// chunk of `touched`, [`ground_path`]): each such chunk's bonded group (its chunks joined to it
/// through intact bonded joints, `graph`) stands on rests alone, and the rest law reads it under
/// gravity and the presses and blows in `loads` ([`unseat`]). The first group, in `touched`
/// order, that leaves its rests is the failure; `None` when every group stays. Adds the chunks it
/// walks and the joints it reads to `steps`.
fn unseated(
    structure: &Structure,
    state: &WallState,
    graph: &Graph,
    touched: &[usize],
    paths: &[Option<Vec<usize>>],
    loads: &[Load],
    steps: &mut usize,
) -> Option<Failure> {
    let mut seen = vec![false; structure.chunks.len()];
    for (&start, path) in touched.iter().zip(paths) {
        if path.is_some() || seen[start] {
            continue;
        }
        let group = bonded_group(start, graph, &mut seen);
        *steps += group.len();
        if let Some(failure) = unseat(
            structure,
            state,
            &group,
            loads,
            Vec3::NEG_Y * GRAVITY,
            steps,
        ) {
            return Some(failure);
        }
    }
    None
}

/// The rest law: whether `group` (ascending), a part with no bonded path to the ground, leaves the
/// rests it stands on under gravity and the presses and blows of `loads` on its chunks. Its rests
/// are the
/// live intact rests joining it to standing chunks outside it; those that bear it are the ones
/// whose normal toward it rises past [`BEARS`], each a disc of its area in its plane (the tension
/// check's reading of a joint), pressed evenly over their area by what holds the part up
/// (ASSUMED: the rests' shares of the load are statically indeterminate, and an even pressure is
/// the reading that needs no stiffness). Each bearing rest's friction coefficient is its pair's,
/// its two materials' rows under the one pair rule ([`crate::substances::pair_friction`]). With
/// `N = Σ A n` over the bearing rests, `θ` its tilt from vertical and `μ = Σ μ_i A_i / |N|` the
/// friction the even pressure raises per unit of the load along `N` (ASSUMED: each rest's
/// friction can turn against the slide, exact where the rests are parallel), the part leaves, in
/// this order of reading:
///
/// 1. lifted by the sustained resultant ([`Cause::LiftForce`]) when it pulls away from the rests;
///    tipped ([`Cause::Tip`]) when its pressure centre, fixed by the resultant moment, crosses
///    outside the horizontal bearing hull (ASSUMED: bearing plane at the discs' mean height);
/// 2. slid ([`Cause::Slide`]) when tangent force exceeds friction times normal compression;
/// 3. turned over by the blows ([`Cause::Push`]) when their angular impulse about the bearing
///    rests' hinge pays for lifting the part over it, a section with no seam to open
///    ([`section_failure`]), which reads only their moment, so a blow through the centre along
///    the rests' normal turns nothing;
/// 4. lifted off by the blows ([`Cause::Lift`]) when their impulse along `N̂`, away from the
///    rests (which carry no tension), raises the part's centre of mass by its own height over
///    them against gravity's share along `N̂`: `J·N̂ ≥ m √(2 g N̂_y h)`, `h` the centre's height
///    along `N̂` over the bearing discs' area centroid. ASSUMED: lifted by as much as it stood
///    over its seat, the part has left it; a lower hop lands it back where it stood, which the
///    law reads as staying;
/// 5. shoved off by the blows ([`Cause::Shove`]) when their impulse along the rests, net of the
///    friction impulse their press into the rests raises, `p = |J_t| − μ max(−J·N̂, 0)`, gives
///    the part the energy to slide its centre of mass off the bearing hull along that impulse
///    against friction and with gravity's share along the slope: `p ≥ √(2 m W)`, `W = m g d (μ
///    cos θ + t_y)`, `d` the slide's length in the rests' plane and `t` its unit direction. The
///    weight is held to raise no friction impulse during a blow (ASSUMED: a blow is
///    instantaneous) and the part to slide in contact the whole way (ASSUMED: a blow that also
///    lifts it slides it farther than this reads).
///
/// In 3–5 a pulse's blow (one stating its peak force, [`Load::peak`]) counts by `√g` of its
/// mode's strength over the peaks ([`plastic_demand`], [`strength_ratio`]): the share of the
/// peak forces at which, with gravity and the presses, the sustained law's twin of that mode
/// fails (tipping for 3, lifting for 4, sliding for 5). A far burst whose peak the part's weight
/// and friction hold moves it by no blow.
///
/// Any of them opens every rest the part stood on, as one failure; `None` when it stays, and when
/// it has no rest left (it is loose, support's to drop, [`fall`]). Once the part is freed the
/// physics' contact and friction decide whether it slides, rocks or settles. ASSUMED: a rest bears
/// no part of another's weight that reaches it through the part it carries; the law reads each
/// part's own weight and external presses on its own rests. Adds the joints and chunks it reads
/// to `steps`.
fn unseat(
    structure: &Structure,
    state: &WallState,
    group: &[usize],
    loads: &[Load],
    gravity: Vec3,
    steps: &mut usize,
) -> Option<Failure> {
    let inside = |c: usize| group.binary_search(&c).is_ok();
    let (mut rests, mut bearing) = (Vec::new(), Vec::new());
    let mut rim: Vec<(f32, f32)> = Vec::new();
    // `Σ A n`, `Σ μ A` and `Σ A c` over the bearing rests.
    let (mut normal, mut grip, mut seat) = (Vec3::ZERO, 0.0, Vec3::ZERO);
    *steps += structure.joints.len() + group.len();
    for (index, joint) in structure.joints.iter().enumerate() {
        if joint.seam().is_some() || state.cracked[index] || !state.live(joint) {
            continue;
        }
        let Side::Chunk(b) = joint.b else {
            continue;
        };
        let held = match (inside(joint.a), inside(b)) {
            (true, false) => joint.a,
            (false, true) => b,
            _ => continue,
        };
        rests.push(index);
        let toward = structure.chunks[held].centroid - joint.centroid;
        let n = if toward.dot(joint.normal) >= 0.0 {
            joint.normal
        } else {
            -joint.normal
        };
        if n.y <= BEARS {
            continue;
        }
        bearing.push(index);
        normal += n * joint.area;
        seat += joint.centroid * joint.area;
        grip += joint.area
            * crate::substances::pair_friction(
                structure.chunks[joint.a].material.friction,
                structure.chunks[b].material.friction,
            );
        let (u, v) = plane_axes(n);
        let radius = (joint.area / PI).sqrt();
        rim.extend((0..8).map(|k| {
            let angle = k as f32 * PI / 4.0;
            let p = joint.centroid + (u * angle.cos() + v * angle.sin()) * radius;
            (p.x, p.z)
        }));
    }
    if rests.is_empty() {
        return None;
    }
    let open = |cause, demand, capacity| {
        Some(Failure {
            joints: rests.clone(),
            cause,
            demand,
            capacity,
        })
    };
    let (mass, moment) = group.iter().fold((0.0, Vec3::ZERO), |(mass, moment), &i| {
        let chunk = &structure.chunks[i];
        (mass + chunk.mass, moment + chunk.centroid * chunk.mass)
    });
    let centre = moment / mass;
    let hull = convex_hull(&mut rim);
    if hull.len() < 3 {
        return open(Cause::Tip, f32::INFINITY, 0.0);
    }
    // ASSUMED: non-coplanar bearing discs share one plane at their area centroid's height,
    // as their horizontal bearing hull does. The resultant's moment fixes its pressure centre.
    let area: f32 = bearing.iter().map(|&j| structure.joints[j].area).sum();
    let seat_centre = seat / area;
    let (force, torque) = force_wrench(structure, group, loads, gravity, seat_centre, |load| {
        Some(load.press)
    });
    let support = normal.normalize();
    let friction = grip / normal.length();
    // The sustained law on a resultant force `f` (N) and moment `t` (N m) about the seat: it
    // lifts the part when it pulls away from the rests; it tips it by how far its pressure
    // centre lies past the bearing hull, infinitely when a resultant with no vertical part
    // still overturns it about a horizontal axis; it slides it when its tangent beats friction
    // times its compression.
    let compression = |f: Vec3| -f.dot(support);
    let lifts = |f: Vec3| f.y > 0.0 || compression(f) < 0.0;
    let tip = |f: Vec3, t: Vec3| {
        if f.y < 0.0 {
            let pressure = seat_centre + Vec3::new(t.z / f.y, 0.0, -t.x / f.y);
            beyond(&hull, (pressure.x, pressure.z))
        } else if t.x != 0.0 || t.z != 0.0 {
            f32::INFINITY
        } else {
            0.0
        }
    };
    let tangent = |f: Vec3| (f + support * compression(f)).length();
    let slides = |f: Vec3| tangent(f) > friction * compression(f);
    if lifts(force) {
        return open(Cause::LiftForce, force.y.max(-compression(force)), 0.0);
    }
    let reach = tip(force, torque);
    if reach > 0.0 {
        return open(Cause::Tip, reach, 0.0);
    }
    if slides(force) {
        let ratio = if compression(force) > 0.0 {
            tangent(force) / compression(force)
        } else {
            f32::INFINITY
        };
        return open(Cause::Slide, ratio, friction);
    }
    if !loads
        .iter()
        .any(|load| load.blow != Vec3::ZERO && inside(load.chunk))
    {
        return None;
    }
    // Each blow mode meets a pulse's peak forces, and a body blow's least mean force, with its
    // twin in the sustained law: the turn with tipping, the lift with lifting, the shove with
    // sliding ([`strength_ratio`], [`Reading`]).
    let (peak_force, peak_torque) =
        force_wrench(structure, group, loads, Vec3::ZERO, seat_centre, |load| {
            load.peak
        });
    let (mean_force, mean_torque) = force_wrench(
        structure,
        group,
        loads,
        Vec3::ZERO,
        seat_centre,
        Load::mean_force,
    );
    let at = |s: f32| (force + peak_force * s, torque + peak_torque * s);
    let (mean, mean_turn) = (force + mean_force, torque + mean_torque);
    let tip_pulse = Reading {
        falling: plastic_demand(strength_ratio(|s| {
            let (f, t) = at(s);
            lifts(f) || tip(f, t) > 0.0
        })),
        yields: lifts(mean) || tip(mean, mean_turn) > 0.0,
    };
    let lift_pulse = Reading {
        falling: plastic_demand(strength_ratio(|s| lifts(at(s).0))),
        yields: lifts(mean),
    };
    let slide_pulse = Reading {
        falling: plastic_demand(strength_ratio(|s| {
            let f = at(s).0;
            lifts(f) || slides(f)
        })),
        yields: lifts(mean) || slides(mean),
    };
    if let Some(turn) = section_failure(structure, &bearing, group, loads, tip_pulse)
        && turn.demand >= turn.capacity
    {
        return open(Cause::Push, turn.demand, turn.capacity);
    }
    // The blows' plastic shares against the part's mass, each read in `pulse`'s mode.
    let felt = |pulse: Reading| -> Vec3 {
        loads
            .iter()
            .filter(|load| inside(load.chunk))
            .map(|load| load.blow * load.origin.share(1.0 / mass) * load.felt(pulse))
            .sum()
    };
    let press = felt(lift_pulse).dot(support);
    if press > 0.0 {
        let height = (centre - seat_centre).dot(support).max(0.0);
        let capacity = mass * (2.0 * GRAVITY * support.y * height).sqrt();
        if press >= capacity {
            return open(Cause::Lift, press, capacity);
        }
    }
    let total = felt(slide_pulse);
    let press = total.dot(support);
    let along = total - support * press;
    let net = along.length() - friction * (-press).max(0.0);
    if net <= 0.0 {
        return None;
    }
    // `t` lies in the rests' plane, which the slope check held within `atan μ` of level, so its
    // horizontal part is at least `cos θ` long.
    let t = along.normalize();
    let level = t.x.hypot(t.z);
    let slide = exit(&hull, (centre.x, centre.z), (t.x / level, t.z / level)) / level;
    let work = mass * GRAVITY * slide * (friction * support.y + t.y);
    let capacity = (2.0 * mass * work.max(0.0)).sqrt();
    if net >= capacity {
        return open(Cause::Shove, net, capacity);
    }
    None
}

/// The convex hull of `points` (reordered), counter-clockwise, by Andrew's monotone chain: fewer
/// than three corners when the points span no area.
fn convex_hull(points: &mut [(f32, f32)]) -> Vec<(f32, f32)> {
    points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    // The lower hull left to right, then the upper right to left.
    let mut hull: Vec<(f32, f32)> = Vec::with_capacity(points.len() + 1);
    let push = |hull: &mut Vec<(f32, f32)>, floor: usize, q: (f32, f32)| {
        while hull.len() >= floor && cross(hull[hull.len() - 2], hull[hull.len() - 1], q) <= 0.0 {
            hull.pop();
        }
        hull.push(q);
    };
    for &q in points.iter() {
        push(&mut hull, 2, q);
    }
    let lower = hull.len() + 1;
    for &q in points.iter().rev().skip(1) {
        push(&mut hull, lower, q);
    }
    // The upper chain ends on the first point again.
    hull.pop();
    hull
}

/// `(a − o) × (b − o)`: positive when `o, a, b` turn counter-clockwise.
fn cross(o: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// How far `p` lies outside the counter-clockwise convex `hull`, m: the largest of its distances
/// past the hull's edges, at most 0 inside, infinite when the hull spans no area.
fn beyond(hull: &[(f32, f32)], p: (f32, f32)) -> f32 {
    if hull.len() < 3 {
        return f32::INFINITY;
    }
    (0..hull.len())
        .map(|k| {
            let (a, b) = (hull[k], hull[(k + 1) % hull.len()]);
            -cross(a, b, p) / (b.0 - a.0).hypot(b.1 - a.1)
        })
        .fold(f32::NEG_INFINITY, f32::max)
}

/// How far `p`, inside the counter-clockwise convex `hull` (three corners or more), travels along
/// unit `e` before it leaves it, m: the nearest crossing of the edges `e` runs toward.
fn exit(hull: &[(f32, f32)], p: (f32, f32), e: (f32, f32)) -> f32 {
    (0..hull.len())
        .filter_map(|k| {
            let (a, b) = (hull[k], hull[(k + 1) % hull.len()]);
            let length = (b.0 - a.0).hypot(b.1 - a.1);
            // The edge's outward unit normal: its direction turned clockwise.
            let out = ((b.1 - a.1) / length, (a.0 - b.0) / length);
            let toward = out.0 * e.0 + out.1 * e.1;
            (toward > 0.0).then(|| (cross(a, b, p) / length).max(0.0) / toward)
        })
        .fold(f32::INFINITY, f32::min)
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

/// The group's resultant force (N) and moment about `about` (N m). Gravity is an acceleration,
/// supplying one force at each chunk's centroid; each load on the group supplies `force` of it
/// at its point (its press, or its peak force, [`Load::peak`]), none where `force` gives none.
fn force_wrench(
    structure: &Structure,
    group: &[usize],
    loads: &[Load],
    gravity: Vec3,
    about: Vec3,
    force: impl Fn(&Load) -> Option<Vec3>,
) -> (Vec3, Vec3) {
    let result = group
        .iter()
        .map(|&c| {
            let chunk = &structure.chunks[c];
            (chunk.centroid, gravity * chunk.mass)
        })
        .chain(
            loads
                .iter()
                .filter(|load| group.binary_search(&load.chunk).is_ok())
                .filter_map(|load| Some((load.at, force(load)?))),
        )
        .fold((Vec3::ZERO, Vec3::ZERO), |(force, moment), (at, push)| {
            (force + push, moment + (at - about).cross(push))
        });
    assert!(
        result.0.is_finite() && result.1.is_finite(),
        "finite chunk weights and load forces have a finite resultant force and moment"
    );
    result
}

/// The peak tension across the joints of `section` that border `group` (ascending), the group the
/// section alone holds ([`weigh`]), against their seam's tensile strength: `None` when the group
/// lies on both sides of the section's plane. The joints carry resultant force `F = Σ m g + Σ P`
/// and moment `M = Σ (x − c) × F_i` about their area centroid `c`, each press `P` at its point.
/// Read as a stress linear over the plane (Navier: plane sections stay plane), direct pull is
/// `F·n / A` (`n` the plane's normal
/// toward the group, tension positive, so a group hanging below its section pulls and one standing
/// on it presses), and the bending part `M_b` of the moment (in the plane) sets the stress's
/// gradient `g` from `J g = n × M_b`, `J` the joints' second moment of area about `c`. Each joint
/// is a disc of its area (the push rule's reading of a joint, [`disc`]), so it adds its own
/// `A²/4π` to `J`, and its peak is the gradient's reach to its rim. Each joint compares with its
/// own seam (its bond's tensile strength, as [`crack`] reads it); the section's
/// demand and capacity are the joint's whose ratio is highest. ASSUMED out of the check: the
/// shear and the twist about `n` the joints also carry (a punch ring reads shear on its own
/// mechanism, [`ring`]), and crushing (self-weight compression is about 1/230 of brick's crushing strength, DERIVED in
/// the collapse survey, `.agents/scratch/destruction-2026-10-31-collapse/collapse-survey.md`).
/// Adds the chunks and joints it reads to `steps`.
fn weight_failure(
    structure: &Structure,
    section: &[usize],
    group: &[usize],
    loads: &[Load],
    gravity: Vec3,
    steps: &mut usize,
) -> Option<Failure> {
    *steps += loads.len();
    let navier = Navier::of(structure, section, group, steps)?;
    let (force, moment) = force_wrench(structure, group, loads, gravity, navier.centre, |load| {
        Some(load.press)
    });
    let (_, demand, capacity) = navier.peak(&structure.joints, force, moment);
    // Only joints whose own traction reaches their strength open. Compression elsewhere
    // does not supply work to separate that part of the section.
    let opened = navier
        .stresses(&structure.joints, force, moment)
        .filter(|&(_, demand, capacity)| demand >= capacity)
        .map(|(j, ..)| j)
        .collect();
    Some(Failure {
        joints: opened,
        cause: Cause::Weight,
        demand,
        capacity,
    })
}

/// A section's Navier reading of the group it alone holds ([`weight_failure`]): the section's
/// joints that border the group, each with its normal turned toward it, their area and area
/// centroid, the plane's axes and the joints' second moment of area about the centroid in them.
struct Navier {
    border: Vec<(usize, Vec3)>,
    normal: Vec3,
    area: f32,
    centre: Vec3,
    axes: (Vec3, Vec3),
    second: (f32, f32, f32),
}

impl Navier {
    /// `section`'s reading of `group` (ascending), `None` when the group lies on both sides of
    /// the section's plane. Adds the joints and chunks it reads to `steps`.
    fn of(
        structure: &Structure,
        section: &[usize],
        group: &[usize],
        steps: &mut usize,
    ) -> Option<Self> {
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
        let (u, v) = plane_axes(normal);
        let (mut juu, mut juv, mut jvv) = (0.0, 0.0, 0.0);
        for &(j, _) in &border {
            let a = joints[j].area;
            let own = a * a / (4.0 * PI);
            let d = joints[j].centroid - centre;
            let (du, dv) = (d.dot(u), d.dot(v));
            juu += own + a * du * du;
            juv += a * du * dv;
            jvv += own + a * dv * dv;
        }
        Some(Self {
            border,
            normal,
            area,
            centre,
            axes: (u, v),
            second: (juu, juv, jvv),
        })
    }

    /// Under resultant force `force` (N) and moment `moment` (N m) about the centre: the peak
    /// tension of the bordering joint whose ratio to its seam's tensile strength is highest, and
    /// that strength, Pa, together with that joint's index.
    fn peak(&self, joints: &[Joint], force: Vec3, moment: Vec3) -> (usize, f32, f32) {
        self.stresses(joints, force, moment)
            .max_by(|a, b| (a.1 / a.2).total_cmp(&(b.1 / b.2)))
            .expect("a Navier reading borders its group with at least one joint")
    }

    /// Each bordering joint's own tensile demand and capacity, Pa.
    fn stresses<'a>(
        &'a self,
        joints: &'a [Joint],
        force: Vec3,
        moment: Vec3,
    ) -> impl Iterator<Item = (usize, f32, f32)> + 'a {
        let direct = force.dot(self.normal) / self.area;
        let bending = moment - self.normal * moment.dot(self.normal);
        let (u, v) = self.axes;
        let (juu, juv, jvv) = self.second;
        let lever = self.normal.cross(bending);
        let (lu, lv) = (lever.dot(u), lever.dot(v));
        let det = juu * jvv - juv * juv;
        let (gu, gv) = ((jvv * lu - juv * lv) / det, (juu * lv - juv * lu) / det);
        let reach = gu.hypot(gv);
        self.border.iter().map(move |&(j, _)| {
            let d = joints[j].centroid - self.centre;
            let (du, dv) = (d.dot(u), d.dot(v));
            let rim = (joints[j].area / PI).sqrt();
            (
                j,
                direct + gu * du + gv * dv + reach * rim,
                joints[j]
                    .seam()
                    .expect("a section holds bonded joints only")
                    .tensile,
            )
        })
    }
}

/// What bonded `section`'s strength leaves the blows on `group` ([`structural`], [`Reading`]):
/// the plastic demand factor ([`plastic_demand`]) whose `r` is the share of the peak forces of
/// `loads` at which, with gravity and their presses, the section's Navier tension reaches its
/// strength ([`weight_failure`], [`strength_ratio`]), so a far burst whose peak the seam holds
/// turns nothing whatever its blow; and whether that tension reaches it under gravity, the
/// presses and the body blows' least mean forces ([`Load::mean_force`]). A group on both sides of
/// the section's plane has no Navier reading (the weight check leaves it standing): its strength
/// is unread there, and every blow keeps the energy limit alone ([`Reading::UNRESISTED`]). Adds
/// the joints and chunks it reads to `steps`.
fn section_pulse(
    structure: &Structure,
    section: &[usize],
    group: &[usize],
    loads: &[Load],
    steps: &mut usize,
) -> Reading {
    let Some(navier) = Navier::of(structure, section, group, steps) else {
        return Reading::UNRESISTED;
    };
    let gravity = Vec3::NEG_Y * GRAVITY;
    let (force, moment) = force_wrench(structure, group, loads, gravity, navier.centre, |load| {
        Some(load.press)
    });
    let (peak_force, peak_moment) =
        force_wrench(structure, group, loads, Vec3::ZERO, navier.centre, |load| {
            load.peak
        });
    let (mean_force, mean_moment) = force_wrench(
        structure,
        group,
        loads,
        Vec3::ZERO,
        navier.centre,
        Load::mean_force,
    );
    let (_, demand, capacity) =
        navier.peak(&structure.joints, force + mean_force, moment + mean_moment);
    Reading {
        falling: plastic_demand(strength_ratio(|s| {
            let (_, demand, capacity) = navier.peak(
                &structure.joints,
                force + peak_force * s,
                moment + peak_moment * s,
            );
            demand >= capacity
        })),
        yields: demand >= capacity,
    }
}

/// Each chunk's joints, the anchors too, in joint order.
fn incident(structure: &Structure) -> Vec<Vec<usize>> {
    let mut incident = vec![Vec::new(); structure.chunks.len()];
    for (index, joint) in structure.joints.iter().enumerate() {
        incident[joint.a].push(index);
        if let Side::Chunk(b) = joint.b {
            incident[b].push(index);
        }
    }
    incident
}

/// How many rings of bonded neighbours a punch-ring candidate grows past the loaded chunks
/// (Resolution). The laws compare these candidates by demand over capacity; they do not search
/// every connected group. Nonuniform seam strengths or concave groups need not have increasing
/// capacity as a candidate grows, so this bound can omit a weaker ring.
const RING_GROWTH: usize = 2;

/// The punch-ring candidates about the standing chunks of `seeds`: the seeds, then the seeds grown
/// by one and by [`RING_GROWTH`] rings of neighbours through `graph`'s intact bonded joints, each
/// split into its bonded components, each component ascending, in growth order and without
/// repeats. Adds the chunks it reaches to `steps`.
fn ring_groups(
    state: &WallState,
    (neighbours, ..): &Graph,
    seeds: &[usize],
    steps: &mut usize,
) -> Vec<Vec<usize>> {
    let mut grown = vec![false; neighbours.len()];
    let mut members: Vec<usize> = Vec::new();
    for &chunk in seeds {
        if state.standing(chunk) && !grown[chunk] {
            grown[chunk] = true;
            members.push(chunk);
        }
    }
    let mut frontier = members.clone();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for ring in 0..=RING_GROWTH {
        if ring > 0 {
            let mut next = Vec::new();
            for &chunk in &frontier {
                for &(neighbour, _) in &neighbours[chunk] {
                    if !grown[neighbour] && state.standing(neighbour) {
                        grown[neighbour] = true;
                        next.push(neighbour);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            members.extend_from_slice(&next);
            frontier = next;
        }
        let mut seen = vec![false; neighbours.len()];
        for &start in &members {
            if seen[start] {
                continue;
            }
            seen[start] = true;
            let mut component = vec![start];
            let mut next = 0;
            while let Some(&chunk) = component.get(next) {
                next += 1;
                *steps += 1;
                for &(neighbour, _) in &neighbours[chunk] {
                    if grown[neighbour] && !seen[neighbour] {
                        seen[neighbour] = true;
                        component.push(neighbour);
                    }
                }
            }
            component.sort_unstable();
            if !groups.contains(&component) {
                groups.push(component);
            }
        }
    }
    groups
}

/// A punch ring's reading of one group ([`ring`]).
struct Ring {
    /// Every live joint from the group to what stands outside it not yet opened, ascending: the
    /// joints the punch opens.
    opens: Vec<usize>,
    /// The unit direction the group translates along.
    along: Vec3,
    /// The ring's strength along `along`, N.
    strength: f32,
    /// The work that opens the ring along `along`, J.
    work: f32,
    /// The group's mass, kg.
    mass: f32,
}

/// ASSUMED dimensionless f32 roundoff allowance for normalized dot/cross candidates in
/// [`translation`]. This is numerical resolution, not clearance through a neighbouring body.
const TRANSLATION_EPS: f32 = 32.0 * f32::EPSILON;

/// The unit translation doing most work under `push` without moving into any outward unit
/// boundary normal.
/// DERIVED: the admissible directions form a convex polyhedral cone in three dimensions. Its
/// closest direction to the load is the load itself, a projection onto one active plane, or an
/// edge formed by two planes. A feasible single-plane projection of a violated constraint is
/// already the cone projection; otherwise compare all feasible edges. `None` means the cone
/// offers no direction on which the load does positive work (parallel planes add no edge).
fn translation(push: Vec3, normals: impl Iterator<Item = Vec3> + Clone) -> Option<Vec3> {
    let push = push.try_normalize()?;
    let allowed = |d: Vec3| normals.clone().all(|n| d.dot(n) <= TRANSLATION_EPS);
    if allowed(push) {
        return Some(push);
    }
    for n in normals.clone() {
        let into = push.dot(n);
        if into > 0.0
            && let Some(d) = (push - n * into).try_normalize()
            && allowed(d)
        {
            return Some(d);
        }
    }
    let (mut best, mut work) = (None, 0.0);
    for (index, a) in normals.clone().enumerate() {
        for b in normals.clone().skip(index + 1) {
            // Parallel boundary planes intersect in a plane, not a new edge direction.
            let Some(edge) = a.cross(b).try_normalize() else {
                continue;
            };
            let along = if edge.dot(push) >= 0.0 { edge } else { -edge };
            let candidate = along.dot(push);
            if candidate > work && allowed(along) {
                best = Some(along);
                work = candidate;
            }
        }
    }
    best
}

/// The punch ring of `group` (ascending) pushed along `push` (`incident`, each chunk's joints): a
/// rigid translation of the group out through every live joint to what stands outside it. The
/// translation does the most work under `push` without entering any neighbour's half-space
/// ([`translation`]); the constraint uses the authored joint normals, including nonorthogonal
/// faces. Each intact bonded joint of the ring, its normal `n` out of the
/// group, opens by `⟨−d·n⟩` and slides by `|d − (d·n) n|` per unit of translation `d`; its
/// strength is `A (f_t ⟨open⟩ + c ⟨slide⟩)` and its work `A (G_I ⟨open⟩ + G_II ⟨slide⟩)`, the
/// linear mode interaction (the wall design, "Bodies in contact"). A rest, or a seam a
/// crack opened, adds neither: the friction the group's own weight would clamp it with is
/// ASSUMED nothing (the wall above's clamp is unknown without a static solve; nothing errs toward
/// failure). `None` for a group with a buried chunk or an intact anchor, one with no translation
/// left, one no intact seam holds, and one across a seam whose shear the data does not state
/// ([`Shear::OpeningOnly`]): that seam answers by its opening law alone, and no ring forms
/// across it.
fn ring(
    structure: &Structure,
    state: &WallState,
    incident: &[Vec<usize>],
    group: &[usize],
    push: Vec3,
) -> Option<Ring> {
    let inside = |c: usize| group.binary_search(&c).is_ok();
    let mut contacts: Vec<(usize, Vec3)> = Vec::new();
    let mut mass = 0.0;
    for &c in group {
        let chunk = &structure.chunks[c];
        if chunk.buried {
            return None;
        }
        mass += chunk.mass;
        for &j in &incident[c] {
            let joint = &structure.joints[j];
            if !state.live(joint) {
                continue;
            }
            match joint.b {
                Side::Chunk(b) if inside(b) && inside(joint.a) => continue,
                Side::Ground if joint.seam().is_some() && !state.cracked[j] => return None,
                _ => {}
            }
            let n = if (joint.centroid - chunk.centroid).dot(joint.normal) >= 0.0 {
                joint.normal
            } else {
                -joint.normal
            };
            contacts.push((j, n));
        }
    }
    let along = translation(push, contacts.iter().map(|&(_, n)| n))?;
    let (mut strength, mut work) = (0.0, 0.0);
    for &(j, n) in &contacts {
        let joint = &structure.joints[j];
        let Some(seam) = joint.seam().filter(|_| !state.cracked[j]) else {
            continue;
        };
        let Shear::Cohesive { cohesion, energy } = seam.shear else {
            return None;
        };
        let normal = along.dot(n);
        let (open, slide) = ((-normal).max(0.0), (along - n * normal).length());
        strength += joint.area * (seam.tensile * open + cohesion * slide);
        work += joint.area * (seam.fracture_energy * open + energy * slide);
    }
    if strength <= 0.0 {
        return None;
    }
    let mut opens: Vec<usize> = contacts
        .iter()
        .map(|&(j, _)| j)
        .filter(|&j| !state.cracked[j])
        .collect();
    opens.sort_unstable();
    opens.dedup();
    Some(Ring {
        opens,
        along,
        strength,
        work,
        mass,
    })
}

/// The punch ring of `group` under the blows of `loads` ([`structural`]), `None` where they push
/// it nowhere or no ring forms ([`ring`]): the translation follows their summed blow; the demand
/// is each blow along it, by its plastic share against the group's mass and by what the ring's
/// strength leaves it ([`Reading`]: a pulse's peak forces against the strength along the
/// translation, a body blow's least mean force with gravity and the presses), against the
/// impulse whose energy opens the ring, `√(2 M W)`.
fn ring_failure(
    structure: &Structure,
    state: &WallState,
    incident: &[Vec<usize>],
    group: &[usize],
    loads: &[Load],
) -> Option<Failure> {
    let on: Vec<&Load> = loads
        .iter()
        .filter(|load| group.binary_search(&load.chunk).is_ok())
        .collect();
    let push: Vec3 = on.iter().map(|load| load.blow).sum();
    let ring = ring(structure, state, incident, group, push)?;
    let reaches = |f: Vec3| f.dot(ring.along) >= ring.strength;
    let sustained =
        on.iter().map(|load| load.press).sum::<Vec3>() + Vec3::NEG_Y * GRAVITY * ring.mass;
    let peak: Vec3 = on.iter().filter_map(|load| load.peak).sum();
    let mean: Vec3 = on.iter().filter_map(|load| load.mean_force()).sum();
    let reading = Reading {
        falling: plastic_demand(strength_ratio(|s| reaches(sustained + peak * s))),
        yields: reaches(sustained + mean),
    };
    let demand: f32 = on
        .iter()
        .map(|load| {
            load.blow.dot(ring.along) * load.origin.share(1.0 / ring.mass) * load.felt(reading)
        })
        .sum();
    (demand > 0.0).then(|| Failure {
        joints: ring.opens,
        cause: Cause::Punch,
        demand,
        capacity: (2.0 * ring.mass * ring.work).sqrt(),
    })
}

/// The punch ring of `group` under `gravity` and the presses of `loads` ([`weigh`]), `None` where
/// they press it nowhere or no ring forms ([`ring`]): the translation follows their summed press,
/// and the demand is their force along it, the group's weight included, against the ring's
/// strength.
fn ring_force_failure(
    structure: &Structure,
    state: &WallState,
    incident: &[Vec<usize>],
    group: &[usize],
    loads: &[Load],
    gravity: Vec3,
) -> Option<Failure> {
    let push: Vec3 = loads
        .iter()
        .filter(|load| group.binary_search(&load.chunk).is_ok())
        .map(|load| load.press)
        .sum();
    let ring = ring(structure, state, incident, group, push)?;
    Some(Failure {
        joints: ring.opens,
        cause: Cause::PunchForce,
        demand: (push + gravity * ring.mass).dot(ring.along),
        capacity: ring.strength,
    })
}

/// The least share `s` of a pulse's peak forces, at most 1, at which `fails(s)`: the strength
/// over the peak force, [`plastic_demand`]'s `r`, read to 1 (past it `g` is 0 whatever `r` is),
/// and 0 when the strength already fails without the pulse. `fails` must hold from one share on
/// and not below it: every strength law it serves holds on a convex set of loadings (Navier
/// tension, and a rest's lift, pressure-centre and friction-cone conditions, each convex in the
/// resultant, which is affine in `s`). Found by bisection.
fn strength_ratio(fails: impl Fn(f32) -> bool) -> f32 {
    if !fails(1.0) {
        return 1.0;
    }
    if fails(0.0) {
        return 0.0;
    }
    let (mut holds, mut breaks) = (0.0_f32, 1.0_f32);
    // DERIVED: 24 halvings take the unit interval to 2⁻²⁴, f32's resolution just under 1.
    for _ in 0..24 {
        let mid = 0.5 * (holds + breaks);
        if fails(mid) {
            breaks = mid;
        } else {
            holds = mid;
        }
    }
    breaks
}

/// A blast field's directions with their shares (summing to 1): the field machinery's ball at
/// ratio 0, uniform over the sphere ([`crate::ballistics::spall_cone`]), [`BLAST_RAYS`] of them.
pub(crate) fn blast_directions() -> &'static [(Vec3, f32)] {
    static DIRECTIONS: std::sync::LazyLock<Vec<(Vec3, f32)>> =
        std::sync::LazyLock::new(|| ball_directions(BLAST_RAYS));
    &DIRECTIONS
}

/// `count` directions uniform over the sphere with their shares (summing to 1), as
/// [`blast_directions`] draws the field at its own count.
fn ball_directions(count: usize) -> Vec<(Vec3, f32)> {
    use crate::ballistics::spall_cone::{AxisRule, ConeShape, FieldDescriptor, FieldLaw};
    let spread = ConeShape::new(AxisRule::ExitDirection, count)
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
/// ray's patch, `4π share d² / cos θ`, grows without bound on the face it stops on, though the
/// face itself bounds the cones that land on it ([`intercepted`]).
const GRAZING_COS: f32 = 0.1;

/// One of Swisdak's fits of a surface burst of TNT (SOURCED: Swisdak 1994, "Simplified Kingery
/// Airblast Calculations", 26th DoD Explosives Safety Seminar, DTIC ADA526744, Table 1's metric
/// rows, PDF pp. 8–10, every coefficient read off the page images; they restate Kingery & Bulmash
/// 1984, ARBRL-TR-02555, with a mean error under 1 % and a worst cell of 1.5 %, Table 5, Z
/// 0.2–40): a scaled quantity at scaled distance
/// `Z = R / W^⅓` (m/kg^⅓) as `exp(Σ cₖ (ln Z)ᵏ)`, each branch good up to its `Z`; the scaled
/// impulse `i / W^⅓` in kPa·ms per kg^⅓ (Pa·s per kg^⅓), the peak overpressure in kPa. Below
/// `from` it holds the edge's value; near the charge the detonation products' floor lies above it
/// ([`Blast::products_floor`]). Nothing is read past the last branch: every walk stops a
/// ray at the reflected impulse fit's edge ([`Blast::reach`]), and a distance over it by rounding
/// reads the edge's value.
struct SwisdakFit {
    from: f32,
    branches: &'static [(f32, &'static [f32])],
}

impl SwisdakFit {
    /// The last scaled distance a branch covers, m/kg^⅓.
    const fn edge(&self) -> f32 {
        self.branches[self.branches.len() - 1].0
    }

    /// The fit's value at scaled distance `z`, in its own unit.
    fn at(&self, z: f32) -> f32 {
        let held = z.clamp(self.from, self.edge());
        let (_, c) = self
            .branches
            .iter()
            .find(|(to, _)| held <= *to)
            .expect("held is within the last branch's edge");
        let l = held.ln();
        c.iter().rev().fold(0.0, |sum, ck| sum * l + ck).exp()
    }
}

/// Swisdak's normally reflected impulse, Z 0.06–40 m/kg^⅓ (SOURCED: Table 1, 3 of 3, PDF p. 10).
/// Its metric row agrees with the same table's imperial row to 0.05 % (DERIVED; the module's
/// tests hold it to 0.1 %).
const REFLECTED_IMPULSE: SwisdakFit = SwisdakFit {
    from: 0.06,
    branches: &[(40.0, &[6.7853, -1.3466, 0.101, -0.01123])],
};

/// Swisdak's incident (side-on) impulse, Z 0.2–158.7 m/kg^⅓ (SOURCED: Table 1, 2 of 3, PDF p. 9).
const INCIDENT_IMPULSE: SwisdakFit = SwisdakFit {
    from: 0.2,
    branches: &[
        (0.96, &[5.522, 1.117, 0.6, -0.292, -0.087]),
        (2.38, &[5.465, -0.308, -1.464, 1.362, -0.432]),
        (33.7, &[5.2749, -0.4677, -0.2499, 0.0588, -0.00554]),
        (158.7, &[5.9825, -1.062]),
    ],
};

/// Swisdak's incident (side-on) peak overpressure, kPa, Z 0.2–198.5 m/kg^⅓ (SOURCED: Table 1, 1
/// of 3, PDF p. 8). Its branches meet within 0.7 % at their joins (DERIVED).
const INCIDENT_PRESSURE: SwisdakFit = SwisdakFit {
    from: 0.2,
    branches: &[
        (2.9, &[7.2106, -2.1069, -0.3229, 0.1117, 0.0685]),
        (23.8, &[7.5938, -3.0523, 0.40977, 0.0261, -0.01267]),
        (198.5, &[6.0536, -1.4066]),
    ],
};

/// Swisdak's normally reflected peak overpressure, kPa, Z 0.06–40 m/kg^⅓ (SOURCED: Table 1, 1 of
/// 3, PDF p. 8, both branches). Its branches meet within 0.1 % at Z = 2, and it stays at least
/// 2.01 times the incident fit over the whole field (DERIVED).
const REFLECTED_PRESSURE: SwisdakFit = SwisdakFit {
    from: 0.06,
    branches: &[
        (
            2.0,
            &[9.006, -2.6893, -0.6295, 0.1011, 0.29255, 0.13505, 0.019736],
        ),
        (
            40.0,
            &[8.8396, -1.733, -2.64, 2.293, -0.8232, 0.14247, -0.0099],
        ),
    ],
};

/// A blast: its energy (J) and its charge `W` (kg of TNT), the TNT that carries that energy
/// (ASSUMED: impulse equivalence is energy equivalence; the wall design notes they need not agree).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Blast {
    pub energy: f32,
    pub charge: f32,
    /// Positive charge radius (m), the lower bound of point-source field distances.
    pub radius: f32,
    /// The detonation products' scalar momentum, N·s: positive, from the burst law's Gurney
    /// profile (`burst::products_momentum`). Near the charge its spread floors the field
    /// ([`Self::products_floor`]).
    pub products: f32,
}

/// ASSUMED fixture products momentum, N·s: its floor lies under the free-air fit at every
/// distance a fixture reads (at least a 1 mm fixture radius), so a test built on it reads the fit
/// alone ([`Blast::products_floor`]).
#[cfg(test)]
pub(crate) const FIT_ONLY_PRODUCTS: f32 = 1e-6;

impl Blast {
    /// A blast of `energy` J in the air from a charge of `radius` m whose detonation products
    /// carry `products` N·s. Production radii and products come from filler data; idealized
    /// tests use ASSUMED small positive fixture values.
    pub(crate) fn of(energy: f32, radius: f32, products: f32) -> Self {
        let radius = (radius.is_finite() && radius > 0.0)
            .then_some(radius)
            .expect(
                "a blast's charge-radius floor is positive and finite, supplied by its filler data",
            );
        assert!(
            energy.is_finite() && energy >= 0.0,
            "a blast carries finite nonnegative energy"
        );
        assert!(
            products.is_finite() && products > 0.0,
            "a blast's detonation products carry positive momentum, refused at load otherwise"
        );
        Self {
            energy,
            charge: energy / TNT_ENERGY,
            radius,
            products,
        }
    }

    /// A point-source field is evaluated no closer than the charge's radius. Geometry still
    /// records the actual face point, including a charge embedded in material; only the field's
    /// distance and its cone section use this floor.
    pub(crate) fn distance(&self, distance: f32) -> f32 {
        (distance.is_finite() && distance >= 0.0)
            .then_some(distance.max(self.radius))
            .expect(
                "a blast ray has a finite nonnegative distance and a positive charge-radius floor",
            )
    }

    /// How far the blast reaches, m: the reflected fit's last scaled distance at this charge, where
    /// every ray stops, whatever it would meet beyond (a chunk, a body, the ground). DERIVED: the
    /// 88 mm HE fixture burst (1.47 MJ) reaches 23.2 m, where a ray that meets a 0.25 m brick cube
    /// square on moves it at 0.017 m/s (the cube's face's share of the ray's cone, [`intercepted`]),
    /// under avian's 0.15 m/s sleep threshold.
    pub(crate) fn reach(&self) -> f32 {
        REFLECTED_IMPULSE.edge() * (self.charge / SURFACE_BURST_FACTOR).cbrt()
    }

    /// The cross-section of the cone a ray of `share` of the field stands for, `distance` m out,
    /// m²: its solid angle `4π share` × `d²`.
    pub(crate) fn section(share: f32, distance: f32) -> f32 {
        4.0 * PI * share * distance * distance
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
        let distance = self.distance(distance);
        let z = distance / scale;
        let cos = cos.clamp(0.0, 1.0);
        let reflected = REFLECTED_IMPULSE.at(z);
        let reflected = self
            .products_floor(distance, scale * reflected)
            .map_or(reflected, |(i, _)| i / scale);
        scale * (reflected * cos * cos + INCIDENT_IMPULSE.at(z) * (1.0 - cos) * (1.0 - cos))
    }

    /// The detonation products' floor under the normally reflected field `distance` m (floored)
    /// from the charge, where the fit's reflected impulse there is `fit` Pa·s: `Some((i, k))`
    /// where the products' spread impulse `i = 2 p / (4π R²)` beats the fit, `k = i / fit` the
    /// factor the reflected peak takes with it; `None` everywhere else, where the fit stands
    /// bit for bit. The products' momentum `p` ([`Self::products`]) spreads over the sphere and
    /// reflects off the face (factor 2, ASSUMED, the #237 study's); the reflected pulse keeps
    /// the fit's duration `2i/P` (ASSUMED), so its peak rises with its impulse. Both branches
    /// are continuous, so their maximum meets the fit continuously where `i = fit`; the
    /// crossing's scaled distance is one number per charge design (core and casing ratio), as
    /// the fit's own scaling is. DERIVED from the shipped cores' data: bare, 1.63–1.82 times the
    /// fit at the charge's surface and meeting it at Z ≈ 0.59–0.72 m/kg^⅓ (the study's ≈ 1.7
    /// and ≈ 0.6); the cased 8.8 cm HE round, its products slowed by the casing, 1.44 times,
    /// meeting it at Z ≈ 0.44.
    pub(crate) fn products_floor(&self, distance: f32, fit: f32) -> Option<(f32, f32)> {
        let products = self.products / (2.0 * PI * distance * distance);
        (products > fit).then(|| (products, products / fit))
    }

    /// The normally reflected peak overpressure at scaled distance `z` (m/kg^⅓), Pa
    /// ([`REFLECTED_PRESSURE`]).
    fn reflected_pressure(z: f32) -> f32 {
        REFLECTED_PRESSURE.at(z) * 1e3
    }

    /// The peak overpressure (Pa) the blast loads a face with `distance` m from the charge, at
    /// incidence cosine `cos`: the normally reflected `p_r` square on, the incident `p_s`
    /// grazing, between them the ConWep blend [`Self::specific_impulse`] reads on impulse,
    /// `p_r cos²θ + p_s (1 + cos²θ − 2 cos θ)` (CLAIMED there, stated for pressure). A function
    /// of the scaled distance alone (Hopkinson–Cranz).
    pub(crate) fn peak_pressure(&self, distance: f32, cos: f32) -> f32 {
        let scale = (self.charge / SURFACE_BURST_FACTOR).cbrt();
        if scale <= 0.0 {
            return 0.0;
        }
        let distance = self.distance(distance);
        let z = distance / scale;
        let cos = cos.clamp(0.0, 1.0);
        let reflected = Self::reflected_pressure(z);
        let reflected = self
            .products_floor(distance, scale * REFLECTED_IMPULSE.at(z))
            .map_or(reflected, |(_, k)| reflected * k);
        reflected * cos * cos + INCIDENT_PRESSURE.at(z) * 1e3 * (1.0 - cos) * (1.0 - cos)
    }

    /// The area (m²) of the face a ray of `share` of the field stands for where it stops
    /// `distance` m out at incidence cosine `cos`: [`Self::section`] / cos θ, [`GRAZING_COS`]
    /// the least.
    fn patch(share: f32, distance: f32, cos: f32) -> f32 {
        Self::section(share, distance) / cos.max(GRAZING_COS)
    }

    /// The impulse (N·s) a ray of `share` of the field delivers where it stops `distance` m out,
    /// travelling along unit `dir`, on a face of outward unit normal `normal`, its cone wholly on
    /// the face: the specific impulse there over the patch of the face the ray stands for
    /// ([`Self::patch`]), pushed into the face (a pressure acts along the normal). A face smaller
    /// than the cone takes its intercepted share of this ([`intercepted`]).
    pub(crate) fn push(&self, share: f32, distance: f32, dir: Vec3, normal: Vec3) -> Vec3 {
        let distance = self.distance(distance);
        let cos = -dir.dot(normal);
        -normal * (self.specific_impulse(distance, cos) * Self::patch(share, distance, cos))
    }

    /// The peak force (N) of the same ray on the same patch as its [`Self::push`]: the peak
    /// overpressure there ([`Self::peak_pressure`]) over the patch, into the face. A face smaller
    /// than the cone takes its intercepted share of this, as of the push.
    pub(crate) fn peak(&self, share: f32, distance: f32, dir: Vec3, normal: Vec3) -> Vec3 {
        let distance = self.distance(distance);
        let cos = -dir.dot(normal);
        -normal * (self.peak_pressure(distance, cos) * Self::patch(share, distance, cos))
    }

    /// Law 1's crushed volume and local front depth on the same patch as [`Self::push`]. The
    /// volume takes the intercepted share of the cone; depth is intensive and keeps its value.
    /// A cone expanding behind the face holds that volume between its entry and its front
    /// ([`Crushing`]); the wall design owns the model and its oblique-projection approximation.
    pub(crate) fn crushed(
        &self,
        material: &Masonry,
        share: f32,
        distance: f32,
        dir: Vec3,
        normal: Vec3,
    ) -> Crushing {
        let distance = self.distance(distance);
        let cos = -dir.dot(normal);
        let pressure = self.peak_pressure(distance, cos);
        let impulse = self.specific_impulse(distance, cos);
        let volume = material.crushed_by(pressure, impulse * Self::patch(share, distance, cos));
        let planar = f64::from(material.crushed_by(pressure, impulse));
        let projected = f64::from(distance) * f64::from(cos.max(GRAZING_COS));
        let q = (1.0 + 3.0 * planar / projected).cbrt();
        // DERIVED: projected * (q - 1), rationalized so a shallow front never loses its depth
        // to subtraction of nearly equal cube roots. The zero-crush case has planar = 0.
        let depth = (3.0 * planar / (q * q + q + 1.0)) as f32;
        Crushing { volume, depth }
    }
}

/// The plastic demand factor `g(r)` of a rigid-plastic section under a blast's positive phase,
/// against `r`, its strength over the pulse's peak force: a section of mass `m` taking blow `J`
/// absorbs `J² g(r) / 2m` before it stops. DERIVED (the #237 study, §4): a triangular pulse,
/// falling from its peak force to zero over its duration, against the strength as a constant
/// resistance; `1 − 4r/3` while the section still moves when the pulse ends (`r < ½`),
/// `(16/3) r (1 − r)³` after, and nothing once the strength holds the peak (`r ≥ 1`). Its two
/// ends are the wall design's two limits: the impulsive (`g → 1` as the strength vanishes
/// against the peak) and the quasi-static (`g = 0` where the peak force cannot beat the
/// strength). The section and rest laws read it ([`section_pulse`], [`unseat`], [`Load::felt`]).
/// A negative or undefined ratio is a bug: strengths and peak forces are not negative.
pub(crate) fn plastic_demand(r: f32) -> f32 {
    assert!(
        r >= 0.0,
        "a strength over a peak force is a ratio of magnitudes, got {r}"
    );
    if r >= 1.0 {
        0.0
    } else if r < 0.5 {
        1.0 - 4.0 * r / 3.0
    } else {
        16.0 / 3.0 * r * (1.0 - r).powi(3)
    }
}

/// The crush factor `f(r)` of a snowplow compaction front under the same triangular pulse,
/// against `r = σ_c,d / P`, the material's dynamic compressive strength over the face's peak
/// pressure: a face patch taking impulse `J` crushes `J f(r) / u_c` of mass, `u_c` its slowest
/// front speed. DERIVED (the #237 study, §3, checked there by numeric integration to 0.3 %):
/// `√(1 − 4r/3)` for `r < ½`, `√((16/3) r (1 − r)³)` for `½ ≤ r < 1`, nothing where the peak
/// cannot beat the strength (`r ≥ 1`); the whole blow's worth as the strength vanishes. A
/// negative or undefined ratio is a bug, as for [`plastic_demand`].
pub(crate) fn crush_factor(r: f32) -> f32 {
    assert!(
        r >= 0.0,
        "a strength over a peak pressure is a ratio of magnitudes, got {r}"
    );
    if r >= 1.0 {
        0.0
    } else if r < 0.5 {
        (1.0 - 4.0 * r / 3.0).sqrt()
    } else {
        (16.0 / 3.0 * r * (1.0 - r).powi(3)).sqrt()
    }
}

/// The game's environment Law 1 result on one field patch: its removed volume and the front's
/// local depth normal to the face, both before limiting to the available chunk material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Crushing {
    /// m³, extensive: interception scales it with the patch's share.
    pub volume: f32,
    /// m, intensive: cone-volume depth, independent of the patch's share. At normal incidence
    /// `depth = (R³ + 3 R² V/A)^(1/3) - R` (DERIVED), `R` the floored field distance. Oblique
    /// depth projects the expanded cone along the face normal; the same incidence floor as the
    /// field's patch bounds that projection (ASSUMED).
    pub depth: f32,
}

/// What Law 2 reads of the chunk a blast face loads ([`Load::blast`]): its hull, the hits' frame's
/// scale over the hull's (a building instance's), and its material.
pub(crate) struct Backing<'a> {
    pub block: &'a Block,
    pub scale: f32,
    pub material: Masonry,
    /// Contacts between standing matter ([`WallState::contacts`]), in the hits' frame. Their
    /// rear-face area is not a free surface, whether bonded or bearing-only.
    pub contacts: &'a [Joint],
}

/// A rear scab (Law 2), about the game's environment destruction (game state only): the layer a
/// loaded face's reflected tension throws off its chunk's back face ([`scab`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Scab {
    pub chunk: usize,
    /// The back-face point it leaves from, opposite the face's load, m.
    pub at: Vec3,
    /// kg.
    pub mass: f32,
    /// m/s, along the face's push.
    pub velocity: Vec3,
}

impl Scab {
    /// What it carries away, N·s: taken off the face's blow, so the pushed chunk and the scab
    /// share the face's momentum.
    pub(crate) fn momentum(&self) -> Vec3 {
        self.velocity * self.mass
    }
}

/// Law 2, the rear scab, on one loaded face (its `rays`, aggregated as `load`) of the chunk
/// `backing` names; the wall design ("Standoff blast", Law 2) owns the model. The ligament `L` is
/// remaining depth behind the crater to the hull's actual exit. The back point `b` lies the
/// full hull depth `H` behind the face's load; the patch nearest that load supplies its crater
/// depth (ASSUMED: nearest-sample reading at the same one back point). The face's compression
/// reaches `b` as `σ_b`: the solid-angle mean of the face's transmitted stress, the sum over its
/// patches of `H / (2π R³)` (each patch's solid angle seen from `b`, over `2π`, per area) times
/// each patch's transmitted force `min(P, σ_c,d) A`; a uniform load over a plane gives back the
/// load (the #237 study's mean; the wall design names the rejected alternative). The pulse lasts `τ
/// = 2J / F` (`2i / P` of the face's blow and peak force) and is `λ = c τ` long. Reflected at the
/// free back face as tension, it lifts a scab where `σ_b min(1, 2L / λ) ≥ σ_t,d`, of thickness `δ =
/// λ σ_t,d / (2 σ_b)` (where the net tension first reaches the strength; at most `L`), at `v =
/// (2σ_b − σ_t,d) / (ρ c)`, over `F_t / σ_b`, the area its transmitted force covers at `σ_b`,
/// bounded by the chunk's exposed rear area projected along the normal (DERIVED: the scab then
/// carries at most `J F_t / F` of the face's momentum, never more than its blow). ASSUMED: that equivalent area is the
/// footprint and the gate reads one back point. Contacts take their projected area off
/// the free rear area; propagation through that contact to a farther free surface is unresolved.
/// A front that crushes through leaves no ligament and throws no rear scab. `None` where no
/// ligament remains or the gate fails. Every input is the face's and its row's; no body type enters.
pub(crate) fn scab(rays: &[&RayHit], load: &Load, backing: &Backing) -> Option<Scab> {
    let normal = rays
        .first()
        .expect("a loaded face has at least one ray")
        .normal;
    let material = &backing.material;
    let Spall { bar_speed, tensile } = material.spall;
    let crush = material.compaction.strength;
    let local = load.at / backing.scale;
    let depth = backing.block.exit(local, -normal) * backing.scale;
    let back = load.at - normal * depth;
    let crater = rays
        .iter()
        .min_by(|a, b| {
            a.at.distance_squared(load.at)
                .total_cmp(&b.at.distance_squared(load.at))
        })
        .expect("a loaded face has at least one ray")
        .depth;
    let ligament = depth - crater;
    if ligament <= 0.0 {
        // Through-crush, or a boundary point already past a plane it leaves through, leaves no
        // intact rear layer to reflect.
        return None;
    }
    let (mut stress, mut transmitted) = (0.0_f32, 0.0_f32);
    for ray in rays {
        let force = ray.peak.length();
        let through = if ray.pressure > crush {
            force * crush / ray.pressure
        } else {
            force
        };
        let reach = (ray.at - back).length();
        stress += depth / (2.0 * PI * reach.powi(3)) * through;
        transmitted += through;
    }
    let peak = load
        .peak
        .expect("a blast face states its peak force")
        .length();
    assert!(
        peak > 0.0,
        "a blast face with a positive blow peaks at a positive force"
    );
    let length = bar_speed * 2.0 * load.blow.length() / peak;
    if stress * (2.0 * ligament / length).min(1.0) < tensile {
        return None;
    }
    let thickness = length * tensile / (2.0 * stress);
    let speed = (2.0 * stress - tensile) / (material.density * bar_speed);
    let shadow = silhouette(&backing.block.face_areas(), -normal) * backing.scale.powi(2);
    let covered: f32 = backing
        .contacts
        .iter()
        .filter(|joint| {
            (joint.a == load.chunk || joint.b == Side::Chunk(load.chunk))
                && joint.normal.dot(back - joint.centroid).abs() <= ON_PLANE * backing.scale
        })
        .map(|joint| joint.area * joint.normal.dot(normal).abs())
        .sum();
    let exposed = (shadow - covered).max(0.0);
    if exposed == 0.0 {
        // A contact transmits compression; it is no free boundary to reflect it as tension.
        return None;
    }
    let mass = material.density * thickness * (transmitted / stress).min(exposed);
    let thrown = Scab {
        chunk: load.chunk,
        at: back,
        mass,
        velocity: -normal * speed,
    };
    assert!(
        thrown.momentum().length() <= load.blow.length() * (1.0 + 1e-4),
        "a scab carries at most its face's blow: {thrown:?} from {load:?}"
    );
    Some(thrown)
}

/// One blast ray's arrival: the chunk it stopped on, where, its energy (J), impulse (N·s), peak
/// force (N, [`Blast::peak`]), the peak overpressure on its patch (Pa, [`Blast::peak_pressure`])
/// and the volume it crushes (m³, [`Blast::crushed`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RayHit {
    pub chunk: usize,
    pub at: Vec3,
    /// Outward normal of the struck face; all rays on that face carry the same normal.
    pub normal: Vec3,
    /// Intercepted chemical-energy share, J, for the sandbox's geometry audit alone. It never
    /// enters a [`Load`], wears a pool, sizes a hole or sources a Lamé field.
    pub energy: f32,
    pub impulse: Vec3,
    pub peak: Vec3,
    /// The patch's peak overpressure, Pa: intensive, so a share of the cone keeps it whole.
    pub pressure: f32,
    pub crushed: f32,
    /// The local crater's depth normal to the face, m; intensive under interception.
    pub depth: f32,
}

/// A blast's rays from `origin`: each of `directions` (unit, with its share of the field; the
/// shares sum to 1) carries that share of the blast's energy, stops on the first standing block it
/// meets within the blast's reach ([`Blast::reach`]), which shadows everything behind it, and
/// pushes the face it meets ([`Blast::push`]) with a peak force ([`Blast::peak`]), crushing the
/// block's `material` there ([`Blast::crushed`]); the block takes the share of the ray's energy,
/// push, peak force and crushed volume its silhouette intercepts ([`intercepted`]). A ray that meets none leaves. Coupling is the
/// geometry's: the share of directions the blocks intercept, and of each cone the block it meets.
pub(crate) fn blast_rays(
    blocks: &[Block],
    standing: impl Fn(usize) -> bool,
    material: impl Fn(usize) -> Masonry,
    origin: Vec3,
    blast: Blast,
    directions: &[(Vec3, f32)],
) -> Vec<RayHit> {
    let reach = blast.reach();
    let mut faces: std::collections::BTreeMap<usize, Vec<(Vec3, f32)>> = Default::default();
    let mut stops = Vec::new();
    let hits: Vec<RayHit> = directions
        .iter()
        .filter_map(|&(dir, share)| {
            let (chunk, t, normal) = first_hit(blocks, &standing, origin, dir)?;
            if t > reach {
                return None;
            }
            let faces = faces
                .entry(chunk)
                .or_insert_with(|| blocks[chunk].face_areas());
            stops.push((
                chunk,
                Blast::section(share, blast.distance(t)),
                silhouette(faces, dir),
            ));
            let crushed = blast.crushed(&material(chunk), share, t, dir, normal);
            Some(RayHit {
                chunk,
                at: origin + dir * t,
                normal,
                energy: blast.energy * share,
                impulse: blast.push(share, t, dir, normal),
                peak: blast.peak(share, t, dir, normal),
                pressure: blast.peak_pressure(t, -dir.dot(normal)),
                crushed: crushed.volume,
                depth: crushed.depth,
            })
        })
        .collect();
    hits.into_iter()
        .zip(intercepted(&stops))
        .map(|(hit, kept)| RayHit {
            energy: hit.energy * kept,
            impulse: hit.impulse * kept,
            peak: hit.peak * kept,
            crushed: hit.crushed * kept,
            ..hit
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture physics step, s: the game's 64 Hz tick (fixture data).
    const STEP: f32 = 1.0 / 64.0;

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
                bond: Bond::Bonded(material.seam),
            })
            .collect();
        joints.push(Joint {
            a: 0,
            b: Side::Ground,
            area: 0.04,
            centroid: Vec3::ZERO,
            normal: Vec3::Y,
            bond: Bond::Bonded(material.seam),
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
                        bond: Bond::Bonded(material.seam),
                    });
                }
                if row + 1 < h {
                    joints.push(Joint {
                        a: i,
                        b: Side::Chunk(i + w),
                        area: size * size,
                        centroid: at(c + 0.5, r + 1.0),
                        normal: Vec3::Y,
                        bond: Bond::Bonded(material.seam),
                    });
                }
                if row == 0 {
                    joints.push(Joint {
                        a: i,
                        b: Side::Ground,
                        area: size * size,
                        centroid: at(c + 0.5, 0.0),
                        normal: Vec3::Y,
                        bond: Bond::Bonded(material.seam),
                    });
                }
            }
        }
        Structure { chunks, joints }
    }

    /// The structural check as specified, every section searched with [`held`] for every load
    /// with a blow whose chunk still reaches the ground: the reference the pruned check answers
    /// to.
    fn structural_by_full_search(
        structure: &Structure,
        state: &mut WallState,
        loads: &[Load],
    ) -> Vec<Failure> {
        let mut failures = Vec::new();
        loop {
            let graph = graph(structure, state);
            let mut worst: Option<(f32, Failure)> = None;
            for section in sections(structure, state) {
                let mut group: Vec<usize> = Vec::new();
                for load in loads.iter().filter(|load| load.blow != Vec3::ZERO) {
                    if !state.standing(load.chunk)
                        || group.binary_search(&load.chunk).is_ok()
                        || held(load.chunk, &[], &graph, &mut 0).is_some()
                    {
                        continue;
                    }
                    if let Some(reached) = held(load.chunk, &section, &graph, &mut 0) {
                        group.extend(reached);
                        group.sort_unstable();
                        group.dedup();
                    }
                }
                if group.is_empty() {
                    continue;
                }
                let pulse = section_pulse(structure, &section, &group, loads, &mut 0);
                if let Some(failure) = section_failure(structure, &section, &group, loads, pulse) {
                    let ratio = failure.demand / failure.capacity;
                    if ratio >= 1.0 && worst.as_ref().is_none_or(|(w, _)| ratio > *w) {
                        worst = Some((ratio, failure));
                    }
                }
            }
            let pushed = pushed(structure, loads);
            for group in ring_groups(state, &graph, &pushed, &mut 0) {
                if let Some(failure) =
                    ring_failure(structure, state, &incident(structure), &group, loads)
                {
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
            if state.cracked[index] || !state.live(joint) || joint.seam().is_none() {
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
    /// would not fall with the section's distance, and one push on a freed chunk would crack
    /// joints anywhere in the structure.
    #[test]
    fn the_pruned_structural_check_is_the_full_search() {
        let (w, h) = (8, 6);
        let structure = grid(w, h);
        let push = |chunk: usize, blow: Vec3| Load {
            chunk,
            at: structure.chunks[chunk].centroid + Vec3::Z * 0.125,
            energy: 0.0,
            crushed: 0.0,
            blow,
            press: Vec3::ZERO,
            peak: None,
            origin: Origin::Field,
        };
        let top = (h - 1) * w + w / 2;
        let loose = 2 * w + 1;
        let cases: [(Vec<usize>, Vec<Load>); 3] = [
            (Vec::new(), vec![push(top, Vec3::Z * 1600.0)]),
            (
                Vec::new(),
                vec![
                    push(top, Vec3::new(0.0, 0.0, -600.0)),
                    push(w + 6, Vec3::new(80.0, 0.0, 1200.0)),
                ],
            ),
            (
                (0..structure.joints.len())
                    .filter(|&j| {
                        let joint = &structure.joints[j];
                        joint.a == loose || joint.b == Side::Chunk(loose)
                    })
                    .collect(),
                vec![push(loose, Vec3::Z * 500.0), push(top, Vec3::Z * 800.0)],
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
    /// DERIVED in the prototype's README from 36.5 ref-mm/m), and the shipped Wood's seam is its
    /// own row's, so a timber parts by its sourced strength, never by an assumed one. A law
    /// reading a constant in place of the row, or breaking every substance, fails here.
    #[test]
    fn the_material_is_the_substance_row() {
        let registry = SubstanceRegistry::shipped_static();
        let brick = brick();
        let bulk = registry.hard("Brick").unwrap().bulk.unwrap();
        assert_eq!(f64::from(brick.seam.tensile), bulk.tensile.unwrap());
        assert_eq!(f64::from(brick.density), bulk.density);
        assert!(
            (brick.crush / 135.5e6 - 1.0).abs() < 0.01,
            "crush {}",
            brick.crush
        );
        let wood = Masonry::of(registry, "Wood")
            .unwrap()
            .expect("Wood states bulk facts");
        let bulk = registry.hard("Wood").unwrap().bulk.unwrap();
        assert_eq!(f64::from(wood.seam.tensile), bulk.tensile.unwrap());
        assert_eq!(
            f64::from(wood.seam.fracture_energy),
            bulk.fracture_energy.unwrap()
        );
        assert!(Masonry::of(registry, "Adobe").is_err());
    }

    /// A POOL WEARS, THEN EMPTIES: crushed volume under a chunk's pool leaves it standing and worn; the
    /// rest of its pool turns it to dust, and a dusted chunk takes no more and joins nothing, so a
    /// chunk beyond it with no other path to the ground falls. The removal alone ([`wear`]): a
    /// load's energy also stresses the joints about it ([`strike`]). A mutant that dusts on the
    /// first deposit, or that leaves a dusted chunk holding its joints, fails here.
    #[test]
    fn a_spent_pool_turns_a_chunk_to_dust_and_drops_what_it_held() {
        let structure = row(3);
        let mut state = WallState::new(&structure);
        let pool = full_pool(&structure.chunks[1]);
        assert!(
            !wear(&structure, &mut state, 1, pool / 2.0),
            "half a pool wears"
        );
        assert!(state.standing(1) && state.pool[1] < pool, "worn");
        assert!(fall(&structure, &mut state).is_empty());
        assert!(
            wear(&structure, &mut state, 1, pool / 2.0),
            "the rest of it dusts"
        );
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
        let source = |d: f32| Load {
            chunk: 0,
            at: joint.centroid + Vec3::Y * d,
            energy: 1.0e5,
            crushed: 1.0e5 / material.crush,
            blow: Vec3::ZERO,
            press: Vec3::ZERO,
            peak: None,
            origin: Origin::Field,
        };
        let (mut near, mut far) = (0.0_f32, 5.0_f32);
        for _ in 0..60 {
            let mid = 0.5 * (near + far);
            let sources = [source(mid)];
            let crushed = crushed_radii(&sources, &material);
            if joint_load(&joint, &material.seam, &sources, &crushed).0 >= material.seam.tensile {
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
        let (left, right) = (w + w / 2 - 1, w + w / 2);
        let mut told_apart = false;
        for step in 0..40 {
            let energy = 1.0e4 * 10.0_f32.powf(step as f32 / 10.0);
            let source = |chunk: usize, energy: f32| Load {
                chunk,
                at: structure.chunks[chunk].centroid,
                energy,
                crushed: energy / structure.chunks[chunk].material.crush,
                blow: Vec3::ZERO,
                press: Vec3::ZERO,
                peak: None,
                origin: Origin::Field,
            };
            let sources = [source(right, energy), source(left, energy / 2.0)];
            let rule = |material_of: &dyn Fn(&Joint) -> Masonry| -> Vec<usize> {
                (0..structure.joints.len())
                    .filter(|&j| {
                        let joint = &structure.joints[j];
                        let material = material_of(joint);
                        let crushed = crushed_radii(&sources, &material);
                        let (load, threshold) =
                            joint_load(joint, &material.seam, &sources, &crushed);
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

    /// LOADS PRESERVE THEIR SOURCE'S VOLUME AND MOMENTUM: a fixture line's loads lie at
    /// most [`LINE_STEP`] apart along its chord, sum to its energy and its blow, and their blows'
    /// moment about any point is the whole blow's at the chord's midpoint; a blast's ray hits give
    /// one load per chunk face, each the sum of that face's volume, impulse and peak force at its
    /// impulse-weighted centroid; a line and a field state no peak force.
    /// The structural check and a freed group's launch read a line's momentum through its loads,
    /// and a blast's dish, push and peak force read the same face point. A line that multiplies
    /// or shifts its blow, or a blast that loses volume, momentum or peak force, fails here.
    /// Blast loads carry no kinetic cavity energy; only the pressure and impulse laws load it.
    #[test]
    fn loads_preserve_their_sources_volume_and_momentum() {
        let close = |got: Vec3, want: Vec3| (got - want).length() <= 1e-5 * want.length();
        let (entry, end) = (Vec3::new(0.0, 0.1, 0.0), Vec3::new(0.23, 0.05, 0.02));
        let (energy, blow) = (3.0e4, Vec3::new(40.0, -10.0, 5.0));
        let material = brick();
        let loads: Vec<Load> = Load::line(2, &material, entry, end, energy, blow).collect();
        assert_eq!(loads.len(), 5, "a 0.236 m chord in steps of at most 5 cm");
        assert!(loads.iter().all(|load| load.chunk == 2));
        assert!(
            loads
                .windows(2)
                .all(|pair| pair[0].at.distance(pair[1].at) <= LINE_STEP)
        );
        let spent: f32 = loads.iter().map(|load| load.energy).sum();
        assert!((spent / energy - 1.0).abs() < 1e-6, "{spent} J");
        let crushed: f32 = loads.iter().map(|load| load.crushed).sum();
        assert!(
            (crushed * material.crush / energy - 1.0).abs() < 1e-5,
            "{crushed} m³"
        );
        assert!(close(loads.iter().map(|load| load.blow).sum(), blow));
        let pivot = Vec3::new(1.0, -2.0, 0.5);
        let moment: Vec3 = loads
            .iter()
            .map(|load| (load.at - pivot).cross(load.blow))
            .sum();
        assert!(close(moment, ((entry + end) / 2.0 - pivot).cross(blow)));
        assert!(loads.iter().all(|load| load.peak.is_none()));
        let field = Load::field(&[(4, entry, 2.0, Vec3::X), (4, end, 1.0, Vec3::Z)], |_| 2.0);
        assert!(
            field.len() == 2 && field.iter().all(|load| load.peak.is_none()),
            "{field:?}"
        );
        assert_eq!(
            field.iter().map(|load| load.crushed).sum::<f32>(),
            1.5,
            "fragments crush their energy's volume"
        );
        let delivered: Vec3 = field
            .iter()
            .map(|load| (load.at - pivot).cross(load.blow))
            .sum();
        let expected = (entry - pivot).cross(Vec3::X) + (end - pivot).cross(Vec3::Z);
        assert!(
            close(delivered, expected),
            "nonparallel fragment pushes preserve their moment"
        );

        // ASSUMED fixture volumes over patches on the slab's top face. Every blow and peak is
        // along its inward normal, far too weak to scab; no energy-to-volume path enters.
        let hit = |chunk: usize, at: Vec3, crushed: f32, (impulse, peak): (Vec3, Vec3)| RayHit {
            chunk,
            at,
            normal: Vec3::Z,
            energy: 0.0,
            impulse,
            peak,
            pressure: peak.length(),
            crushed,
            depth: 0.0,
        };
        let slab = Block::cuboid(Vec3::new(-5.0, -5.0, -1.0), Vec3::new(5.0, 5.0, 0.0));
        let (a, b, c) = (Vec3::X, Vec3::Y, Vec3::new(0.0, 2.0, 0.0));
        let (i, j) = (Vec3::NEG_Z * 3.0, Vec3::NEG_Z * 7.0);
        let (f, g) = (Vec3::NEG_Z * 2.0e5, Vec3::NEG_Z * 3.0e5);
        let (loads, scabs) = Load::blast(
            &[
                hit(3, a, 0.1, (i, f)),
                hit(1, b, 0.2, (Vec3::NEG_Z, Vec3::NEG_Z)),
                hit(3, c, 0.3, (j, g)),
            ],
            |_| Backing {
                block: &slab,
                scale: 1.0,
                material: brick(),
                contacts: &[],
            },
        );
        assert!(scabs.is_empty(), "{scabs:?}");
        let chunks: Vec<usize> = loads.iter().map(|load| load.chunk).collect();
        assert_eq!(chunks, vec![1, 3]);
        assert_eq!(
            (loads[0].at, loads[0].energy, loads[0].blow, loads[0].peak),
            (b, 0.0, Vec3::NEG_Z, Some(Vec3::NEG_Z))
        );
        assert_eq!(loads[1].energy, 0.0, "a blast supplies no cavity source");
        assert!(
            (loads[1].crushed - 0.4).abs() < 1e-6,
            "a chunk's rays' volumes add"
        );
        assert!(close(
            loads[1].at,
            (a * i.length() + c * j.length()) / (i.length() + j.length())
        ));
        assert!(close(loads[1].blow, i + j));
        assert!(close(
            loads[1].peak.expect("a blast load states its peak"),
            f + g
        ));
    }

    /// IMPULSE-WEIGHTED CENTROIDS PRESERVE ANGULAR IMPULSE (the game's environment destruction
    /// model; study test 9). The structural check and a freed piece's spin rely on the summed
    /// moment about any pivot. An energy-weighted point, merging opposite faces, or dropping a
    /// load with no energy breaks this invariant. ASSUMED fixture patches and impulses.
    #[test]
    fn impulse_weighted_centroids_preserve_angular_impulse_without_an_energy_gate() {
        let hit = |at: Vec3, normal: Vec3, impulse: f32, energy: f32| RayHit {
            chunk: 3,
            at,
            normal,
            energy,
            impulse: -normal * impulse,
            peak: -normal * (100.0 * impulse),
            pressure: 100.0,
            crushed: impulse * 1e-5,
            depth: 0.0,
        };
        // Two faces of one slab, loaded far too weakly to scab (fixture data).
        let slab = Block::cuboid(Vec3::new(-10.0, -10.0, -1.0), Vec3::new(10.0, 10.0, 3.0));
        let hits = [
            hit(Vec3::new(1.0, 2.0, 3.0), Vec3::Z, 2.0, 50.0),
            hit(Vec3::new(4.0, -2.0, 3.0), Vec3::Z, 8.0, 1.0),
            hit(Vec3::new(2.0, 4.0, -1.0), Vec3::NEG_Z, 10.0, 0.0),
        ];
        for no_energy in [false, true] {
            let hits = hits.map(|hit| RayHit {
                energy: if no_energy { 0.0 } else { hit.energy },
                ..hit
            });
            let (loads, scabs) = Load::blast(&hits, |_| Backing {
                block: &slab,
                scale: 1.0,
                material: brick(),
                contacts: &[],
            });
            assert!(scabs.is_empty(), "{scabs:?}");
            assert_eq!(loads.len(), 2, "opposing faces retain their own loads");
            let expected: Vec3 = hits.iter().map(|hit| hit.impulse).sum();
            let delivered: Vec3 = loads.iter().map(|load| load.blow).sum();
            assert!((delivered - expected).length() < 1e-5);
            for pivot in [Vec3::ZERO, Vec3::new(-3.0, 2.0, 7.0)] {
                let expected: Vec3 = hits
                    .iter()
                    .map(|hit| (hit.at - pivot).cross(hit.impulse))
                    .sum();
                let delivered: Vec3 = loads
                    .iter()
                    .map(|load| (load.at - pivot).cross(load.blow))
                    .sum();
                assert!(
                    (delivered - expected).length() <= 1e-5 * expected.length(),
                    "{pivot}: {delivered} vs {expected}"
                );
            }
            let expected: f32 = hits.iter().map(|hit| hit.crushed).sum();
            let delivered: f32 = loads.iter().map(|load| load.crushed).sum();
            assert!((delivered - expected).abs() < 1e-7);
        }
    }

    /// THE BLAST'S IMPULSES ARE SWISDAK'S FITS: at Z = 1 and 10 m/kg^⅓ the reflected fit reads
    /// 884.7 and 59.33 Pa·s per kg^⅓ and the incident 236.3 and 31.04 (SOURCED: Swisdak 1994,
    /// Table 3, PDF p. 12), and the reflected agrees within 0.1 % with the same table's imperial
    /// row (psi·ms per lb^⅓ at ft/lb^⅓, SOURCED: Table 1, 3 of 3) converted to metric. A coefficient mistyped, or a branch
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

    /// THE BLAST'S PEAK PRESSURES ARE SWISDAK'S FITS, REFLECTED AT LEAST TWOFOLD: at Z = 1 the
    /// reflected and incident rows read 8152 and 1354 kPa, and at Z = 10 31.54 and 14.89 kPa
    /// (SOURCED: Swisdak 1994, Table 3, PDF p. 12); over the whole field (Z 0.06–40 m/kg^⅓) the
    /// normally reflected peak is at least twice the incident, both fall with distance but for
    /// the fits' joins (each a step under 1 %), and the reflected branches meet at Z = 2 within
    /// 0.1 %. The peak depends on the scaled distance alone and blends from reflected square on
    /// to incident grazing. Every blast load's peak force reads these, and the section and rest
    /// laws read that force against their strength: a mistyped coefficient, a branch taken at the
    /// wrong Z, a pressure scaled by the charge like an impulse, or the blend's weights swapped
    /// fails here.
    #[test]
    fn the_blast_peak_pressures_are_swisdaks_fits_reflected_at_least_twofold() {
        let close = |got: f32, want: f32| (got / want - 1.0).abs() < 1e-3;
        assert!(close(Blast::reflected_pressure(1.0), 8.1518e6));
        assert!(close(INCIDENT_PRESSURE.at(1.0) * 1e3, 1.3537e6));
        assert!(close(INCIDENT_PRESSURE.at(10.0) * 1e3, 14.889e3));
        assert!(close(Blast::reflected_pressure(10.0), 31.54e3));
        let join = REFLECTED_PRESSURE.branches[0].0;
        let far = REFLECTED_PRESSURE.branches[1].1;
        let l = join.ln();
        let beyond = far.iter().rev().fold(0.0, |sum, ck| sum * l + ck).exp();
        assert!(
            (beyond / REFLECTED_PRESSURE.at(join) - 1.0).abs() < 1e-3,
            "the reflected branches meet at their join"
        );
        let n = 2000;
        let z = |k: usize| 0.06 * (40.0_f32 / 0.06).powf(k as f32 / n as f32);
        for k in 0..=n {
            let (r, s) = (
                Blast::reflected_pressure(z(k)),
                INCIDENT_PRESSURE.at(z(k)) * 1e3,
            );
            assert!(
                r >= 2.0 * s,
                "Z {}: reflected {r} Pa under twice {s} Pa",
                z(k)
            );
            if k < n {
                let (r1, s1) = (
                    Blast::reflected_pressure(z(k + 1)),
                    INCIDENT_PRESSURE.at(z(k + 1)) * 1e3,
                );
                assert!(r1 <= r * 1.01 && s1 <= s * 1.01, "Z {}: rises", z(k));
            }
        }
        // W / 1.8 = 1 kg and 8 kg: the same scaled distance at twice the range (fixture data).
        let (one, eight) = (
            Blast::of(TNT_ENERGY * SURFACE_BURST_FACTOR, 1e-3, FIT_ONLY_PRODUCTS),
            Blast::of(
                8.0 * TNT_ENERGY * SURFACE_BURST_FACTOR,
                1e-3,
                FIT_ONLY_PRODUCTS,
            ),
        );
        for (d, cos) in [(0.5, 1.0), (3.0, 0.4), (12.0, 0.0)] {
            let (near, far) = (one.peak_pressure(d, cos), eight.peak_pressure(2.0 * d, cos));
            assert!((far / near - 1.0).abs() < 1e-4, "{d} m, cos {cos}");
        }
        assert!(close(
            one.peak_pressure(3.0, 1.0),
            Blast::reflected_pressure(3.0)
        ));
        assert!(close(
            one.peak_pressure(3.0, 0.0),
            INCIDENT_PRESSURE.at(3.0) * 1e3
        ));
    }

    /// THE PULSE'S FACTORS HIT THEIR LIMITS AND FALL BETWEEN THEM (the #237 study's test 7):
    /// `g(0) = f(0) = 1`, `g(1) = f(1) = 0` and both zero past 1, both falling strictly over
    /// `(0, 1)` and meeting their own branches at ½; and `g` is the work a rigid-plastic section
    /// absorbs under the triangular pulse, `J² g(r) / 2m`, integrated here directly within 0.5 %
    /// (fixture pulse). The section law of #237 relies on `g`'s ends being the wall design's two
    /// limits (impulsive, and a peak force under the strength moving nothing), the crush law on
    /// `f`'s: a branch flipped or taken at the wrong `r`, a ratio inverted, or a factor that does
    /// not vanish once the strength holds the peak fails here.
    #[test]
    fn the_pulse_factors_hit_their_limits_and_fall_between_them() {
        for factor in [plastic_demand, crush_factor] {
            assert_eq!((factor(0.0), factor(1.0)), (1.0, 0.0));
            for r in [1.5, 10.0, f32::INFINITY] {
                assert_eq!(factor(r), 0.0, "r {r}");
            }
            let n = 1000;
            for k in 0..n {
                let (r0, r1) = (k as f32 / n as f32, (k + 1) as f32 / n as f32);
                assert!(factor(r1) < factor(r0), "r {r0} to {r1}");
            }
            assert!(
                (factor(0.5 - 1e-4) - factor(0.5)).abs() < 1e-3,
                "the branches meet"
            );
        }
        // A section of unit mass under a pulse of unit peak force and duration (fixture data):
        // `F(t) = 1 − t` against resistance `r`, moving while the load beats it or it has speed.
        let blow = 0.5_f64;
        for r in [0.1_f64, 0.3, 0.5, 0.6, 0.8, 0.95] {
            let (dt, mut t, mut x, mut v) = (1e-5_f64, 0.0_f64, 0.0_f64, 0.0_f64);
            loop {
                let force = (1.0 - t).max(0.0);
                if v <= 0.0 && force <= r && t > 0.0 {
                    break;
                }
                let next = (v + (force - r) * dt).max(0.0);
                x += 0.5 * (v + next) * dt;
                v = next;
                t += dt;
            }
            let absorbed = r * x / (blow * blow / 2.0);
            let g = f64::from(plastic_demand(r as f32));
            assert!(
                (absorbed / g - 1.0).abs() < 5e-3,
                "r {r}: {absorbed} against {g}"
            );
        }
    }

    /// A bare fixture charge of `mass` kg (ASSUMED fixture core: 1650 kg/m³, `K` = 2400 m/s, the
    /// TNT convention's energy), its radius the equal-volume sphere's and its products at the
    /// burst law's bare speed `K √2`.
    fn bare_charge(mass: f32) -> Blast {
        let radius = (3.0 * mass / (4.0 * PI * 1650.0)).cbrt();
        let speed = 2400.0 * std::f32::consts::SQRT_2;
        Blast::of(
            mass * TNT_ENERGY,
            radius,
            crate::ballistics::burst::products_momentum(mass, speed),
        )
    }

    /// The free-air fit's normally reflected impulse (Pa·s) `distance` m from `blast`.
    fn fit_reflected(blast: &Blast, distance: f32) -> f32 {
        let scale = (blast.charge / SURFACE_BURST_FACTOR).cbrt();
        scale * REFLECTED_IMPULSE.at(blast.distance(distance) / scale)
    }

    /// Where `blast`'s products floor meets the fit at normal incidence, m: the least distance
    /// at which the floor no longer lies above it, bisected between the charge and the reach.
    fn products_crossing(blast: &Blast) -> f32 {
        let floored = |r: f32| blast.products_floor(r, fit_reflected(blast, r)).is_some();
        let (mut lo, mut hi) = (blast.radius, blast.reach());
        assert!(
            floored(lo) && !floored(hi),
            "the fixture's floor crosses the fit"
        );
        for _ in 0..64 {
            let mid = 0.5 * (lo + hi);
            if floored(mid) { lo = mid } else { hi = mid }
        }
        hi
    }

    /// THE PRODUCTS FLOOR MEETS THE FIT CONTINUOUSLY AND SCALES AS IT DOES (the game's
    /// environment blast field, near contact). Across the crossing the normally reflected
    /// impulse and peak move by no more than the distance does, and the crossing's scaled
    /// distance is one number for every charge of a core (Hopkinson-Cranz: the products'
    /// momentum and the fit both scale with the charge). Law 1's crater and Law 3's blow read
    /// this field: a floor that steps on at a fixed `Z` (a clamp at a constant product
    /// pressure), or a products term that falls faster than `R⁻²`, fails here. ASSUMED
    /// fixture charges; the crossing printed is the fixture core's, DERIVED by the code.
    #[test]
    fn the_products_floor_meets_the_fit_continuously_at_one_scaled_distance() {
        let mut scaled = Vec::new();
        for mass in [0.5_f32, 5.0, 50.0] {
            let blast = bare_charge(mass);
            let crossing = products_crossing(&blast);
            let (inside, outside) = (crossing * (1.0 - 1e-3), crossing * (1.0 + 1e-3));
            for (near, far) in [
                (
                    blast.specific_impulse(inside, 1.0),
                    blast.specific_impulse(outside, 1.0),
                ),
                (
                    blast.peak_pressure(inside, 1.0),
                    blast.peak_pressure(outside, 1.0),
                ),
            ] {
                assert!(
                    near >= far && (near / far - 1.0) < 1e-2,
                    "{mass} kg: the field jumps at the crossing, {near} against {far}"
                );
            }
            let at_surface =
                blast.specific_impulse(blast.radius, 1.0) / fit_reflected(&blast, blast.radius);
            let z = crossing / (blast.charge / SURFACE_BURST_FACTOR).cbrt();
            println!(
                "{mass} kg bare: crossing Z {z:.3} m/kg^(1/3), {at_surface:.2} x the fit at its surface"
            );
            scaled.push(z);
        }
        for z in &scaled {
            assert!((z / scaled[0] - 1.0).abs() < 1e-3, "{scaled:?}");
        }
    }

    /// THE PRODUCTS FLOOR NEVER LOWERS THE FIT (the game's environment blast field). At every
    /// distance from the charge to the reach and every incidence, the impulse and the peak lie
    /// at or above the free-air fit's. The near-contact crater and push rely on the floor
    /// only ever adding what the fit under-reads; a branch taken as a minimum, or a peak
    /// scaled down where the products win, fails here. ASSUMED fixture charges.
    #[test]
    fn the_products_floor_never_lowers_the_fit() {
        for mass in [0.5_f32, 5.0, 50.0] {
            let blast = bare_charge(mass);
            let fit = Blast {
                products: FIT_ONLY_PRODUCTS,
                ..blast
            };
            let steps = 400;
            for k in 0..=steps {
                let r = blast.radius * (blast.reach() / blast.radius).powf(k as f32 / steps as f32);
                for cos in [1.0_f32, 0.7, 0.3, 0.0] {
                    assert!(blast.specific_impulse(r, cos) >= fit.specific_impulse(r, cos));
                    assert!(blast.peak_pressure(r, cos) >= fit.peak_pressure(r, cos));
                }
            }
            assert!(
                blast.specific_impulse(blast.radius, 1.0) > fit.specific_impulse(blast.radius, 1.0),
                "{mass} kg: the fixture's floor acts at the charge"
            );
        }
    }

    /// OUTSIDE ITS CROSSING THE FIELD IS THE FIT, BIT FOR BIT (the game's environment blast
    /// field). Past the crossing the impulse and peak at every incidence are the ConWep blend
    /// of Swisdak's fits exactly, the expression the far field was before the floor existed.
    /// Every standoff outcome (the far topple range, the scab fixture numbers, the reach)
    /// relies on it; a floor added to the fit rather than taken over it, or one reaching past
    /// its crossing, fails here. ASSUMED fixture charges.
    #[test]
    fn outside_its_crossing_the_field_is_the_fit_bit_for_bit() {
        for mass in [0.5_f32, 5.0, 50.0] {
            let blast = bare_charge(mass);
            let scale = (blast.charge / SURFACE_BURST_FACTOR).cbrt();
            let crossing = products_crossing(&blast);
            let steps = 200;
            for k in 1..=steps {
                let r = crossing * (blast.reach() / crossing).powf(k as f32 / steps as f32);
                let z = blast.distance(r) / scale;
                for cos in [1.0_f32, 0.7, 0.3, 0.0] {
                    let impulse = scale
                        * (REFLECTED_IMPULSE.at(z) * cos * cos
                            + INCIDENT_IMPULSE.at(z) * (1.0 - cos) * (1.0 - cos));
                    let peak = Blast::reflected_pressure(z) * cos * cos
                        + INCIDENT_PRESSURE.at(z) * 1e3 * (1.0 - cos) * (1.0 - cos);
                    assert_eq!(
                        blast.specific_impulse(r, cos).to_bits(),
                        impulse.to_bits(),
                        "{mass} kg at {r} m"
                    );
                    assert_eq!(blast.peak_pressure(r, cos).to_bits(), peak.to_bits());
                }
            }
        }
    }

    /// A CHARGE'S RADIUS FLOORS EVERY POINT-SOURCE FIELD TERM (the game's environment
    /// destruction model). The contact adapter relies on a finite loaded patch even when the
    /// geometric stop lies at the source. A floor on pressure alone leaves zero impulse and
    /// crush; a missing floor on interception scales that patch inconsistently. ASSUMED fixture
    /// radius and energy; the class condition is peak pressure above the material strength.
    #[test]
    fn the_charge_radius_floors_a_contact_patch_and_its_crush() {
        let blast = Blast::of(1.47e6, 0.05, FIT_ONLY_PRODUCTS);
        let material = brick();
        let share = 1.0 / BLAST_RAYS as f32;
        assert_eq!(blast.distance(0.0), blast.radius);
        for distance in [0.0, blast.radius / 2.0, blast.radius] {
            let impulse = blast.push(share, distance, Vec3::Z, Vec3::NEG_Z);
            let peak = blast.peak(share, distance, Vec3::Z, Vec3::NEG_Z);
            let crushed = blast.crushed(&material, share, distance, Vec3::Z, Vec3::NEG_Z);
            assert!(impulse.z > 0.0 && peak.z > 0.0 && crushed.volume > 0.0 && crushed.depth > 0.0);
            assert_eq!(
                impulse,
                blast.push(share, blast.radius, Vec3::Z, Vec3::NEG_Z)
            );
            assert_eq!(peak, blast.peak(share, blast.radius, Vec3::Z, Vec3::NEG_Z));
        }
        let blocks = [Block::cuboid(
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.25),
        )];
        let hits = blast_rays(
            &blocks,
            |_| true,
            |_| material,
            Vec3::ZERO,
            blast,
            &[(Vec3::Z, share)],
        );
        assert_eq!(hits.len(), 1);
        assert!(hits[0].crushed > 0.0 && hits[0].impulse.z > 0.0);
    }

    /// CRUSH WORK STAYS WITHIN THE GAME'S BLAST BUDGET (study test 5). The independent audit is
    /// `work = sigma_c,d epsilon_v volume = rho u_c^2 volume`; the Law 1 input remains pressure
    /// and impulse. The field budget relies on no hidden energy source in the crater response.
    /// ASSUMED bare-charge fixtures and planar geometry; the sourced rows supply each material.
    #[test]
    fn crush_work_is_an_audit_within_half_the_blast_energy() {
        let blocks = [Block::cuboid(
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.25),
        )];
        let directions = ball_directions(4096);
        let mut largest = 0.0_f32;
        for material in [brick(), wood()] {
            for mass in [0.1_f32, 1.0, 5.0] {
                // ASSUMED fixture filler density, kg/m3; DERIVED equal-volume sphere radius.
                let radius = (3.0 * mass / (4.0 * PI * 1600.0)).cbrt();
                let blast = Blast::of(mass * TNT_ENERGY, radius, FIT_ONLY_PRODUCTS);
                for range in [radius, 0.1_f32.max(radius), 0.5, 1.0] {
                    let hits = blast_rays(
                        &blocks,
                        |_| true,
                        |_| material,
                        Vec3::new(0.0, 0.0, -range),
                        blast,
                        &directions,
                    );
                    let volume: f32 = hits.iter().map(|hit| hit.crushed).sum();
                    let work = volume * material.density * material.compaction.front_speed.powi(2);
                    let fraction = work / blast.energy;
                    largest = largest.max(fraction);
                    assert!(
                        fraction <= 0.5,
                        "{material:?}, {mass} kg at {range} m: {fraction}"
                    );
                }
            }
        }
        println!("MEASURED maximum crush-work / blast-energy fraction {largest:.6e}");
    }

    /// MOMENTUM SETS THE CRUSHED MASS (the game's environment destruction model, the #237 study's
    /// test 1). At a fixed peak pressure a face patch of each shipped breakable material crushes
    /// `Δm = J f(σ_c,d / P) / u_c`, so doubling its impulse doubles the mass; and the pool takes
    /// a blast load's crushed volume, whatever diagnostic energy the ray carries. Law 1 and the crater
    /// footprint rely on it: a law reading the energy, squaring the impulse, or a pool worn by
    /// the blast's energy fails here. Fixture pressures, impulses and energies.
    #[test]
    fn momentum_sets_the_crushed_mass() {
        for material in [brick(), wood()] {
            let Compaction {
                strength,
                front_speed,
            } = material.compaction;
            for over in [1.5_f32, 4.0, 50.0] {
                let pressure = strength * over;
                let once = material.crushed_by(pressure, 100.0);
                let mass = once * material.density * front_speed / 100.0;
                assert!(
                    (mass / crush_factor(1.0 / over) - 1.0).abs() < 1e-5,
                    "{material:?} at {over} σ"
                );
                let twice = material.crushed_by(pressure, 200.0);
                assert!(
                    (twice / once - 2.0).abs() < 1e-5,
                    "{over} σ: {once} {twice}"
                );
            }
        }

        let structure = row(2);
        let chunk = &structure.chunks[0];
        let crushed = 0.25 * full_pool(chunk);
        let hit = |energy: f32| RayHit {
            chunk: 0,
            at: chunk.centroid,
            normal: Vec3::Z,
            energy,
            impulse: Vec3::NEG_Z,
            peak: Vec3::NEG_Z,
            pressure: 1.0,
            crushed,
            depth: 0.0,
        };
        // A hull about the fixture chunk; its one newton peak is far too weak to scab.
        let hull = Block::cuboid(chunk.centroid - 0.05, chunk.centroid + 0.05);
        let backing = |_: usize| Backing {
            block: &hull,
            scale: 1.0,
            material: chunk.material,
            contacts: &[],
        };
        let pools: Vec<f32> = [1.0, 1.0e3 * chunk.material.crush * chunk.volume]
            .into_iter()
            .map(|energy| {
                let mut state = WallState::new(&structure);
                strike(
                    &structure,
                    &mut state,
                    &Load::blast(&[hit(energy)], backing).0,
                );
                assert_eq!(state.fate[0], Fate::Standing, "{energy} J");
                state.pool[0]
            })
            .collect();
        assert_eq!(pools[0], pools[1], "the blast's energy wears nothing");
        assert!((pools[0] - (full_pool(chunk) - crushed)).abs() <= 1e-6 * full_pool(chunk));
    }

    /// A LONG LOW PULSE CRUSHES NOTHING (the game's environment destruction model, the #237
    /// study's test 2). Where the peak pressure does not beat a material's dynamic compressive
    /// strength nothing crushes, however large the impulse; a far burst's ray carrying many
    /// times a chunk's crushing work as diagnostic energy leaves its pool full and cracks no
    /// cavity-driven joint. Standoff bursts leaving no crater
    /// rely on it: a crush law that drops its pressure gate, or a pool the blast's energy wears,
    /// fails here. Fixture pressures, impulses and burst.
    #[test]
    fn a_long_low_pulse_crushes_nothing() {
        for material in [brick(), wood()] {
            let strength = material.compaction.strength;
            for pressure in [strength, 0.99 * strength, 0.1 * strength, 0.0] {
                for impulse in [1.0_f32, 1.0e3, 1.0e6] {
                    assert_eq!(material.crushed_by(pressure, impulse), 0.0, "{pressure} Pa");
                }
            }
        }
        // W / 1.8 = 1000 kg at 20 m: Z = 2 m/kg^⅓, a peak far under brick's dynamic strength
        // with a long, heavy pulse (fixture burst).
        let blast = Blast::of(
            1000.0 * TNT_ENERGY * SURFACE_BURST_FACTOR,
            1e-3,
            FIT_ONLY_PRODUCTS,
        );
        let cube = [Block::cuboid(Vec3::splat(-0.125), Vec3::splat(0.125))];
        let origin = Vec3::new(0.0, 0.0, -20.125);
        let hits = blast_rays(
            &cube,
            |_| true,
            |_| brick(),
            origin,
            blast,
            &[(Vec3::Z, 1e-6)],
        );
        let [hit] = hits[..] else {
            panic!("one ray meets the cube: {hits:?}");
        };
        assert!(blast.peak_pressure(20.0, 1.0) < 0.1 * brick().compaction.strength);
        assert!(hit.impulse.length() > 0.0 && hit.crushed == 0.0, "{hit:?}");
        let structure = row(1);
        let chunk = &structure.chunks[0];
        let heavy = RayHit {
            energy: 10.0 * chunk.material.crush * chunk.volume,
            ..hit
        };
        let mut state = WallState::new(&structure);
        let (loads, scabs) = Load::blast(&[heavy], |_| Backing {
            block: &cube[0],
            scale: 1.0,
            material: brick(),
            contacts: &[],
        });
        assert!(scabs.is_empty(), "a far pulse throws no scab: {scabs:?}");
        let struck = strike(&structure, &mut state, &loads);
        assert!(
            struck.worn.is_empty() && struck.dusted.is_empty() && struck.cracked.is_empty(),
            "{struck:?}"
        );
        assert_eq!(state.pool[0], full_pool(chunk), "the pool stays full");
    }

    /// How far the ray sampler's crushed volume may stray from its finest cast, relative: MEASURED
    /// at most 3.17e-4 over the fixtures of [`crushed_mass_is_representation_independent`] (1024 to
    /// 65536 rays, one block or 8 × 8 chunks), a margin of about three over it (ASSUMED). A near
    /// burst's crush lies where its rays are densest; the far field, where the sampler's coverage
    /// varies with distance (#204), is not certified by these fixtures. Their standoff fields
    /// do not beat this material's crush threshold; larger charges and other rows need their
    /// own sampling evidence.
    const SAMPLED_CRUSH_SPREAD: f32 = 1e-3;

    /// CRUSHED MASS IS REPRESENTATION-INDEPENDENT (the game's environment destruction model, the
    /// #237 study's test 4). Law 1 is linear in the impulse at a fixed pressure, so a face cut
    /// into any number of patches under one field crushes the mass it does as one patch, exactly
    /// (to f32 rounding). #204's freedom to change how the field finds surfaces relies on it: a
    /// crush law with a per-patch term or a power of the impulse other than one fails here.
    ///
    /// The ray sampler is today's representation. A near burst on a fixture brick face 2 m
    /// square (fixture geometry and charges), one block or 8 × 8 chunks, is cast at several ray
    /// counts; the crushed volumes are printed MEASURED for #204 and must agree within
    /// [`SAMPLED_CRUSH_SPREAD`] of the finest cast.
    #[test]
    fn crushed_mass_is_representation_independent() {
        for material in [brick(), wood()] {
            let pressure = 3.0 * material.compaction.strength;
            let whole = material.crushed_by(pressure, 1000.0);
            for parts in [2_u16, 3, 7, 16, 100] {
                let cut: f32 = (0..parts)
                    .map(|_| material.crushed_by(pressure, 1000.0 / f32::from(parts)))
                    .sum();
                assert!(
                    (cut / whole - 1.0).abs() < 1e-5,
                    "{parts} parts: {cut} {whole}"
                );
            }
        }

        let slab = [Block::cuboid(
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.25),
        )];
        let chunks: Vec<Block> = (0..64)
            .map(|k| {
                let low = Vec3::new(
                    -1.0 + 0.25 * (k % 8) as f32,
                    -1.0 + 0.25 * (k / 8) as f32,
                    0.0,
                );
                Block::cuboid(low, low + Vec3::new(0.25, 0.25, 0.25))
            })
            .collect();
        for (name, energy, standoff) in [
            ("88 mm HE burst", 1.47e6_f32, 0.1_f32),
            ("bare 5 kg", 5.0 * TNT_ENERGY, 0.3),
        ] {
            let blast = Blast::of(energy, 1e-3, FIT_ONLY_PRODUCTS);
            let origin = Vec3::new(0.1, -0.05, -standoff);
            let crushed = |blocks: &[Block], rays: usize| -> f32 {
                blast_rays(
                    blocks,
                    |_| true,
                    |_| brick(),
                    origin,
                    blast,
                    &ball_directions(rays),
                )
                .iter()
                .map(|hit| hit.crushed)
                .sum()
            };
            let finest = crushed(&slab, 65_536);
            assert!(finest > 0.0, "{name}: the fixture burst crushes the face");
            for rays in [1024, 4096, 16_384, 65_536] {
                for (form, blocks) in [("one block", &slab[..]), ("8 x 8 chunks", &chunks[..])] {
                    let volume = crushed(blocks, rays);
                    let spread = volume / finest - 1.0;
                    println!(
                        "MEASURED {name}, {form}, {rays} rays: crushed {:.4e} m³, {:+.6e} of \
                         one block at 65536",
                        volume, spread
                    );
                    assert!(
                        spread.abs() <= SAMPLED_CRUSH_SPREAD,
                        "{name}, {form}, {rays} rays: {volume} against {finest}"
                    );
                }
            }
        }
    }

    /// A BLAST RAY'S PEAK FORCE IS ITS PEAK PRESSURE OVER THE PATCH ITS PUSH LOADS: square on, at
    /// 60° and grazing, the peak force lies along the push, into the face, and stands to it as the
    /// peak pressure to the specific impulse; a cube's face that intercepts a share of a far
    /// ray's cone takes that share of both (fixture geometry). The section law of #237 reads the
    /// peak force against a section's strength: a peak along the ray, over another patch, or kept
    /// whole where the push is intercepted fails here.
    #[test]
    fn a_blast_rays_peak_force_is_its_peak_pressure_over_its_pushs_patch() {
        // W / 1.8 = 1 kg, so the scaled distance is the distance (fixture data).
        let blast = Blast::of(TNT_ENERGY * SURFACE_BURST_FACTOR, 1e-3, FIT_ONLY_PRODUCTS);
        let share = 1.0 / BLAST_RAYS as f32;
        let d = 2.0;
        for angle in [0.0_f32, 60.0, 89.4] {
            let (sin, cos) = angle.to_radians().sin_cos();
            let dir = Vec3::new(sin, 0.0, -cos);
            let (push, peak) = (
                blast.push(share, d, dir, Vec3::Z),
                blast.peak(share, d, dir, Vec3::Z),
            );
            let ratio = blast.peak_pressure(d, cos) / blast.specific_impulse(d, cos);
            assert!(
                (peak - push * ratio).length() <= 1e-4 * peak.length() && peak.z < 0.0,
                "{angle}°: peak {peak} against push {push}"
            );
        }
        let cube = [Block::cuboid(Vec3::ZERO, Vec3::splat(CUBE))];
        let origin = Vec3::new(CUBE / 2.0, CUBE / 2.0, -10.0);
        let hits = blast_rays(
            &cube,
            |_| true,
            |_| brick(),
            origin,
            blast,
            &[(Vec3::Z, share)],
        );
        let kept = CUBE * CUBE / Blast::section(share, 10.0);
        assert!(kept < 1.0, "the far ray's cone outgrows the face");
        let want = Blast::reflected_pressure(10.0) * Blast::section(share, 10.0) * kept;
        assert!(
            (hits[0].peak - Vec3::Z * want).length() <= 1e-4 * want,
            "{} against {want}",
            hits[0].peak
        );
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
        let blast = Blast::of(TNT_ENERGY * SURFACE_BURST_FACTOR, 1e-3, FIT_ONLY_PRODUCTS);
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

    /// A RAY'S CONE PUSHES AND LOADS ONLY THE SILHOUETTE IT LANDS ON, AND NO RAY GOES PAST REACH:
    /// one ray square onto a cube face (fixture geometry), at a range where its cone's section
    /// outgrows the face, delivers the reflected impulse over the face's own area and the share of
    /// its energy the face intercepts, `A / section`; close in, where the section is under the
    /// face, the whole ray; and past [`Blast::reach`] the cube takes nothing. A push over the
    /// ray's patch (one far ray throwing a whole house), an energy deposit independent of range, or
    /// a field read past its fit's range fails here; every chunk and body a blast meets reads this.
    #[test]
    fn a_ray_pushes_only_the_face_it_meets_and_stops_at_reach() {
        // W / 1.8 = 1 kg, so the scaled distance is the distance and reach is the fit's edge.
        let blast = Blast::of(TNT_ENERGY * SURFACE_BURST_FACTOR, 1e-3, FIT_ONLY_PRODUCTS);
        let share = 1.0 / BLAST_RAYS as f32;
        let face = CUBE * CUBE;
        let cube = [Block::cuboid(Vec3::ZERO, Vec3::splat(CUBE))];
        let hit = |d: f32| {
            let origin = Vec3::new(CUBE / 2.0, CUBE / 2.0, -d);
            blast_rays(
                &cube,
                |_| true,
                |_| brick(),
                origin,
                blast,
                &[(Vec3::Z, share)],
            )
        };
        for d in [0.5_f32, 10.0, 39.0] {
            let section = Blast::section(share, d);
            let kept = (face / section).min(1.0);
            let hits = hit(d);
            assert_eq!(hits.len(), 1, "{d} m");
            let want = REFLECTED_IMPULSE.at(d) * section * kept;
            assert!(
                (hits[0].impulse - Vec3::Z * want).length() <= 1e-4 * want,
                "{d} m: {} against {want}",
                hits[0].impulse
            );
            let energy = blast.energy * share * kept;
            assert!((hits[0].energy - energy).abs() <= 1e-4 * energy, "{d} m");
        }
        assert!(Blast::section(share, 0.5) < face && Blast::section(share, 10.0) > face);
        assert!(hit(blast.reach() + 0.5).is_empty(), "past reach");
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
                        bond: Bond::Bonded(material.seam),
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
            bond: Bond::Bonded(material.seam),
        }));
        Structure { chunks, joints }
    }

    /// Spend each of `chunks`' pool ([`wear`]), the removal alone with no stress about it: the
    /// chunks it turned to dust, ascending.
    fn spend(structure: &Structure, state: &mut WallState, chunks: &[usize]) -> Vec<usize> {
        let mut dusted: Vec<usize> = chunks
            .iter()
            .copied()
            .filter(|&c| wear(structure, state, c, full_pool(&structure.chunks[c])))
            .collect();
        dusted.sort_unstable();
        dusted
    }

    /// Turn the chunks `props` (ascending) to dust ([`spend`]), run the weight check on that
    /// change, then support: the state, the check's result and the groups that fell.
    fn dust_and_weigh(
        structure: &Structure,
        props: &[usize],
    ) -> (WallState, Weighed, Vec<Vec<usize>>) {
        let mut state = WallState::new(structure);
        let dusted = spend(structure, &mut state, props);
        assert_eq!(dusted, props, "the props turn to dust");
        let weighed = weigh(
            structure,
            &mut state,
            &dusted,
            &[],
            &[],
            &[],
            &mut SectionCache::default(),
        );
        let fell = fall(structure, &mut state);
        (state, weighed, fell)
    }

    /// Each failure's cause and joints.
    fn cracks(weighed: &Weighed) -> Vec<(Cause, Vec<usize>)> {
        weighed
            .failures
            .iter()
            .map(|failure| (failure.cause, failure.joints.clone()))
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
        PI * r.powi(3) * brick().seam.tensile / (2.0 * GRAVITY * CUBE * (n * n) as f32)
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
                assert_eq!(cracks(&weighed), vec![(Cause::Weight, vec![root])]);
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
        let limit = brick().seam.tensile * CUBE * CUBE / (deep as f32 * GRAVITY);
        for (share, falls) in [(0.98, false), (1.02, true)] {
            let structure = cubes(&cells, 2, &props, limit * share);
            let (_, weighed, fell) = dust_and_weigh(&structure, &props);
            if falls {
                let top = vec![
                    joint_between(&structure, 0, panel[panel.len() - 2]),
                    joint_between(&structure, 1, panel[panel.len() - 1]),
                ];
                let mut opened: Vec<_> = weighed
                    .failures
                    .iter()
                    .flat_map(|failure| {
                        assert_eq!(failure.cause, Cause::Weight);
                        failure.joints.iter().copied()
                    })
                    .collect();
                opened.sort_unstable();
                assert_eq!(opened, top);
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
        let idle = weigh(&structure, &mut state, &[], &[], &[], &[], &mut sections);
        assert_eq!(idle, Weighed::default(), "no change, no work");
        let half = full_pool(&structure.chunks[2]) / 2.0;
        assert!(!wear(&structure, &mut state, 2, half), "worn, not dust");
        let idle = weigh(&structure, &mut state, &[], &[], &[], &[], &mut sections);
        assert_eq!(idle, Weighed::default(), "a wear alone, no work");
        assert!(fall(&structure, &mut state).is_empty());

        let tip = n as usize;
        let dusted = spend(&structure, &mut state, &[tip]);
        let weighed = weigh(
            &structure,
            &mut state,
            &dusted,
            &[],
            &[],
            &[],
            &mut sections,
        );
        let root = joint_between(&structure, 0, 1);
        assert_eq!(cracks(&weighed), vec![(Cause::Weight, vec![root])]);
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

    /// AN ARM THAT STILL SITS ON A CRACKED SEAM OR A REST IS NO CANTILEVER: the fixture arm of four
    /// cubes at 2 % over its cantilever limit ([`cantilever_limit`]) loses every prop but the
    /// tip's, and the tip stands on that prop through a seam already cracked, or through a rest.
    /// Either way the arm bears on the prop as well as hanging from its root, so the check cracks
    /// nothing, nothing falls, and the arm's touched chunks stand unassessed. A check that read a
    /// crack as the end of a contact, or a rest under brick as no support (a pier whose foot a
    /// round cracked, weighed as hanging from the lintel beside it), fails here.
    #[test]
    fn an_arm_that_sits_on_a_cracked_seam_or_a_rest_is_no_cantilever() {
        let n = 4;
        let cells = propped_arm(n);
        let props: Vec<usize> = (n as usize + 1..=2 * n as usize).collect();
        let (tip, tip_prop) = (n as usize, 2 * n as usize);
        let gone = &props[..props.len() - 1];
        for rest in [false, true] {
            let mut structure = cubes(&cells, 1, &props, cantilever_limit(n) * 1.02);
            let seat = joint_between(&structure, tip, tip_prop);
            let mut state = WallState::new(&structure);
            if rest {
                structure.joints[seat].bond = Bond::Resting;
            } else {
                state.cracked[seat] = true;
            }
            let dusted = spend(&structure, &mut state, gone);
            assert_eq!(dusted, gone, "the props turn to dust");
            let weighed = weigh(
                &structure,
                &mut state,
                &dusted,
                &[],
                &[],
                &[],
                &mut SectionCache::default(),
            );
            assert_eq!(cracks(&weighed), Vec::new(), "rest {rest}");
            assert!(fall(&structure, &mut state).is_empty(), "rest {rest}");
            assert!(
                (1..tip).all(|c| weighed.unassessed.contains(&c)),
                "rest {rest}: {:?}",
                weighed.unassessed
            );
        }
    }

    /// A REST NEVER CARRIES TENSION: a fixture cube run out from a support no section cuts, on a
    /// prop that turns to dust, at 2 % under the cantilever limit of its root joint
    /// ([`cantilever_limit`]). Joined by a seam it hangs from the root and stands; joined by a
    /// rest (its plane upright, so it bears nothing) it falls at that first change, every rest it
    /// stood on opened as one [`Cause::Tip`] failure, though no load reaches anything's strength.
    /// The roof planes and wall plates of a building rely on it: a part set on another holds only
    /// by bearing on it.
    #[test]
    fn a_rest_never_carries_tension() {
        let cells = propped_arm(1);
        let props = [2];
        let mut structure = cubes(&cells, 1, &props, 0.98 * cantilever_limit(1));
        let (_, weighed, fell) = dust_and_weigh(&structure, &props);
        assert_eq!(
            cracks(&weighed),
            Vec::new(),
            "bonded, it hangs within its limit"
        );
        assert!(fell.is_empty());
        let root = joint_between(&structure, 0, 1);
        structure.joints[root].bond = Bond::Resting;
        let (state, weighed, fell) = dust_and_weigh(&structure, &props);
        assert_eq!(cracks(&weighed), vec![(Cause::Tip, vec![root])]);
        assert!(state.cracked[root]);
        assert_eq!(fell, vec![vec![1]]);
    }

    /// A PUSH LIFTS A SEATED PART OVER ITS RESTS AT ITS DERIVED IMPULSE: a fixture cube resting on
    /// an anchored one, pushed at its top edge along -z. Its rest has no seam, so the push pays
    /// only for raising its centre of mass over the hinge, the rest disc's rim toward -z: capacity
    /// `√(2 I m g (√(h² + r²) − h))`, `I` its inertia about the hinge, `h` its centre's height over
    /// the rest and `r` the disc's radius (DERIVED, fixture geometry). At 1 % over, the rest opens
    /// and the cube falls with its push; at 1 % under, it stays seated. A push that never reached a
    /// part held by rests alone (a roof plane a blast under it cannot lift), or that opened one on
    /// any push, fails here.
    #[test]
    fn a_push_lifts_a_seated_part_over_its_rests_at_its_derived_impulse() {
        let mass = 28.8;
        let mut structure = cubes(&[[0, 0, 0], [0, 1, 0]], 0, &[0], mass);
        let rest = joint_between(&structure, 0, 1);
        structure.joints[rest].bond = Bond::Resting;
        let joint = structure.joints[rest];
        let top = structure.chunks[1];
        let rim = (joint.area / PI).sqrt();
        let hinge = joint.centroid - Vec3::Z * rim;
        let r = top.centroid - hinge;
        let own = (3.0 * top.volume / (4.0 * PI)).cbrt();
        let inertia = mass * (r.length_squared() - r.x * r.x + 0.4 * own * own);
        let lift = mass * GRAVITY * (r.y.hypot(r.z) - r.y);
        let capacity = (2.0 * inertia * lift).sqrt();
        let at = top.centroid + Vec3::new(0.0, CUBE / 2.0, CUBE / 2.0);
        let lever = at.y - joint.centroid.y;
        for (scale, opens) in [(1.01, true), (0.99, false)] {
            let mut state = WallState::new(&structure);
            let push = Load {
                chunk: 1,
                at,
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::NEG_Z * (scale * capacity / lever),
                press: Vec3::ZERO,
                peak: None,
                origin: Origin::Field,
            };
            let failures = structural(
                &structure,
                &mut state,
                &[push],
                &mut SectionCache::default(),
            );
            assert_eq!(state.cracked[rest], opens, "{scale} x capacity");
            let fell = fall(&structure, &mut state);
            if opens {
                assert_eq!(failures.len(), 1);
                assert_eq!(failures[0].joints, vec![rest]);
                assert_eq!(fell, vec![vec![1]]);
            } else {
                assert!(failures.is_empty() && fell.is_empty(), "{scale} x capacity");
            }
        }
    }

    /// A SEATED PART STANDS WHILE ITS CENTRE OF MASS IS OVER ITS RESTS: a fixture bar of two cubes
    /// joined by a seam, one resting on a support no section cuts, the other on a prop turned to
    /// dust. The rest is a disc of its area, radius `r = √(A/π)` (DERIVED: the disc reading of a
    /// joint), so the bar stands with its centre of mass `0.9 r` from the rest's centroid,
    /// horizontally, and falls whole at `1.1 r`, its one rest opened. A check that weighed a seated
    /// part by tension, or by the rest's centroid alone, fails here; a roof plane seated on its
    /// plate and gables stands by it, and falls by it once those go.
    #[test]
    fn a_seated_part_stands_while_its_centre_is_over_its_rests() {
        let cells = [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]];
        let props = [3];
        let radius = (CUBE * CUBE / PI).sqrt();
        for (reach, falls) in [(0.9, false), (1.1, true)] {
            let mut structure = cubes(&cells, 1, &props, 100.0);
            let seat = joint_between(&structure, 0, 1);
            structure.joints[seat].bond = Bond::Resting;
            // The bar's centre lies `share` of a cube width from chunk 1 toward chunk 2.
            let share = reach * radius / CUBE;
            structure.chunks[1].mass = 100.0 * (1.0 - share);
            structure.chunks[2].mass = 100.0 * share;
            let (_, weighed, fell) = dust_and_weigh(&structure, &props);
            if falls {
                assert_eq!(cracks(&weighed), vec![(Cause::Tip, vec![seat])]);
                assert_eq!(fell, vec![vec![1, 2]]);
            } else {
                assert_eq!(cracks(&weighed), Vec::new(), "at {reach} r it stands");
                assert!(fell.is_empty());
            }
        }
    }

    /// The shipped wood, as the laws read it.
    fn wood() -> Masonry {
        Masonry::of(SubstanceRegistry::shipped_static(), "Wood")
            .expect("the shipped registry holds Wood")
            .expect("Wood states bulk facts")
    }

    /// `structure` turned by `angle` (radians) about the z axis through the origin, its +y tilting
    /// toward +x: every centroid and joint normal turned with it (fixture geometry).
    fn tilted(mut structure: Structure, angle: f32) -> Structure {
        let turn = bevy::math::Quat::from_rotation_z(-angle);
        for chunk in &mut structure.chunks {
            chunk.centroid = turn * chunk.centroid;
        }
        for joint in &mut structure.joints {
            joint.centroid = turn * joint.centroid;
            joint.normal = turn * joint.normal;
        }
        structure
    }

    /// A RESTING BLOCK SLIDES DOWN AN INCLINE STEEPER THAN ITS PAIR'S FRICTION, AND STAYS ON ONE
    /// SHALLOWER: a fixture block of the shipped wood laid on an anchored cube of the shipped
    /// brick, kept from sliding by a stop downhill, the whole tilted until the rest's slope is
    /// 0.5 % over or under the pair's coefficient (the geometric mean of the two rows, the one
    /// pair rule, about 0.57, DERIVED). The stop turns to dust. The block's centre of mass lies
    /// well over its rest, so only the slope can free it: over, its one rest opens as a
    /// [`Cause::Slide`] failure and the block falls; under, it stands. A law that held
    /// every seated part by friction, read one side's coefficient (wood's 0.5, brick's 0.65) or
    /// another pair rule (the mean, 0.9 % over the geometric), fails here; a timber laid unfixed on
    /// a slope slides by it.
    #[test]
    fn a_resting_block_slides_down_an_incline_steeper_than_its_pairs_friction() {
        // The one pair rule, the geometric mean (`substances::PAIR_RULE`), restated.
        let friction = (wood().friction * brick().friction).sqrt();
        let stop = 2;
        for (share, slides) in [(1.005, true), (0.995, false)] {
            let mut structure = cubes(
                &[[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]],
                0,
                &[0, 3],
                100.0,
            );
            structure.chunks[1].material = wood();
            let seat = joint_between(&structure, 0, 1);
            structure.joints[seat].bond = Bond::Resting;
            let structure = tilted(structure, (share * friction).atan());
            let (_, weighed, fell) = dust_and_weigh(&structure, &[stop]);
            if slides {
                assert_eq!(cracks(&weighed), vec![(Cause::Slide, vec![seat])]);
                let failure = &weighed.failures[0];
                assert!((failure.demand / failure.capacity / share - 1.0).abs() < 1e-4);
                assert_eq!(fell, vec![vec![1]]);
            } else {
                assert_eq!(cracks(&weighed), Vec::new(), "at {share} x its friction");
                assert!(fell.is_empty(), "at {share} x its friction");
            }
        }
    }

    /// A PUSH SHOVES A SEATED PART OFF ITS RESTS AT ITS DERIVED IMPULSE, NET OF THE FRICTION ITS
    /// A PUSH THROUGH A SEATED PART'S CENTRE ALONG ITS RESTS' NORMAL LIFTS IT AT ITS DERIVED
    /// IMPULSE: a fixture cube resting level on an anchored one, pushed straight up at its centre
    /// of mass, so the push turns it about no hinge and slides it nowhere. Its rests carry no
    /// tension, so the push pays only for raising its centre `h = CUBE / 2` over the rest: the
    /// rest opens from the impulse `m √(2 g h)` (DERIVED, fixture geometry). At 1 % over it opens,
    /// read as a lift, and the cube falls free of it; at 1 % under it stays seated. A law that
    /// read a rest by its moment alone (every centred normal push ignored, whatever its size), or
    /// that freed a part on any lift, fails here.
    #[test]
    fn a_centred_push_along_the_rests_normal_lifts_a_seated_part_at_its_derived_impulse() {
        let mass = 20.0;
        let mut structure = cubes(&[[0, 0, 0], [0, 1, 0]], 0, &[0], mass);
        let rest = joint_between(&structure, 0, 1);
        structure.joints[rest].bond = Bond::Resting;
        let top = structure.chunks[1].centroid;
        let height = top.y - structure.joints[rest].centroid.y;
        let capacity = mass * (2.0 * GRAVITY * height).sqrt();
        for (scale, opens) in [(1.01, true), (0.99, false)] {
            let mut state = WallState::new(&structure);
            let push = Load {
                chunk: 1,
                at: top,
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::Y * (scale * capacity),
                press: Vec3::ZERO,
                peak: None,
                origin: Origin::Field,
            };
            let failures = structural(
                &structure,
                &mut state,
                &[push],
                &mut SectionCache::default(),
            );
            let fell = fall(&structure, &mut state);
            if opens {
                let causes: Vec<(Cause, Vec<usize>)> = failures
                    .iter()
                    .map(|failure| (failure.cause, failure.joints.clone()))
                    .collect();
                assert_eq!(
                    causes,
                    vec![(Cause::Lift, vec![rest])],
                    "{scale} x capacity"
                );
                assert_eq!(fell, vec![vec![1]], "{scale} x capacity");
            } else {
                assert!(failures.is_empty() && fell.is_empty(), "{scale} x capacity");
            }
        }
    }

    /// PRESS RAISES: a fixture block of the shipped wood laid level on an anchored cube of the
    /// shipped brick, pushed along -z at its rest's centroid, so the push turns it about no hinge.
    /// Sliding its centre of mass off the rest, a disc of radius `r` read as its octagon, which has
    /// a corner along -z (a level plane's axes run along x and z, [`plane_axes`]), costs the
    /// friction work `μ m g r`, `μ` the pair's coefficient, so the rest opens from the impulse
    /// `m √(2 μ g r)` (DERIVED, fixture geometry). At 1 % over the block falls with its push and at
    /// 1 % under it stays; at 1 % over with a press into the rest whose friction impulse takes 5 %
    /// back, it stays. A law that let only a turn carry a part off its rests (a beam shoved along a
    /// wall's top at its foot), or that read no friction impulse from a press, fails here.
    #[test]
    fn a_push_shoves_a_seated_part_off_its_rests_at_its_derived_impulse() {
        let friction = (wood().friction * brick().friction).sqrt();
        let mass = 20.0;
        let mut structure = cubes(&[[0, 0, 0], [0, 1, 0]], 0, &[0], mass);
        structure.chunks[1].material = wood();
        let rest = joint_between(&structure, 0, 1);
        structure.joints[rest].bond = Bond::Resting;
        let joint = structure.joints[rest];
        let radius = (joint.area / PI).sqrt();
        let capacity = mass * (2.0 * friction * GRAVITY * radius).sqrt();
        for (along, press, opens) in [(1.01, 0.0, true), (0.99, 0.0, false), (1.01, 0.05, false)] {
            let mut state = WallState::new(&structure);
            let push = Load {
                chunk: 1,
                at: joint.centroid,
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::NEG_Z * (along * capacity)
                    + Vec3::NEG_Y * (press * capacity / friction),
                press: Vec3::ZERO,
                peak: None,
                origin: Origin::Field,
            };
            let failures = structural(
                &structure,
                &mut state,
                &[push],
                &mut SectionCache::default(),
            );
            let fell = fall(&structure, &mut state);
            let case = format!("{along} x along, {press} back by the press");
            if opens {
                let causes: Vec<(Cause, Vec<usize>)> = failures
                    .iter()
                    .map(|failure| (failure.cause, failure.joints.clone()))
                    .collect();
                assert_eq!(causes, vec![(Cause::Shove, vec![rest])], "{case}");
                assert_eq!(fell, vec![vec![1]], "{case}");
            } else {
                assert!(failures.is_empty() && fell.is_empty(), "{case}");
            }
        }
    }
    /// A LOCKED SOURCE NEVER CONTRIBUTES PLASTIC DEMAND (the game's environment destruction
    /// model). Hinge demand relies on a finite share even when a squared lever underflows to
    /// zero. ASSUMED fixture inverse masses span zero, subnormal and ordinary responses;
    /// DERIVED infinite-source-mass share is zero throughout.
    #[test]
    fn a_locked_sources_share_is_zero_even_when_hinge_response_underflows() {
        let locked = Origin::Body {
            body: Entity::PLACEHOLDER,
            mass: f32::INFINITY,
            speed: 0.0,
            step: STEP,
        };
        for inverse in [0.0, 1e-45, 1e-30, 1.0] {
            assert_eq!(locked.share(inverse), 0.0, "inverse mass {inverse}");
            assert_eq!(Origin::Field.share(inverse), 1.0);
        }
    }

    /// A BODY'S DEMAND IS ITS PLASTIC EXCHANGE WITH THE HELD HINGE MASS (the game's
    /// environment destruction model). The section and seated-part laws rely on that share;
    /// feeding them the whole blow, the group's free mass, or its total mass fails the sweep.
    /// The field and vanishing-source-mass verdicts agree on either side of failure, the body's
    /// blow delivered within a vanishing step so that its force beats any strength (a field
    /// keeps the energy limit alone; `strength_and_energy_are_both_required` pins the force limit). Fixture masses, heights and
    /// steps below are ASSUMED; the hinge inertia and demands are DERIVED.
    #[test]
    fn body_demand_uses_the_held_hinge_mass_and_fields_are_the_zero_mass_limit() {
        let mass = 28.8;
        let structure = cubes(&[[0, 0, 0], [0, 1, 0]], 0, &[0], mass);
        let section = [joint_between(&structure, 0, 1)];
        let joint = structure.joints[section[0]];
        let top = structure.chunks[1];
        let hinge = joint.centroid - Vec3::Z * (joint.area / PI).sqrt();
        let r = top.centroid - hinge;
        let own = (3.0 * top.volume / (4.0 * PI)).cbrt();
        let inertia = mass * (r.length_squared() - r.x * r.x + 0.4 * own * own);
        for height in [1e-4, 0.05, 0.2, 1.0] {
            let at = hinge + Vec3::Y * height;
            let field = Load {
                chunk: 1,
                at,
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::NEG_Z * 100.0,
                press: Vec3::ZERO,
                peak: None,
                origin: Origin::Field,
            };
            let failure =
                section_failure(&structure, &section, &[1], &[field], Reading::UNRESISTED).unwrap();
            // A locked source can touch the group without exchanging momentum. Its infinite
            // source mass must not turn a zero angular impulse into an undefined demand.
            let locked = Load {
                blow: Vec3::ZERO,
                origin: Origin::Body {
                    body: Entity::PLACEHOLDER,
                    mass: f32::INFINITY,
                    speed: 0.0,
                    step: STEP,
                },
                ..field
            };
            assert_eq!(
                section_failure(
                    &structure,
                    &section,
                    &[1],
                    &[field, locked],
                    Reading::UNRESISTED
                ),
                Some(failure.clone())
            );
            let lever = at.y - hinge.y;
            let hinge_mass = inertia / (lever * lever);
            for ratio in [0.001, 0.1, 1.0, 10.0, 1000.0] {
                let origin = Origin::Body {
                    body: Entity::PLACEHOLDER,
                    mass: ratio * hinge_mass,
                    speed: 3.0,
                    step: STEP,
                };
                let body = Load { origin, ..field };
                let reduced =
                    section_failure(&structure, &section, &[1], &[body], Reading::UNRESISTED)
                        .unwrap();
                let expected = failure.demand / (1.0 + ratio);
                assert!(
                    (reduced.demand - expected).abs() <= expected * 2e-5,
                    "height {height}, ratio {ratio}: {} vs {expected}",
                    reduced.demand
                );
                assert_eq!(reduced.capacity, failure.capacity);
            }
            for scale in [0.99, 1.01] {
                let field = Load {
                    blow: field.blow * (scale * failure.capacity / failure.demand),
                    ..field
                };
                let body = Load {
                    origin: Origin::Body {
                        body: Entity::PLACEHOLDER,
                        mass: 1e-12,
                        speed: 3.0,
                        step: 1e-9,
                    },
                    ..field
                };
                let mut left = WallState::new(&structure);
                let mut right = left.clone();
                let a = structural(
                    &structure,
                    &mut left,
                    &[field],
                    &mut SectionCache::default(),
                );
                let b = structural(
                    &structure,
                    &mut right,
                    &[body],
                    &mut SectionCache::default(),
                );
                assert_eq!(left, right, "height {height}, threshold scale {scale}");
                assert_eq!(a, b, "a field is the vanishing-source-mass exchange");
            }
        }
        let origin = Origin::Body {
            body: Entity::PLACEHOLDER,
            mass: 1e12,
            speed: 3.0,
            step: STEP,
        };
        assert!(
            origin.share(1.0) < 1e-11,
            "an infinite source mass is the opposite limit"
        );
    }
    /// A section's peak tensile stress opens the joint that reaches its own strength,
    /// not its still-compressed neighbours. Standing walls rely on this local onset: a
    /// section-strength reading is not work sufficient to separate the entire section.
    /// ASSUMED fixture: two coplanar support discs under one rigid chunk, with eccentric press.
    #[test]
    fn a_local_tensile_overload_does_not_open_the_compressed_section() {
        let mut structure = row(1);
        structure.chunks[0].centroid = Vec3::Y;
        structure.joints.clear();
        for x in [-1.0, 1.0] {
            structure.joints.push(Joint {
                a: 0,
                b: Side::Ground,
                area: 0.1,
                centroid: Vec3::X * x,
                normal: Vec3::Y,
                bond: Bond::Bonded(brick().seam),
            });
        }
        let load = Load {
            chunk: 0,
            at: Vec3::new(-3.0, 1.0, 0.0),
            press: Vec3::NEG_Y * 1.0e5,
            blow: Vec3::ZERO,
            peak: None,
            energy: 0.0,
            crushed: 0.0,
            origin: Origin::Field,
        };
        let failure =
            weight_failure(&structure, &[0, 1], &[0], &[load], Vec3::ZERO, &mut 0).unwrap();
        assert!(failure.demand > failure.capacity);
        assert_eq!(
            failure.joints,
            vec![1],
            "the opposite support remains in compression"
        );
    }

    /// GRAVITY IS ONE FORCE (the game's environment destruction model, study test 5).
    /// The authority's strength and rest verdicts must be identical when the group's weight is
    /// supplied instead as one press at its mass centroid with gravity off. Special-casing weight in
    /// either the direct force or bending moment, or ignoring presses on rests, breaks this.
    #[test]
    fn gravity_is_one_force_at_the_groups_mass_centroid() {
        let cells: Vec<_> = propped_arm(4).into_iter().take(5).collect();
        for fraction in [0.98, 1.02] {
            let structure = cubes(&cells, 1, &[], fraction * cantilever_limit(4));
            let group = [1, 2, 3, 4];
            let section = [joint_between(&structure, 0, 1)];
            let mass: f32 = group.iter().map(|&c| structure.chunks[c].mass).sum();
            let centroid = group
                .iter()
                .map(|&c| structure.chunks[c].centroid * structure.chunks[c].mass)
                .sum::<Vec3>()
                / mass;
            let presses = [Load {
                chunk: group[0],
                at: centroid,
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::ZERO,
                press: Vec3::NEG_Y * (GRAVITY * mass),
                peak: None,
                origin: Origin::Field,
            }];
            let weight = weight_failure(
                &structure,
                &section,
                &group,
                &[],
                Vec3::NEG_Y * GRAVITY,
                &mut 0,
            )
            .unwrap();
            let pressed =
                weight_failure(&structure, &section, &group, &presses, Vec3::ZERO, &mut 0).unwrap();
            assert_eq!(weight.joints, pressed.joints);
            assert_eq!(weight.capacity, pressed.capacity);
            assert!((weight.demand - pressed.demand).abs() < 1e-5 * weight.capacity);
            assert_eq!(weight.demand >= weight.capacity, fraction > 1.0);
            assert_eq!(pressed.demand >= pressed.capacity, fraction > 1.0);
        }
        let mut structure = cubes(&[[0, 0, 0], [0, 1, 0]], 0, &[0], 28.8);
        let rest = joint_between(&structure, 0, 1);
        structure.joints[rest].bond = Bond::Resting;
        // ASSUMED fixture shifts: inside the bearing disc, and well outside it.
        for shift in [0.0, 2.0 * CUBE] {
            structure.chunks[1].centroid.x = structure.joints[rest].centroid.x + shift;
            let top = structure.chunks[1];
            let press = Load {
                chunk: 1,
                at: top.centroid,
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::ZERO,
                press: Vec3::NEG_Y * (top.mass * GRAVITY),
                peak: None,
                origin: Origin::Field,
            };
            let state = WallState::new(&structure);
            let weight = unseat(&structure, &state, &[1], &[], Vec3::NEG_Y * GRAVITY, &mut 0);
            let pressed = unseat(&structure, &state, &[1], &[press], Vec3::ZERO, &mut 0);
            assert_eq!(weight, pressed);
            assert_eq!(weight.is_some(), shift > 0.0);
        }
    }

    /// A SUSTAINED FORCE OVERTURNS INDEPENDENTLY OF THE TICK (the game's environment destruction
    /// model, study test 6). The authority's resting-panel law reads N, never N s: a force reading
    /// built from F dt / dt must free the panel under 6 kN and hold under 4.5 kN at every rate.
    /// ASSUMED fixture: 4 x 2 x 0.25 m, density 1845 kg/m3, sufficient base friction; DERIVED
    /// mass 3690 kg, restoring moment 4524.9 N m, and force height 1 m above the bearing plane.
    #[test]
    fn sustained_overturning_is_independent_of_tick_rate() {
        let mut structure = cubes(&[[0, 0, 0], [0, 1, 0]], 1, &[], 3690.0);
        structure.chunks[1].centroid = Vec3::Y;
        structure.chunks[1].volume = 2.0;
        for chunk in &mut structure.chunks {
            chunk.material.friction = 1.0;
        }
        // ASSUMED fixture bearing discs: their outer edges coincide with the panel's footprint.
        let radius = 0.001;
        structure.joints = [-1.0, 1.0]
            .into_iter()
            .flat_map(|x| {
                [-1.0, 1.0].into_iter().map(move |z| Joint {
                    a: 0,
                    b: Side::Chunk(1),
                    area: PI * radius * radius,
                    centroid: Vec3::new(x * (2.0 - radius), 0.0, z * (0.125 - radius)),
                    normal: Vec3::Y,
                    bond: Bond::Resting,
                })
            })
            .collect();
        for hz in [64.0_f32, 128.0, 256.0] {
            let dt = 1.0 / hz;
            for (force, leaves) in [(4500.0, false), (6000.0, true)] {
                let load = Load {
                    chunk: 1,
                    at: Vec3::Y,
                    energy: 0.0,
                    crushed: 0.0,
                    blow: Vec3::ZERO,
                    press: Vec3::Z * ((force * dt) / dt),
                    peak: None,
                    origin: Origin::Field,
                };
                let mut state = WallState::new(&structure);
                let result = weigh(
                    &structure,
                    &mut state,
                    &[],
                    &[],
                    &[load],
                    &[],
                    &mut SectionCache::default(),
                );
                assert_eq!(!result.failures.is_empty(), leaves, "{force} N at {hz} Hz");
                assert_eq!(
                    !fall(&structure, &mut state).is_empty(),
                    leaves,
                    "support agrees"
                );
                if leaves {
                    assert_eq!(result.failures[0].cause, Cause::Tip);
                }
                println!("MEASURED panel {force} N at {hz} Hz: leaves {leaves}");
            }
        }
    }

    /// STATIC PRESSES SLIDE ONLY PAST FRICTION AND LIFT ONLY AGAINST COMPRESSION (the game's
    /// environment destruction model). A resting component's authority law must read the force
    /// resultant, including a stabilizing normal press. Ignoring that press or using impulse
    /// capacities for steady forces fails these analytical limits (DERIVED from fixture mass,
    /// gravity and paired friction; mass and load multipliers are ASSUMED fixture data).
    #[test]
    fn static_presses_respect_friction_and_bearing_compression() {
        let mut structure = cubes(&[[0, 0, 0], [0, 1, 0]], 0, &[0], 28.8);
        let rest = joint_between(&structure, 0, 1);
        structure.joints[rest].bond = Bond::Resting;
        let chunk = structure.chunks[1];
        let weight = chunk.mass * GRAVITY;
        let friction = crate::substances::pair_friction(
            structure.chunks[0].material.friction,
            chunk.material.friction,
        );
        let load = Load {
            chunk: 1,
            at: structure.joints[rest].centroid,
            energy: 0.0,
            crushed: 0.0,
            blow: Vec3::ZERO,
            press: Vec3::ZERO,
            peak: None,
            origin: Origin::Field,
        };
        for (force, expected) in [
            (Vec3::Z * (0.99 * friction * weight), None),
            (Vec3::Z * (1.01 * friction * weight), Some(Cause::Slide)),
            (
                Vec3::Z * (1.01 * friction * weight) - Vec3::Y * weight,
                None,
            ),
            (Vec3::Y * (0.99 * weight), None),
            (Vec3::Y * (1.01 * weight), Some(Cause::LiftForce)),
        ] {
            let mut state = WallState::new(&structure);
            let result = weigh(
                &structure,
                &mut state,
                &[],
                &[],
                &[Load {
                    press: force,
                    ..load
                }],
                &[],
                &mut SectionCache::default(),
            );
            assert_eq!(
                result.failures.first().map(|failure| failure.cause),
                expected,
                "{force}"
            );
        }
    }

    /// A VERTICAL-AXIS TWIST IS NOT A TIP (the game's environment destruction model). The
    /// bearing-hull check used by resting components reads only overturning moments; a pure
    /// twist has no pressure-centre displacement, even with no vertical resultant. ASSUMED
    /// fixture couples isolate each axis. Torsional friction remains outside this rest model.
    #[test]
    fn a_rest_does_not_report_pure_twist_as_overturning() {
        let mut structure = cubes(&[[0, 0, 0], [0, 1, 0]], 1, &[], 28.8);
        let rest = joint_between(&structure, 0, 1);
        structure.joints[rest].bond = Bond::Resting;
        let seat = structure.joints[rest].centroid;
        let load = Load {
            chunk: 1,
            at: seat + Vec3::X,
            energy: 0.0,
            crushed: 0.0,
            blow: Vec3::ZERO,
            press: Vec3::Z,
            peak: None,
            origin: Origin::Field,
        };
        for (offset, expected) in [(Vec3::X, None), (Vec3::Y, Some(Cause::Tip))] {
            let loads = [
                Load {
                    at: seat + offset,
                    ..load
                },
                Load {
                    at: seat - offset,
                    press: -load.press,
                    ..load
                },
            ];
            let failure = unseat(
                &structure,
                &WallState::new(&structure),
                &[1],
                &loads,
                Vec3::ZERO,
                &mut 0,
            );
            assert_eq!(failure.map(|failure| failure.cause), expected);
        }
        let sideways = unseat(
            &structure,
            &WallState::new(&structure),
            &[1],
            &[load],
            Vec3::ZERO,
            &mut 0,
        );
        assert_eq!(sideways.unwrap().cause, Cause::Slide);
    }

    /// A STRENGTH-HELD PULSE CANNOT HIDE AN IMPULSIVE TURN (the game's environment destruction
    /// model). The authority must choose the hinge from the same effective blows it uses for
    /// demand. ASSUMED fixture opposing blows cancel before weighting; DERIVED pulse factor
    /// zero leaves exactly the line's hinge and demand. Choosing the raw-blow axis fails here.
    #[test]
    fn a_strength_held_pulse_cannot_cancel_an_impulsive_hinge() {
        let structure = cubes(&[[0, 0, 0], [0, 1, 0]], 1, &[], 28.8);
        let section = [joint_between(&structure, 0, 1)];
        let line = Load {
            chunk: 1,
            at: structure.chunks[1].centroid + Vec3::Y * (CUBE / 2.0),
            energy: 0.0,
            crushed: 0.0,
            blow: Vec3::NEG_Z * 100.0,
            press: Vec3::ZERO,
            peak: None,
            origin: Origin::Field,
        };
        let expected = section_failure(
            &structure,
            &section,
            &[1],
            &[line],
            Reading {
                falling: 0.0,
                yields: false,
            },
        )
        .unwrap();
        for scale in [1.0, 2.0] {
            let held = Load {
                blow: -line.blow * scale,
                peak: Some(Vec3::Z),
                ..line
            };
            let loads = [line, held];
            let pulse = section_pulse(&structure, &section, &[1], &loads, &mut 0);
            assert_eq!(pulse.falling, 0.0, "fixture peak is below seam strength");
            assert_eq!(
                section_failure(&structure, &section, &[1], &loads, pulse),
                Some(expected.clone()),
                "a resisted pulse neither cancels nor reverses the line's hinge"
            );
        }
    }

    /// A PULSE WHOSE PEAK FORCE THE STRENGTH HOLDS FAILS NOTHING, WHATEVER ITS BLOW (the game's
    /// environment destruction model, the #237 study's test 6). Far bursts stop toppling walls
    /// and lifting roof boards by this alone: the section and rest laws read a blast's blow
    /// through `g(F_R / F_peak)`, zero once the strength holds the peak. A bonded section and a
    /// seated part each hold a blast blow that pays their energy limit up to a million times over
    /// while its peak force stays 1 % under their strength (DERIVED below from the Navier and lift
    /// laws), though the same blow read impulsively fails them; at twice their strength (`r = ½`,
    /// `g = ⅓`) they fail exactly where `√g` of the blow pays the limit. A mutant that drops the
    /// peak-force term (`g ≡ 1`), reads a strength other than the law's own, or reads `g` for `√g`
    /// fails here. Fixture masses and load points are ASSUMED.
    #[test]
    fn a_pulse_whose_peak_the_strength_holds_fails_nothing_whatever_its_blow() {
        let mass = 28.8;
        let third = 1.0_f32 / 3.0;
        // A cube bonded on a buried one, pushed sideways at its top face, `h` over the seam: the
        // seam's Navier strength as a force there, `F_R = (f_t + m g / A) π r³ / 4h` (DERIVED: one
        // disc joint, its weight's compression against bending's rim peak `4 F h / π r³`).
        let structure = cubes(&[[0, 0, 0], [0, 1, 0]], 1, &[], mass);
        let section = [joint_between(&structure, 0, 1)];
        let joint = structure.joints[section[0]];
        let at = structure.chunks[1].centroid + Vec3::Y * (CUBE / 2.0);
        let h = at.y - joint.centroid.y;
        let r = (joint.area / PI).sqrt();
        let strength =
            (brick().seam.tensile + mass * GRAVITY / joint.area) * PI * r.powi(3) / (4.0 * h);
        let impulsive = Load {
            chunk: 1,
            at,
            energy: 0.0,
            crushed: 0.0,
            blow: Vec3::NEG_Z,
            press: Vec3::ZERO,
            peak: None,
            origin: Origin::Field,
        };
        let unit = section_failure(
            &structure,
            &section,
            &[1],
            &[impulsive],
            Reading::UNRESISTED,
        )
        .unwrap();
        let paying = unit.capacity / unit.demand;
        let blast = |blow: f32, peak: f32| Load {
            blow: Vec3::NEG_Z * blow,
            peak: Some(Vec3::NEG_Z * peak),
            ..impulsive
        };
        let fails = |load: Load| {
            !structural(
                &structure,
                &mut WallState::new(&structure),
                &[load],
                &mut SectionCache::default(),
            )
            .is_empty()
        };
        for times in [1.01, 1e3, 1e6] {
            let blow = times * paying;
            assert!(fails(Load {
                blow: Vec3::NEG_Z * blow,
                ..impulsive
            }));
            assert!(!fails(blast(blow, 0.99 * strength)), "{times} x the energy");
        }
        assert!(!fails(blast(0.99 * paying / third.sqrt(), 2.0 * strength)));
        assert!(fails(blast(1.01 * paying / third.sqrt(), 2.0 * strength)));

        // The same cube seated on a rest and lifted at its centre: its weight is the lift's
        // strength, `F_R = m g`, and the lift's limit `J ≥ m √(2 g h)`, `h` its centre's height
        // over the rest (DERIVED, the rest law).
        let mut seated = structure.clone();
        seated.joints[section[0]].bond = Bond::Resting;
        let centre = seated.chunks[1].centroid;
        let paying = mass * (2.0 * GRAVITY * (centre.y - joint.centroid.y)).sqrt();
        let weight = mass * GRAVITY;
        let lift = |blow: f32, peak: Option<f32>| Load {
            chunk: 1,
            at: centre,
            energy: 0.0,
            crushed: 0.0,
            blow: Vec3::Y * blow,
            press: Vec3::ZERO,
            peak: peak.map(|peak| Vec3::Y * peak),
            origin: Origin::Field,
        };
        let unseats = |load: Load| {
            let state = WallState::new(&seated);
            unseat(
                &seated,
                &state,
                &[1],
                &[load],
                Vec3::NEG_Y * GRAVITY,
                &mut 0,
            )
            .map(|failure| failure.cause)
        };
        for times in [1.01, 1e3, 1e6] {
            let blow = times * paying;
            assert_eq!(unseats(lift(blow, None)), Some(Cause::Lift));
            assert_eq!(
                unseats(lift(blow, Some(0.99 * weight))),
                None,
                "{times} x the energy"
            );
        }
        let blow = paying / third.sqrt();
        assert_eq!(unseats(lift(0.99 * blow, Some(2.0 * weight))), None);
        assert_eq!(
            unseats(lift(1.01 * blow, Some(2.0 * weight))),
            Some(Cause::Lift)
        );
    }

    /// The shipped brick's shear cohesion (Pa) and mode-II energy (J/m²).
    fn brick_shear() -> (f32, f32) {
        match brick().seam.shear {
            Shear::Cohesive { cohesion, energy } => (cohesion, energy),
            Shear::OpeningOnly => panic!("the shipped brick states its shear pair"),
        }
    }

    /// STRENGTH AND ENERGY ARE BOTH REQUIRED (the game's environment destruction model, the
    /// collision study's test 7). A fixture plug cube held only by its four side seams to buried
    /// neighbours (so no planar section holds it, and only its punch ring can free it) takes a
    /// body's blow square on. Its ring slides on all four seams: strength `4 A c` and work
    /// `4 A G_II` (DERIVED from the shipped brick's sourced row). A light body's blow whose share
    /// pays twice the energy but whose least mean force is short of the strength frees nothing; a
    /// heavy body's slow blow whose force beats the strength many times but whose share pays half
    /// the energy frees nothing; with both met the plug is punched out through exactly its four
    /// seams. The wall laws rely on both limits for every body contact: a tank's touch that
    /// shatters a wall it barely loads, or a brick's bounce that breaks the wall it lands on,
    /// fails here, so a law reading either limit alone fails one case. Masses, speeds and steps are
    /// fixture data.
    #[test]
    fn strength_and_energy_are_both_required() {
        let cells = [[-1, 0, 0], [1, 0, 0], [0, -1, 0], [0, 1, 0], [0, 0, 0]];
        let mass = 28.8;
        let structure = cubes(&cells, 4, &[], mass);
        let plug = 4;
        let ring: Vec<usize> = (0..4).map(|k| joint_between(&structure, k, plug)).collect();
        let (cohesion, energy) = brick_shear();
        let area = CUBE * CUBE;
        let strength = 4.0 * area * cohesion;
        let capacity = (2.0 * mass * 4.0 * area * energy).sqrt();
        let blow = |source: f32, demand: f32, step: f32| {
            let share = mass / (source + mass);
            Load {
                chunk: plug,
                at: structure.chunks[plug].centroid + Vec3::Z * (CUBE / 2.0),
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::NEG_Z * (demand * capacity / share),
                press: Vec3::ZERO,
                peak: None,
                origin: Origin::Body {
                    body: Entity::PLACEHOLDER,
                    mass: source,
                    speed: 1.0,
                    step,
                },
            }
        };
        let force = |load: &Load| load.mean_force().expect("a body's blow").length();
        let run = |load: Load| {
            let mut state = WallState::new(&structure);
            let failures = structural(
                &structure,
                &mut state,
                &[load],
                &mut SectionCache::default(),
            );
            (failures, fall(&structure, &mut state))
        };
        let light = blow(100.0, 2.0, STEP);
        assert!(
            force(&light) < 0.5 * strength,
            "fixture: the light blow's force is short"
        );
        assert_eq!(
            run(light),
            (Vec::new(), Vec::new()),
            "energy alone frees nothing"
        );
        let heavy = blow(50e3, 0.5, STEP);
        assert!(
            force(&heavy) > 10.0 * strength,
            "fixture: the heavy blow's force beats it"
        );
        assert_eq!(
            run(heavy),
            (Vec::new(), Vec::new()),
            "force alone frees nothing"
        );
        let both = blow(100.0, 2.0, light.blow.length() / (2.0 * strength));
        let (failures, fell) = run(both);
        assert_eq!(fell, vec![vec![plug]], "both met: the plug is punched out");
        let mut opened = ring;
        opened.sort_unstable();
        assert_eq!(failures.len(), 1);
        assert_eq!(
            (failures[0].cause, &failures[0].joints),
            (Cause::Punch, &opened)
        );
        assert!((failures[0].demand / failures[0].capacity - 2.0).abs() < 1e-3);
    }

    /// A NONORTHOGONAL RING KEEPS ITS ADMISSIBLE TRANSLATION (the game's environment destruction
    /// model). The side normals of a triangular prism surround its cross-section: a plug can
    /// leave only along its extrusion. The punch laws must find that direction under an oblique
    /// load, in either sense and under every boundary ordering; a fixed number of successive
    /// plane projections can retain an inadmissible residue and silently reject the ring.
    /// ASSUMED fixture geometry; direction and shear work are DERIVED from that geometry.
    #[test]
    fn an_oblique_push_finds_a_nonorthogonal_rings_translation() {
        let material = brick();
        let normals = [
            Vec3::X,
            Vec3::new(-0.5, 3.0_f32.sqrt() / 2.0, 0.0),
            Vec3::new(-0.5, -3.0_f32.sqrt() / 2.0, 0.0),
        ];
        let chunk = |centroid| Chunk {
            volume: 0.01,
            centroid,
            mass: 28.8,
            material,
            buried: false,
        };
        let mut chunks = vec![chunk(Vec3::ZERO)];
        chunks.extend(normals.map(|n| chunk(n * 0.25)));
        let joints: Vec<Joint> = normals
            .iter()
            .enumerate()
            .map(|(i, &n)| Joint {
                a: 0,
                b: Side::Chunk(i + 1),
                area: 0.05,
                centroid: n * 0.125,
                normal: n,
                bond: Bond::Bonded(material.seam),
            })
            .collect();
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let structure = Structure {
                chunks: chunks.clone(),
                joints: order.map(|j| joints[j]).to_vec(),
            };
            let state = WallState::new(&structure);
            for sense in [-1.0, 1.0] {
                let ring = ring(
                    &structure,
                    &state,
                    &incident(&structure),
                    &[0],
                    Vec3::new(1.0, 1.0, sense),
                )
                .expect("the extrusion remains unobstructed");
                assert!((ring.along - Vec3::Z * sense).length() < TRANSLATION_EPS);
                let (cohesion, energy) = brick_shear();
                assert!((ring.strength / (3.0 * 0.05 * cohesion) - 1.0).abs() < 1e-5);
                assert!((ring.work / (3.0 * 0.05 * energy) - 1.0).abs() < 1e-5);
            }
        }
    }

    /// A SOLVER STEP BOUNDS A BODY CONTACT'S PEAK FROM BELOW (the game's environment destruction
    /// model). With impulse and source inertia held fixed, shortening the step can enable the
    /// strength gate, never disable it. The authority's conservative two-limit reading relies
    /// on this; it is not a claim of timestep-independent contact physics. ASSUMED fixture
    /// geometry and rates; impulse chosen from the ring's strength to straddle its force gate.
    #[test]
    fn a_body_contact_step_bounds_its_peak_from_below() {
        let cells = [[-1, 0, 0], [1, 0, 0], [0, -1, 0], [0, 1, 0], [0, 0, 0]];
        let structure = cubes(&cells, 4, &[], 28.8);
        let state = WallState::new(&structure);
        let incident = incident(&structure);
        let ring = ring(&structure, &state, &incident, &[4], Vec3::NEG_Z).unwrap();
        let impulse = ring.strength / 96.0;
        let mut has_failed = false;
        let mut outcomes = Vec::new();
        for hz in [64.0_f32, 128.0, 256.0] {
            let load = Load {
                chunk: 4,
                at: structure.chunks[4].centroid,
                energy: 0.0,
                crushed: 0.0,
                blow: Vec3::NEG_Z * impulse,
                press: Vec3::ZERO,
                peak: None,
                origin: Origin::Body {
                    body: Entity::PLACEHOLDER,
                    mass: 1.0,
                    speed: impulse,
                    step: hz.recip(),
                },
            };
            let failure = ring_failure(&structure, &state, &incident, &[4], &[load]);
            let failed = failure.is_some_and(|f| f.demand >= f.capacity);
            assert!(
                !has_failed || failed,
                "a shorter step cannot lower the force bound"
            );
            has_failed |= failed;
            outcomes.push(failed);
            println!(
                "MEASURED body contact {hz} Hz: force bound {} N, strength {} N, fails {failed}",
                impulse * hz,
                ring.strength
            );
        }
        assert!(
            outcomes.contains(&false) && outcomes.contains(&true),
            "fixture straddles the numerical force gate"
        );
    }

    /// THE BREACH FOLLOWS STRENGTH (the game's environment destruction model, the collision study's
    /// test 9). A fixture 7 × 7 wall of cubes whose border the soil holds (buried, so no section
    /// frees anything and only punch rings can) takes a press square on, shared over a 2 × 2 patch.
    /// The patch's own ring (8 sliding seams, `8 A c`, DERIVED) has the highest force-to-strength
    /// ratio of the candidates (the next ring out has 12 seams' perimeter), so 1 % under `8 A c`
    /// frees nothing and 1 % over it frees exactly the patch; doubling every seam's cohesion
    /// doubles the press needed. The wall laws rely on the breach taking the contact's shape: a
    /// ring cap that decides the size (the grown ring, or the one past it, which reaches the
    /// buried border and so frees nothing) fails here, as does a ring read on any cohesion but its
    /// seams'. Fixture geometry; the press is fixture data.
    #[test]
    fn the_breach_follows_strength() {
        let mut cells = Vec::new();
        for x in 0..7 {
            for y in 0..7 {
                if x == 0 || y == 0 || x == 6 || y == 6 {
                    cells.push([x, y, 0]);
                }
            }
        }
        let border = cells.len();
        for x in 1..6 {
            for y in 1..6 {
                cells.push([x, y, 0]);
            }
        }
        let patch: Vec<usize> = [[2, 2, 0], [2, 3, 0], [3, 2, 0], [3, 3, 0]]
            .iter()
            .map(|cell| cells.iter().position(|c| c == cell).unwrap())
            .collect();
        let (cohesion, _) = brick_shear();
        let threshold = 8.0 * CUBE * CUBE * cohesion;
        for doubled in [false, true] {
            let mut structure = cubes(&cells, border, &[], 28.8);
            if doubled {
                for joint in &mut structure.joints {
                    if let Bond::Bonded(seam) = &mut joint.bond {
                        seam.shear = Shear::Cohesive {
                            cohesion: 2.0 * cohesion,
                            energy: brick_shear().1,
                        };
                    }
                }
            }
            let needed = threshold * if doubled { 2.0 } else { 1.0 };
            for (scale, frees) in [(0.99, false), (1.01, true)] {
                let loads: Vec<Load> = patch
                    .iter()
                    .map(|&chunk| Load {
                        chunk,
                        at: structure.chunks[chunk].centroid + Vec3::Z * (CUBE / 2.0),
                        energy: 0.0,
                        crushed: 0.0,
                        blow: Vec3::ZERO,
                        press: Vec3::NEG_Z * (scale * needed / 4.0),
                        peak: None,
                        origin: Origin::Field,
                    })
                    .collect();
                let mut state = WallState::new(&structure);
                let weighed = weigh(
                    &structure,
                    &mut state,
                    &[],
                    &[],
                    &loads,
                    &[],
                    &mut SectionCache::default(),
                );
                let fell = fall(&structure, &mut state);
                let mut freed = patch.clone();
                freed.sort_unstable();
                if frees {
                    assert_eq!(
                        fell,
                        vec![freed],
                        "doubled {doubled}: the patch is punched out"
                    );
                    assert_eq!(cracks(&weighed).len(), 1);
                    assert_eq!(weighed.failures[0].cause, Cause::PunchForce);
                    assert_eq!(weighed.failures[0].joints.len(), 8);
                } else {
                    assert!(fell.is_empty(), "doubled {doubled}, {scale}: {fell:?}");
                    assert!(weighed.failures.is_empty(), "doubled {doubled}, {scale}");
                }
            }
        }
    }

    /// A SEAM WHOSE SHEAR THE DATA DOES NOT STATE FORMS NO RING (the game's environment
    /// destruction model). A nailed fixing's attachment states its opening alone
    /// ([`Shear::OpeningOnly`]); the plug of the strength test, its four seams made such a
    /// fixing, takes a blow and a press that punch out the brick-seamed plug many times over and
    /// frees nothing: that seam answers by its opening law alone. The ring law relies on it to
    /// read no shear the data never stated; a ring that reads a nailed seam with a stand-in
    /// cohesion, or with none (zero strength), fails here. Fixture geometry and loads.
    #[test]
    fn a_seam_with_no_stated_shear_forms_no_ring() {
        let cells = [[-1, 0, 0], [1, 0, 0], [0, -1, 0], [0, 1, 0], [0, 0, 0]];
        let mut structure = cubes(&cells, 4, &[], 28.8);
        for joint in &mut structure.joints {
            if let Bond::Bonded(seam) = &mut joint.bond {
                seam.shear = Shear::OpeningOnly;
            }
        }
        let (cohesion, _) = brick_shear();
        let at = structure.chunks[4].centroid + Vec3::Z * (CUBE / 2.0);
        let load = Load {
            chunk: 4,
            at,
            energy: 0.0,
            crushed: 0.0,
            blow: Vec3::NEG_Z * 1e4,
            press: Vec3::NEG_Z * (100.0 * CUBE * CUBE * cohesion),
            peak: None,
            origin: Origin::Field,
        };
        let mut state = WallState::new(&structure);
        assert!(
            structural(
                &structure,
                &mut state,
                &[load],
                &mut SectionCache::default()
            )
            .is_empty()
        );
        let weighed = weigh(
            &structure,
            &mut state,
            &[],
            &[],
            &[load],
            &[],
            &mut SectionCache::default(),
        );
        assert!(weighed.failures.is_empty());
        assert!(fall(&structure, &mut state).is_empty());
    }

    /// The game's environment scab reads the same dynamic strength the material load gate
    /// validated, with one final f32 rounding. Asset authors rely on this agreement; rounding
    /// the static strength first breaks it on this ASSUMED fractional fixture datum.
    #[test]
    fn dynamic_tension_uses_the_validated_material_product() {
        let text = include_str!("../../assets/materials/materials.ron");
        let fixture = text.replace("tensile: Some((0.69e6,", "tensile: Some((690000.04,");
        assert_ne!(fixture, text, "the fixture changes the static strength");
        let registry = SubstanceRegistry::from_ron(&fixture).unwrap();
        for name in registry.keys() {
            let Some(bulk) = registry
                .get(name)
                .unwrap()
                .hard()
                .and_then(|hard| hard.bulk)
            else {
                continue; // Non-bulk matter supplies no environment fracture law.
            };
            let Some(facts) = bulk.blast else {
                continue; // Unbreakable bulk matter supplies no scab law.
            };
            let material = Masonry::of(&registry, name).unwrap().unwrap();
            assert_eq!(
                material.spall.tensile,
                (bulk.tensile.unwrap() * facts.tensile_dif) as f32,
                "{name}: the load gate and simulation must use the same product"
            );
        }
    }

    /// One fixture patch at the centre of a 0.25 m slab's lower face, at `material`'s dynamic
    /// compressive strength over 0.1 m², delivering `blow` N·s: a back stress far over every
    /// shipped row's dynamic tensile strength (ASSUMED fixture loading). With the slab.
    fn scab_face(material: &Masonry, blow: f32) -> (RayHit, Block) {
        let pressure = material.compaction.strength;
        let hit = RayHit {
            chunk: 0,
            at: Vec3::ZERO,
            normal: Vec3::NEG_Z,
            energy: 0.0,
            impulse: Vec3::Z * blow,
            peak: Vec3::Z * (0.1 * pressure),
            pressure,
            crushed: 0.0,
            depth: 0.0,
        };
        let slab = Block::cuboid(Vec3::new(-1.0, -1.0, 0.0), Vec3::new(1.0, 1.0, 0.25));
        (hit, slab)
    }

    /// The blow at which `material`'s [`scab_face`] pulse is `lengths` ligaments long:
    /// `λ = c τ`, `τ = 2J / F`.
    fn scab_blow(material: &Masonry, lengths: f32) -> f32 {
        let force = 0.1 * material.compaction.strength;
        lengths * 0.25 * force / (2.0 * material.spall.bar_speed)
    }

    /// The game's environment Law 2 gate reads intact material behind Law 1's crater. A front
    /// that reaches the back emits no scab, and a partial crater can leave too short a ligament
    /// to pass the pulse gate. The wall's retained push and volume accounting rely on it; a
    /// mutant reading the original full thickness fails. ASSUMED slab and pulse fixture.
    #[test]
    fn a_scab_needs_an_intact_ligament_behind_the_crater() {
        let material = brick();
        let (hit, slab) = scab_face(&material, scab_blow(&material, 0.1));
        let thrown = |depth| {
            Load::blast(&[RayHit { depth, ..hit }], |_| Backing {
                block: &slab,
                scale: 1.0,
                material,
                contacts: &[],
            })
        };
        let (_, scabs) = thrown(0.0);
        let [scab] = scabs[..] else {
            panic!("a free intact fixture scabs: {scabs:?}")
        };
        let stress = hit.peak.length() / (2.0 * PI * 0.25_f32.powi(2));
        let area = hit.peak.length() / stress;
        let scab_thickness = scab.mass / (material.density * area);
        let partial = 0.25 - 0.5 * scab_thickness;
        assert!(
            partial > 0.0 && partial < 0.25,
            "the fixture leaves a partial ligament"
        );
        for depth in [partial, 0.25, 0.5] {
            let (loads, scabs) = thrown(depth);
            assert!(
                scabs.is_empty(),
                "crater depth {depth} leaves no scab: {scabs:?}"
            );
            assert_eq!(loads.len(), 1, "no phantom rear volume removal");
            assert_eq!(loads[0].blow, hit.impulse, "no phantom momentum export");
        }
    }

    /// A game's environment crater depth is intensive and holds Law 1's volume in its expanding
    /// cone. The ligament gate relies on a depth independent of ray count or intercepted share;
    /// a planar depth or an extensive depth fails. ASSUMED charge, distances and obliquities.
    #[test]
    fn a_craters_cone_depth_is_independent_of_its_patch_share() {
        let blast = Blast::of(5.0 * TNT_ENERGY, 0.05, FIT_ONLY_PRODUCTS);
        for cos in [1.0_f32, 0.5] {
            let dir = Vec3::new((1.0 - cos * cos).sqrt(), 0.0, cos);
            let mut previous = None;
            for share in [1e-3_f32, 1e-5, 1e-7] {
                let crushed = blast.crushed(&brick(), share, 0.1, dir, Vec3::NEG_Z);
                assert!(crushed.volume > 0.0 && crushed.depth > 0.0);
                if let Some(depth) = previous {
                    assert_eq!(crushed.depth, depth);
                }
                previous = Some(crushed.depth);
                let r = 0.1_f64;
                let end = r + f64::from(crushed.depth / cos);
                let volume =
                    4.0 * std::f64::consts::PI * f64::from(share) * (end.powi(3) - r.powi(3)) / 3.0;
                assert!((volume / f64::from(crushed.volume) - 1.0).abs() < 1e-5);
            }
        }
    }

    /// A back scab in the game's environment model leaves the actual hull surface. On tapered
    /// geometry the farthest corner's projection lies beyond that surface. Rear contacts and the
    /// ligament gate rely on the true exit; a corner-projection mutant fails this ASSUMED wedge
    /// fixture at both instance scales.
    #[test]
    fn a_scab_leaves_the_hulls_actual_back_face() {
        let material = brick();
        let (hit, _) = scab_face(&material, scab_blow(&material, 0.1));
        let wedge = Block::hull(vec![
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(-1.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(-1.0, -1.0, 0.25),
            Vec3::new(1.0, -1.0, 0.5),
            Vec3::new(-1.0, 1.0, 0.25),
            Vec3::new(1.0, 1.0, 0.5),
        ]);
        for scale in [1.0_f32, 2.0] {
            let hit = RayHit {
                impulse: hit.impulse * scale.powi(2),
                peak: hit.peak * scale.powi(2),
                ..hit
            };
            let backing = |contacts| Backing {
                block: &wedge,
                scale,
                material,
                contacts,
            };
            let (_, scabs) = Load::blast(&[hit], |_| backing(&[]));
            let [scab] = scabs[..] else {
                panic!("the fixture pulse scabs: {scabs:?}")
            };
            assert!((scab.at - Vec3::Z * (0.375 * scale)).length() < 1e-5);
            assert!(
                wedge
                    .planes
                    .iter()
                    .all(|(n, o)| n.dot(scab.at / scale) <= o + 1e-5)
            );
            let (normal, _) = wedge.planes.iter().find(|(n, _)| n.z > 0.0).unwrap();
            let contact = Joint {
                a: hit.chunk,
                b: Side::Ground,
                area: 4.0 * scale.powi(2) / normal.z,
                centroid: scab.at,
                normal: *normal,
                bond: Bond::Bonded(material.seam),
            };
            assert!(
                Load::blast(&[hit], |_| backing(std::slice::from_ref(&contact)))
                    .1
                    .is_empty(),
                "a contact on the actual rear plane covers it"
            );
        }
    }

    /// A game's environment face load at its face's edge has a rear exit wherever rounding puts
    /// it. A single ray stopping at an edge sets its face's centroid there, and that centroid,
    /// one rounding past the side plane, must still read the slab's thickness behind it; the
    /// server's per-deposit path relies on it, and a law that walks the hull's full span from the
    /// centroid (refusing a point outside any plane) panics here. ASSUMED slab and pulse fixture.
    #[test]
    fn an_edge_load_one_rounding_outside_its_face_exits_through_the_back() {
        let material = brick();
        let (hit, slab) = scab_face(&material, scab_blow(&material, 0.1));
        let edge = 1.0_f32;
        for x in [f32::from_bits(edge.to_bits() - 1), edge, edge.next_up()] {
            let at = Vec3::new(x, 0.0, 0.0);
            assert_eq!(slab.exit(at, Vec3::Z), 0.25, "{x}: the slab's thickness");
            let (_, scabs) = Load::blast(&[RayHit { at, ..hit }], |_| Backing {
                block: &slab,
                scale: 1.0,
                material,
                contacts: &[],
            });
            let [scab] = scabs[..] else {
                panic!("{x}: the fixture pulse scabs: {scabs:?}")
            };
            assert_eq!(
                scab.at,
                Vec3::new(x, 0.0, 0.25),
                "{x}: it leaves the back face"
            );
        }
    }

    /// Only exposed rear area reflects the game's environment blast as tensile stress. A full
    /// contact, bonded or resting, throws no scab, nor does a cracked seam whose chunks still
    /// touch; an opened rest, a departed neighbour or a lateral joint covers no rear area.
    /// Structure loads rely on retaining the blow at internal interfaces, a second burst on a
    /// cracked wall included. ASSUMED fixture geometry/loading; a mutant that treats each baked
    /// chunk boundary, or each cracked seam, as air fails here.
    #[test]
    fn a_scab_needs_exposed_rear_area() {
        let material = brick();
        let (hit, slab) = scab_face(&material, scab_blow(&material, 0.1));
        let contact = Joint {
            a: 0,
            b: Side::Chunk(1),
            area: 4.0,
            centroid: Vec3::Z * 0.25,
            normal: Vec3::Z,
            bond: Bond::Bonded(material.seam),
        };
        let thrown = |contacts: &[Joint]| {
            Load::blast(&[hit], |_| Backing {
                block: &slab,
                scale: 1.0,
                material,
                contacts,
            })
        };
        assert_eq!(thrown(&[]).1.len(), 1, "the fixture's free rear scabs");
        for bond in [Bond::Bonded(material.seam), Bond::Resting] {
            let (loads, scabs) = thrown(&[Joint { bond, ..contact }]);
            assert!(scabs.is_empty(), "a contact is no free surface: {bond:?}");
            assert_eq!(loads.len(), 1, "no rear volume is removed");
            assert_eq!(loads[0].blow, hit.impulse, "the face retains its blow");
        }
        let mut structure = row(2);
        structure.joints = vec![contact];
        let mut state = WallState::new(&structure);
        assert!(
            thrown(&state.contacts(&structure).collect::<Vec<_>>())
                .1
                .is_empty()
        );
        state.cracked[0] = true;
        assert!(
            thrown(&state.contacts(&structure).collect::<Vec<_>>())
                .1
                .is_empty(),
            "a cracked seam's chunks still touch"
        );
        structure.joints[0].bond = Bond::Resting;
        assert_eq!(
            thrown(&state.contacts(&structure).collect::<Vec<_>>())
                .1
                .len(),
            1,
            "an opened rest exposes the back"
        );
        state.cracked[0] = false;
        structure.joints[0].bond = contact.bond;
        for fate in [Fate::Dust, Fate::Fallen] {
            state.fate[1] = fate;
            assert_eq!(
                thrown(&state.contacts(&structure).collect::<Vec<_>>())
                    .1
                    .len(),
                1
            );
        }
        let lateral = Joint {
            normal: Vec3::X,
            centroid: Vec3::X,
            ..contact
        };
        assert_eq!(thrown(&[lateral]).1.len(), 1, "a side bond covers no back");
        let unrelated = Joint {
            a: 2,
            b: Side::Chunk(3),
            ..contact
        };
        assert_eq!(
            thrown(&[unrelated]).1.len(),
            1,
            "other chunks' bonds cover no back"
        );
    }

    /// A LONG PULSE THROWS NO SCAB (the game's environment destruction model, the #237 study's
    /// test 3, Law 2). At a fixed peak stress on a face, lengthening the pulse (a larger blow at
    /// the same peak force, `τ = 2J / F`) thins the reflected tension by `min(1, 2L / λ)` until
    /// it no longer beats the dynamic tensile strength: a short pulse scabs, a long one never
    /// does, and once too long every longer pulse throws none. Thick walls and far, long pulses
    /// that leave the back face whole rely on it; a mutant that ignores the pulse's length throws a
    /// scab at every length and fails here. ASSUMED fixture loading; each shipped breakable row.
    #[test]
    fn a_long_pulse_throws_no_scab() {
        for material in [brick(), wood()] {
            let thrown: Vec<bool> = [0.01_f32, 0.1, 1.0, 10.0, 100.0, 1.0e3, 1.0e4]
                .into_iter()
                .map(|lengths| {
                    let (hit, slab) = scab_face(&material, scab_blow(&material, lengths));
                    let (_, scabs) = Load::blast(&[hit], |_| Backing {
                        block: &slab,
                        scale: 1.0,
                        material,
                        contacts: &[],
                    });
                    !scabs.is_empty()
                })
                .collect();
            assert!(thrown[0], "{material:?}: a short pulse scabs");
            assert!(
                !thrown[thrown.len() - 1],
                "{material:?}: a long pulse throws none"
            );
            assert!(
                thrown.windows(2).all(|pair| pair[0] || !pair[1]),
                "{material:?}: once too long, a longer pulse throws none: {thrown:?}"
            );
        }
    }

    /// A SCAB'S MOMENTUM COMES OFF ITS FACE'S PUSH (the game's environment destruction model, Law
    /// 2). A near burst's rays on one brick face: the pushed chunk keeps the face's blow less the
    /// scab's momentum, so the two sum to the rays' delivered impulse, both along it; the scab's
    /// volume leaves the pool as a load at the back point with no blow. The structural check and a
    /// freed piece's launch read the kept blow; a mutant that throws a scab without taking it off
    /// the push, or takes it off twice, fails here. ASSUMED fixture slabs; the charge is the
    /// 88 mm HE fixture burst's energy and charge radius (DERIVED), its filler centre 0.178 m off
    /// the face.
    #[test]
    fn a_scab_takes_its_momentum_off_its_faces_push() {
        let blast = Blast::of(1.47e6, 0.0513, FIT_ONLY_PRODUCTS);
        let directions = ball_directions(4096);
        let material = brick();
        for thickness in [0.12_f32, 0.25, 0.5] {
            let slab = [Block::cuboid(
                Vec3::new(-1.0, -1.0, 0.0),
                Vec3::new(1.0, 1.0, thickness),
            )];
            let hits = blast_rays(
                &slab,
                |_| true,
                |_| material,
                Vec3::new(0.0, 0.0, -0.178),
                blast,
                &directions,
            );
            let (loads, scabs) = Load::blast(&hits, |_| Backing {
                block: &slab[0],
                scale: 1.0,
                material,
                contacts: &[],
            });
            let [scab] = scabs[..] else {
                panic!("the near burst scabs the {thickness} m slab's back: {scabs:?}");
            };
            let delivered: Vec3 = hits.iter().map(|hit| hit.impulse).sum();
            let kept: Vec3 = loads.iter().map(|load| load.blow).sum();
            assert!(
                (kept + scab.momentum() - delivered).length() <= 1e-4 * delivered.length(),
                "{thickness} m: kept {kept} + scab {} vs {delivered}",
                scab.momentum()
            );
            assert!(kept.dot(delivered) > 0.0 && scab.momentum().dot(delivered) > 0.0);
            let [_, removal] = loads[..] else {
                panic!("one face load and one removal: {loads:?}");
            };
            assert_eq!((removal.at, removal.blow), (scab.at, Vec3::ZERO));
            assert!((removal.crushed * material.density / scab.mass - 1.0).abs() < 1e-5);
            assert!(
                (scab.at.z - thickness).abs() < 1e-5,
                "it leaves the back face"
            );
            println!(
                "MEASURED brick {thickness} m slab, 88 mm HE fixture burst at 0.178 m: scab \
                 {:.3} kg at {:.2} m/s, {:.1} of the face's {:.1} N s",
                scab.mass,
                scab.velocity.length(),
                scab.momentum().length(),
                delivered.length()
            );
        }
    }

    /// THE SCAB IS THE MATERIAL ROW'S, NEVER ITS NAME'S (the game's environment destruction model,
    /// Law 2; ADR-0034). One face loading on two shipped breakable rows throws two different
    /// scabs, and on the shipped brick row read under another name throws the identical one.
    /// Every breakable body's scab relies on reading its row alone; a law branching on a name or
    /// on a body kind, or reading a constant in place of the row, fails here. ASSUMED fixture
    /// loading; the rows are the shipped data.
    #[test]
    fn a_scab_is_its_rows_and_not_its_names() {
        let text = include_str!("../../assets/materials/materials.ron");
        assert_eq!(text.matches("\"Brick\":").count(), 1);
        let renamed = SubstanceRegistry::from_ron(&text.replacen("\"Brick\":", "\"Clinker\":", 1))
            .expect("the shipped rows load under another name");
        let clinker = Masonry::of(&renamed, "Clinker")
            .unwrap()
            .expect("the renamed row states bulk facts");
        let scab = |material: Masonry, loaded: &Masonry| {
            let (hit, slab) = scab_face(loaded, scab_blow(loaded, 0.1));
            let (_, scabs) = Load::blast(&[hit], |_| Backing {
                block: &slab,
                scale: 1.0,
                material,
                contacts: &[],
            });
            let [scab] = scabs[..] else {
                panic!("the short fixture pulse scabs {material:?}: {scabs:?}");
            };
            scab
        };
        let (brick, wood) = (brick(), wood());
        assert_eq!(
            scab(clinker, &brick),
            scab(brick, &brick),
            "a name changes nothing"
        );
        let (on_brick, on_wood) = (scab(brick, &wood), scab(wood, &wood));
        assert!(
            on_brick.mass != on_wood.mass && on_brick.velocity != on_wood.velocity,
            "a row changes the scab: {on_brick:?} vs {on_wood:?}"
        );
    }
}
