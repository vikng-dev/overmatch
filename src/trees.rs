//! Trees in the game's damage model and environment destruction (game state only): a hard body of
//! segments, each with its own HP pool, a structure the walls' laws read, and a state. Game rounds
//! dose it, bodies load it, and it falls at a segment.
//!
//! A TREE IS DATA (ADR-0034). `assets/trees/<id>/<id>.tree.ron` ([`TreeAsset`]) is the trunk as a
//! chain of tapered segments, each boundary's radius stored once — every segment's base radius,
//! then the tip's — so adjacent segments meet by construction and a segment's top radius is read
//! through [`TreeKind::r_top_m`]. TREES ARE JUST WOOD: `assets/trees/wood.ron` ([`Wood`]) is what
//! the sim reads of every tree — the wood substance and the HP per square metre of a segment's
//! cross-section, and per species the grain, the green density and the root plate the trunk is
//! built from as a structure, its root plate a kind of `assets/materials/attachments.ron` — and
//! `assets/trees/species.ron` only names the species an asset may claim (the generator's look and
//! ranges; the sim reads no value of a row). They are read once at startup into the map's
//! [`TreeLibrary`] and validated there; every number the sim uses comes from them, the glb is the
//! view (ADR-0014).
//!
//! THE BODY. One static root per tree, carrying the state, and one child per segment
//! ([`TreeSegment`]): its own convex frustum collider (two 12-gon rings) on `[Terrain, Armor]`
//! with filters `ALL`, its own [`BallisticVolume`] of the wood and its own HP pool
//! ([`TreeBody::segment_hp`]: the wood's HP per square metre × the segment's cross-section at
//! mid-height), charged the way a module's pool is. Hulls contact a segment; the union walk slows
//! the round by chord × factor and doses each segment the energy the round lost at that segment's
//! depths, so a dose charges only the segments it crosses. Never a compound: the walk's convex
//! collector reads one solid per collider. The instance's uniform scale is applied to the
//! dimensions at spawn and the root's own scale is one.
//!
//! THE TRUNK IS A STRUCTURE ([`TreeBody::structure`]), one fracturing component as a brick wall
//! is: its segments chunks of one wood, joined by the wood's own along-grain seam, the trunk held
//! to the ground by its species' root plate, an attachment kind of the shared list. Its
//! structural state ([`TrunkState`]) is the one definition of where it breaks: any body's contact
//! loads it through the walls' laws, read from the solver's transfers ([`fell_on_loads`],
//! [`load_trunk`]), so the cut emerges where bending stress beats a section, or at the root plate
//! when that is weaker; a segment's emptied pool opens the joint at its base.
//!
//! The state records only the cut and its direction. The freed segments belong to a dynamic
//! body ([`TreePiece`]), launched through [`crate::debris::launch`] and simulated only by the
//! authority. Each piece retains its construction and structural state for subsequent loads.
//! Replicas partition segments by sorted carrier cuts; every segment retains its HP pool when
//! it changes body. See the construction design's "Trees are structures by the same laws".

use crate::construction::fracture::GroupLoads;
use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::PI;
use std::path::Path;
use std::sync::Arc;

use avian3d::prelude::{
    Collider, CollisionLayers, Forces, LayerMask, ReadRigidBodyForces, RigidBody,
    WriteRigidBodyForces,
};
use avian3d::schedule::PhysicsSystems;
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::prelude::*;

use crate::ClientReplica;
use crate::Layer;
use crate::ballistics::{BallisticVolume, ComponentHealth, PoolEmptied, VolumeMaterial};
use crate::construction::attachments::{ATTACHMENTS_PATH, Attachments, RootPlate};
use crate::construction::fracture::{
    self, Bond, Chunk, Joint, Load, Material, Origin, Seam, SectionCache, Shear, Side, State,
    Structure,
};
use crate::contact_transfer::ContactTransfers;
#[cfg(test)]
use crate::physical_laws::gravity_mps2;
use crate::provenance::Sourced;
use crate::scatter::MapInstance;
use crate::substances::SubstanceRegistry;

mod motion;
pub(crate) mod view;
pub(crate) use motion::{
    LooseTrunk, TreeParts, TreePiece, TrunkLoads, place_colliders, refit_colliders,
};

/// Directory under the asset root holding the species table, the wood row and one folder per tree
/// asset.
const TREES_DIR: &str = "trees";

/// The species table's file name inside [`TREES_DIR`].
const SPECIES_FILE: &str = "species.ron";

/// The wood row's file name inside [`TREES_DIR`].
const WOOD_FILE: &str = "wood.ron";

/// How far below 1 an instance's up vector's Y may fall before the instance counts as tilted:
/// Resolution: float rounding of a pure yaw (a few ulps), never a real lean. The standing
/// strength law reads gravity along the trunk's upright prefix sums.
const UPRIGHT_EPSILON: f32 = 1e-6;

/// Most segments a trunk may have: `cut` travels as one byte, and the wire word reserves
/// `cut + 1 == 0` for a standing tree.
const MAX_SEGMENTS: usize = 255;

/// The fall azimuth's resolution: bins round the compass, from `+X` toward `+Z`.
pub(crate) const AZIMUTH_BINS: u32 = 256;

/// One trunk segment: a frustum `height_m` tall with `r_bottom_m` at its base, metres at asset
/// scale 1. Its top radius is the next segment's `r_bottom_m`, the last segment's the asset's
/// `r_tip_m` ([`TreeKind::r_top_m`]).
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Segment {
    pub(crate) height_m: f32,
    pub(crate) r_bottom_m: f32,
}

/// `<id>.tree.ron`: the trunk, base to tip, the radius at its tip, and the species it belongs to.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TreeAsset {
    pub(crate) species: String,
    pub(crate) segments: Vec<Segment>,
    /// The last segment's top radius, metres at asset scale 1.
    pub(crate) r_tip_m: f32,
}

/// The species table as the sim reads it: the species an asset may name. A row's values are the
/// generator's (look and ranges) and are not read here.
type SpeciesTable = BTreeMap<String, serde::de::IgnoredAny>;

/// `wood.ron`: the trunk's substance and HP pools, shared, and every species' structure facts.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Wood {
    /// The trunk's substance, a hard key of the substance registry that states bulk facts.
    substance: String,
    /// A segment's HP pool per square metre of its mid-height cross-section.
    hp_per_m2: Sourced<f32>,
    /// Every declared species has grain, fresh-mass and root-plate facts, checked before any
    /// instance spawns.
    structure: StructureRows,
}

/// A species' row as `wood.ron` authors it ([`StructuralWood`] once resolved).
#[derive(serde::Deserialize)]
#[serde(rename = "StructuralWood", deny_unknown_fields)]
struct StructuralRow {
    along_grain_rupture_pa: Sourced<f32>,
    along_grain_fracture_j_per_m2: Sourced<f32>,
    fresh_density_kg_per_m3: Sourced<f32>,
    anchor: String,
}

/// A species' grain, fresh-mass and root-plate facts: what its trunk is built from as a
/// structure ([`TreeBody::structure`]).
#[derive(Debug, Clone, PartialEq)]
struct StructuralWood {
    /// Modulus of rupture under bending, longitudinal extreme-fibre stress, Pa.
    along_grain_rupture_pa: f32,
    /// Mode-I opening work per section area, J/m²: tension along the grain, breaking across
    /// fibres (LR/LT), not the RL/TL splitting energy of a construction's timber seam.
    along_grain_fracture_j_per_m2: f32,
    /// The green wood's density, kg/m³: a segment's mass per volume, and the fresh stem mass a
    /// root plate's coefficient multiplies.
    fresh_density_kg_per_m3: f32,
    /// The root plate the trunk holds to the ground by: the `RootPlate` kind of the shared
    /// attachment list the row's `anchor` names, which binds its species, soil and rooting depth.
    root_plate: RootPlate,
}

impl StructuralWood {
    /// Resolve row `row` against the shipped attachment list: its three numbers positive and
    /// finite, its anchor a root plate the list holds.
    fn resolve(row: &str, authored: StructuralRow) -> Result<Self, String> {
        let StructuralRow {
            along_grain_rupture_pa: Sourced(rupture),
            along_grain_fracture_j_per_m2: Sourced(fracture),
            fresh_density_kg_per_m3: Sourced(density),
            anchor,
        } = authored;
        for (field, value) in [
            ("along_grain_rupture_pa", rupture),
            ("along_grain_fracture_j_per_m2", fracture),
            ("fresh_density_kg_per_m3", density),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "structure row {row:?}: {field} is {value}; author a positive finite value \
                     in its stated SI units with its material-property source"
                ));
            }
        }
        let root_plate = Attachments::shipped_static()
            .root_plate(&anchor)
            .map_err(|err| format!("structure row {row:?}: anchor: {err}"))?;
        Ok(Self {
            along_grain_rupture_pa: rupture,
            along_grain_fracture_j_per_m2: fracture,
            fresh_density_kg_per_m3: density,
            root_plate,
        })
    }
}

/// A wood table that preserves the row name in parser refusals and refuses duplicate rows.
#[derive(Debug, Clone, PartialEq)]
struct StructureRows(BTreeMap<String, StructuralWood>);

impl<'de> serde::Deserialize<'de> for StructureRows {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowsVisitor;
        impl<'de> serde::de::Visitor<'de> for RowsVisitor {
            type Value = StructureRows;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a structural wood row for each declared tree species")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                use serde::de::Error;
                let mut rows = BTreeMap::new();
                while let Some(name) = access.next_key::<String>()? {
                    let row: StructuralRow = access.next_value().map_err(|err| {
                        A::Error::custom(format!(
                            "structure row {name:?}: {err}; author along_grain_rupture_pa (Pa), \
                             along_grain_fracture_j_per_m2 (J/m²) and fresh_density_kg_per_m3 \
                             (kg/m³), each with provenance, and the anchor, a root plate of \
                             {ATTACHMENTS_PATH}"
                        ))
                    })?;
                    let row = StructuralWood::resolve(&name, row).map_err(A::Error::custom)?;
                    if rows.insert(name.clone(), row).is_some() {
                        return Err(A::Error::custom(format!(
                            "structure row {name:?} is declared twice; keep one sourced row per species"
                        )));
                    }
                }
                Ok(StructureRows(rows))
            }
        }
        deserializer.deserialize_map(RowsVisitor)
    }
}

impl Wood {
    /// Parse once at the load edge. All declared species need structural facts, including ones
    /// not placed by the selected map; no later instance may acquire an invented wood default.
    fn parse(path: &Path, text: &str, species: &SpeciesTable) -> Result<Self, String> {
        let load = || {
            let wood: Self = ron::from_str(text).map_err(|err| {
                format!("{err}; author the required sourced fields in the wood table")
            })?;
            for name in species.keys() {
                if !wood.structure.0.contains_key(name) {
                    return Err(format!(
                        "structure row {name:?} is missing; author its along_grain_rupture_pa, \
                         along_grain_fracture_j_per_m2 and fresh_density_kg_per_m3 with their \
                         sources, and its anchor"
                    ));
                }
            }
            for name in wood.structure.0.keys() {
                if !species.contains_key(name) {
                    return Err(format!(
                        "structure row {name:?} has no species in {SPECIES_FILE}; declare the \
                         species there or remove its unused wood row"
                    ));
                }
            }
            Ok(wood)
        };
        load().map_err(|err| format!("trees: {}: {err}", path.display()))
    }
}

