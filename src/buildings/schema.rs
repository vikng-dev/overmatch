//! `assets/buildings/<id>/<id>.building.ron`: one building as the bake writes it and the game reads
//! it. The one definition of the format: the asset tools (`tools/`, the map compiler's building
//! stage) serialize it and the game deserializes it, both through these types.
//!
//! # The building frame
//!
//! Metres, `+Y` up, the origin on the instance's ground point: the scatter places a building at the
//! terrain height under its origin, so the foundation plane ([`BuildingAsset::foundation_y_m`])
//! sits below zero as a skirt into a slope, as the graybox box's `min.y` did.
//!
//! # Elements, chunks, joints, anchors
//!
//! - An [`Element`] is a generator terminal (a pier, a sill, a lintel, a gable, a wall plate, a roof
//!   plane): a right prism, a convex profile extruded through a depth. Intact, each is one convex
//!   collider; elements tile the building and never overlap.
//! - A [`Chunk`] is the fracture bake's piece: a convex hull inside one element. An element's
//!   chunks are a contiguous run of [`BuildingAsset::chunks`] ([`Element::chunks`]) and tile it.
//! - A component is a part the generator builds as one (a brick wall, a wall plate, a roof plane):
//!   each element names its own ([`Element::component`]).
//! - A [`Joint`] is a contact face between two chunks: its area, centroid and the normal from the
//!   first chunk into the second (NVIDIA Blast's bond), and what holds it ([`Bond`]): inside one
//!   component its own seam, between two the bond the generator laid there. An [`Anchor`] is a
//!   chunk face on the foundation plane (Blast's world bond), held by its chunk's own seam.
//!
//! The simulation reads chunks and joints only; it never reads how the bake chose the cuts.
//!
//! [`BuildingAsset::check`] is the format's contract: every index in range, every element's chunk
//! run contiguous and covering, every joint and anchor of positive area, every anchor on the
//! foundation plane. The game refuses a file that breaks it (ADR-0011).

use bevy::math::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// The format revision this build reads and writes. A file of another revision is refused.
pub const FORMAT: u32 = 2;

/// How far a vertex, a centroid or a plane may sit from where the contract places it, metres:
/// the f32 rounding of a building-sized coordinate, many times over, and far below any modelled
/// feature (a brick course is 75 mm).
pub const PLANE_TOLERANCE_M: f32 = 1.0e-4;

/// One building (module doc).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingAsset {
    /// The asset id: the folder and the file stem.
    pub id: String,
    /// [`FORMAT`].
    pub format: u32,
    /// The foundation plane's height in the building frame, metres. Every anchor lies on it.
    pub foundation_y_m: f32,
    /// The substance keys (`assets/materials/materials.ron`) elements and chunks name, by index.
    pub materials: Vec<String>,
    /// The components' names, unique, by index ([`Element::component`]).
    pub components: Vec<String>,
    pub elements: Vec<Element>,
    pub chunks: Vec<Chunk>,
    /// Ordered by `(a, b)`, each pair once.
    pub joints: Vec<Joint>,
    /// Ordered by chunk, each chunk once.
    pub anchors: Vec<Anchor>,
}

/// What an element is, as the generator labelled it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ElementKind {
    /// Full wall height between openings or at a wall's end.
    Pier,
    /// Under an opening, from the foundation to the opening's bottom.
    Sill,
    /// Over an opening, from the opening's head to the eaves.
    Lintel,
    /// A gable wall's part above the eaves, under the roof.
    Gable,
    /// The timber beam a roof plane rests on along a wall's top.
    WallPlate,
    /// A roof plane: its boards.
    Roof,
}

/// An element's frame: origin and two in-plane axes, building frame. The extrusion axis is
/// `w = u × v`; `u` and `v` are unit and orthogonal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub origin: [f32; 3],
    pub u: [f32; 3],
    pub v: [f32; 3],
}

impl Frame {
    /// The extrusion axis `u × v`.
    pub fn w(&self) -> Vec3 {
        Vec3::from_array(self.u).cross(Vec3::from_array(self.v))
    }

    /// The building-frame point at profile coordinates `p` and depth `d`.
    pub fn point(&self, p: Vec2, d: f32) -> Vec3 {
        Vec3::from_array(self.origin)
            + Vec3::from_array(self.u) * p.x
            + Vec3::from_array(self.v) * p.y
            + self.w() * d
    }
}

/// One generator terminal (module doc): the prism `profile × depth` in `frame`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Element {
    /// The split-tree path, `<wall>/<kind>_<n>`: unique within the building.
    pub name: String,
    pub kind: ElementKind,
    /// Index into [`BuildingAsset::components`]: the component it is part of.
    pub component: u16,
    /// Index into [`BuildingAsset::materials`].
    pub material: u8,
    pub frame: Frame,
    /// The convex profile in `(u, v)`, counter-clockwise about `w`.
    pub profile: Vec<[f32; 2]>,
    /// The extrusion range along `w`, `[near, far]`, `near < far`.
    pub depth: [f32; 2],
    /// This element's chunks: `chunks[first..end]` of [`BuildingAsset::chunks`].
    pub chunks: [u32; 2],
}

