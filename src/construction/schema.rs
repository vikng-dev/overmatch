//! `assets/constructions/<id>/<id>.construction.ron`: one construction as the bake writes it and the game reads
//! it. The one definition of the format: the asset tools (`tools/`, the map compiler's construction
//! stage) serialize it and the game deserializes it, both through these types.
//!
//! # The construction frame
//!
//! Metres, `+Y` up, the origin on the instance's ground point: the scatter places a construction at the
//! terrain height under its origin, so the foundation plane ([`ConstructionAsset::foundation_y_m`])
//! sits below zero as a skirt into a slope, as the graybox box's `min.y` did.
//!
//! # Elements, chunks, joints, anchors
//!
//! - An [`Element`] is a construction's piece (a pier, a sill, a lintel, a gable, a wall plate, a
//!   roof plane, a tree's trunk): a convex solid, the hull of its corners. Intact, each is one
//!   convex collider; elements tile the construction and never overlap.
//! - A [`Chunk`] is the fracture bake's piece: a convex hull inside one element. An element's
//!   chunks are a contiguous run of [`ConstructionAsset::chunks`] ([`Element::chunks`]) and tile it.
//! - A component is a part the generator builds as one (a brick wall, a wall plate, a roof plane):
//!   each element names its own ([`Element::component`]).
//! - A [`Joint`] is a contact face between two chunks: its area, centroid and the normal from the
//!   first chunk into the second (NVIDIA Blast's bond), and what holds it ([`Bond`]): inside one
//!   component its own seam; between two, the attachment kind the construction's construction names
//!   for that pair (mortared, nailed, resting), from the shared list
//!   ([`super::attachments`]), which holds every joint strength. An [`Anchor`] is a chunk face on
//!   the foundation plane (Blast's world bond), held by the attachment it names. Its component
//!   owns one anchored footprint.
//!
//! The simulation reads chunks and joints only; it never reads how the bake chose the cuts.
//!
//! [`ConstructionAsset::check`] is the format's contract: every coordinate finite, every index in
//! range, every element's hull of four corners or more, every element's chunk run contiguous and
//! covering, every joint and anchor of positive area, every joint normal unit, every attachment
//! kind named once, every anchor on the foundation plane. The game refuses a file that breaks it,
//! one whose element or chunk hull bounds no solid, and one naming a material or an attachment kind
//! its registries do not hold (ADR-0011).

use bevy::math::Vec3;
use serde::{Deserialize, Serialize};

/// The format revision this build reads and writes. A file of another revision is refused.
pub const FORMAT: u32 = 8;

/// How far a vertex, a centroid or a plane may sit from where the contract places it, metres:
/// the f32 rounding of a construction-sized coordinate, many times over, and far below any modelled
/// feature (a brick course is 75 mm).
pub const PLANE_TOLERANCE_M: f32 = 1.0e-4;

/// One construction (module doc).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConstructionAsset {
    /// The asset id: the folder and the file stem.
    pub id: String,
    /// [`FORMAT`].
    pub format: u32,
    /// The foundation plane's height in the construction frame, metres. Every anchor lies on it.
    pub foundation_y_m: f32,
    /// The substance keys (`assets/materials/materials.ron`) elements and chunks name, by index.
    pub materials: Vec<String>,
    /// The attachment kinds (`assets/materials/attachments.ron`) the bonds between components
    /// and anchors name, by index ([`Bond::Attached`], [`Anchor::attachment`]).
    pub attachments: Vec<String>,
    /// The components' names, unique, by index ([`Element::component`]).
    pub components: Vec<String>,
    pub elements: Vec<Element>,
    pub chunks: Vec<Chunk>,
    /// Ordered by `(a, b)`, each pair once.
    pub joints: Vec<Joint>,
    /// Ordered by chunk, each chunk once.
    pub anchors: Vec<Anchor>,
}

/// The authored surface frame: upright image mapping, or grain in the construction frame.
/// Grain is a unit direction; its projection onto each face sets that face's tangent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SurfaceFrame {
    Upright,
    Grain([f32; 3]),
}
impl SurfaceFrame {
    pub fn is_valid(self) -> bool {
        match self {
            Self::Upright => true,
            Self::Grain(axis) => {
                let axis = Vec3::from_array(axis);
                axis.is_finite() && (axis.length() - 1.0).abs() <= PLANE_TOLERANCE_M
            }
        }
    }
}

/// One generator terminal (module doc): the convex hull of its corners.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Element {
    /// The authored element path, unique within the construction.
    pub name: String,
    /// A part label authored by this construction, shared by elements its joins group together.
    pub part: String,
    /// Index into [`ConstructionAsset::components`]: the component it is part of.
    pub component: u16,
    /// Index into [`ConstructionAsset::materials`].
    pub material: u8,
    pub surface_frame: SurfaceFrame,
    /// The convex hull's corners, construction frame.
    pub hull: Vec<[f32; 3]>,
    /// This element's chunks: `chunks[first..end]` of [`ConstructionAsset::chunks`].
    pub chunks: [u32; 2],
}

impl Element {
    /// The hull's corners, construction frame.
    pub fn points(&self) -> Vec<Vec3> {
        self.hull.iter().map(|p| Vec3::from_array(*p)).collect()
    }
}