/// One tree asset resolved against the species table, the wood row and the substance registry,
/// validated: what every instance of it spawns from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TreeKind {
    pub(crate) id: String,
    pub(crate) segments: Vec<Segment>,
    /// The last segment's top radius at asset scale 1; read a segment's top through
    /// [`Self::r_top_m`].
    r_tip_m: f32,
    pub(crate) material: VolumeMaterial,
    pub(crate) substance: String,
    /// The friction its segments carry: the substance's (`substances::HardSubstance::contact_friction`).
    pub(crate) friction: avian3d::prelude::Friction,
    /// HP points per square metre of a segment's mid-height cross-section
    /// ([`TreeBody::segment_hp`]).
    pub(crate) hp_per_m2: f32,
    /// The wood's values as the structure laws read a chunk of it: the substance's bulk facts.
    pub(crate) masonry: Material,
    /// The seam between two segments of the trunk: the wood along its grain, its modulus of
    /// rupture and its opening work across the fibres. It answers by the opening law alone: no
    /// sourced shear pair holds a section of a trunk, so no punch ring forms in one.
    pub(crate) grain: Seam,
    /// The green wood's density, kg/m³ (the species' `fresh_density_kg_per_m3`).
    pub(crate) fresh_density: f32,
    /// The root plate the trunk holds to the ground by (the species' `anchor`).
    pub(crate) root_plate: RootPlate,
}

impl TreeKind {
    /// Resolve and validate one asset. Every refusal is a broken ship (ADR-0011): the message
    /// names what, the caller names the file.
    fn resolve(
        id: &str,
        asset: TreeAsset,
        table: &SpeciesTable,
        wood: &Wood,
        registry: &SubstanceRegistry,
    ) -> Result<Self, String> {
        let TreeAsset {
            species,
            segments,
            r_tip_m,
        } = asset;
        if segments.is_empty() || segments.len() > MAX_SEGMENTS {
            return Err(format!(
                "{} segments; a trunk has 1 to {MAX_SEGMENTS}",
                segments.len()
            ));
        }
        let positive = |v: f32| v.is_finite() && v > 0.0;
        for (k, segment) in segments.iter().enumerate() {
            if !(positive(segment.height_m) && positive(segment.r_bottom_m)) {
                return Err(format!(
                    "segment {k} has a non-positive dimension: {segment:?}"
                ));
            }
        }
        if !positive(r_tip_m) {
            return Err(format!("r_tip_m {r_tip_m} is not a positive finite radius"));
        }
        if !table.contains_key(&species) {
            return Err(format!("species {species:?} is not in {SPECIES_FILE}"));
        }
        let material = VolumeMaterial::of(registry, &wood.substance)
            .map_err(|err| format!("{WOOD_FILE} substance: {err}"))?;
        let friction = registry
            .contact_friction(&wood.substance)
            .map_err(|err| format!("{WOOD_FILE} substance: {err}"))?;
        let masonry = Material::of(registry, &wood.substance)
            .map_err(|err| format!("{WOOD_FILE} substance: {err}"))?
            .ok_or_else(|| {
                format!(
                    "{WOOD_FILE} substance {:?} states no bulk facts in materials.ron: a trunk's \
                     segments are chunks of it, so name a substance that states them",
                    wood.substance
                )
            })?;
        let structure =
            wood.structure.0.get(&species).ok_or_else(|| {
                format!("species {species:?} has no structure row in {WOOD_FILE}")
            })?;
        let hp_per_m2 = wood.hp_per_m2.0;
        if !positive(hp_per_m2) {
            return Err(format!("{WOOD_FILE} hp_per_m2 {hp_per_m2} is not positive"));
        }
        Ok(Self {
            id: id.to_owned(),
            segments,
            r_tip_m,
            material,
            substance: wood.substance.clone(),
            friction,
            hp_per_m2,
            masonry,
            grain: Seam {
                tensile: structure.along_grain_rupture_pa,
                fracture_energy: structure.along_grain_fracture_j_per_m2,
                shear: Shear::OpeningOnly,
            },
            fresh_density: structure.fresh_density_kg_per_m3,
            root_plate: structure.root_plate,
        })
    }

    /// Segment `k`'s top radius at asset scale 1: the boundary it shares with segment `k + 1`,
    /// whose `r_bottom_m` stores it, or the tip's `r_tip_m` above the last segment. The one
    /// reading of a top radius, so a segment's top and its upper neighbour's base are one float.
    pub(crate) fn r_top_m(&self, k: usize) -> f32 {
        self.segments
            .get(k + 1)
            .map_or(self.r_tip_m, |next| next.r_bottom_m)
    }
}

/// Every tree asset the selected map places, resolved and validated, keyed by asset id.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct TreeLibrary {
    kinds: BTreeMap<String, Arc<TreeKind>>,
    /// The texts the kinds were read from — the species table, the wood row, then each asset in id
    /// order — for the map's content digest.
    texts: Vec<String>,
}

impl TreeLibrary {
    /// Read and validate the assets `ids` name under `root`. Nothing is read when `ids` is empty;
    /// otherwise every file must exist and hold (ADR-0011): a map that places a tree this build
    /// cannot read is present-but-broken.
    pub(crate) fn load<'a>(root: &Path, ids: impl IntoIterator<Item = &'a str>) -> Self {
        crate::provenance::with_shipped_policy(|| Self::load_files(root, ids))
    }

    // Shared I/O and validation. Tests writing their own inline fixtures to a temporary directory
    // enter here under test admission; shipped callers enter through `load`'s refusing policy.
    fn load_files<'a>(root: &Path, ids: impl IntoIterator<Item = &'a str>) -> Self {
        let ids: BTreeSet<&str> = ids.into_iter().collect();
        if ids.is_empty() {
            return Self::default();
        }
        let read = |path: &Path| {
            std::fs::read_to_string(path)
                .unwrap_or_else(|err| panic!("trees: {} cannot be read: {err}", path.display()))
        };
        let species_path = root.join(TREES_DIR).join(SPECIES_FILE);
        let species_text = read(&species_path);
        let table: SpeciesTable = ron::from_str(&species_text).unwrap_or_else(|err| {
            panic!("trees: {} failed to parse: {err}", species_path.display())
        });
        let wood_path = root.join(TREES_DIR).join(WOOD_FILE);
        let wood_text = read(&wood_path);
        let wood =
            Wood::parse(&wood_path, &wood_text, &table).unwrap_or_else(|err| panic!("{err}"));
        let registry = SubstanceRegistry::shipped_static();
        let mut texts = vec![species_text, wood_text];
        let mut kinds = BTreeMap::new();
        for id in ids {
            let path = root.join(TREES_DIR).join(id).join(format!("{id}.tree.ron"));
            let text = read(&path);
            let asset: TreeAsset = ron::from_str(&text)
                .unwrap_or_else(|err| panic!("trees: {} failed to parse: {err}", path.display()));
            let kind = TreeKind::resolve(id, asset, &table, &wood, registry)
                .unwrap_or_else(|err| panic!("trees: {}: {err}", path.display()));
            texts.push(text);
            kinds.insert(id.to_owned(), Arc::new(kind));
        }
        Self { kinds, texts }
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Arc<TreeKind>> {
        self.kinds.get(id)
    }

    /// The bytes of every file the library was read from, in a fixed order.
    pub(crate) fn texts(&self) -> impl Iterator<Item = &[u8]> {
        self.texts.iter().map(String::as_bytes)
    }
}

/// The tree's state, authority-set, peer-derived (module doc).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TreeState {
    Standing,
    /// Broken at the bottom of segment `cut`, fallen toward azimuth bin `dir`.
    Down {
        cut: u8,
        dir: u8,
    },
}

/// A malformed peer tree word, refused once at the topology receive boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TreeWordError {
    StandingDirectionBits(u16),
    CutOutside {
        word: u16,
        cut: u16,
        segments: usize,
    },
}
impl std::fmt::Display for TreeWordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StandingDirectionBits(word) => {
                write!(f, "tree word {word:#06x}: standing with direction bits")
            }
            Self::CutOutside {
                word,
                cut,
                segments,
            } => write!(
                f,
                "tree word {word:#06x}: cut {cut} on a tree of {segments} segments"
            ),
        }
    }
}
impl std::error::Error for TreeWordError {}

impl TreeState {
    /// The state's wire word: `Standing` is `0`, `Down { cut, dir }` is `((cut + 1) << 8) | dir`.
    /// The `+ 1` keeps every `Down` word non-zero, so the high byte alone tells the two apart and
    /// `Standing` has exactly one encoding.
    pub(crate) fn encode(self) -> u16 {
        match self {
            Self::Standing => 0,
            Self::Down { cut, dir } => {
                assert!(
                    usize::from(cut) < MAX_SEGMENTS,
                    "tree cut {cut} has no word: the high byte carries cut + 1"
                );
                ((u16::from(cut) + 1) << 8) | u16::from(dir)
            }
        }
    }

    /// The inverse of [`Self::encode`] for a tree of `segments` segments; a word that no state
    /// of that tree encodes to (`Standing` with direction bits, a cut at or past the last segment)
    /// is refused rather than clamped, so a peer never derives a pose the authority did not set.
    pub(crate) fn decode(word: u16, segments: usize) -> Result<Self, TreeWordError> {
        let (high, dir) = (word >> 8, (word & 0xFF) as u8);
        if high == 0 {
            return if word == 0 {
                Ok(Self::Standing)
            } else {
                Err(TreeWordError::StandingDirectionBits(word))
            };
        }
        let cut = high - 1;
        if usize::from(cut) >= segments {
            return Err(TreeWordError::CutOutside {
                word,
                cut,
                segments,
            });
        }
        Ok(Self::Down {
            cut: cut as u8,
            dir,
        })
    }
}

/// A tree's body as spawned: its kind at the instance's uniform scale, every dimension here in
/// world metres. A structure, whose contacts the solver's reading keeps.
#[derive(Component, Debug, Clone)]
#[require(crate::contact_transfer::StructureBody)]
pub(crate) struct TreeBody {
    pub(crate) kind: Arc<TreeKind>,
    /// Segment bottoms above the base, then the tip: `prefix[k]` is segment `k`'s bottom,
    /// `prefix[n]` the tip.
    pub(crate) prefix: Vec<f32>,
    /// The instance's uniform scale, asset metres to world metres.
    pub(crate) scale: f32,
    /// The trunk as the structure laws read it ([`Self::structure_of`]), in the tree's frame:
    /// its base at the origin, `+Y` up the trunk, world metres.
    pub(crate) structure: Structure,
}

impl TreeBody {
    pub(crate) fn new(kind: Arc<TreeKind>, scale: f32) -> Self {
        let mut prefix = Vec::with_capacity(kind.segments.len() + 1);
        let mut bottom = 0.0;
        for segment in &kind.segments {
            prefix.push(bottom);
            bottom += segment.height_m * scale;
        }
        prefix.push(bottom);
        let structure = Self::structure_of(&kind, &prefix, scale);
        Self {
            kind,
            prefix,
            scale,
            structure,
        }
    }

