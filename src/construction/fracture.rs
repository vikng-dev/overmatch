//! The game's environment destruction laws (game state only). [`wear`] consumes volume,
//! [`crack`] opens kinetic-cavity seams, [`bondsolve::Assessment`] solves every live joint's
//! equilibrium force and moment, and [`loose_groups`] releases matter disconnected from support.
//! The authority records their effects; replicas derive geometry from that record.
//!
//! [`Structure`] supplies chunk volume, mass, centroid and material, and the joints' area, centre,
//! normal and attachment kind. [`Load`] supplies crushed volume, cavity energy, impulse and press
//! at a chunk-local point in the construction frame. [`trace_blast`] is the field's nearest-stop
//! reduction; [`Blast`] and the material's compaction and scab laws supply its surface response.
//! The construction design's "Structural calculation" and "Standoff blast" own the derivations.

pub(crate) mod bondsolve;

use std::{f32::consts::PI, sync::Arc};

use bevy::ecs::entity::Entity;
use bevy::math::{DMat3, DVec3, Vec3};

use crate::physical_laws::PhysicalLaws;
use crate::substances::{BlastFacts, REFERENCE_FACTOR, SubstanceRegistry};

/// Spacing of the point sources a line deposit is split into along its chord, m: a quarter of a
/// 20 cm chunk (Resolution).
pub(crate) const LINE_STEP: f32 = 0.05;

/// One material's values, as the laws read them, all from its substance row
/// ([`Material::of`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Material {
    /// kg/m³.
    pub density: f32,
    /// Young's and shear moduli, Pa: authored material compliance, never a failure strength.
    pub young: f64,
    pub shear_modulus: f64,
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
    /// An authored interface slip slope, N/m³; intrinsic solid seams use bulk compliance.
    pub fixing: Option<Fixing>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fixing {
    pub normal: f32,
    pub shear: f32,
}

/// Cohesive shear strength (Pa) and mode-II separation work (J/m²), excluding friction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shear {
    pub cohesion: f32,
    pub energy: f32,
}

/// How a material crushes under a blast's pulse, about the game's environment destruction (game
/// state only): Law 1, a rigid–plastic–locking (snowplow) compaction front. A face patch taking
/// impulse `J` at peak pressure `P` crushes `Δm = J f(σ_c,d / P) / u_c` ([`crush_factor`]);
/// nothing where `P` does not beat `σ_c,d`, however long the pulse. The construction design ("Standoff
/// blast", Law 1) owns the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Compaction {
    /// The dynamic compressive strength `σ_c,d`, Pa: the row's quasi-static compressive strength
    /// times its stated dynamic increase factor at the row's reference rate. ASSUMED effective
    /// plateau: the law supplies no per-pulse strain rate; applicability is a validation limit
    /// recorded in the construction design's material-data section.
    pub strength: f32,
    /// The slowest compaction front's speed `u_c = √(σ_c,d ε_v / ρ)`, m/s, `ε_v = 1 − ρ / ρ_s` the
    /// volumetric strain to lock-up at the row's fully dense density `ρ_s` (DERIVED: the locking
    /// idealization compacts the pores shut and no further).
    pub front_speed: f32,
}

/// How a material throws a rear scab under a blast's pulse, about the game's environment
/// destruction (game state only): Law 2's material inputs ([`scab`]). The construction design ("Standoff
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

impl Material {
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
                        fixing: None,
                        tensile: tensile as f32,
                        fracture_energy: energy as f32,
                        shear: Shear {
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
            young: blast.youngs_modulus,
            shear_modulus: bulk.shear_modulus.map(f64::from).ok_or_else(|| {
                format!(
                    "substance {name:?}: bulk.shear_modulus missing; author G (Pa) with its source"
                )
            })?,
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
    pub material: Material,
    /// Held by the soil: its hull lies wholly below grade, the plane its construction stands on. The
    /// soil bears it as an anchor does and no crack frees it, so a group still joined to it stands
    /// and no body is ever released inside the ground.
    pub buried: bool,
}

impl Chunk {
    /// DERIVED inertia of the equal-volume sphere, 2 m r² / 5. This geometric assessment
    /// approximation is independent of the occupied hull tensor used by physics bodies.
    fn intrinsic_inertia(&self) -> f64 {
        let radius = (3.0 * f64::from(self.volume) / (4.0 * std::f64::consts::PI)).cbrt();
        2.0 * f64::from(self.mass) * radius * radius / 5.0
    }
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
    /// A uniform area share of one bonded footprint. StandIn: the footprint is an equivalent
    /// disk; each cell contributes its fraction of that disk's area and second moment, so cutting
    /// it more finely cannot change its hold. The seam is converted from the whole footprint.
    AreaShare {
        seam: Seam,
        footprint_area: f32,
        footprint_centroid: Vec3,
    },
    /// Compression and pair friction only. A contact mechanism separates it in lift, tip or slide.
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
    /// The game's environment destruction model reads a bond's circular patch: its centre and
    /// total area. An area share integrates over its whole footprint, weighted by `Self::area`.
    fn patch(&self) -> (Vec3, f32) {
        match self.bond {
            Bond::AreaShare {
                footprint_area,
                footprint_centroid,
                ..
            } => (footprint_centroid, footprint_area),
            _ => (self.centroid, self.area),
        }
    }

    /// Its seam, `None` for a rest.
    pub(crate) fn seam(&self) -> Option<Seam> {
        match self.bond {
            Bond::Bonded(seam) | Bond::AreaShare { seam, .. } => Some(seam),
            Bond::Resting => None,
        }
    }
}

/// A chunk table and its joint graph.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Structure {
    pub chunks: Vec<Chunk>,
    pub joints: Vec<Joint>,
    pub geometry: bondsolve::Geometry,
}

/// A convex chunk shape as the collision library reconstructs its hull ([`Block::hull`]): its
/// corners, the plane bounding each face (outward unit normal `n` and offset `o`, a point `x`
/// inside when `n · x ≤ o`), and its bounding box.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Block {
    pub points: Vec<Vec3>,
    pub planes: Vec<(Vec3, f32)>,
    /// Per plane, its face's corners as indices into `points`, counter-clockwise seen from outside.
    rings: Vec<Vec<usize>>,
    pub min: Vec3,
    pub max: Vec3,
}

/// Resolution: plane-membership tolerance, m: how far a baked joint's plane may pass a block's
/// corner, or a point of its face, and still hold it. It absorbs the bake's micrometre rounding and
/// f32 construction-frame coordinates (ulp 1.9 µm from 16 to 32 m, DERIVED) with margin. A law block
/// must be thicker across ([`Block::hull`]), or a plane on one face would hold the opposite face.
pub(crate) const ON_PLANE: f32 = 1e-4;

impl Block {
    /// The convex hull of `points` as the collider has it: the collision library's faces
    /// (`ConvexPolyhedron::from_convex_hull`, what `Collider::convex_hull` builds and the view
    /// draws), each bounded by its supporting plane, through the corner farthest along its normal.
    /// A face is the library's topology, never a comparison of plane offsets, so the verdict and
    /// the faces move with the corners. `None` unless the corners are finite and bound a closed
    /// solid ([`Self::is_solid`]).
    pub fn hull(points: Vec<Vec3>) -> Option<Self> {
        if !points.iter().all(|p| p.is_finite()) {
            return None;
        }
        let polyhedron = avian3d::parry::shape::ConvexPolyhedron::from_convex_hull(&points)?;
        let points = polyhedron.points().to_vec();
        let (planes, rings): (Vec<_>, Vec<_>) = polyhedron
            .faces()
            .iter()
            .map(|face| {
                let first = face.first_vertex_or_edge as usize;
                let ring: Vec<usize> = polyhedron.vertices_adj_to_face()
                    [first..first + face.num_vertices_or_edges as usize]
                    .iter()
                    .map(|&index| index as usize)
                    .collect();
                let offset = points
                    .iter()
                    .map(|p| face.normal.dot(*p))
                    .fold(f32::NEG_INFINITY, f32::max);
                ((face.normal, offset), ring)
            })
            .unzip();
        let min = points.iter().fold(Vec3::INFINITY, |a, b| a.min(*b));
        let max = points.iter().fold(Vec3::NEG_INFINITY, |a, b| a.max(*b));
        let block = Self {
            points,
            planes,
            rings,
            min,
            max,
        };
        block.is_solid().then_some(block)
    }