impl Element {
    /// The prism's corners, building frame: the profile at the near depth, then at the far.
    pub fn points(&self) -> Vec<Vec3> {
        self.depth
            .iter()
            .flat_map(|&d| {
                self.profile
                    .iter()
                    .map(move |&p| self.frame.point(Vec2::from_array(p), d))
            })
            .collect()
    }

    /// The prism's faces, each as its outward unit normal and its corners counter-clockwise seen
    /// from outside: the far cap, the near cap, then one side per profile edge.
    pub fn faces(&self) -> Vec<(Vec3, Vec<Vec3>)> {
        let n = self.profile.len();
        let [near, far] = self.depth;
        let at = |k: usize, d: f32| self.frame.point(Vec2::from_array(self.profile[k]), d);
        let w = self.frame.w();
        let mut faces = vec![
            (w, (0..n).map(|k| at(k, far)).collect()),
            (-w, (0..n).rev().map(|k| at(k, near)).collect()),
        ];
        let (u, v) = (
            Vec3::from_array(self.frame.u),
            Vec3::from_array(self.frame.v),
        );
        for k in 0..n {
            let next = (k + 1) % n;
            let edge = Vec2::from_array(self.profile[next]) - Vec2::from_array(self.profile[k]);
            // Counter-clockwise about `w`, the interior lies left of each edge.
            let out = Vec2::new(edge.y, -edge.x).normalize();
            faces.push((
                (u * out.x + v * out.y).normalize(),
                vec![at(k, near), at(next, near), at(next, far), at(k, far)],
            ));
        }
        faces
    }

    /// The prism's volume, m³.
    pub fn volume(&self) -> f32 {
        polygon_area(&self.profile) * (self.depth[1] - self.depth[0])
    }
}

/// One fracture piece (module doc).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    /// Index into [`BuildingAsset::elements`]: the element it lies in.
    pub element: u16,
    /// Index into [`BuildingAsset::materials`]: its element's.
    pub material: u8,
    /// The convex hull's corners, building frame.
    pub hull: Vec<[f32; 3]>,
    pub volume_m3: f32,
    /// Building frame.
    pub centroid: [f32; 3],
    /// `volume_m3 ×` the material's density (`materials.ron`), kg.
    pub mass_kg: f32,
}

/// One contact face between chunks `a < b` (module doc).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Joint {
    pub a: u32,
    pub b: u32,
    /// The shared face's area, m².
    pub area_m2: f32,
    /// The shared face's centroid, building frame.
    pub centroid: [f32; 3],
    /// Unit, from `a` into `b`.
    pub normal: [f32; 3],
    pub bond: Bond,
}

/// What holds a joint closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Bond {
    /// Inside one component: the component's own seam, its chunks' material's.
    Seam,
    /// Between two components, bonded with the seam of [`BuildingAsset::materials`]`[_]` (a
    /// wall's corner in mortar).
    Bonded(u8),
    /// Between two components, one set on the other: no pull strength and no fracture work, held
    /// only while weight presses it together (a roof plane on its wall plate).
    Resting,
}

/// One chunk face on the foundation plane: the ground holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub chunk: u32,
    /// The face's area on the plane, m².
    pub area_m2: f32,
    /// The face's centroid, on the foundation plane.
    pub centroid: [f32; 3],
}