    /// THE TRUNK AS A STRUCTURE, one fracturing component as a wall is, built from data at spawn
    /// (ADR-0014). Chunk `k` is segment `k`: its frustum's volume, centroid and mass at the wood's
    /// fresh density, of the wood's material. Joint `k` holds segment `k` at its base: joint 0 is
    /// the anchor, the root plate holding the trunk to the ground; joint `k ≥ 1` is the wood's own
    /// along-grain seam between segments `k − 1` and `k` ([`TreeKind::grain`]). Each is a disc of
    /// the trunk's cross-section there, `π r²`, so a section's Navier second moment is the disc's
    /// `π r⁴ / 4` and its peak bending stress `32 M / π d³`, beam bending exactly (DERIVED,
    /// `fracture::weigh`): a trunk breaks wherever `M(z) / S(z)` beats its seam, no segment
    /// boundary a planted weak joint.
    ///
    /// Root-plate law inputs come from the shared attachment boundary (`RootPlate::seam`).
    fn structure_of(kind: &TreeKind, prefix: &[f32], scale: f32) -> Structure {
        let radius = |k: usize| {
            kind.segments
                .get(k)
                .map_or(kind.r_tip_m, |segment| segment.r_bottom_m)
                * scale
        };
        let chunks: Vec<Chunk> = (0..kind.segments.len())
            .map(|k| {
                let (r0, r1, h) = (radius(k), radius(k + 1), prefix[k + 1] - prefix[k]);
                let squares = r0 * r0 + r0 * r1 + r1 * r1;
                let volume = PI * h * squares / 3.0;
                // A frustum's centroid above its base: h (r0² + 2 r0 r1 + 3 r1²) / 4 (r0² + r0 r1
                // + r1²) (DERIVED by integrating π r(z)² z along the linear taper).
                let rise = h * (r0 * r0 + 2.0 * r0 * r1 + 3.0 * r1 * r1) / (4.0 * squares);
                Chunk {
                    volume,
                    centroid: Vec3::Y * (prefix[k] + rise),
                    mass: volume * kind.fresh_density,
                    material: kind.masonry,
                    buried: false,
                }
            })
            .collect();
        let mass: f32 = chunks.iter().map(|chunk| chunk.mass).sum();
        let area = PI * radius(0) * radius(0);
        let anchor = kind.root_plate.seam(area, mass).expect(
            "a root plate holds a positive finite moment beyond its trunk's own weight pressure",
        );
        let disc = |k: usize, b: Side, seam: Seam| Joint {
            a: k,
            b,
            area: PI * radius(k) * radius(k),
            centroid: Vec3::Y * prefix[k],
            normal: Vec3::Y,
            bond: Bond::Bonded(seam),
        };
        let joints = std::iter::once(disc(0, Side::Ground, anchor))
            .chain((1..chunks.len()).map(|k| Joint {
                a: k - 1,
                ..disc(k, Side::Chunk(k), kind.grain)
            }))
            .collect();
        Structure { chunks, joints }
    }

    /// Segment `k`'s HP pool, in whole points: the wood's HP per square metre times the
    /// segment's cross-section at mid-height, `π r²` for `r` the mean of its two end radii at this
    /// instance's scale (the taper is linear along a segment, so that is its radius halfway up).
    /// Rounded to the nearest point (f32 multiplies and `roundf` in one fixed order, the same on
    /// every target), at least one point (a pool of none would never empty) and at most
    /// [`crate::ballistics::MAX_POOL_POINTS`] (the most a pool holds). The area makes a pool grow
    /// with the square of the diameter while a line's dose grows with its chord, so a thinner
    /// segment empties in fewer equal shots.
    pub(crate) fn segment_hp(&self, k: usize) -> u32 {
        let r = 0.5 * (self.kind.segments[k].r_bottom_m + self.kind.r_top_m(k)) * self.scale;
        (libm::roundf(self.kind.hp_per_m2 * PI * r * r) as u32)
            .clamp(1, crate::ballistics::MAX_POOL_POINTS)
    }
}

/// A segment's stable construction index; its collider is a child of its current owner.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TreeSegment(pub(crate) u8);

/// The authority's structural transitions, before collider changes and launch finalization.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TreeFallSet;

/// The game's damage model: collider topology changes only after all walks.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TreeDerived;

/// The horizontal unit direction of azimuth bin `dir`, from +X toward +Z.
pub(crate) fn azimuth(dir: u8) -> (f32, f32) {
    let theta = f32::from(dir) / AZIMUTH_BINS as f32 * std::f32::consts::TAU;
    (libm::cosf(theta), libm::sinf(theta))
}

/// Rebuild the stump and freed-body ownership, then refresh collider poses and query bounds.
pub(crate) fn derive_systems() -> ScheduleConfigs<ScheduleSystem> {
    motion::derive_systems()
}

/// A standing segment collides as terrain and is visible to the game's damage-model walk.
fn segment_layers() -> CollisionLayers {
    CollisionLayers::new([Layer::Terrain, Layer::Armor], LayerMask::ALL)
}

/// The 12-gon the frustum colliders are hulled from: exact unit-circle points at 30° steps, pinned
/// as literals so every peer builds the same solid.
const RING: [(f32, f32); 12] = [
    (1.0, 0.0),
    (0.866_025_4, 0.5),
    (0.5, 0.866_025_4),
    (0.0, 1.0),
    (-0.5, 0.866_025_4),
    (-0.866_025_4, 0.5),
    (-1.0, 0.0),
    (-0.866_025_4, -0.5),
    (-0.5, -0.866_025_4),
    (0.0, -1.0),
    (0.5, -0.866_025_4),
    (0.866_025_4, -0.5),
];

/// Segment `k`'s collider in its own frame: base ring at y = 0, top ring at the segment's height.
fn segment_collider(kind: &TreeKind, k: usize, scale: f32) -> Collider {
    let segment = &kind.segments[k];
    let (height, r_bottom, r_top) = (
        segment.height_m * scale,
        segment.r_bottom_m * scale,
        kind.r_top_m(k) * scale,
    );
    let points: Vec<Vec3> = RING
        .iter()
        .flat_map(|&(c, s)| {
            [
                Vec3::new(r_bottom * c, 0.0, r_bottom * s),
                Vec3::new(r_top * c, height, r_top * s),
            ]
        })
        .collect();
    Collider::convex_hull(points).expect("two rings of positive radius at distinct heights")
}

/// The same data-built segment geometry on a streamed replica.
pub(crate) fn replica_segment_collider(body: &TreeBody, k: usize) -> Collider {
    segment_collider(&body.kind, k, body.scale)
}

/// The azimuth bin of a horizontal `(x, z)` direction, [`AZIMUTH_BINS`] round from `+X` toward
/// `+Z`. A zero vector lands in bin 0. `libm`, so the bin is the same on every platform.
pub(crate) fn azimuth_bin(x: f32, z: f32) -> u8 {
    let turns = libm::atan2f(z, x) / std::f32::consts::TAU;
    // `turns` is in [-0.5, 0.5], so the rounded bin is in [-128, 128] and wraps once.
    let bin = libm::roundf(turns * AZIMUTH_BINS as f32) as i32;
    u8::try_from(bin.rem_euclid(AZIMUTH_BINS as i32)).expect("reduced modulo the bin count")
}

/// Spawn one tree of `kind` at `pose` — the ground pose: the authored XZ, the grid's Y, the
/// authored yaw and uniform scale. A tilted or non-uniformly scaled instance is refused (ADR-0011):
/// the standing strength law reads gravity along the trunk. Every segment's pool is built here, full, from
/// the data (ADR-0014). Returns the root; the body is whole without a view, which a composition
/// that draws attaches under the root itself.
pub(crate) fn spawn(
    commands: &mut Commands,
    kind: &Arc<TreeKind>,
    identity: MapInstance,
    pose: Transform,
) -> Entity {
    let scale = pose.scale.x;
    assert!(
        pose.scale.y == scale && pose.scale.z == scale && scale > 0.0,
        "trees: instance {} has scale {:?}; a tree scales uniformly",
        identity.id,
        pose.scale,
    );
    assert!(
        (pose.rotation * Vec3::Y).y >= 1.0 - UPRIGHT_EPSILON,
        "trees: instance {} is tilted ({:?}); a tree stands upright",
        identity.id,
        pose.rotation,
    );
    let body = TreeBody::new(kind.clone(), scale);
    let segments: Vec<(f32, u32, Collider)> = (0..kind.segments.len())
        .map(|k| {
            (
                body.prefix[k],
                body.segment_hp(k),
                segment_collider(kind, k, scale),
            )
        })
        .collect();
    let root = commands
        .spawn((
            Name::new(format!("tree {}", identity.id)),
            Transform {
                translation: pose.translation,
                rotation: pose.rotation,
                scale: Vec3::ONE,
            },
            RigidBody::Static,
            TreeState::Standing,
            TrunkState::new(&body.structure),
            identity,
            body,
        ))
        .id();
    let mut parts = Vec::with_capacity(segments.len());
    for (k, (bottom, hp, collider)) in segments.into_iter().enumerate() {
        let segment = commands
            .spawn((
                ChildOf(root),
                Transform::from_xyz(0.0, bottom, 0.0),
                collider,
                segment_layers(),
                kind.friction,
                BallisticVolume {
                    material: kind.material,
                    substance: kind.substance.clone(),
                },
                ComponentHealth::full(hp),
                TreeSegment(u8::try_from(k).expect("resolve bounded the segment count")),
            ))
            .id();
        parts.push(segment);
    }
    commands
        .entity(root)
        .insert((TreeParts(parts), TrunkLoads::default()));
    root
}

/// A tree's structural state, the authority's: its trunk's live state under the structure laws
/// (each joint's crack; [`TreeBody::structure`] is what they read) and the sections they cache.
/// Built whole at spawn from the body (ADR-0014). One definition of when a trunk breaks: a joint
/// opened, whoever opened it (a round's dose, a blast's, a body's contact), and the cut is the
/// lowest open joint ([`Self::cut`]).
#[derive(Component, Debug, Clone)]
pub(crate) struct TrunkState {
    state: State,
    sections: SectionCache,
    owned: std::ops::Range<usize>,
}

impl TrunkState {
    pub(crate) fn new(structure: &Structure) -> Self {
        Self {
            state: State::new(structure),
            sections: SectionCache::default(),
            owned: 0..structure.chunks.len(),
        }
    }

    /// Where the trunk broke: the lowest open joint, joint `k` holding segment `k` at its base
    /// ([`TreeBody::structure_of`]), so the piece from segment `k` up is what falls. `None` while
    /// every joint holds.
    pub(crate) fn cut(&self) -> Option<u8> {
        self.state
            .cracked
            .iter()
            .position(|&open| open)
            .map(|joint| {
                u8::try_from(joint)
                    .expect("resolve bounded the segment count, one joint per segment")
            })
    }
}