    /// Whether the faces bound a closed solid the law resolves: each face's normal is a unit
    /// vector, each edge has its opposite, the boundary encloses positive volume, and the block is
    /// thicker across than [`ON_PLANE`].
    fn is_solid(&self) -> bool {
        let mut edges = std::collections::BTreeMap::<(usize, usize), usize>::new();
        let centre = self.center().as_dvec3();
        let mut six_volume = 0.0;
        for ((normal, _), ring) in self.planes.iter().zip(&self.rings) {
            if !normal.is_normalized() || ring.len() < 3 {
                return false;
            }
            for k in 0..ring.len() {
                *edges
                    .entry((ring[k], ring[(k + 1) % ring.len()]))
                    .or_default() += 1;
            }
            let corner = |k: usize| self.points[ring[k]].as_dvec3() - centre;
            for k in 1..ring.len() - 1 {
                six_volume += corner(0).dot(corner(k).cross(corner(k + 1)));
            }
        }
        self.planes.len() >= 4
            && six_volume.is_finite()
            && six_volume > 0.0
            && edges
                .iter()
                .all(|(&(a, b), &count)| count == 1 && edges.get(&(b, a)) == Some(&1))
            && {
                let (_, lo, hi) = self.thinnest();
                hi - lo > ON_PLANE
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
        Self::hull(points).expect("a box thicker than the plane tolerance is a closed solid")
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

    /// The axis this shape is thinnest across and its extent along it, `(axis, lo, hi)`: the
    /// outward normal of the bounding plane over which its corners span least (the first such
    /// plane on a tie), and their least and greatest offsets along it. A wall's is its thickness.
    pub fn thinnest(&self) -> (Vec3, f32, f32) {
        self.planes
            .iter()
            .map(|&(n, _)| {
                let along = || self.points.iter().map(move |p| p.dot(n));
                (
                    n,
                    along().fold(f32::INFINITY, f32::min),
                    along().fold(f32::NEG_INFINITY, f32::max),
                )
            })
            .min_by(|a, b| (a.2 - a.1).total_cmp(&(b.2 - b.1)))
            .expect("a certified solid has bounding planes")
    }

    /// The distance from the centroid of the corners to the nearest bounding plane, m.
    /// The caller supplies a certified solid with at least four corners.
    pub fn inradius(&self) -> f32 {
        let centre = self.points.iter().sum::<Vec3>() / self.points.len() as f32;
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

    /// The convex perimeter of the corners within [`ON_PLANE`] of the plane through `at` of unit
    /// normal `normal` (a baked joint's), counter-clockwise about `normal`, excluding redundant
    /// points. Fewer than three corners span no face.
    pub fn face_on(&self, at: Vec3, normal: Vec3) -> Vec<Vec3> {
        let mut on: Vec<Vec3> = self
            .points
            .iter()
            .copied()
            .filter(|p| normal.dot(*p - at).abs() < ON_PLANE)
            .collect();
        if on.len() < 3 {
            return Vec::new();
        }
        let (u, v) = plane_axes(normal);
        convex_hull(&mut on, |p| (p.dot(u), p.dot(v)))
    }

    /// Every face, as its outward unit normal and its corners counter-clockwise seen from outside.
    pub fn faces(&self) -> Vec<(Vec3, Vec<Vec3>)> {
        self.planes
            .iter()
            .zip(&self.rings)
            .map(|(&(normal, _), ring)| (normal, ring.iter().map(|&k| self.points[k]).collect()))
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
/// joint's cohesion and contact separation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct State {
    pub pool: Vec<f32>,
    pub fate: Vec<Fate>,
    pub cracked: Vec<bool>,
    /// Contact separation is distinct from loss of cohesion: a closed crack still bears load.
    pub separated: Vec<bool>,
    topology: Arc<Topology>,
    members: Vec<usize>,
    joints: Vec<usize>,
}

/// Immutable source addresses shared by every owner of a construction. Live edges are filtered
/// at traversal, so cracks, wear and support loss cannot leave a cached edge behind.
#[derive(Clone, Debug, PartialEq)]
struct Topology {
    incident: Vec<Vec<usize>>,
    neighbours: Vec<Vec<(usize, usize)>>,
    anchors: Vec<Vec<usize>>,
}

impl Topology {
    fn new(structure: &Structure) -> Self {
        let mut topology = Self {
            incident: incident(structure),
            neighbours: vec![Vec::new(); structure.chunks.len()],
            anchors: vec![Vec::new(); structure.chunks.len()],
        };
        for (j, joint) in structure.joints.iter().enumerate() {
            match joint.b {
                Side::Chunk(b) => {
                    topology.neighbours[joint.a].push((b, j));
                    topology.neighbours[b].push((joint.a, j));
                }
                Side::Ground => topology.anchors[joint.a].push(j),
            }
        }
        topology
    }
}

impl State {
    /// Every chunk standing with a full pool, every joint intact.
    pub fn new(structure: &Structure) -> Self {
        Self {
            pool: structure.chunks.iter().map(full_pool).collect(),
            fate: vec![Fate::Standing; structure.chunks.len()],
            cracked: vec![false; structure.joints.len()],
            separated: vec![false; structure.joints.len()],
            topology: Arc::new(Topology::new(structure)),
            members: (0..structure.chunks.len()).collect(),
            joints: (0..structure.joints.len()).collect(),
        }
    }

    /// Ownership narrows traversal without renumbering the construction's ledger addresses.
    pub fn retain_group(&mut self, group: &[usize]) {
        assert!(
            !group.is_empty() && group.windows(2).all(|w| w[0] < w[1]),
            "an owned group is nonempty and strictly ordered"
        );
        assert!(
            group.iter().all(|c| self.members.binary_search(c).is_ok()),
            "a split can retain only chunks its parent owns"
        );
        self.members = group.to_vec();
        self.joints = group
            .iter()
            .flat_map(|&c| self.topology.incident[c].iter().copied())
            .collect();
        self.joints.sort_unstable();
        self.joints.dedup();
        for (c, fate) in self.fate.iter_mut().enumerate() {
            *fate = if group.binary_search(&c).is_ok() {
                Fate::Standing
            } else {
                Fate::Dust
            };
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
        self.joints.iter().copied().filter_map(|j| {
            let joint = &structure.joints[j];
            let touching = !self.separated[j];
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
pub(crate) fn wear(structure: &Structure, state: &mut State, chunk: usize, volume: f32) -> bool {
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

/// One source at one point in the game's environment destruction model (game state only).
/// Crushed volume consumes the pool, cavity energy enters [`crack`], and impulse and press enter
/// [`bondsolve::Assessment`]. Field loads never press; physics contacts supply their resolved
/// blow and press without cavity energy. The construction design owns their derivations.
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
    /// ([`Material::crush`]); a blast's, its rays' Law 1 volumes ([`Material::crushed_by`]), which
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
    /// stop (an impulsive blow) and a body's contact (its force is its press). The force solve caps its one-tick average force by this peak.
    pub peak: Option<Vec3>,
    pub origin: Origin,
}

/// Where a load comes from: what its blow shares momentum with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Origin {
    /// A field (a round's line, a fragment, a blast): its blow is its own law's, and it has no
    /// mass to share with what it frees.
    Field,
    /// A contact of a moving group with another physics body at this tick. Its actual solver
    /// impulse already changed both bodies' motion; it must never undergo another plastic
    /// exchange. Joint geometry supplies bearing directions; contacts supply applied loads.
    Contact { other: Entity, step: f32 },
    /// A body in contact, by its entity: it shares its blow's momentum with whatever the blow
    /// frees. `mass` is the body's effective mass along the contact normal at the combined
    /// blow's centroid, kg, shared by that contact's points ([`crate::contact_transfer`]).
    Body {
        body: Entity,
        mass: f32,
        /// Closing speed along the normal at the solver's start, m/s; shared launch caps read it.
        speed: f32,
        /// The numerical physics step the solver delivered its blow within, s, not a material
        /// contact duration. Retained contact provenance; the force solve uses its configured
        /// numerical tick for the average pulse force (the construction design owns that limit).
        step: f32,
    },
}

impl Origin {
    /// The plastic share against a receiver's inverse effective mass (kg⁻¹). DERIVED:
    /// `m_receiver / (m_source + m_receiver)`, written without an infinite hinge mass when
    /// a load acts on the hinge. A field is the zero-source-mass limit of this same exchange.
    pub(crate) fn share(self, inverse_mass: f32) -> f32 {
        let mass = match self {
            Self::Field | Self::Contact { .. } => 0.0,
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
        material: &Material,
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

    /// One load per (chunk, face) of `hits`, each ray keyed by the chunk it stopped on, at its
    /// rays' impulse-weighted centroid. The face normal is
    /// shared by its rays, so their pushes are parallel and this point preserves their angular
    /// impulse about every pivot. Opposing faces stay separate even if their blows cancel.
    /// Energy never gates the volume or the push: a pressure load needs neither chemical-energy
    /// wear nor a Lamé source to exist. Each face then meets Law 2 ([`scab`]) on the chunk
    /// `backing` names: a scab it throws takes its momentum off the face's blow, and its volume
    /// leaves the chunk's pool as one more load at the back point, with no blow. Returns the loads
    /// and the scabs.
    pub(crate) fn blast<'a>(
        hits: &[(usize, RayHit)],
        backing: impl Fn(usize) -> Backing<'a>,
    ) -> (Vec<Self>, Vec<Scab>) {
        let mut faces = std::collections::BTreeMap::new();
        for (chunk, hit) in hits {
            let weight = hit.impulse.length();
            if weight == 0.0 {
                // A cone whose intercepted share is zero delivers nothing.
                assert!(
                    hit.crushed == 0.0 && hit.peak == Vec3::ZERO,
                    "a nonempty blast patch carries a positive impulse"
                );
                continue;
            }
            let key = (*chunk, hit.normal.to_array().map(f32::to_bits));
            let (load, total, rays) = faces.entry(key).or_insert_with(|| {
                (
                    Self {
                        chunk: *chunk,
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

    /// The radius of the ball this load's energy alone crushes in `material`, m: where its stress
    /// is clamped, since nothing inside it is left to load.
    fn crushed(&self, material: &Material) -> f32 {
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

/// Seven points of `joint`'s circular patch in its plane, with weights summing to 1: the centre at
/// ¼ and six on the ring at `√(2/3)` of its radius at ⅛ each.
fn disc(joint: &Joint) -> [(Vec3, f32); 7] {
    let (u, v) = plane_axes(joint.normal);
    let (centre, area) = joint.patch();
    let ring = (area / PI).sqrt() * (2.0_f32 / 3.0).sqrt();
    let mut points = [(centre, 0.25); 7];
    for (k, point) in points.iter_mut().skip(1).enumerate() {
        let angle = k as f32 * PI / 3.0;
        *point = (centre + (u * angle.cos() + v * angle.sin()) * ring, 0.125);
    }
    points
}

/// Each source's crushed radius in `material`, in source order ([`Load::crushed`]).
fn crushed_radii(sources: &[Load], material: &Material) -> Vec<f32> {
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
pub(crate) fn crack(structure: &Structure, state: &mut State, loads: &[Load]) -> Vec<usize> {
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
    for &index in &state.joints {
        let joint = &structure.joints[index];
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

/// The groups of standing chunks with no path of cohesive or closed contact joints to a ground
/// anchor or a buried chunk, each a list of chunk indices, ascending, the groups ordered by their first chunk. Reads
/// the joint graph and the burial only: no float enters, so every peer that holds the same fates
/// and cracks finds the same groups.
pub(crate) fn loose_groups(structure: &Structure, state: &State) -> Vec<Vec<usize>> {
    let count = structure.chunks.len();
    let grounded = |c: usize| {
        structure.chunks[c].buried
            || state.topology.anchors[c]
                .iter()
                .any(|&j| !state.separated[j] && state.live(&structure.joints[j]))
    };
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
            stack.extend(
                state.topology.neighbours[chunk]
                    .iter()
                    .copied()
                    .filter(|&(n, j)| {
                        !seen[n] && !state.separated[j] && state.live(&structure.joints[j])
                    })
                    .map(|(n, _)| n),
            );
        }
        reached.sort_unstable();
        reached
    };
    flood(
        state
            .members
            .iter()
            .copied()
            .filter(|&i| grounded(i) && state.standing(i))
            .collect(),
        &mut seen,
    );
    let mut groups = Vec::new();
    for &chunk in &state.members {
        if !seen[chunk] && state.standing(chunk) {
            groups.push(flood(vec![chunk], &mut seen));
        }
    }
    groups
}

/// The first standing block a ray from `origin` along unit `dir` enters: its index, distance and
/// the outward normal of the face it enters through ([`Block::entry_face`]).
#[cfg(any(feature = "dev_ui", test))]
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
pub(crate) fn strike(structure: &Structure, state: &mut State, loads: &[Load]) -> Struck {
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
pub(crate) fn fall(structure: &Structure, state: &mut State) -> Vec<Vec<usize>> {
    let groups = loose_groups(structure, state);
    for &chunk in groups.iter().flatten() {
        state.fate[chunk] = Fate::Fallen;
    }
    groups
}

/// Which component of a joint's own resultant exceeded its capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cause {
    Tension,
    Shear,
    LiftForce,
    Tip,
    Slide,
}

/// A failed joint. Cohesive causes carry demand/capacity in Pa; contact mechanisms have no
/// local scalar threshold and carry zero in those fields. Their cause names the free motion.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Failure {
    pub joint: usize,
    pub cause: Cause,
    pub demand: f32,
    pub capacity: f32,
    /// Fracture work paid by this tick's impulses, J; a force-controlled driver pays its own.
    pub work: f32,
}

/// Deduct an owned share of fracture work from a departing group's field and plastic body
/// impulses. Sum their coherent force and moment first; splitting one load into points cannot
/// change the work. Contacts already integrated by physics carry no second launch impulse.
pub(crate) fn debit_work(
    structure: &Structure,
    group: &[usize],
    work: f32,
    loads: impl IntoIterator<Item = Load>,
) -> Vec<Load> {
    let mut loads: Vec<_> = loads.into_iter().collect();
    if work == 0.0 {
        return loads;
    } // A maintained driver supplied the opening work.
    let mass: f64 = group
        .iter()
        .map(|&c| f64::from(structure.chunks[c].mass))
        .sum();
    let centre = group
        .iter()
        .map(|&c| structure.chunks[c].centroid.as_dvec3() * f64::from(structure.chunks[c].mass))
        .sum::<DVec3>()
        / mass;
    let inertia = group.iter().fold(DMat3::ZERO, |sum, &c| {
        let chunk = &structure.chunks[c];
        let r = chunk.centroid.as_dvec3() - centre;
        sum + DMat3::from_diagonal(DVec3::splat(
            chunk.intrinsic_inertia() + f64::from(chunk.mass) * r.length_squared(),
        )) - DMat3::from_cols(r * r.x, r * r.y, r * r.z) * f64::from(chunk.mass)
    });
    let inverse = inertia.inverse();
    assert!(
        inverse.is_finite(),
        "a departing group has positive occupied assessment inertia"
    );
    let mut impulse = DVec3::ZERO;
    let mut angular = DVec3::ZERO;
    for load in &loads {
        if matches!(load.origin, Origin::Contact { .. }) || load.blow == Vec3::ZERO {
            continue;
        }
        let arm = load.at.as_dvec3() - centre;
        let direction = load.blow.as_dvec3().normalize();
        let lever = arm.cross(direction);
        let response = 1.0 / mass + lever.dot(inverse * lever);
        let delivered = load.blow.as_dvec3() * f64::from(load.origin.share(response as f32));
        impulse += delivered;
        angular += arm.cross(delivered);
    }
    let available = (impulse.length_squared() / mass + angular.dot(inverse * angular)) / 2.0;
    let kept = if available > f64::from(work) {
        (1.0 - f64::from(work) / available).sqrt() as f32
    } else {
        0.0
    };
    for load in &mut loads {
        if !matches!(load.origin, Origin::Contact { .. }) {
            load.blow *= kept;
        }
    }
    loads
}

fn convex_hull<T: Copy>(points: &mut [T], project: impl Fn(T) -> (f32, f32)) -> Vec<T> {
    points.sort_by(|&a, &b| {
        let (a, b) = (project(a), project(b));
        a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
    });
    // The lower hull left to right, then the upper right to left.
    let mut hull: Vec<T> = Vec::with_capacity(points.len() + 1);
    let push = |hull: &mut Vec<T>, floor: usize, q: T| {
        while hull.len() >= floor
            && cross(
                project(hull[hull.len() - 2]),
                project(hull[hull.len() - 1]),
                project(q),
            ) <= 0.0
        {
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
/// Source adjacency shared by owners; cracks are filtered when reading it.
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

/// The least cosine of incidence a ray's patch is reckoned at (ASSUMED, Resolution): past 84° one
/// ray's patch, `4π share d² / cos θ`, grows without bound on the face it stops on, though the
/// face itself bounds the cones that land on it ([`intercepted`]).
const GRAZING_COS: f32 = 0.1;

/// A blast: its energy (J) and its charge `W` (kg of TNT), the TNT that carries that energy
/// (ASSUMED: impulse equivalence is energy equivalence; the construction design notes they need not agree).
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
            charge: energy / PhysicalLaws::shipped().tnt_energy_j_per_kg,
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
        PhysicalLaws::shipped().reflected_impulse.edge()
            * (self.charge / PhysicalLaws::shipped().surface_burst_factor).cbrt()
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
        let scale = (self.charge / PhysicalLaws::shipped().surface_burst_factor).cbrt();
        if scale <= 0.0 {
            return 0.0;
        }
        let distance = self.distance(distance);
        let z = distance / scale;
        let cos = cos.clamp(0.0, 1.0);
        let reflected = PhysicalLaws::shipped().reflected_impulse.at(z);
        let reflected = self
            .products_floor(distance, scale * reflected)
            .map_or(reflected, |(i, _)| i / scale);
        scale
            * (reflected * cos * cos
                + PhysicalLaws::shipped().incident_impulse.at(z) * (1.0 - cos) * (1.0 - cos))
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
    /// (the environment's reflected-pressure fit).
    fn reflected_pressure(z: f32) -> f32 {
        PhysicalLaws::shipped().reflected_pressure.at(z) * 1e3
    }

    /// The peak overpressure (Pa) the blast loads a face with `distance` m from the charge, at
    /// incidence cosine `cos`: the normally reflected `p_r` square on, the incident `p_s`
    /// grazing, between them the ConWep blend [`Self::specific_impulse`] reads on impulse,
    /// `p_r cos²θ + p_s (1 + cos²θ − 2 cos θ)` (CLAIMED there, stated for pressure). A function
    /// of the scaled distance alone (Hopkinson–Cranz).
    pub(crate) fn peak_pressure(&self, distance: f32, cos: f32) -> f32 {
        let scale = (self.charge / PhysicalLaws::shipped().surface_burst_factor).cbrt();
        if scale <= 0.0 {
            return 0.0;
        }
        let distance = self.distance(distance);
        let z = distance / scale;
        let cos = cos.clamp(0.0, 1.0);
        let reflected = Self::reflected_pressure(z);
        let reflected = self
            .products_floor(
                distance,
                scale * PhysicalLaws::shipped().reflected_impulse.at(z),
            )
            .map_or(reflected, |(_, k)| reflected * k);
        reflected * cos * cos
            + PhysicalLaws::shipped().incident_pressure.at(z) * 1e3 * (1.0 - cos) * (1.0 - cos)
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
    /// ([`Crushing`]); the construction design owns the model and its oblique-projection approximation.
    pub(crate) fn crushed(
        &self,
        material: &Material,
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

/// The crush factor `f(r)` of a snowplow compaction front under the same triangular pulse,
/// against `r = σ_c,d / P`, the material's dynamic compressive strength over the face's peak
/// pressure: a face patch taking impulse `J` crushes `J f(r) / u_c` of mass, `u_c` its slowest
/// front speed. DERIVED (the #237 study, §3, checked there by numeric integration to 0.3 %):
/// `√(1 − 4r/3)` for `r < ½`, `√((16/3) r (1 − r)³)` for `½ ≤ r < 1`, nothing where the peak
/// cannot beat the strength (`r ≥ 1`); the whole blow's worth as the strength vanishes. A
/// negative or undefined ratio is a bug.
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

/// What Law 2 reads of a loaded chunk ([`Load::blast`]): its authored hull and material.
pub(crate) struct Backing<'a> {
    pub block: &'a Block,
    pub material: Material,
    /// Contacts between standing matter ([`State::contacts`]), in the hits' frame. Their
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
/// `backing` names; the construction design ("Standoff blast", Law 2) owns the model. The ligament `L` is
/// remaining depth behind the crater to the hull's actual exit. The back point `b` lies the
/// full hull depth `H` behind the face's load; the patch nearest that load supplies its crater
/// depth (ASSUMED: nearest-sample reading at the same one back point). The face's compression
/// reaches `b` as `σ_b`: the solid-angle mean of the face's transmitted stress, the sum over its
/// patches of `H / (2π R³)` (each patch's solid angle seen from `b`, over `2π`, per area) times
/// each patch's transmitted force `min(P, σ_c,d) A`; a uniform load over a plane gives back the
/// load (the #237 study's mean; the construction design names the rejected alternative). The pulse lasts `τ
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
    let depth = backing.block.exit(load.at, -normal);
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
    let shadow = silhouette(&backing.block.face_areas(), -normal);
    let covered: f32 = backing
        .contacts
        .iter()
        .filter(|joint| {
            (joint.a == load.chunk || joint.b == Side::Chunk(load.chunk))
                && joint.normal.dot(back - joint.centroid).abs() <= ON_PLANE
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

/// One blast ray's arrival on the surface its stop key names ([`trace_blast`]): where, its energy (J), impulse (N·s), peak
/// force (N, [`Blast::peak`]), the peak overpressure on its patch (Pa, [`Blast::peak_pressure`])
/// and the volume it crushes (m³, [`Blast::crushed`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RayHit {
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

/// One geometric stop for the shared blast driver. Position, normal and direction are in
/// the target's load frame; distance and silhouette are world metres and square metres.
/// `material` is absent for targets whose own law reads only the delivered impulse.
pub(crate) struct BlastSurface<K> {
    pub target: K,
    pub at: Vec3,
    pub normal: Vec3,
    pub direction: Vec3,
    pub distance: f32,
    pub silhouette: f32,
    pub material: Option<Material>,
}

/// The game's one blast-ray driver: geometry supplies the nearest visible surface for each
/// direction; the driver applies the source-radius floor, pressure, impulse, crushing and
/// silhouette interception once. A shaded direction supplies no target. The world and sandbox
/// differ only in the geometry they mount here, never in the field laws or their grouping.
pub(crate) fn trace_blast<K: Ord + Copy>(
    blast: Blast,
    directions: &[(Vec3, f32)],
    mut surface: impl FnMut(Vec3) -> Option<BlastSurface<K>>,
) -> Vec<(K, RayHit)> {
    let mut stops = Vec::new();
    let hits: Vec<_> = directions
        .iter()
        .filter_map(|&(direction, share)| {
            let hit = surface(direction)?;
            if hit.distance > blast.reach() {
                return None;
            }
            stops.push((
                hit.target,
                Blast::section(share, blast.distance(hit.distance)),
                hit.silhouette,
            ));
            let crushed = match hit.material {
                Some(material) => {
                    blast.crushed(&material, share, hit.distance, hit.direction, hit.normal)
                }
                None => Crushing {
                    volume: 0.0,
                    depth: 0.0,
                },
            };
            Some((
                hit.target,
                RayHit {
                    at: hit.at,
                    normal: hit.normal,
                    energy: blast.energy * share,
                    impulse: blast.push(share, hit.distance, hit.direction, hit.normal),
                    peak: blast.peak(share, hit.distance, hit.direction, hit.normal),
                    pressure: blast.peak_pressure(hit.distance, -hit.direction.dot(hit.normal)),
                    crushed: crushed.volume,
                    depth: crushed.depth,
                },
            ))
        })
        .collect();
    hits.into_iter()
        .zip(intercepted(&stops))
        .map(|((target, hit), kept)| {
            (
                target,
                RayHit {
                    energy: hit.energy * kept,
                    impulse: hit.impulse * kept,
                    peak: hit.peak * kept,
                    crushed: hit.crushed * kept,
                    ..hit
                },
            )
        })
        .collect()
}

/// The convex-block geometry adapter for the production driver: the sandbox's wall and the
/// tests mount blocks instead of a world query, each chunk's face areas read once; all field
/// sampling and interception still run through [`trace_blast`].
#[cfg(any(test, feature = "dev_ui"))]
pub(crate) fn sample_blocks(
    blocks: &[Block],
    standing: impl Fn(usize) -> bool,
    material: impl Fn(usize) -> Material,
    origin: Vec3,
    blast: Blast,
    directions: &[(Vec3, f32)],
) -> Vec<(usize, RayHit)> {
    let mut faces = std::collections::BTreeMap::new();
    trace_blast(blast, directions, |dir| {
        let (chunk, distance, normal) = first_hit(blocks, &standing, origin, dir)?;
        let areas = faces
            .entry(chunk)
            .or_insert_with(|| blocks[chunk].face_areas());
        Some(BlastSurface {
            target: chunk,
            at: origin + dir * distance,
            normal,
            direction: dir,
            distance,
            silhouette: silhouette(areas, dir),
            material: Some(material(chunk)),
        })
    })
}

#[cfg(test)]
mod tests {

    /// The game's environment destruction model requires a closed law boundary. A missing face
    /// can leave positive volume and several planes, but its exposed edges cannot enclose a solid.
    #[test]
    fn a_law_block_requires_a_closed_boundary_not_only_some_planes() {
        let mut block = super::Block::cuboid(bevy::math::Vec3::ZERO, bevy::math::Vec3::ONE);
        assert!(block.is_solid());
        block.planes.pop();
        block.rings.pop();
        assert!(!block.is_solid());
    }

    /// The game's construction destruction law reads the same solid with redundant points: face
    /// areas and closed edges must survive face-centre, edge and interior points in either order.
    /// Fixture: a unit cube, whose six square faces each have unit area and four corners.
    #[test]
    fn redundant_hull_points_do_not_change_the_law_boundary() {
        let mut points = Block::cuboid(Vec3::ZERO, Vec3::ONE).points;
        points.extend([
            Vec3::new(0.5, 0.5, 1.0),
            Vec3::new(0.5, 0.0, 1.0),
            Vec3::splat(0.5),
        ]);
        for _ in 0..2 {
            let block = Block::hull(points.clone()).expect("the cube is a closed solid");
            let faces = block.faces();
            assert_eq!(faces.len(), 6);
            assert!(faces.iter().all(|(_, corners)| corners.len() == 4));
            assert!(block.face_areas().iter().all(|(_, area)| *area == 1.0));
            points.reverse();
        }
    }

    use super::*;
    use crate::physical_laws::gravity_mps2;

    /// A LAW BLOCK IS THE SAME WHEREVER IT SITS (the game's environment destruction model): a hull
    /// admitted at the frame's origin stays admitted, with the same faces, moved as far as the
    /// shipped map's side, so a piece's validity never depends on where it sits in its construction.
    /// A reconstruction that tells faces apart by plane offsets measured from the origin splits a
    /// slanted face once its normal's rounding times that distance passes its tolerance, and fails
    /// here. Fixture: a slanted parallelepiped on a 1/64 m grid, so its moved corners are exact in
    /// f32 and only the reconstruction can differ.
    #[test]
    fn a_law_block_is_the_same_wherever_it_sits() {
        const GRID: f32 = 64.0;
        let side = crate::map::tests::shipped_manifest().extent.world_size_m;
        let offset = Vec3::new(1.0, 0.5, -1.0) * (side * GRID).round() / GRID;
        let edges = [
            Vec3::new(0.3125, 0.0625, -0.125),
            Vec3::new(0.046875, 0.25, 0.09375),
            Vec3::new(-0.0625, 0.03125, 0.1875),
        ];
        let corners: Vec<Vec3> = (0..8)
            .map(|i| (0..3).filter(|k| i >> k & 1 == 1).map(|k| edges[k]).sum())
            .collect();
        // Each face as its outward normal and the fixture corners it holds, by index.
        let faces = |shift: Vec3| {
            let block = Block::hull(corners.iter().map(|p| *p + shift).collect())
                .unwrap_or_else(|| panic!("admitted {shift} from the origin"));
            let mut faces: Vec<(Vec3, Vec<usize>)> = block
                .faces()
                .into_iter()
                .map(|(normal, ring)| {
                    let mut held: Vec<usize> = ring
                        .iter()
                        .map(|p| {
                            corners
                                .iter()
                                .position(|c| *c + shift == *p)
                                .expect("a face's corner is a fixture corner")
                        })
                        .collect();
                    held.sort_unstable();
                    (normal, held)
                })
                .collect();
            faces.sort_by(|a, b| a.1.cmp(&b.1));
            faces
        };
        let (home, moved) = (faces(Vec3::ZERO), faces(offset));
        assert_eq!(home.len(), 6, "a parallelepiped has six faces");
        assert_eq!(home.len(), moved.len());
        for ((n, at_home), (m, there)) in home.iter().zip(&moved) {
            assert_eq!(at_home, there, "the same corners bound each face");
            assert!(n.dot(*m) > 1.0 - 1e-6, "{n} turned to {m}");
        }
    }

    /// A BOARD'S SLIVER IS A LAW BLOCK WHILE IT IS THICKER ACROSS THAN THE PLANE TOLERANCE (the
    /// game's environment destruction model): a cell smaller than a board is thick cuts wedges
    /// through it, and each is a law block with all its faces while it is thicker across than
    /// [`ON_PLANE`]; thinner, a plane on one face would hold the other, and it is refused. A
    /// reconstruction that reads a face as every corner near its plane pulls the thin edge's
    /// corners into the next face and refuses a wedge the law resolves. Fixture: a 5 cm board's
    /// section cut to a right-angled wedge 20 cm along the grain, standing 6 m up.
    #[test]
    fn a_board_sliver_is_a_law_block_while_thicker_than_the_plane_tolerance() {
        const BOARD: f32 = 0.05;
        let wedge = |across: f32| {
            Block::hull(
                [Vec3::ZERO, Vec3::X * BOARD, Vec3::new(BOARD, across, 0.0)]
                    .into_iter()
                    .flat_map(|p| [p, p + Vec3::Z * 0.2])
                    .map(|p| p + Vec3::Y * 6.0)
                    .collect(),
            )
        };
        let block = wedge(4.0 * ON_PLANE).expect("a wedge thicker than the tolerance is a block");
        let mut rings: Vec<usize> = block.rings.iter().map(Vec::len).collect();
        rings.sort_unstable();
        assert_eq!(rings, [3, 3, 4, 4, 4], "two ends and three sides");
        assert!(wedge(ON_PLANE / 2.0).is_none());
    }

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
        Structure {
            chunks,
            joints,
            geometry: bondsolve::Geometry::CircularFixture,
        }
    }

    /// ASSUMED characterization: a three-chunk slab with one weaker opening seam. These
    /// values discriminate contact response; they are not a material calibration or asset gate.
    fn unsupported_slab(n: usize) -> Structure {
        let mut slab = row(n);
        slab.joints.retain(|joint| !matches!(joint.b, Side::Ground));
        for (j, joint) in slab.joints.iter_mut().enumerate() {
            let mut seam = joint.seam().expect("the fixture row has bonded seams");
            seam.tensile = if j == 0 { 1.0e5 } else { 1.0e9 };
            seam.fracture_energy = if j == 0 { 1.0 } else { 1.0e6 };
            joint.bond = Bond::Bonded(seam);
        }
        slab
    }

    fn slab_contact(slab: &Structure, impulse: f32) -> Load {
        Load {
            chunk: 0,
            at: slab.chunks[0].centroid,
            energy: 0.0,
            crushed: 0.0,
            blow: Vec3::Y * impulse,
            press: Vec3::ZERO,
            peak: None,
            origin: Origin::Contact {
                other: Entity::PLACEHOLDER,
                step: STEP,
            },
        }
    }

    fn assess_slab(slab: Structure, loads: &[Load]) -> (State, super::super::breakage::Changes) {
        let n = slab.chunks.len();
        let mut live = super::super::breakage::Live::from_structure(slab, 1);
        let changes = super::super::breakage::run_laws(
            &vec![0; n],
            &mut live,
            loads,
            Vec::new(),
            Vec3::ZERO,
            1.0 / 64.0,
        );
        (live.state, changes)
    }

    /// Fixture: equal unit cubes, an axial unit-area seam, no physical support. This keeps
    /// relative acceleration and finite pulse work analytically separable from bending.
    fn axial_pair() -> Structure {
        let mut pair = unsupported_slab(2);
        for (c, chunk) in pair.chunks.iter_mut().enumerate() {
            chunk.centroid = Vec3::X * c as f32;
            chunk.volume = 1.0;
            chunk.mass = chunk.material.density;
        }
        let joint = &mut pair.joints[0];
        joint.area = 1.0;
        joint.centroid = Vec3::X * 0.5;
        joint.bond = Bond::Bonded(pair.chunks[0].material.seam);
        pair
    }

    /// DERIVED axial resultant threshold: T = sigma*A for a press; a pulse also needs
    /// T²/(2k) >= G_f*A, where k = A / sum(half_cube_length/E).
    fn axial_limit(pair: &Structure, sustained: bool) -> f32 {
        let joint = &pair.joints[0];
        let seam = joint.seam().unwrap();
        let strength = seam.tensile * joint.area;
        if sustained {
            return strength;
        }
        let compliance: f64 = pair
            .chunks
            .iter()
            .map(|c| f64::from(c.volume).cbrt() / (2.0 * c.material.young))
            .sum();
        let stiffness = f64::from(joint.area) / compliance;
        strength.max((2.0 * stiffness * f64::from(seam.fracture_energy * joint.area)).sqrt() as f32)
    }

    /// An accelerating pair carries T = |F_left-F_right|/2 for equal masses. Single-ended,
    /// opposed and codirectional contacts straddle the same independently derived traction/work
    /// threshold; a uniform acceleration carries no internal force. Runtime splitting relies on it.
    #[test]
    fn axial_contacts_cross_the_relative_acceleration_threshold() {
        for sustained in [false, true] {
            for (left, right, differential) in [
                (-2.0, 0.0, true),
                (-1.0, 1.0, true),
                (-3.0, -1.0, true),
                (-2.0, -2.0, false),
            ] {
                for ratio in [0.99, 1.01] {
                    let pair = axial_pair();
                    let force = axial_limit(&pair, sustained) * ratio;
                    let loads = [left, right]
                        .into_iter()
                        .enumerate()
                        .map(|(c, share)| Load {
                            chunk: c,
                            at: pair.chunks[c].centroid,
                            blow: if sustained {
                                Vec3::ZERO
                            } else {
                                Vec3::X * (force * share * STEP)
                            },
                            press: if sustained {
                                Vec3::X * (force * share)
                            } else {
                                Vec3::ZERO
                            },
                            ..slab_contact(&pair, 0.0)
                        })
                        .collect::<Vec<_>>();
                    let (state, changes) = assess_slab(pair, &loads);
                    let opens = differential && ratio > 1.0;
                    assert_eq!(
                        state.cracked[0], opens,
                        "sustained={sustained} shares={left},{right} ratio={ratio}: {:?}",
                        changes.failures
                    );
                    assert_eq!(changes.groups.len(), if opens { 2 } else { 1 });
                }
            }
        }
    }

    /// Uniform free fall has no relative gravitational demand (DERIVED): in a group's frame its
    /// weight is cancelled by the frame's acceleration, so a seam its own weight would open were
    /// it held at one end stays shut while it falls under a negligible contact. A frame that
    /// kept gravity (no cancel) opens it. ASSUMED fixture: the slab, its weak seam at 1e4 Pa,
    /// which a standing cantilever's weight overloads (DERIVED, about 53 kPa at the seam).
    #[test]
    fn a_free_falling_group_splits_nothing() {
        let mut slab = unsupported_slab(3);
        let mut seam = slab.joints[0]
            .seam()
            .expect("the fixture row has bonded seams");
        seam.tensile = 1.0e4;
        slab.joints[0].bond = Bond::Bonded(seam);
        let mut touch = slab_contact(&slab, 1.0e-6);
        touch.press = Vec3::Y * 1.0e-6;
        let (state, changes) = assess_slab(slab, &[touch]);
        assert!(state.cracked.iter().all(|cracked| !cracked));
        assert_eq!(changes.groups.len(), 1);
        assert!(changes.failures.is_empty(), "{:?}", changes.failures);
    }

    /// Dust can remove every externally loaded chunk while the surviving group is still
    /// freely accelerating. Its rests must not acquire construction-frame weight.
    /// Fixture: three row chunks, the first two resting together, the struck third dusted.
    #[test]
    fn dusting_the_only_source_does_not_restore_gravity_to_a_free_group() {
        let mut slab = unsupported_slab(3);
        slab.joints[0].bond = Bond::Resting;
        let load = Load {
            chunk: 2,
            at: slab.chunks[2].centroid,
            crushed: full_pool(&slab.chunks[2]),
            blow: Vec3::Y,
            origin: Origin::Field,
            ..slab_contact(&slab, 0.0)
        };
        let (state, changes) = assess_slab(slab, &[load]);
        assert_eq!(state.fate[2], Fate::Dust);
        assert!(
            !state.cracked[0],
            "free fall does not open an unloaded rest"
        );
        assert_eq!(changes.groups.len(), 1);
        assert_eq!(changes.groups[0].0, vec![0, 1]);
    }

    /// A single occupied chunk has no seam to fail. Its contact may change its rigid motion
    /// but can never manufacture more chunk groups.
    #[test]
    fn a_single_chunk_never_splits() {
        let slab = unsupported_slab(1);
        let load = slab_contact(&slab, 1000.0);
        let (state, changes) = assess_slab(slab, &[load]);
        assert!(state.cracked.is_empty());
        assert_eq!(changes.groups.len(), 1);
        assert_eq!(changes.groups[0].0, vec![0]);
    }

    /// A resting joint transmits compression in its own normal and never tension, even in
    /// an accelerating frame whose load crosses the construction's original horizontal axis.
    #[test]
    fn a_moving_rest_carries_compression_from_either_side() {
        for (chunk, direction, stays) in [
            (0, Vec3::X, true),
            (1, Vec3::NEG_X, true),
            (0, Vec3::NEG_X, false),
        ] {
            let mut slab = unsupported_slab(2);
            slab.joints[0].bond = Bond::Resting;
            let mut load = slab_contact(&slab, 0.0);
            load.chunk = chunk;
            load.at = slab.chunks[chunk].centroid;
            load.blow = direction * 100.0;
            let (_, changes) = assess_slab(slab, &[load]);
            assert_eq!(
                changes.groups.len(),
                if stays { 1 } else { 2 },
                "{chunk}, {direction}"
            );
        }
    }

    /// Contact presence creates no support. Adding uniform acceleration or zero reactions
    /// preserves an axial pair's independently calculated differential tension.
    #[test]
    fn contacts_cannot_erase_internal_demand_or_turn_zero_reactions_into_support() {
        for common in [0.0, 4.0] {
            for zeros in [false, true] {
                let pair = axial_pair();
                let force = axial_limit(&pair, true) * 1.01; // Fixture: above the direct-tension threshold.
                let mut loads: Vec<_> = [-2.0 - common, -common]
                    .into_iter()
                    .enumerate()
                    .map(|(c, share)| Load {
                        chunk: c,
                        at: pair.chunks[c].centroid,
                        press: Vec3::X * (force * share),
                        ..slab_contact(&pair, 0.0)
                    })
                    .collect();
                let expected =
                    (loads[0].press.x - loads[1].press.x).abs() / (2.0 * pair.joints[0].area);
                if zeros {
                    loads.extend((0..2).map(|c| Load {
                        chunk: c,
                        at: pair.chunks[c].centroid,
                        ..slab_contact(&pair, 0.0)
                    }));
                }
                let (state, changes) = assess_slab(pair, &loads);
                assert!(state.cracked[0]);
                assert_eq!(changes.failures.len(), 1);
                assert!((changes.failures[0].demand / expected - 1.0).abs() < 1e-5);
            }
        }
    }

    /// A slab set down evenly along its length takes each chunk's share of the blow where that
    /// chunk's inertia meets it: no section carries any of it, whatever its size. The single
    /// reference must not turn an even landing into a bending one. ASSUMED fixture loads.
    #[test]
    fn an_evenly_landing_slab_stays_whole() {
        let slab = unsupported_slab(3);
        let loads: Vec<Load> = (0..3)
            .map(|c| {
                let mut load = slab_contact(&slab, 1000.0);
                load.chunk = c;
                load.at = slab.chunks[c].centroid;
                load
            })
            .collect();
        let (state, changes) = assess_slab(slab, &loads);
        assert_eq!(state.cracked, vec![false, false]);
        assert_eq!(changes.groups.len(), 1);
    }

    /// Ground press accelerates the free stack's frame upward; its upper mass therefore bears
    /// with m*g. Equal opposed side loads at the seam add no rigid torque. Slide at mu*m*g and
    /// lift at m*g follow independently of the solve and of any earlier contact outcome.
    #[test]
    fn a_supported_moving_rest_obeys_its_friction_and_lift_thresholds() {
        for lift in [false, true] {
            for ratio in [0.99, 1.01] {
                let mut stack = unsupported_slab(2);
                stack.chunks[1].centroid = Vec3::Y * 0.3;
                stack.joints[0].centroid = Vec3::Y * 0.2;
                stack.joints[0].normal = Vec3::Y;
                stack.joints[0].bond = Bond::Resting;
                let weight = stack.chunks[1].mass * gravity_mps2();
                let mu = crate::substances::pair_friction(
                    stack.chunks[0].material.friction,
                    stack.chunks[1].material.friction,
                );
                let force = if lift {
                    Vec3::Y * weight
                } else {
                    Vec3::X * (mu * weight)
                } * ratio;
                let ground = Load {
                    press: Vec3::Y
                        * (stack.chunks.iter().map(|c| c.mass).sum::<f32>() * gravity_mps2()),
                    ..slab_contact(&stack, 0.0)
                };
                let upper = Load {
                    chunk: 1,
                    at: stack.joints[0].centroid,
                    press: force,
                    ..slab_contact(&stack, 0.0)
                };
                let lower = Load {
                    chunk: 0,
                    press: -force,
                    ..upper
                };
                let (state, changes) = assess_slab(stack, &[ground, lower, upper]);
                assert_eq!(
                    state.cracked[0],
                    ratio > 1.0,
                    "lift={lift} ratio={ratio}: {:?}",
                    changes.failures
                );
                if ratio > 1.0 {
                    assert_eq!(
                        changes.failures[0].cause,
                        if lift { Cause::LiftForce } else { Cause::Slide }
                    );
                }
            }
        }
    }

    /// A spinning group's seams carry its chunks' centripetal pull in its own frame: fast
    /// enough, the weak seam opens under a contact far too light to open it alone. ASSUMED
    /// fixture: the weak-seam slab spun about its middle chunk, which a light contact holds.
    /// DERIVED: the end chunk pulls `m ω² r` = 3.6 ω² N on the 0.04 m² seam (1e5 Pa), so 60
    /// rad/s opens it (about 3.2 times its strength) and 5 rad/s does not.
    #[test]
    fn a_spinning_group_pulls_its_weak_seam_open() {
        for (omega, opens) in [(5.0, false), (60.0, true)] {
            for source_survives in [false, true] {
                let slab = unsupported_slab(3);
                let mut hold = slab_contact(&slab, 0.01);
                hold.chunk = 1;
                hold.at = slab.chunks[1].centroid;
                hold.press = Vec3::Y * 0.01;
                let n = slab.chunks.len();
                let mut live = super::super::breakage::Live::from_structure(slab, 1);
                super::super::breakage::run_laws(
                    &vec![0; n],
                    &mut live,
                    if source_survives {
                        std::slice::from_ref(&hold)
                    } else {
                        &[]
                    },
                    Vec::new(),
                    Vec3::Y * omega,
                    1.0 / 64.0,
                );
                assert_eq!(
                    live.state.cracked[0] || live.state.cracked[1],
                    opens,
                    "{omega} rad/s"
                );
            }
        }
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
        Structure {
            chunks,
            joints,
            geometry: bondsolve::Geometry::CircularFixture,
        }
    }

    fn brick() -> Material {
        Material::of(SubstanceRegistry::shipped_static(), "Brick")
            .expect("the shipped registry holds Brick")
            .expect("Brick states a seam strength")
    }

    /// The game's environment destruction reads material facts from each admitted registry row.
    /// Changing resistance or reference facts must change crushing work without changing this test.
    #[test]
    fn the_material_is_the_substance_row() {
        let registry = SubstanceRegistry::shipped_static();
        for name in registry.keys() {
            let substance = registry.get(name).unwrap();
            let Some(hard) = substance.hard() else {
                continue; // Witnesses and unworn rows are not structural matter.
            };
            let material = Material::of(registry, name);
            let Some(bulk) = hard.bulk else {
                assert!(
                    material.unwrap().is_none(),
                    "{name}: no bulk means no fracture law"
                );
                continue;
            };
            let complete = bulk.tensile.is_some()
                && bulk.fracture_energy.is_some()
                && bulk.shear_cohesion.is_some()
                && bulk.shear_fracture_energy.is_some()
                && hard.friction.is_some()
                && bulk.blast.is_some();
            if !complete {
                assert!(
                    material.is_err(),
                    "{name}: incomplete structural facts are refused"
                );
                continue;
            }
            let material = material.unwrap().unwrap();
            assert_eq!(material.density, bulk.density as f32, "{name}");
            assert_eq!(
                material.seam.tensile,
                bulk.tensile.unwrap() as f32,
                "{name}"
            );
            assert_eq!(
                material.seam.fracture_energy,
                bulk.fracture_energy.unwrap() as f32,
                "{name}"
            );
            let resistance = f64::from(hard.factor()) / f64::from(REFERENCE_FACTOR);
            let expected = (resistance * registry.reference_facts().cavity_strength()) as f32;
            assert!(
                (material.crush - expected).abs() <= 2.0 * f32::EPSILON * expected.abs(),
                "{name}"
            );
        }
    }

    /// A POOL WEARS, THEN EMPTIES: crushed volume under a chunk's pool leaves it standing and worn; the
    /// rest of its pool turns it to dust, and a dusted chunk takes no more and joins nothing, so a
    /// chunk beyond it with no other path to the ground falls. The removal alone ([`wear`]): a
    /// load's energy also stresses the joints about it ([`strike`]). A mutant that dusts on the
    /// first deposit, or that leaves a dusted chunk holding its joints, fails here.
    #[test]
    fn a_spent_pool_turns_a_chunk_to_dust_and_drops_what_it_held() {
        let structure = row(3);
        let mut state = State::new(&structure);
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

    /// The game's kinetic-cavity law reaches a seam at its independently integrated Lamé
    /// traction. The source and seven circular quadrature points share the seam plane, so every
    /// point has cos(theta)=0. No production stress call supplies the expected threshold.
    #[test]
    fn a_seam_cracks_where_the_push_reaches_its_strength() {
        let structure = row(2);
        let joint = structure.joints[0];
        let material = structure.chunks[0].material;
        let distance = 2.0_f64; // Fixture source offset in the joint plane, m.
        let ring = (f64::from(joint.area) * 2.0 / (3.0 * std::f64::consts::PI)).sqrt();
        let mean = 0.25 / distance.powi(3)
            + (0..6)
                .map(|k| {
                    let angle = k as f64 * std::f64::consts::PI / 3.0;
                    0.125
                        / (distance.powi(2) + ring.powi(2) - 2.0 * distance * ring * angle.cos())
                            .powf(1.5)
                })
                .sum::<f64>();
        let limit = f64::from(material.seam.tensile) * 8.0 * std::f64::consts::PI / (3.0 * mean);
        for (ratio, opens) in [(0.99, false), (1.01, true)] {
            let energy = limit * ratio;
            let radius =
                (3.0 * energy / (4.0 * std::f64::consts::PI * f64::from(material.crush))).cbrt();
            assert!(
                radius < distance - ring,
                "the analytical field lies outside its crushed core"
            );
            let load = Load {
                chunk: 0,
                at: joint.centroid + Vec3::Z * distance as f32,
                energy: energy as f32,
                ..slab_contact(&structure, 0.0)
            };
            let mut state = State::new(&structure);
            assert_eq!(crack(&structure, &mut state, &[load]).contains(&0), opens);
        }
    }

    /// THE PUSH RULE CLAMPS EACH SOURCE AT ITS CRUSHED RADIUS IN THE JOINT'S OWN MATERIAL: on a
    /// fixture wall whose right half is a fixture masonry of an eighth of brick's crush energy (its
    /// crushed radii twice brick's), two sources, one in each half, crack at every energy of a
    /// sweep exactly the joints whose [`joint_load`] reaches its threshold, joint by joint. Some
    /// energy of the sweep cracks other joints when every joint takes brick's radii, so a [`crack`]
    /// that kept the radii of the first material it met fails here; a construction of more than one
    /// substance relies on each seam reading its own.
    #[test]
    fn each_joint_clamps_the_sources_at_its_own_materials_radius() {
        let (w, h) = (6, 4);
        let mut structure = grid(w, h);
        let brick = brick();
        let soft = Material {
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
            let rule = |material_of: &dyn Fn(&Joint) -> Material| -> Vec<usize> {
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
            let mut state = State::new(&structure);
            assert_eq!(
                crack(&structure, &mut state, &sources),
                expected,
                "{energy} J"
            );
        }
        assert!(told_apart, "the sweep tells the materials' radii apart");
    }

    /// IMPULSE-WEIGHTED CENTROIDS PRESERVE ANGULAR IMPULSE (the game's environment destruction
    /// model; study test 9). The structural check and a freed piece's spin rely on the summed
    /// moment about any pivot. An energy-weighted point, merging opposite faces, or dropping a
    /// load with no energy breaks this invariant. ASSUMED fixture patches and impulses.
    #[test]
    fn impulse_weighted_centroids_preserve_angular_impulse_without_an_energy_gate() {
        let hit = |at: Vec3, normal: Vec3, impulse: f32, energy: f32| RayHit {
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
            let keyed = hits.map(|hit| (3, hit));
            let (loads, scabs) = Load::blast(&keyed, |_| Backing {
                block: &slab,
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
            let r = PhysicalLaws::shipped().reflected_impulse.at(z);
            let i = PhysicalLaws::shipped().incident_impulse.at(z);
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
        assert!(close(
            PhysicalLaws::shipped().incident_pressure.at(1.0) * 1e3,
            1.3537e6
        ));
        assert!(close(
            PhysicalLaws::shipped().incident_pressure.at(10.0) * 1e3,
            14.889e3
        ));
        assert!(close(Blast::reflected_pressure(10.0), 31.54e3));
        let join = PhysicalLaws::shipped().reflected_pressure.branches[0].0;
        let far = &PhysicalLaws::shipped().reflected_pressure.branches[1].1;
        let l = join.ln();
        let beyond = far.iter().rev().fold(0.0, |sum, ck| sum * l + ck).exp();
        assert!(
            (beyond / PhysicalLaws::shipped().reflected_pressure.at(join) - 1.0).abs() < 1e-3,
            "the reflected branches meet at their join"
        );
        let n = 2000;
        let z = |k: usize| 0.06 * (40.0_f32 / 0.06).powf(k as f32 / n as f32);
        for k in 0..=n {
            let (r, s) = (
                Blast::reflected_pressure(z(k)),
                PhysicalLaws::shipped().incident_pressure.at(z(k)) * 1e3,
            );
            assert!(
                r >= 2.0 * s,
                "Z {}: reflected {r} Pa under twice {s} Pa",
                z(k)
            );
            if k < n {
                let (r1, s1) = (
                    Blast::reflected_pressure(z(k + 1)),
                    PhysicalLaws::shipped().incident_pressure.at(z(k + 1)) * 1e3,
                );
                assert!(r1 <= r * 1.01 && s1 <= s * 1.01, "Z {}: rises", z(k));
            }
        }
        // W / 1.8 = 1 kg and 8 kg: the same scaled distance at twice the range (fixture data).
        let (one, eight) = (
            Blast::of(
                PhysicalLaws::shipped().tnt_energy_j_per_kg
                    * PhysicalLaws::shipped().surface_burst_factor,
                1e-3,
                FIT_ONLY_PRODUCTS,
            ),
            Blast::of(
                8.0 * PhysicalLaws::shipped().tnt_energy_j_per_kg
                    * PhysicalLaws::shipped().surface_burst_factor,
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
            PhysicalLaws::shipped().incident_pressure.at(3.0) * 1e3
        ));
    }

    /// A bare fixture charge of `mass` kg (ASSUMED fixture core: 1650 kg/m³, `K` = 2400 m/s, the
    /// TNT convention's energy), its radius the equal-volume sphere's and its products at the
    /// burst law's bare speed `K √2`.
    fn bare_charge(mass: f32) -> Blast {
        let radius = (3.0 * mass / (4.0 * PI * 1650.0)).cbrt();
        let speed = 2400.0 * std::f32::consts::SQRT_2;
        Blast::of(
            mass * PhysicalLaws::shipped().tnt_energy_j_per_kg,
            radius,
            crate::ballistics::burst::products_momentum(mass, speed),
        )
    }

    /// The free-air fit's normally reflected impulse (Pa·s) `distance` m from `blast`.
    fn fit_reflected(blast: &Blast, distance: f32) -> f32 {
        let scale = (blast.charge / PhysicalLaws::shipped().surface_burst_factor).cbrt();
        scale
            * PhysicalLaws::shipped()
                .reflected_impulse
                .at(blast.distance(distance) / scale)
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
            let z = crossing / (blast.charge / PhysicalLaws::shipped().surface_burst_factor).cbrt();
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
            let scale = (blast.charge / PhysicalLaws::shipped().surface_burst_factor).cbrt();
            let crossing = products_crossing(&blast);
            let steps = 200;
            for k in 1..=steps {
                let r = crossing * (blast.reach() / crossing).powf(k as f32 / steps as f32);
                let z = blast.distance(r) / scale;
                for cos in [1.0_f32, 0.7, 0.3, 0.0] {
                    let impulse = scale
                        * (PhysicalLaws::shipped().reflected_impulse.at(z) * cos * cos
                            + PhysicalLaws::shipped().incident_impulse.at(z)
                                * (1.0 - cos)
                                * (1.0 - cos));
                    let peak = Blast::reflected_pressure(z) * cos * cos
                        + PhysicalLaws::shipped().incident_pressure.at(z)
                            * 1e3
                            * (1.0 - cos)
                            * (1.0 - cos);
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
        let hits = sample_blocks(
            &blocks,
            |_| true,
            |_| material,
            Vec3::ZERO,
            blast,
            &[(Vec3::Z, share)],
        );
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.crushed > 0.0 && hits[0].1.impulse.z > 0.0);
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
                let blast = Blast::of(
                    mass * PhysicalLaws::shipped().tnt_energy_j_per_kg,
                    radius,
                    FIT_ONLY_PRODUCTS,
                );
                for range in [radius, 0.1_f32.max(radius), 0.5, 1.0] {
                    let hits = sample_blocks(
                        &blocks,
                        |_| true,
                        |_| material,
                        Vec3::new(0.0, 0.0, -range),
                        blast,
                        &directions,
                    );
                    let volume: f32 = hits.iter().map(|(_, hit)| hit.crushed).sum();
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
            material: chunk.material,
            contacts: &[],
        };
        let pools: Vec<f32> = [1.0, 1.0e3 * chunk.material.crush * chunk.volume]
            .into_iter()
            .map(|energy| {
                let mut state = State::new(&structure);
                strike(
                    &structure,
                    &mut state,
                    &Load::blast(&[(0, hit(energy))], backing).0,
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
            1000.0
                * PhysicalLaws::shipped().tnt_energy_j_per_kg
                * PhysicalLaws::shipped().surface_burst_factor,
            1e-3,
            FIT_ONLY_PRODUCTS,
        );
        let cube = [Block::cuboid(Vec3::splat(-0.125), Vec3::splat(0.125))];
        let origin = Vec3::new(0.0, 0.0, -20.125);
        let hits = sample_blocks(
            &cube,
            |_| true,
            |_| brick(),
            origin,
            blast,
            &[(Vec3::Z, 1e-6)],
        );
        let [(_, hit)] = hits[..] else {
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
        let mut state = State::new(&structure);
        let (loads, scabs) = Load::blast(&[(0, heavy)], |_| Backing {
            block: &cube[0],
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

    /// CRUSHED MASS IS REPRESENTATION-INDEPENDENT (the game's environment destruction model, the
    /// #237 study's test 4). Law 1 is linear in the impulse at a fixed pressure, so a face cut
    /// into any number of patches under one field crushes the mass it does as one patch, exactly
    /// (to f32 rounding). #204's freedom to change how the field finds surfaces relies on it: a
    /// crush law with a per-patch term or a power of the impulse other than one fails here.
    ///
    /// The ray sampler is today's representation. A near burst on a fixture brick face 2 m
    /// square (fixture geometry and charges), one block or 8 × 8 chunks, is cast at several ray
    /// counts; crushed volumes and spreads from the finest cast are observations for #204,
    /// with no independently derived sampling-error bound.
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
            (
                "bare 5 kg",
                5.0 * PhysicalLaws::shipped().tnt_energy_j_per_kg,
                0.3,
            ),
        ] {
            let blast = Blast::of(energy, 1e-3, FIT_ONLY_PRODUCTS);
            let origin = Vec3::new(0.1, -0.05, -standoff);
            let crushed = |blocks: &[Block], rays: usize| -> f32 {
                sample_blocks(
                    blocks,
                    |_| true,
                    |_| brick(),
                    origin,
                    blast,
                    &ball_directions(rays),
                )
                .iter()
                .map(|(_, hit)| hit.crushed)
                .sum()
            };
            let finest = crushed(&slab, 65_536);
            for rays in [1024, 4096, 16_384, 65_536] {
                for (form, blocks) in [("one block", &slab[..]), ("8 x 8 chunks", &chunks[..])] {
                    let volume = crushed(blocks, rays);
                    let spread = volume / finest - 1.0;
                    println!(
                        "MEASURED {name}, {form}, {rays} rays: crushed {:.4e} m³, {:+.6e} of \
                         one block at 65536",
                        volume, spread
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
        let blast = Blast::of(
            PhysicalLaws::shipped().tnt_energy_j_per_kg
                * PhysicalLaws::shipped().surface_burst_factor,
            1e-3,
            FIT_ONLY_PRODUCTS,
        );
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
        let hits = sample_blocks(
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
            (hits[0].1.peak - Vec3::Z * want).length() <= 1e-4 * want,
            "{} against {want}",
            hits[0].1.peak
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
        let blast = Blast::of(
            PhysicalLaws::shipped().tnt_energy_j_per_kg
                * PhysicalLaws::shipped().surface_burst_factor,
            1e-3,
            FIT_ONLY_PRODUCTS,
        );
        let share = 1.0 / BLAST_RAYS as f32;
        let d = 2.0;
        let patch = 4.0 * PI * share * d * d;
        let (reflected, incident) = (
            PhysicalLaws::shipped().reflected_impulse.at(d),
            PhysicalLaws::shipped().incident_impulse.at(d),
        );
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
        let blast = Blast::of(
            PhysicalLaws::shipped().tnt_energy_j_per_kg
                * PhysicalLaws::shipped().surface_burst_factor,
            1e-3,
            FIT_ONLY_PRODUCTS,
        );
        let share = 1.0 / BLAST_RAYS as f32;
        let face = CUBE * CUBE;
        let cube = [Block::cuboid(Vec3::ZERO, Vec3::splat(CUBE))];
        let hit = |d: f32| {
            let origin = Vec3::new(CUBE / 2.0, CUBE / 2.0, -d);
            sample_blocks(
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
            let want = PhysicalLaws::shipped().reflected_impulse.at(d) * section * kept;
            assert!(
                (hits[0].1.impulse - Vec3::Z * want).length() <= 1e-4 * want,
                "{d} m: {} against {want}",
                hits[0].1.impulse
            );
            let energy = blast.energy * share * kept;
            assert!((hits[0].1.energy - energy).abs() <= 1e-4 * energy, "{d} m");
        }
        assert!(Blast::section(share, 0.5) < face && Blast::section(share, 10.0) > face);
        assert!(hit(blast.reach() + 0.5).is_empty(), "past reach");
    }

    /// The fixture cube's width, m, and so its faces' area, m².
    const CUBE: f32 = 0.25;

    /// The shipped wood, as the laws read it.
    fn wood() -> Material {
        Material::of(SubstanceRegistry::shipped_static(), "Wood")
            .expect("the shipped registry holds Wood")
            .expect("Wood states bulk facts")
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
            let material = Material::of(&registry, name).unwrap().unwrap();
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
    fn scab_face(material: &Material, blow: f32) -> (RayHit, Block) {
        let pressure = material.compaction.strength;
        let hit = RayHit {
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
    fn scab_blow(material: &Material, lengths: f32) -> f32 {
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
            Load::blast(&[(0, RayHit { depth, ..hit })], |_| Backing {
                block: &slab,
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
        let blast = Blast::of(
            5.0 * PhysicalLaws::shipped().tnt_energy_j_per_kg,
            0.05,
            FIT_ONLY_PRODUCTS,
        );
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
    /// fixture at its authored dimensions.
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
        ])
        .expect("the wedge is a closed solid");
        let backing = |contacts| Backing {
            block: &wedge,
            material,
            contacts,
        };
        let (_, scabs) = Load::blast(&[(0, hit)], |_| backing(&[]));
        let [scab] = scabs[..] else {
            panic!("the fixture pulse scabs: {scabs:?}")
        };
        assert!((scab.at - Vec3::Z * 0.375).length() < 1e-5);
        assert!(wedge.planes.iter().all(|(n, o)| n.dot(scab.at) <= o + 1e-5));
        let (normal, _) = wedge.planes.iter().find(|(n, _)| n.z > 0.0).unwrap();
        let contact = Joint {
            a: 0,
            b: Side::Ground,
            area: 4.0 / normal.z,
            centroid: scab.at,
            normal: *normal,
            bond: Bond::Bonded(material.seam),
        };
        assert!(
            Load::blast(&[(0, hit)], |_| backing(std::slice::from_ref(&contact)))
                .1
                .is_empty(),
            "a contact on the actual rear plane covers it"
        );
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
            let (_, scabs) = Load::blast(&[(0, RayHit { at, ..hit })], |_| Backing {
                block: &slab,
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
            Load::blast(&[(0, hit)], |_| Backing {
                block: &slab,
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
        let mut state = State::new(&structure);
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
        state.separated[0] = true;
        assert_eq!(
            thrown(&state.contacts(&structure).collect::<Vec<_>>())
                .1
                .len(),
            1,
            "an opened rest exposes the back"
        );
        state.cracked[0] = false;
        state.separated[0] = false;
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
                    let (_, scabs) = Load::blast(&[(0, hit)], |_| Backing {
                        block: &slab,
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

    /// The game's environment destruction model conserves the delivered face impulse and
    /// removed volume. Fixture rays guarantee admission analytically: rear stress 2*sigma_t,
    /// pulse length one slab thickness, no crater, and an exposed rear face.
    #[test]
    fn a_scab_takes_its_momentum_off_its_faces_push() {
        let material = brick();
        for thickness in [0.12_f32, 0.25, 0.5] {
            let slab = Block::cuboid(Vec3::new(-1.0, -1.0, 0.0), Vec3::new(1.0, 1.0, thickness));
            let stress = 2.0 * material.spall.tensile;
            // DERIVED central Boussinesq stress F/(2*pi*t²); lambda=c*2J/F=t.
            let force = stress * 2.0 * PI * thickness.powi(2);
            let impulse = thickness * force / (2.0 * material.spall.bar_speed);
            let hit = RayHit {
                at: Vec3::ZERO,
                normal: Vec3::NEG_Z,
                energy: 0.0,
                impulse: Vec3::Z * impulse,
                peak: Vec3::Z * force,
                pressure: material.compaction.strength,
                crushed: 0.0,
                depth: 0.0,
            };
            let (loads, scabs) = Load::blast(&[(0, hit)], |_| Backing {
                block: &slab,
                material,
                contacts: &[],
            });
            let [scab] = scabs[..] else {
                panic!("the independently admitted pulse emits one rear scab: {scabs:?}");
            };
            let kept: Vec3 = loads.iter().map(|l| l.blow).sum();
            assert!((kept + scab.momentum() - hit.impulse).length() <= 1e-4 * impulse);
            assert!(kept.dot(hit.impulse) > 0.0 && scab.momentum().dot(hit.impulse) > 0.0);
            let removal = loads
                .iter()
                .find(|l| l.crushed > 0.0)
                .expect("the scab removes its occupied volume");
            assert_eq!((removal.at, removal.blow), (scab.at, Vec3::ZERO));
            assert!((removal.crushed * material.density / scab.mass - 1.0).abs() < 1e-5);
            assert!((scab.at.z - thickness).abs() < 1e-5);
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
        let clinker = Material::of(&renamed, "Clinker")
            .unwrap()
            .expect("the renamed row states bulk facts");
        let scab = |material: Material, loaded: &Material| {
            let (hit, slab) = scab_face(loaded, scab_blow(loaded, 0.1));
            let (_, scabs) = Load::blast(&[(0, hit)], |_| Backing {
                block: &slab,
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