/// One fracture piece (module doc).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    /// Index into [`ConstructionAsset::elements`]: the element it lies in.
    pub element: u16,
    /// Index into [`ConstructionAsset::materials`]: its element's.
    pub material: u8,
    /// The convex hull's corners, construction frame.
    pub hull: Vec<[f32; 3]>,
    pub volume_m3: f32,
    /// Construction frame.
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
    /// The shared face's centroid, construction frame.
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
    /// Between two components, held as the attachment kind [`ConstructionAsset::attachments`]`[_]`
    /// holds (a wall's corner mortared, a roof plane nailed to its plate, a roof plane resting on
    /// a gable's rake).
    Attached(u8),
}

/// One chunk face on the foundation plane: the ground holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub chunk: u32,
    /// Index into [`ConstructionAsset::attachments`]. All base faces of one component name the
    /// same kind and share its footprint budget, using that component's total mass.
    pub attachment: u8,
    /// The face's area on the plane, m².
    pub area_m2: f32,
    /// The face's centroid, on the foundation plane.
    pub centroid: [f32; 3],
}

impl ConstructionAsset {
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
        if !self.foundation_y_m.is_finite() {
            return Err(format!(
                "foundation_y_m {} is not finite; rebake the construction",
                self.foundation_y_m
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
        if self.attachments.len() > usize::from(u8::MAX) + 1 {
            return Err(format!("{} attachment kinds", self.attachments.len()));
        }
        for (k, name) in self.components.iter().enumerate() {
            if self.components[..k].contains(name) {
                return Err(format!("component {name} is named twice"));
            }
        }
        for (k, kind) in self.attachments.iter().enumerate() {
            if self.attachments[..k].contains(kind) {
                return Err(format!("attachment kind {kind:?} is named twice"));
            }
        }
        let finite = |values: &[f32]| values.iter().all(|v| v.is_finite());
        // NaN fails this comparison, so a NaN normal is refused with the rest.
        let unit = |axis: Vec3| (axis.length() - 1.0).abs() <= PLANE_TOLERANCE_M;
        let material = |index: u8, what: &str| {
            (usize::from(index) < self.materials.len())
                .then_some(())
                .ok_or_else(|| format!("{what} names material {index}, past the table"))
        };
        let mut next = 0u32;
        for (index, element) in self.elements.iter().enumerate() {
            let name = &element.name;
            if element.part.trim().is_empty() {
                return Err(format!(
                    "element {name}.part is blank; author its construction part name and rebake"
                ));
            }
            material(element.material, name)?;
            if usize::from(element.component) >= self.components.len() {
                return Err(format!(
                    "element {name} names component {}, past the table",
                    element.component
                ));
            }
            if !element.surface_frame.is_valid() {
                return Err(format!(
                    "element {name}'s surface_frame is no finite unit grain; author Upright or a unit Grain direction and rebake the construction"
                ));
            }
            if element.hull.len() < 4 || !element.hull.iter().all(|p| finite(p)) {
                return Err(format!(
                    "element {name}'s hull has {} corners, or one not finite: a solid needs four \
                     finite corners or more; rebake the construction",
                    element.hull.len()
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
            if !chunk.hull.iter().all(|p| finite(p)) || !finite(&chunk.centroid) {
                return Err(format!(
                    "chunk {index}'s hull or centroid is not finite; rebake the construction"
                ));
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
            if !finite(&joint.normal) || !unit(Vec3::from_array(joint.normal)) {
                return Err(format!(
                    "joint {pair:?}'s normal is not unit; rebake the construction"
                ));
            }
            if !finite(&joint.centroid) {
                return Err(format!(
                    "joint {pair:?}'s centroid is not finite; rebake the construction"
                ));
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
                Bond::Attached(_) if inside => {
                    return Err(format!("joint {pair:?} is a bond inside one component"));
                }
                Bond::Attached(index) if usize::from(index) >= self.attachments.len() => {
                    return Err(format!(
                        "joint {pair:?} names attachment kind {index}, past the table"
                    ));
                }
                _ => {}
            }
            last = Some(pair);
        }
        self.check_anchors(count)
    }

    /// Foundation faces are ordered, on their plane, and declare one attachment per footprint.
    fn check_anchors(&self, count: u32) -> Result<(), String> {
        let mut last: Option<u32> = None;
        let mut footprint_kinds = std::collections::BTreeMap::new();
        for anchor in &self.anchors {
            if anchor.chunk >= count || last.is_some_and(|last| last >= anchor.chunk) {
                return Err(format!(
                    "anchor on chunk {} is out of order or out of range",
                    anchor.chunk
                ));
            }
            if usize::from(anchor.attachment) >= self.attachments.len() {
                return Err(format!(
                    "anchor on chunk {} names attachment kind {}, past the table; rebake with a declared attachment",
                    anchor.chunk, anchor.attachment
                ));
            }
            let component =
                self.elements[usize::from(self.chunks[anchor.chunk as usize].element)].component;
            if footprint_kinds
                .insert(component, anchor.attachment)
                .is_some_and(|previous| previous != anchor.attachment)
            {
                return Err(format!(
                    "component {component}'s anchors name different attachments; author one kind for its footprint"
                ));
            }
            if !(anchor.area_m2.is_finite() && anchor.area_m2 > 0.0) {
                return Err(format!("anchor on chunk {} has no area", anchor.chunk));
            }
            if !anchor
                .centroid
                .iter()
                .all(|coordinate| coordinate.is_finite())
            {
                return Err(format!(
                    "anchor on chunk {}'s centroid is not finite; rebake the construction",
                    anchor.chunk
                ));
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