/// THE TRUNK MEETS ITS LOADS: `loads` (the tree's frame) through the structure laws a wall's
/// chunks meet (`fracture`), on `trunk`'s state: the blows' structural check, then strength under
/// gravity and the presses, from the joints the first opened. Returns the cut after them
/// ([`TrunkState::cut`]). Nothing of the tree's kind or of the loads' sources enters: the trunk's
/// construction and the loads alone decide where, and whether, it breaks.
pub(crate) fn load_trunk(
    body: &TreeBody,
    trunk: &mut TrunkState,
    loads: &[Load],
    spin: Vec3,
) -> Option<u8> {
    let structure = &body.structure;
    let opened: Vec<usize> = fracture::structural(
        structure,
        &mut trunk.state,
        GroupLoads::new(loads, spin),
        &mut trunk.sections,
    )
    .into_iter()
    .flat_map(|failure| failure.joints)
    .collect();
    fracture::weigh(
        structure,
        &mut trunk.state,
        &[],
        &opened,
        GroupLoads::new(loads, spin),
        &[],
        &mut trunk.sections,
    );
    trunk.cut()
}

/// The game's damage model: one structural cut, in the delivered load's direction.
fn fell(state: &mut TreeState, cut: u8, along: Vec3) {
    *state = TreeState::Down {
        cut,
        dir: azimuth_bin(along.x, along.z),
    };
}

/// The game's damage model: an emptied segment opens its base seam on its current owner,
/// standing or moving. A stump's lower cut also updates the map topology word.
pub(crate) fn fell_on_pool_emptied(
    emptied: On<PoolEmptied>,
    segments: Query<(&TreeSegment, &ChildOf)>,
    mut trees: Query<(Option<&mut TreeState>, &mut TrunkState, &Transform)>,
) {
    let Ok((&TreeSegment(segment), parent)) = segments.get(emptied.entity) else {
        return; // Other HP pools belong to other laws.
    };
    let (state, mut trunk, transform) = trees
        .get_mut(parent.parent())
        .expect("a trunk segment's current owner carries its structural state");
    trunk.state.cracked[usize::from(segment)] = true;
    if let Some(mut state) = state {
        let cut = trunk
            .cut()
            .expect("the emptied segment's base joint is open");
        if matches!(*state, TreeState::Standing)
            || matches!(*state, TreeState::Down { cut: previous, .. } if cut < previous)
        {
            fell(
                &mut state,
                cut,
                emptied.along.unwrap_or(transform.translation - emptied.at),
            );
        }
    }
}

/// AUTHORITY: queued field loads and each physics step's contacts, read once a step from the
/// solver's own transfers (`crate::contact_transfer`, the reading a wall's contacts come from),
/// each contact split at its body's rest speed into a blow and a press
/// ([`ContactTransfers::split`]), and every resting body's press; each point a load on the
/// segment its collider is, in the tree's frame. A tree whose trunk those loads break
/// ([`load_trunk`]) falls at its cut along their push. Field loads use the same batch; sleeping
/// presses join runs an awake contact or field starts. No body is read by its kind.
fn fell_on_loads(
    transfers: Res<ContactTransfers>,
    mut read: Local<u64>,
    segments: Query<(&TreeSegment, &ChildOf)>,
    mut trees: Query<(
        Entity,
        Option<&mut TreeState>,
        &TreeBody,
        &mut TrunkState,
        &mut TrunkLoads,
        Option<&TreePiece>,
        (&avian3d::prelude::Position, &avian3d::prelude::Rotation),
    )>,
    mut bodies: Query<Forces>,
) {
    if transfers.steps() == *read {
        return;
    }
    *read = transfers.steps();
    let mut per_tree: BTreeMap<Entity, Vec<Load>> = trees
        .iter_mut()
        .filter_map(|(root, _, _, _, mut pending, _, _)| {
            (!pending.0.is_empty()).then(|| (root, std::mem::take(&mut pending.0)))
        })
        .collect();
    let awake = transfers
        .split()
        .map(|(point, blow, press, mass)| (point, blow, press, mass, transfers.dt(), false));
    let resting = transfers.resting().iter().map(|rest| {
        (
            &rest.transfer,
            Vec3::ZERO,
            rest.press(),
            rest.transfer.effective_mass,
            rest.dt,
            true,
        )
    });
    for (transfer, blow, press, mass, step, asleep) in awake.chain(resting) {
        let Ok((&TreeSegment(segment), parent)) = segments.get(transfer.structure_collider) else {
            continue; // Constructions read their own contacts.
        };
        let root = parent.parent();
        let (_, _, body, _, _, piece, (position, rotation)) = trees
            .get_mut(root)
            .expect("a trunk segment's owner retains its construction and structural state");
        let frame = body.construction_frame(piece, position.0, rotation.0);
        let entry = if asleep {
            per_tree.get_mut(&root)
        } else {
            Some(per_tree.entry(root).or_default())
        };
        let Some(loads) = entry else {
            continue; // An unchanged sleeping press starts no assessment.
        };
        let into = frame.rotation.inverse();
        loads.push(Load {
            chunk: usize::from(segment),
            at: into * (transfer.point - frame.translation),
            energy: 0.0,
            crushed: 0.0,
            blow: into * blow,
            press: into * press,
            peak: None,
            origin: if piece.is_some() {
                Origin::Contact {
                    other: transfer.body,
                    step,
                }
            } else {
                Origin::Body {
                    body: transfer.body,
                    mass,
                    speed: transfer.closing_speed,
                    step,
                }
            },
        });
    }
    for (root, loads) in per_tree {
        let (_, state, body, mut trunk, mut pending, piece, (position, rotation)) = trees
            .get_mut(root)
            .expect("a loaded trunk survives through the same tick's laws");
        let frame = body.construction_frame(piece, position.0, rotation.0);
        let spin = piece.map_or(Vec3::ZERO, |_| {
            frame.rotation.inverse()
                * bodies
                    .get_mut(root)
                    .expect("a moving construction owner is a complete dynamic body")
                    .angular_velocity()
        });
        if let Some(TreeState::Down { cut, .. }) = state.as_deref() {
            trunk.state.cracked[usize::from(*cut)] = true;
        }
        let cut = load_trunk(body, &mut trunk, &loads, spin);
        if let Some(mut state) = state {
            if let Some(cut) = cut {
                if matches!(*state, TreeState::Standing)
                    || matches!(*state, TreeState::Down { cut: previous, .. } if cut < previous)
                {
                    let push = loads
                        .iter()
                        .map(|load| load.blow + load.press * transfers.dt())
                        .sum::<Vec3>();
                    fell(&mut state, cut, frame.rotation * push);
                }
                if usize::from(cut) < trunk.owned.end {
                    pending.0 = loads;
                }
            }
        } else {
            let mut physical = bodies
                .get_mut(root)
                .expect("a moving construction owner is a complete dynamic body");
            for load in loads
                .iter()
                .filter(|load| matches!(load.origin, Origin::Field))
            {
                physical.apply_linear_impulse_at_point(
                    frame.rotation * load.blow,
                    frame.translation + frame.rotation * load.at,
                );
            }
        }
    }
}