/// The signed area of a polygon, positive when counter-clockwise.
pub fn polygon_area(points: &[[f32; 2]]) -> f32 {
    let n = points.len();
    0.5 * (0..n)
        .map(|k| {
            let (p, q) = (points[k], points[(k + 1) % n]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum::<f32>()
}

impl BuildingAsset {
    /// Parse and check a file's text (ADR-0011): a file that does not parse or breaks the contract
    /// is refused, naming why.
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let asset: Self = ron::from_str(text).map_err(|err| err.to_string())?;
        asset.check()?;
        Ok(asset)
    }

    /// The format's contract (module doc). Names the first violation.
    pub fn check(&self) -> Result<(), String> {
        if self.format != FORMAT {
            return Err(format!(
                "format {} where this build reads {FORMAT}",
                self.format
            ));
        }
        if self.materials.is_empty() || self.materials.len() > usize::from(u8::MAX) + 1 {
            return Err(format!("{} materials", self.materials.len()));
        }
        if self.elements.is_empty() || self.elements.len() > usize::from(u16::MAX) + 1 {
            return Err(format!("{} elements", self.elements.len()));
        }
        if self.components.len() > usize::from(u16::MAX) + 1 {
            return Err(format!("{} components", self.components.len()));
        }
        for (k, name) in self.components.iter().enumerate() {
            if self.components[..k].contains(name) {
                return Err(format!("component {name} is named twice"));
            }
        }
        let material = |index: u8, what: &str| {
            (usize::from(index) < self.materials.len())
                .then_some(())
                .ok_or_else(|| format!("{what} names material {index}, past the table"))
        };
        let mut next = 0u32;
        for (index, element) in self.elements.iter().enumerate() {
            let name = &element.name;
            material(element.material, name)?;
            if usize::from(element.component) >= self.components.len() {
                return Err(format!(
                    "element {name} names component {}, past the table",
                    element.component
                ));
            }
            if element.profile.len() < 3 || polygon_area(&element.profile) <= 0.0 {
                return Err(format!(
                    "element {name}'s profile is no counter-clockwise polygon"
                ));
            }
            if element.depth[0].partial_cmp(&element.depth[1]) != Some(std::cmp::Ordering::Less) {
                return Err(format!(
                    "element {name}'s depth {:?} is empty",
                    element.depth
                ));
            }
            let [first, end] = element.chunks;
            if first != next || end <= first || end as usize > self.chunks.len() {
                return Err(format!(
                    "element {name}'s chunks {first}..{end} do not continue the run at {next}"
                ));
            }
            for chunk in &self.chunks[first as usize..end as usize] {
                if usize::from(chunk.element) != index || chunk.material != element.material {
                    return Err(format!(
                        "a chunk in element {name}'s run names element {} material {}",
                        chunk.element, chunk.material
                    ));
                }
            }
            next = end;
        }
        if next as usize != self.chunks.len() {
            return Err(format!(
                "the elements' runs cover {next} of {} chunks",
                self.chunks.len()
            ));
        }
        for (index, chunk) in self.chunks.iter().enumerate() {
            let positive = |v: f32| v.is_finite() && v > 0.0;
            if chunk.hull.len() < 4 || !positive(chunk.volume_m3) || !positive(chunk.mass_kg) {
                return Err(format!("chunk {index} has no volume or no mass"));
            }
        }
        let count = self.chunks.len() as u32;
        let mut last: Option<(u32, u32)> = None;
        for joint in &self.joints {
            let pair = (joint.a, joint.b);
            if !(joint.a < joint.b && joint.b < count) || last.is_some_and(|last| last >= pair) {
                return Err(format!("joint {pair:?} is out of order or out of range"));
            }
            if !(joint.area_m2.is_finite() && joint.area_m2 > 0.0) {
                return Err(format!("joint {pair:?} has area {}", joint.area_m2));
            }
            if (Vec3::from_array(joint.normal).length() - 1.0).abs() > PLANE_TOLERANCE_M {
                return Err(format!("joint {pair:?}'s normal is not unit"));
            }
            let component = |c: u32| {
                let element = self.chunks[c as usize].element;
                self.elements[usize::from(element)].component
            };
            let inside = component(joint.a) == component(joint.b);
            match joint.bond {
                Bond::Seam if !inside => {
                    return Err(format!("joint {pair:?} is a seam between two components"));
                }
                Bond::Bonded(_) | Bond::Resting if inside => {
                    return Err(format!("joint {pair:?} is a bond inside one component"));
                }
                Bond::Bonded(index) => material(index, &format!("joint {pair:?}"))?,
                _ => {}
            }
            last = Some(pair);
        }
        let mut last: Option<u32> = None;
        for anchor in &self.anchors {
            if anchor.chunk >= count || last.is_some_and(|last| last >= anchor.chunk) {
                return Err(format!(
                    "anchor on chunk {} is out of order or out of range",
                    anchor.chunk
                ));
            }
            if !(anchor.area_m2.is_finite() && anchor.area_m2 > 0.0) {
                return Err(format!("anchor on chunk {} has no area", anchor.chunk));
            }
            if (anchor.centroid[1] - self.foundation_y_m).abs() > PLANE_TOLERANCE_M {
                return Err(format!(
                    "anchor on chunk {} sits at y {}, off the foundation plane {}",
                    anchor.chunk, anchor.centroid[1], self.foundation_y_m
                ));
            }
            last = Some(anchor.chunk);
        }
        Ok(())
    }
}

/// The density (kg/m³) of the shipped substance `name`, from its row's facts: what a bake
/// multiplies a chunk's volume by. Refuses an unknown name, a substance that is not hard matter
/// and a row that states no density.
pub fn substance_density(name: &str) -> Result<f64, String> {
    crate::substances::SubstanceRegistry::shipped_static()
        .hard(name)
        .map_err(|err| err.to_string())?
        .density()
        .ok_or_else(|| format!("substance {name:?} states no density"))
}