/// Authority transitions and shared free-body launch; replicas derive topology from the ledger.
pub(crate) fn sim_plugin(app: &mut App) {
    use crate::debris::launch::LaunchPlugin;
    if !app.is_plugin_added::<LaunchPlugin>() {
        app.add_plugins(LaunchPlugin);
    }
    crate::collider_rehome::plugin(app);
    app.add_observer(fell_on_pool_emptied)
        .add_observer(motion::read_shell)
        .configure_sets(
            FixedPostUpdate,
            TreeFallSet
                .after(PhysicsSystems::Writeback)
                .after(crate::state::SimPhase::ProjectileMarch),
        )
        .add_systems(
            FixedPostUpdate,
            (
                motion::reset_standing.before(PhysicsSystems::First),
                fell_on_loads.in_set(TreeFallSet),
                derive_systems().in_set(TreeDerived).after(TreeFallSet),
                (motion::split_moving, place_colliders, refit_colliders())
                    .chain()
                    .after(crate::debris::launch::LaunchSet)
                    .before(crate::state::SimPhase::Publish),
            )
                .run_if(not(resource_exists::<ClientReplica>)),
        );
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::terrain_grid::{FIXTURE_EXTENT, HeightGrid};
    use avian3d::prelude::{Position, Rotation, SimpleCollider};

    /// A three-segment fixture trunk (fixture data): 2 m segments, radii 0.3 → 0.2 → 0.1 → 0.05.
    pub(crate) fn fixture_asset() -> TreeAsset {
        TreeAsset {
            species: "fixture".to_owned(),
            segments: vec![
                Segment {
                    height_m: 2.0,
                    r_bottom_m: 0.3,
                },
                Segment {
                    height_m: 2.0,
                    r_bottom_m: 0.2,
                },
                Segment {
                    height_m: 2.0,
                    r_bottom_m: 0.1,
                },
            ],
            r_tip_m: 0.05,
        }
    }

    fn fixture_table() -> SpeciesTable {
        BTreeMap::from([("fixture".to_owned(), serde::de::IgnoredAny)])
    }

    /// The fixture wood (fixture data): the shipped Wood substance, 1000 HP per square metre, so
    /// the fixture trunk's segments hold 196, 71 and 18 points at scale 1 (DERIVED: `1000 π r²` at
    /// the mid-height radii 0.25, 0.15 and 0.075 m, rounded); a 40 MPa grain, 850 kg/m³ green and
    /// a 150 N·m/kg root plate.
    fn fixture_wood() -> Wood {
        Wood {
            substance: "Wood".to_owned(),
            hp_per_m2: Sourced(1000.0),
            structure: StructureRows(BTreeMap::from([(
                "fixture".to_owned(),
                fixture_structure(150.0),
            )])),
        }
    }

    /// The fixture species' structure facts (fixture data), its root plate `plate` N·m/kg.
    fn fixture_structure(plate: f32) -> StructuralWood {
        StructuralWood {
            along_grain_rupture_pa: 4.0e7,
            along_grain_fracture_j_per_m2: 2.0e4,
            fresh_density_kg_per_m3: 850.0,
            root_plate: RootPlate {
                overturning_nm_per_kg: plate,
            },
        }
    }

    // ASSUMED material fixture data, distinct fields so each omission/mutation is observable.
    const GRAIN_RUPTURE: &str = "along_grain_rupture_pa: (4.0e7, Fixture(\"fixture\")),";
    const GRAIN_FRACTURE: &str = "along_grain_fracture_j_per_m2: (2.0e4, Fixture(\"fixture\")),";
    const FRESH_DENSITY: &str = "fresh_density_kg_per_m3: (850.0, Fixture(\"fixture\")),";

    // A shipped root plate: an anchor resolves against the shipped attachment list.
    const ROOT_PLATE: &str = "anchor: \"scots_pine_root_plate\",";

    fn structural_wood_text(rows: &str) -> String {
        format!(
            r#"Wood(substance: "Wood", hp_per_m2: (1000.0, Fixture("fixture")),
                structure: {{ {rows} }})"#
        )
    }

    fn structural_row(name: &str, fields: &str) -> String {
        format!("{name:?}: StructuralWood({fields}),")
    }

    fn structural_fields() -> String {
        format!("{GRAIN_RUPTURE} {GRAIN_FRACTURE} {FRESH_DENSITY} {ROOT_PLATE}")
    }

    fn parse_fixture_wood(rows: &str) -> Result<Wood, String> {
        Wood::parse(
            Path::new("assets/trees/wood.ron"),
            &structural_wood_text(rows),
            &fixture_table(),
        )
    }

    /// Structural laws must never inherit the across-grain construction seam when a trunk's grain
    /// datum is missing. Every missing field is refused before play with file, species, field
    /// and repair; a defaulted grain or root strength/work breaks this load contract.
    #[test]
    fn every_missing_structural_fact_names_the_file_row_field_and_repair() {
        parse_fixture_wood(&structural_row("fixture", &structural_fields()))
            .expect("complete fixture material loads");
        for (missing, field) in [
            (GRAIN_RUPTURE, "along_grain_rupture_pa"),
            (GRAIN_FRACTURE, "along_grain_fracture_j_per_m2"),
            (FRESH_DENSITY, "fresh_density_kg_per_m3"),
            (ROOT_PLATE, "anchor"),
        ] {
            let fields = structural_fields().replace(missing, "");
            let error = parse_fixture_wood(&structural_row("fixture", &fields))
                .expect_err("missing material fact is refused");
            for required in ["assets/trees/wood.ron", "fixture", field, "author"] {
                assert!(error.contains(required), "missing {required:?}: {error}");
            }
        }
    }

    /// A trunk's anchor is a root plate of the shared attachment list, resolved at load: an
    /// anchor naming no kind, or a kind that joins two parts (a mortar bed), is refused naming
    /// the row, the list and the fix. A loader that took any name would hold a trunk to the
    /// ground by a code default, or by a construction's seam.
    #[test]
    fn a_trunks_anchor_is_a_root_plate_of_the_shared_list() {
        let wood = parse_fixture_wood(&structural_row("fixture", &structural_fields()))
            .expect("a shipped root plate resolves");
        assert_eq!(
            wood.structure.0["fixture"].root_plate,
            Attachments::shipped_static()
                .root_plate("scots_pine_root_plate")
                .unwrap()
        );
        for (anchor, names) in [
            ("unlisted_fixture_plate", "is not in"),
            ("mortared", "joins two parts"),
        ] {
            let fields = structural_fields().replace(ROOT_PLATE, &format!("anchor: {anchor:?},"));
            let error = parse_fixture_wood(&structural_row("fixture", &fields))
                .expect_err("an anchor that is no listed root plate is refused");
            for required in ["wood.ron", "fixture", anchor, ATTACHMENTS_PATH, names] {
                assert!(error.contains(required), "missing {required:?}: {error}");
            }
        }
    }

    /// A structural row is usable in simulation units at load, even before its first instance
    /// exists. A zero, negative or non-finite grain fact cannot disable failure (a root plate's
    /// coefficient is refused so by the attachment list's own loader).
    #[test]
    fn structural_wood_refuses_nonpositive_nonfinite_or_unsourced_facts() {
        for datum in [GRAIN_RUPTURE, GRAIN_FRACTURE, FRESH_DENSITY] {
            let field = datum.split(':').next().unwrap();
            for value in ["0.0", "-1.0", "NaN", "inf"] {
                let replacement = format!("{field}: ({value}, Fixture(\"fixture\")),");
                let fields = structural_fields().replace(datum, &replacement);
                let error = parse_fixture_wood(&structural_row("fixture", &fields))
                    .expect_err("unusable material fact is refused");
                for required in ["assets/trees/wood.ron", "fixture", field, "author"] {
                    assert!(error.contains(required), "missing {required:?}: {error}");
                }
            }
            let fields = structural_fields().replace(datum, &format!("{field}: 42.0,"));
            let error = parse_fixture_wood(&structural_row("fixture", &fields))
                .expect_err("a material fact must name its provenance");
            assert!(
                error.contains("fixture") && error.contains("provenance"),
                "{error}"
            );
        }
    }

    /// Every declared species has exactly one explicit row, including species not placed by the
    /// current map. Duplicates and stale keys cannot conceal missing construction from a later
    /// map or leave an ambiguous answer for the tree builder.
    #[test]
    fn structural_wood_refuses_missing_duplicate_and_unknown_species_rows() {
        let row = structural_row("fixture", &structural_fields());
        for (rows, name) in [
            (String::new(), "fixture"),
            (format!("{row} {row}"), "fixture"),
            (
                format!("{row} {}", structural_row("unknown", &structural_fields())),
                "unknown",
            ),
        ] {
            let error = parse_fixture_wood(&rows).expect_err("bad row set is refused");
            for required in ["assets/trees/wood.ron", name, "row"] {
                assert!(error.contains(required), "missing {required:?}: {error}");
            }
        }
    }

    /// The map loader must refuse missing structure facts before it constructs any tree asset.
    /// Startup relies on the actual file-loading path retaining the wood gate: testing the parser
    /// alone would miss a loader that bypassed it. ASSUMED fixture wood, species and file layout.
    #[test]
    fn the_library_refuses_missing_structure_facts_before_reading_a_tree_asset() {
        for datum in [GRAIN_RUPTURE, GRAIN_FRACTURE, FRESH_DENSITY, ROOT_PLATE] {
            let field = datum.split(':').next().unwrap();
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the fixture directory clock is after the epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "overmatch-wood-gate-{}-{unique}",
                std::process::id()
            ));
            let dir = root.join(TREES_DIR);
            std::fs::create_dir_all(&dir).expect("create fixture tree directory");
            std::fs::write(dir.join(SPECIES_FILE), r#"{"fixture": ()}"#)
                .expect("write fixture species");
            let fields = structural_fields().replace(datum, "");
            std::fs::write(
                dir.join(WOOD_FILE),
                structural_wood_text(&structural_row("fixture", &fields)),
            )
            .expect("write incomplete fixture wood");
            // The shipped entry refuses test provenance first. After its unwind, the inline
            // fixture uses the same file-loading implementation to exercise the structural gate.
            let shipped = std::panic::catch_unwind(|| TreeLibrary::load(&root, ["fixture"]))
                .expect_err("a shipped wood file must refuse Fixture in the test build too");
            let report = shipped
                .downcast_ref::<String>()
                .expect("a formatted load refusal");
            assert!(
                report.contains("wood.ron") && report.contains("Fixture provenance"),
                "{report}"
            );
            let outcome = std::panic::catch_unwind(|| TreeLibrary::load_files(&root, ["fixture"]));
            std::fs::remove_dir_all(&root).expect("remove fixture files");
            let failure = outcome.expect_err("the wood gate refuses before any tree asset is read");
            let error = failure
                .downcast_ref::<String>()
                .expect("the load gate panics with its formatted diagnostic");
            for required in ["wood.ron", "fixture", field, "author"] {
                assert!(error.contains(required), "missing {required:?}: {error}");
            }
        }
    }

    fn resolve(asset: TreeAsset, table: &SpeciesTable, wood: &Wood) -> Result<TreeKind, String> {
        TreeKind::resolve(
            "fixture",
            asset,
            table,
            wood,
            SubstanceRegistry::shipped_static(),
        )
    }

    /// The fixture trunk of `substance` whose middle segment — 2 to 4 m up, where the march's
    /// tests strike — holds `hp` points at scale 1: fixture data for tests that need a trunk a
    /// round goes through and a pool a given dose empties. Its HP per square metre is `hp` over
    /// that segment's mid-height cross-section, so [`TreeBody::segment_hp`] reads `hp` back there;
    /// the wider segment below holds about 2.8 times as much, the narrower one above a quarter.
    pub(crate) fn fixture_kind_of(substance: &str, hp: u32) -> Arc<TreeKind> {
        let wood = Wood {
            substance: substance.to_owned(),
            ..fixture_wood()
        };
        let mut kind =
            resolve(fixture_asset(), &fixture_table(), &wood).expect("the fixture trunk holds");
        let r = 0.5 * (kind.segments[1].r_bottom_m + kind.r_top_m(1));
        kind.hp_per_m2 = hp as f32 / (PI * r * r);
        let kind = Arc::new(kind);
        assert_eq!(
            TreeBody::new(kind.clone(), 1.0).segment_hp(1),
            hp,
            "the fixture pool"
        );
        kind
    }

    /// Inline game-state fixture for the separate topology tests awaiting consolidation.
    /// No legacy simulation file ships with the construction assets.
    pub(crate) fn fixture_library(id: &str) -> TreeLibrary {
        TreeLibrary {
            kinds: BTreeMap::from([(id.to_owned(), fixture_kind())]),
            texts: Vec::new(),
        }
    }

    pub(crate) fn fixture_kind() -> Arc<TreeKind> {
        Arc::new(
            resolve(fixture_asset(), &fixture_table(), &fixture_wood())
                .expect("the fixture trunk holds"),
        )
    }

    /// EVERY A6 REFUSAL IS A REFUSAL, NOT A CLAMP: an empty trunk, a non-finite or non-positive
    /// segment dimension, a missing, non-positive or non-finite tip radius, an unknown species, a
    /// wood row whose HP per square metre is not positive or whose substance states no bulk facts
    /// for the trunk's chunks to be of, and more segments than the wire word's cut byte can name
    /// are each refused by name. Any one of these laxed to a default — a NaN height carried, a zero or
    /// infinite tip spawned, a pool of no HP per square metre spawned, a 256th segment accepted —
    /// reds its row.
    #[test]
    fn every_malformed_asset_or_wood_row_is_refused_by_name() {
        let refusal = |asset: TreeAsset, wood: Wood| {
            resolve(asset, &fixture_table(), &wood).expect_err("refused")
        };
        let mut asset = fixture_asset();
        asset.segments.clear();
        assert!(refusal(asset, fixture_wood()).contains("segments"));
        let mut asset = fixture_asset();
        asset.segments[0].height_m = f32::NAN;
        assert!(refusal(asset, fixture_wood()).contains("dimension"));
        let mut asset = fixture_asset();
        asset.segments[2].r_bottom_m = 0.0;
        assert!(refusal(asset, fixture_wood()).contains("dimension"));
        for r_tip_m in [0.0, f32::INFINITY] {
            let mut asset = fixture_asset();
            asset.r_tip_m = r_tip_m;
            assert!(refusal(asset, fixture_wood()).contains("r_tip_m"));
        }
        let missing = ron::from_str::<TreeAsset>(
            r#"TreeAsset(species: "fixture", segments: [Segment(height_m: 2.0, r_bottom_m: 0.3)])"#,
        )
        .expect_err("a trunk without its tip radius")
        .to_string();
        assert!(missing.contains("r_tip_m"), "{missing}");
        let mut asset = fixture_asset();
        asset.species = "no_such_species".to_owned();
        assert!(refusal(asset, fixture_wood()).contains("species"));
        for hp_per_m2 in [0.0, -1.0, f32::NAN] {
            let wood = Wood {
                hp_per_m2: Sourced(hp_per_m2),
                ..fixture_wood()
            };
            assert!(refusal(fixture_asset(), wood).contains("hp_per_m2"));
        }
        let wood = Wood {
            substance: "RHA".to_owned(),
            ..fixture_wood()
        };
        assert!(refusal(fixture_asset(), wood).contains("bulk facts"));
        let mut asset = fixture_asset();
        let stub = asset.segments[2];
        asset.segments = vec![stub; MAX_SEGMENTS + 1];
        assert!(refusal(asset, fixture_wood()).contains("segments"));
    }

    /// A wood row whose substance the registry does not carry, or carries as a witness, cannot be
    /// a layer of the walk: refused at load.
    #[test]
    fn an_unknown_or_witness_substance_is_refused() {
        for substance in ["Balsa", "Flesh"] {
            let wood = Wood {
                substance: substance.to_owned(),
                ..fixture_wood()
            };
            resolve(fixture_asset(), &fixture_table(), &wood).expect_err(substance);
        }
    }

    /// EACH SEGMENT'S POOL IS THE WOOD'S RATE TIMES ITS OWN CROSS-SECTION: `hp_per_m2 × π r²` for
    /// `r` the segment's mid-height radius at the instance's scale, rounded to the nearest point
    /// and never below one. The fixture's segments at 1000 HP per square metre hold 196, 71 and
    /// 18 points at scale 1 (DERIVED: r 0.25, 0.15, 0.075 m) and four times that, 785, 283 and
    /// 71, at scale 2 (DERIVED: 785.4, 282.7, 70.7). A mutant sizing every segment by one
    /// diameter holds one value three times; one reading the bottom radius gives the base 283;
    /// one linear in the diameter doubles with the scale instead of quadrupling; one ignoring the
    /// instance's scale reads the same at both; one truncating reads 70 for the middle segment at
    /// scale 1 (70.69). A pool of no points would never empty, so a sliver of a trunk still holds
    /// one in every segment. The tree's HP pools — what equal shots must empty — rely on this.
    #[test]
    fn each_segment_pool_is_the_wood_rate_times_its_cross_section() {
        let pools = |scale: f32| {
            let body = TreeBody::new(fixture_kind(), scale);
            (0..3).map(|k| body.segment_hp(k)).collect::<Vec<_>>()
        };
        assert_eq!(pools(1.0), [196, 71, 18]);
        assert_eq!(pools(2.0), [785, 283, 71]);
        assert_eq!(pools(1.0e-4), [1, 1, 1], "never below one point");
    }

    /// The bins run from `+X` toward `+Z` and wrap once: the four axes land on 0, 64, 128, 192,
    /// and the bin just short of a full turn is 255, not 256.
    #[test]
    fn azimuth_bins_run_from_x_toward_z_and_wrap() {
        assert_eq!(azimuth_bin(1.0, 0.0), 0);
        assert_eq!(azimuth_bin(0.0, 1.0), 64);
        assert_eq!(azimuth_bin(-1.0, 0.0), 128);
        assert_eq!(azimuth_bin(0.0, -1.0), 192);
        assert_eq!(
            azimuth_bin(1.0, -0.01),
            0,
            "a hair short of a turn rounds to 0"
        );
        assert_eq!(azimuth_bin(1.0, -0.02), 255);
        assert_eq!(azimuth_bin(0.0, 0.0), 0);
    }

    /// THE WIRE WORD IS A BIJECTION ONTO THE TREE'S STATES: every state of a tree of `n`
    /// segments survives a round trip, `Standing` is the only word `0`, and the words no state
    /// encodes to are refused, not clamped. A decoder that drops the `+ 1` maps `0x0100` to cut 1
    /// (the round trip fails at cut 0); one that clamps the cut accepts `0x0400` on a 3-segment
    /// tree; one that masks the high byte for `Standing` accepts `0x0040`. The peer-side derive
    /// (ADR-0016) relies on a refused word never reaching a pose.
    #[test]
    fn the_wire_word_round_trips_every_state_and_refuses_the_rest() {
        let segments = 3;
        assert_eq!(TreeState::Standing.encode(), 0);
        assert_eq!(
            TreeState::decode(0x0040, segments),
            Err(TreeWordError::StandingDirectionBits(0x0040))
        );
        assert_eq!(
            TreeState::decode(0x0400, segments),
            Err(TreeWordError::CutOutside {
                word: 0x0400,
                cut: 3,
                segments
            })
        );
        assert_eq!(TreeState::decode(0, segments), Ok(TreeState::Standing));
        for cut in 0..segments as u8 {
            for dir in [0u8, 1, 64, 255] {
                let state = TreeState::Down { cut, dir };
                let word = state.encode();
                assert_eq!(word >> 8, u16::from(cut) + 1, "{state:?}");
                assert_eq!(TreeState::decode(word, segments), Ok(state));
            }
        }
        assert_eq!(
            TreeState::Down { cut: 0, dir: 0 }.encode(),
            0x0100,
            "the first down word is the cut-0 word, never 0"
        );
        assert!(
            TreeState::decode(0x0040, segments).is_err(),
            "standing with dir bits"
        );
        assert!(
            TreeState::decode(0x0400, segments).is_err(),
            "cut == segments"
        );
        assert!(
            TreeState::decode(0xFFFF, segments).is_err(),
            "cut 254 past this tree (legal only on a 255-segment one)"
        );
        assert_eq!(
            TreeState::decode(0x0300, segments),
            Ok(TreeState::Down { cut: 2, dir: 0 }),
            "the last segment is a legal cut"
        );
    }

    /// A cut the word's high byte cannot carry (`cut + 1` past a byte) has no encoding: the
    /// authority never sets one — resolve bounds the segment count — so encoding it is a broken
    /// invariant, not a wrapped word.
    #[test]
    #[should_panic(expected = "has no word")]
    fn a_cut_past_the_word_has_no_encoding() {
        let _ = TreeState::Down { cut: 255, dir: 0 }.encode();
    }

    /// A BODY FELLS A TREE BY ITS CONTACT AND THE TRUNK'S CONSTRUCTION, NEVER BY WHAT THE BODY IS:
    /// in a physics world with the real schedule and the solver's transfer reading, three dynamic
    /// bodies each drive a cube into a standing fixture trunk at 4 m/s — a tank's shape (root body
    /// → `Hull` node → proxy, as `tank::spawn` builds it), a launched turret's (a free body carrying
    /// its collider) of the same 20 t, and a 200 kg body. The first two fell their trees at the
    /// root plate along the push; the light one's blow reaches its tree and the tree stands. A law
    /// gating on the body's identity spares the turret's tree; a law reading the blow without the
    /// trunk's strength fells the light one's; either reds this. Who relies on it: every collision
    /// that fells a tree in play. DERIVED (fixture data): the fixture trunk's fresh mass is 494 kg,
    /// its root plate's moment 74 kN·m; 20 t stopped from 4 m/s is 80 kN·s, its least mean force
    /// over a 64 Hz step 5.1 MN, about 5 MN·m on the anchor at the cube's metre of lever; 200 kg is
    /// 0.8 kN·s and 51 kN, about 51 kN·m, under the plate's moment, so its blow meets the strength
    /// asymptote and stops there.
    #[test]
    fn a_body_fells_a_tree_by_its_contact_whatever_the_body_is() {
        use crate::tank::Hull;
        use avian3d::prelude::{Gravity, LinearVelocity, Mass, PhysicsPlugins};
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            // Avian's collider cache reads `AssetEvent<Mesh>`.
            AssetPlugin::default(),
            PhysicsPlugins::default(),
            crate::contact_transfer::sim_plugin,
            sim_plugin,
        ))
        .init_asset::<Mesh>()
        .insert_resource(Gravity::ZERO)
        // One fixed tick per update at the fixed clock's default 64 Hz.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        let kind = fixture_kind();
        let tree_at = |app: &mut App, z: f32, ordinal: u16| -> Entity {
            let world = app.world_mut();
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, world);
            spawn(
                &mut commands,
                &kind,
                MapInstance {
                    id: format!("fir_{ordinal}"),
                    ordinal,
                },
                Transform::from_xyz(0.0, 0.0, z),
            );
            queue.apply(world);
            world
                .query::<(Entity, &MapInstance)>()
                .iter(world)
                .find(|(_, identity)| identity.ordinal == ordinal)
                .map(|(entity, _)| entity)
                .expect("the tree just spawned")
        };
        let rammed = tree_at(&mut app, 0.0, 0);
        let nudged = tree_at(&mut app, 20.0, 1);
        let brushed = tree_at(&mut app, 40.0, 2);
        let body = |app: &mut App, z: f32, mass: f32| -> Entity {
            app.world_mut()
                .spawn((
                    Transform::from_xyz(-1.2, 1.0, z),
                    RigidBody::Dynamic,
                    Mass(mass),
                    LinearVelocity(Vec3::X * 4.0),
                ))
                .id()
        };
        let proxy = (
            Collider::cuboid(1.0, 1.0, 1.0),
            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
        );
        let tank = body(&mut app, 0.0, 20_000.0);
        let hull = app
            .world_mut()
            .spawn((ChildOf(tank), Transform::IDENTITY, Hull))
            .id();
        app.world_mut()
            .spawn((ChildOf(hull), Transform::IDENTITY, proxy.clone()));
        let turret = body(&mut app, 20.0, 20_000.0);
        app.world_mut().entity_mut(turret).insert(proxy.clone());
        let light = body(&mut app, 40.0, 200.0);
        app.world_mut().entity_mut(light).insert(proxy);
        // Each tick: the greatest blow the reading split off the light body's contact with its tree.
        let mut blow = 0.0_f32;
        for _ in 0..20 {
            app.update();
            let transfers = app.world().resource::<ContactTransfers>();
            for (point, struck, ..) in transfers.split() {
                if point.structure == brushed && point.body == light {
                    blow = blow.max(struck.length());
                }
            }
        }
        assert!(
            blow > 0.0,
            "the light body's contact delivers a blow to its tree"
        );
        for (tree, what) in [(rammed, "the tank's proxy"), (nudged, "the turret")] {
            let state = app.world().get::<TreeState>(tree);
            let Some(&TreeState::Down { cut: 0, dir }) = state else {
                panic!("{what} fells the trunk at its root plate: {state:?}");
            };
            // The trunk falls along the momentum it took, through the faceted 12-gon's normals
            // and the friction along them: within a facet's 15° of the +x push, 10.7 bins.
            let off = i32::from(dir.wrapping_add(128)) - 128;
            assert!(
                off.abs() <= 11,
                "{what} fells the trunk along +x: bin {dir}"
            );
        }
        assert_eq!(
            app.world().get::<TreeState>(brushed),
            Some(&TreeState::Standing),
            "a blow whose force the trunk's strength holds leaves it standing ({blow} N·s)"
        );
    }

    /// A TRUNK BREAKS WHERE BENDING STRESS PEAKS (the collision study's test 10): a point press on
    /// a tapered fixture trunk opens the joint where the press's moment over the section's
    /// strength peaks, the analytical argmax of `M(z) / S(z)` against the wood's rupture strength
    /// (the sections' own weight read too), or the root plate when its moment `C m` is the weaker;
    /// just under that least failing press nothing opens. A fixed cut (always the base) fails the
    /// strong-plate case; a cut at the pressed segment fails both; a section strength that is not
    /// `32 M / π d³` against the rupture strength fails the threshold. Who relies on it: every
    /// contact and blast that fells a tree, which all meet the trunk through [`load_trunk`].
    /// FIXTURE DATA: ten 1 m segments tapering 0.3 → 0.05 m, the press at 7.5 m.
    #[test]
    fn a_trunk_breaks_where_bending_stress_peaks() {
        let segments = 10;
        let radius = |z: f32| 0.3 - 0.025 * z;
        let asset = TreeAsset {
            species: "fixture".to_owned(),
            segments: (0..segments)
                .map(|k| Segment {
                    height_m: 1.0,
                    r_bottom_m: radius(k as f32),
                })
                .collect(),
            r_tip_m: radius(segments as f32),
        };
        let (h, rupture) = (7.5_f32, 4.0e7_f32);
        for (plate, expected_anchor) in [(1.0e5_f32, false), (150.0, true)] {
            let wood = Wood {
                structure: StructureRows(BTreeMap::from([(
                    "fixture".to_owned(),
                    fixture_structure(plate),
                )])),
                ..fixture_wood()
            };
            let kind = Arc::new(resolve(asset.clone(), &fixture_table(), &wood).unwrap());
            let body = TreeBody::new(kind, 1.0);
            // Analytically: the least press that fails each joint. A seam at height z holds
            // `M = (σ + W/A) S`, `S = π r³ / 4`, `W` the weight above it; the plate `C m`.
            let frustum = |r0: f32, r1: f32| PI * (r0 * r0 + r0 * r1 + r1 * r1) / 3.0;
            let mass_above = |j: usize| -> f32 {
                (j..segments)
                    .map(|k| frustum(radius(k as f32), radius(k as f32 + 1.0)) * 850.0)
                    .sum()
            };
            let failing: Vec<f32> = (0..segments)
                .map(|j| {
                    let (z, r) = (j as f32, radius(j as f32));
                    if j == 0 {
                        plate * mass_above(0) / h
                    } else {
                        let weight = mass_above(j) * gravity_mps2() / (PI * r * r);
                        (rupture + weight) * PI * r * r * r / 4.0 / (h - z)
                    }
                })
                .collect();
            let (weakest, least) = failing
                .iter()
                .copied()
                .enumerate()
                .take(8)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            assert_eq!(
                weakest == 0,
                expected_anchor,
                "fixture: the plate {plate} N·m/kg decides which law is weaker: {failing:?}"
            );
            if !expected_anchor {
                assert_eq!(
                    weakest, 5,
                    "the tapered fixture's argmax of M/S: {failing:?}"
                );
            }
            let press = |force: f32| {
                let mut trunk = TrunkState::new(&body.structure);
                let load = Load {
                    chunk: 7,
                    at: Vec3::new(-0.1, h, 0.0),
                    energy: 0.0,
                    crushed: 0.0,
                    blow: Vec3::ZERO,
                    press: Vec3::X * force,
                    peak: None,
                    origin: Origin::Body {
                        body: Entity::PLACEHOLDER,
                        mass: 2.0e4,
                        speed: 0.0,
                        step: 1.0 / 64.0,
                    },
                };
                load_trunk(&body, &mut trunk, &[load], Vec3::ZERO)
            };
            assert_eq!(
                press(0.99 * least),
                None,
                "under its weakest section's strength the trunk holds"
            );
            assert_eq!(
                press(1.01 * least),
                Some(weakest as u8),
                "just past it, the trunk breaks there (plate {plate} N·m/kg)"
            );
        }
    }

    /// The game's damage model and environment construction: implicit belt contacts must enter
    /// the real trunk load reader. A convex hull alone cannot prove this route; this fixture is
    /// a spawned tree with TreeBody and TreeSegment, and a source clear of solver contacts.
    /// FIXTURE DATA: a lateral press above the trunk's strength limit must open a joint.
    #[test]
    fn a_trunk_reads_implicit_belt_contacts_and_breaks_under_their_load() {
        use avian3d::prelude::{ColliderDensity, Gravity, PhysicsPlugins};
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
            crate::contact_transfer::sim_plugin,
            sim_plugin,
        ))
        .init_asset::<Mesh>()
        .insert_resource(Gravity::ZERO)
        .insert_resource(fixture_grid(|_, _| 0.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        let kind = fixture_kind();
        let tree = {
            let world = app.world_mut();
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, world);
            let root = spawn(
                &mut commands,
                &kind,
                MapInstance {
                    id: "fixture".to_owned(),
                    ordinal: 0,
                },
                Transform::IDENTITY,
            );
            queue.apply(world);
            root
        };
        let source = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Position(Vec3::X * 10.0),
                Collider::cuboid(1.0, 1.0, 1.0),
                ColliderDensity(1000.0),
            ))
            .id();
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(
            app.world().get::<TreeState>(tree),
            Some(&TreeState::Standing)
        );
        let collider = app.world().get::<TreeParts>(tree).unwrap().0[1];
        app.world_mut().resource_mut::<ContactTransfers>().stage(
            crate::contact_transfer::AppliedContact {
                structure: tree,
                structure_collider: collider,
                body: source,
                point: Vec3::Y * 3.0,
                normal: Vec3::X,
                force: Vec3::X * 1.0e9,
                closing_speed: 0.0,
            },
        );
        app.update();
        let transfers = app.world().resource::<ContactTransfers>();
        assert!(
            transfers
                .split()
                .any(|(p, ..)| p.structure == tree && p.body_collider.is_none()),
            "the physics step publishes an implicit belt point"
        );
        assert!(
            app.world().get::<TrunkState>(tree).unwrap().cut().is_some(),
            "the trunk reader must apply the implicit press to its joints"
        );
        assert!(
            matches!(
                app.world().get::<TreeState>(tree),
                Some(TreeState::Down { .. })
            ),
            "the structural cut drives the tree state"
        );
    }

    /// A sleeping contact starts no structural run: the last awake step checked its press and
    /// the standing trunk has no topology change until it falls. Server work relies on this;
    /// re-running the laws would mark TrunkState changed. A cube asleep on the standing fixture
    /// trunk's tip wakes in the step after the fall first moves the trunk, and falls. Avian wakes
    /// nothing when a static collider moves, so a world without the game's wake for a moved
    /// support (`debris::wake_on_static_moved`) leaves the cube hanging where the tip stood.
    #[test]
    fn a_sleeping_contact_starts_no_law_run_and_wakes_when_the_trunk_moves() {
        use avian3d::prelude::{ColliderDensity, Gravity, PhysicsPlugins, Sleeping};
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
            crate::contact_transfer::sim_plugin,
            sim_plugin,
            crate::debris::sim_plugin,
        ))
        .init_asset::<Mesh>()
        .insert_resource(Gravity(Vec3::NEG_Y * gravity_mps2()))
        .insert_resource(fixture_grid(|_, _| 0.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        let kind = fixture_kind();
        let tip: f32 = kind.segments.iter().map(|segment| segment.height_m).sum();
        let tree = {
            let world = app.world_mut();
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, world);
            let tree = spawn(
                &mut commands,
                &kind,
                MapInstance {
                    id: "fir_0".to_owned(),
                    ordinal: 0,
                },
                Transform::IDENTITY,
            );
            queue.apply(world);
            tree
        };
        // A 6 cm cube wholly over the trunk's 5 cm-radius tip (fixture data). It rocks on the tip's
        // ring past avian's thresholds, so once settled it is put to sleep by avian's own lever
        // (`Sleeping` inserted sleeps its island).
        let cube = app
            .world_mut()
            .spawn((
                Position(Vec3::Y * (tip + 0.031)),
                Rotation::default(),
                RigidBody::Dynamic,
                Collider::cuboid(0.06, 0.06, 0.06),
                ColliderDensity(1000.0),
            ))
            .id();
        for _ in 0..128 {
            app.update();
        }
        app.world_mut().entity_mut(cube).insert(Sleeping);
        app.update();
        let rested = app.world().get::<Position>(cube).unwrap().0.y;
        assert!(
            app.world().get::<Sleeping>(cube).is_some() && (rested - tip).abs() < 0.05,
            "the cube sleeps on the tip, at {rested} m"
        );
        let checked = app
            .world()
            .entity(tree)
            .get_ref::<TrunkState>()
            .unwrap()
            .last_changed();
        for _ in 0..4 {
            app.update();
            assert!(
                !app.world()
                    .resource::<ContactTransfers>()
                    .resting()
                    .is_empty()
            );
            assert_eq!(
                app.world()
                    .entity(tree)
                    .get_ref::<TrunkState>()
                    .unwrap()
                    .last_changed(),
                checked,
                "an unchanged sleeping press starts no structural law run"
            );
        }
        app.world_mut().entity_mut(tree).insert((
            TreeState::Down { cut: 0, dir: 0 },
            TrunkLoads(vec![motion::fixture_blow(&kind, Vec3::X * 500.0)]),
        ));
        // The tick the trunk falls at, upright; the tick its fall first moves it, after the step;
        // then the step that reads the move.
        app.update();
        app.update();
        app.update();
        assert!(
            app.world().get::<Sleeping>(cube).is_none(),
            "the fall wakes it"
        );
        for _ in 0..256 {
            app.update();
        }
        assert!(
            app.world().get::<Position>(cube).unwrap().0.y < tip - 0.1,
            "it falls"
        );
    }

    /// The game's damage model: a segment's emptied HP pool opens its own base seam,
    /// independent of the dose point's height. The topology uses the lowest open seam and the
    /// input direction, including a later stump cut. Fixture points lie in other segments so
    /// selecting a cut from height or preserving an earlier higher cut fails.
    #[test]
    fn the_fall_cuts_at_the_segment_that_emptied() {
        let mut world = World::new();
        world.add_observer(fell_on_pool_emptied);
        let base = Vec3::new(10.0, 3.0, -4.0);
        let spawn = |world: &mut World| -> Vec<Entity> {
            let body = TreeBody::new(fixture_kind(), 1.0);
            let root = world
                .spawn((
                    TreeState::Standing,
                    TrunkState::new(&body.structure),
                    body,
                    Transform::from_translation(base),
                ))
                .id();
            (0..3)
                .map(|k| world.spawn((ChildOf(root), TreeSegment(k))).id())
                .collect()
        };
        let root = |world: &World, segment: Entity| world.get::<ChildOf>(segment).unwrap().parent();
        let shot = spawn(&mut world);
        world.trigger(PoolEmptied {
            entity: shot[2],
            at: base + Vec3::new(0.2, 0.5, 0.0),
            along: Some(Vec3::new(0.0, -0.3, 1.0)),
        });
        world.flush();
        let shot_root = root(&world, shot[2]);
        assert_eq!(
            world.get::<TreeState>(shot_root),
            Some(&TreeState::Down { cut: 2, dir: 64 }),
            "the emptied segment, down the line"
        );
        assert_eq!(
            world.get::<TrunkState>(shot_root).map(TrunkState::cut),
            Some(Some(2)),
            "the emptied segment's base joint is open in the structural state contacts break"
        );

        let burst = spawn(&mut world);
        world.trigger(PoolEmptied {
            entity: burst[1],
            at: base + Vec3::new(5.0, 4.5, 0.0),
            along: None,
        });
        assert_eq!(
            world.get::<TreeState>(root(&world, burst[1])),
            Some(&TreeState::Down { cut: 1, dir: 128 }),
            "a field breaks the emptied segment, away from its centre",
        );

        world.trigger(PoolEmptied {
            entity: shot[0],
            at: base + Vec3::new(0.0, 1.0, 0.0),
            along: Some(Vec3::X),
        });
        assert_eq!(
            world.get::<TreeState>(shot_root),
            Some(&TreeState::Down { cut: 0, dir: 0 }),
            "a lower stump cut updates the topology",
        );
    }

    /// A static map body is not eased: under the offline compositions' physics
    /// ([`crate::offline_physics`]) a standing tree, the map's largest static population, takes no
    /// transform write between state changes, so its root, collider and view nodes stay out of
    /// transform propagation and render extraction. Breaks if the offline physics eases every
    /// rigid body again (`PhysicsInterpolationPlugin::interpolate_all()`), which rewrites a static
    /// root's pose every fixed step and frame.
    #[test]
    fn a_static_map_body_is_not_eased() {
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        #[derive(Resource, Default)]
        struct Dirty(usize, usize);
        fn audit(
            local: Query<(), Changed<Transform>>,
            global: Query<(), Changed<GlobalTransform>>,
            mut dirty: ResMut<Dirty>,
        ) {
            *dirty = Dirty(local.iter().count(), global.iter().count());
        }
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            // Avian's collider cache reads `AssetEvent<Mesh>`.
            AssetPlugin::default(),
            crate::offline_physics(),
        ))
        .init_asset::<Mesh>()
        // 10 ms frames against the 64 Hz fixed step: frames end between fixed steps, where an
        // eased body would be written.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            10,
        )))
        .init_resource::<Dirty>()
        .add_systems(Last, audit);
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        let world = app.world_mut();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, world);
        let root = spawn(
            &mut commands,
            &fixture_kind(),
            MapInstance {
                id: "static-fixture".to_owned(),
                ordinal: 0,
            },
            Transform::from_xyz(1.0, 2.0, 3.0).with_rotation(Quat::from_rotation_y(0.7)),
        );
        // A view subtree beneath the real spawned body, as the glb is attached in the client.
        let view = commands.spawn((ChildOf(root), Transform::IDENTITY)).id();
        commands.spawn((ChildOf(view), Transform::from_xyz(0.0, 1.0, 0.0)));
        queue.apply(world);
        for _ in 0..8 {
            app.update();
        }
        for _ in 0..8 {
            app.update();
            let dirty = app.world().resource::<Dirty>();
            assert_eq!(
                (dirty.0, dirty.1),
                (0, 0),
                "a standing tree's transforms (local, global) changed with no state change"
            );
        }
    }

    /// The spawn builds the body the module doc describes: one root with the state and no volume
    /// or pool of its own, nothing on its scale; one child per segment, in order from the base,
    /// with a frustum collider on `[Terrain, Armor]`, the kind's volume — its material and
    /// substance — and a full pool of its own cross-section's points; and no view — the sim
    /// spawns none. A segment carrying another substance's volume or another segment's pool, a
    /// pool left on the root (a dose the walk charges to no segment), or a segment hulled as a
    /// cylinder at either ring's radius, reds this.
    #[test]
    fn spawn_builds_a_root_and_a_collider_per_segment() {
        let mut world = World::new();
        let kind = fixture_kind();
        let pose = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_y(0.7),
            scale: Vec3::splat(1.5),
        };
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        spawn(
            &mut commands,
            &kind,
            MapInstance {
                id: "fir_0001".to_owned(),
                ordinal: 7,
            },
            pose,
        );
        queue.apply(&mut world);
        let (root, _, transform) = world
            .query::<(Entity, &TreeBody, &Transform)>()
            .single(&world)
            .expect("one root");
        assert!(
            world.get::<ComponentHealth>(root).is_none()
                && world.get::<BallisticVolume>(root).is_none(),
            "the root holds no pool and is no volume"
        );
        assert_eq!(transform.scale, Vec3::ONE);
        assert_eq!(transform.translation, pose.translation);
        assert_eq!(
            transform.rotation, pose.rotation,
            "the yaw is the instance's"
        );
        let standing = CollisionLayers::new([Layer::Terrain, Layer::Armor], LayerMask::ALL);
        let mut segments: Vec<(Entity, f32, Collider)> = world
            .query::<(
                &TreeSegment,
                &ChildOf,
                &Transform,
                &Collider,
                &CollisionLayers,
                &BallisticVolume,
                &ComponentHealth,
            )>()
            .iter(&world)
            .map(
                |(&TreeSegment(k), parent, transform, collider, layers, volume, pool)| {
                    assert_eq!(
                        *layers, standing,
                        "a standing segment stops hulls and rounds"
                    );
                    assert_eq!(
                        (volume.material, volume.substance.as_str()),
                        (kind.material, kind.substance.as_str()),
                        "segment {k}'s volume is the kind's"
                    );
                    // DERIVED, fixture data: 1000 π r² at the mid-height radii 0.375, 0.225 and
                    // 0.1125 m (the fixture's at scale 1.5): 441.8, 159.0, 39.8.
                    let points = [442, 159, 40][usize::from(k)];
                    assert_eq!(
                        (pool.shown(), pool.points(), pool.held()),
                        (
                            points,
                            points,
                            points * crate::ballistics::POOL_UNITS_PER_HP
                        ),
                        "segment {k}'s pool, full"
                    );
                    assert_eq!(
                        transform.translation.y,
                        [0.0, 3.0, 6.0][usize::from(k)],
                        "segment {k} stands at its own bottom"
                    );
                    (parent.parent(), transform.translation.y, collider.clone())
                },
            )
            .collect();
        assert_eq!(segments.len(), 3);
        assert!(segments.iter().all(|(parent, _, _)| *parent == root));
        segments.sort_by(|a, b| a.1.total_cmp(&b.1));
        let bottoms: Vec<f32> = segments.iter().map(|(_, y, _)| *y).collect();
        assert_eq!(
            bottoms,
            [0.0, 3.0, 6.0],
            "segment bottoms at the scaled prefix sums"
        );
        let indices: Vec<u8> = world
            .get::<Children>(root)
            .expect("segments")
            .iter()
            .map(|child| world.get::<TreeSegment>(child).expect("a segment").0)
            .collect();
        assert_eq!(indices, [0, 1, 2], "the children in order from the base");
        // Each hull spans its frustum's wider ring and its scaled height from the segment's own
        // origin: the ring table holds the four axis points, so the bounds are exact. And each
        // end has its own ring's radius: a tenth of a millimetre inside either end, a ray along Z
        // past the 12-gon's +X vertex meets the hull a millimetre inside that ring's scaled
        // radius and misses it a millimetre outside. DERIVED margin: that far in, the
        // cross-section's radius is off the end ring's by `|r_bottom - r_top| × 1e-4 / height`,
        // under 10 µm on this fixture.
        const PROBE: f32 = 1.0e-3;
        const INSIDE: f32 = 1.0e-4;
        for (k, ((_, _, collider), segment)) in segments.iter().zip(&kind.segments).enumerate() {
            let r_top_m = kind.r_top_m(k);
            let aabb = collider.aabb(Vec3::ZERO, Quat::IDENTITY);
            let r = segment.r_bottom_m.max(r_top_m) * 1.5;
            let h = segment.height_m * 1.5;
            assert!(
                aabb.min.abs_diff_eq(Vec3::new(-r, 0.0, -r), 1e-6)
                    && aabb.max.abs_diff_eq(Vec3::new(r, h, r), 1e-6),
                "the hull is the scaled frustum: {aabb:?} for {segment:?}"
            );
            for (y, ring) in [
                (INSIDE, segment.r_bottom_m * 1.5),
                (h - INSIDE, r_top_m * 1.5),
            ] {
                let meets = |x: f32| {
                    collider
                        .cast_ray(
                            Vec3::ZERO,
                            Quat::IDENTITY,
                            Vec3::new(x, y, -1.0),
                            Vec3::Z,
                            2.0,
                            true,
                        )
                        .is_some()
                };
                assert!(
                    meets(ring - PROBE) && !meets(ring + PROBE),
                    "at {y} m the hull's radius is the ring's {ring}: {segment:?}"
                );
            }
        }
        assert_eq!(world.query::<&view::TreeView>().iter(&world).count(), 0);
    }

    /// A non-uniform scale is refused: one scalar multiplies every height and radius, so an
    /// instance that stretches one axis has no body this module can build.
    #[test]
    #[should_panic(expected = "scales uniformly")]
    fn a_non_uniform_scale_is_refused() {
        let mut world = World::new();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        spawn(
            &mut commands,
            &fixture_kind(),
            MapInstance {
                id: "fir_0001".to_owned(),
                ordinal: 0,
            },
            Transform::from_scale(Vec3::new(1.0, 1.5, 1.0)),
        );
        queue.apply(&mut world);
    }

    /// A negative scale is refused: it is uniform, but no trunk is built inside out.
    #[test]
    #[should_panic(expected = "scales uniformly")]
    fn a_negative_scale_is_refused() {
        let mut world = World::new();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        spawn(
            &mut commands,
            &fixture_kind(),
            MapInstance {
                id: "fir_0001".to_owned(),
                ordinal: 0,
            },
            Transform::from_scale(Vec3::splat(-1.0)),
        );
        queue.apply(&mut world);
    }

    /// A tilted instance is refused: standing statics reads an upright trunk.
    #[test]
    #[should_panic(expected = "is tilted")]
    fn a_tilted_instance_is_refused() {
        let mut world = World::new();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        spawn(
            &mut commands,
            &fixture_kind(),
            MapInstance {
                id: "fir_0001".to_owned(),
                ordinal: 0,
            },
            // Half a degree of lean: its up falls 3.8e-5 short of 1 (DERIVED: 1 − cos 0.5°),
            // 38 times `UPRIGHT_EPSILON`.
            Transform::from_rotation(Quat::from_rotation_x(0.5_f32.to_radians())),
        );
        queue.apply(&mut world);
    }

    /// An 11 × 11 fixture grid over `FIXTURE_EXTENT` (100 m cells) sampled from `height`.
    pub(crate) fn fixture_grid(height: impl Fn(f32, f32) -> f32) -> HeightGrid {
        let size = 11_u32;
        let half = FIXTURE_EXTENT.half_extent();
        let step = FIXTURE_EXTENT.world_size_m / (size - 1) as f32;
        let height = &height;
        let samples: Vec<f32> = (0..size)
            .flat_map(|j| {
                (0..size).map(move |i| height(-half + i as f32 * step, -half + j as f32 * step))
            })
            .collect();
        HeightGrid::new(samples.into(), size, FIXTURE_EXTENT)
    }
}
