//! A construction breaking in a match, about the game's environment destruction (game state only):
//! the authority runs the fracture laws ([`super::fracture`]) on what the rounds deposit, records
//! every change, and every peer derives the wall's colliders from that record.
//!
//! * **The law's view of a construction** ([`Layout`], once per asset): the chunks of the elements
//!   whose substance states bulk facts ([`Material::of`]), numbered in asset order, and the joints
//!   and anchors among them, each holding by the bond the asset gives it ([`Bond`]: a seam inside
//!   a component; between two, what the attachment kind it names holds by
//!   ([`super::attachments`]: a seam, or a rest)). A chunk and a joint keep that number on the
//!   wire.
//! * **First touch swaps an element.** An element stays one collider until a deposit, a crack, a
//!   fall or dust touches one of its chunks; from then on each of its standing chunks is a static
//!   collider of its substance ([`ChunkCollider`]) and the element's own collider is gone. A
//!   construction's [`Breakage`] holds its complete state, built from data at spawn.
//! * **Deposits** (the authority's, [`ShellMarched`]): a round's line through an element is shared
//!   among the chunks it crosses by chord; through a chunk it is that chunk's. Every field ray (a
//!   spall cone's, a burst's fragments) that stops on a chunk deposits its energy there. A burst
//!   feeds the blast field ([`fracture::trace_blast`]) from its burst point, its energy the core's
//!   chemical energy less the casing's share. A line pushes with all the momentum the round lost
//!   in it (the resolved line's share; how much the crushed brick carries away is open); a
//!   blast pushes each face its rays stop on with its reflected impulse ([`Blast::push`]), over the
//!   share of the ray's cone the struck chunk or body intercepts, and crushes a chunk there where
//!   its peak pressure beats the chunk's dynamic compressive strength ([`Blast::crushed`]). A
//!   blast ray stops, within its
//!   reach ([`Blast::reach`]), on the nearest of every chunk group's standing chunks (a placed
//!   construction's or a moving group's) and a placed construction's unbreakable elements, the other moving
//!   bodies (a hull, a launched turret) and every other surface (the ground); one
//!   that stops on such a body pushes it the same way, through avian at the point it stops.
//!   Terrain and other nonstructural surfaces only shade the field.
//! * **Contacts**: [`sweep_intake::run`] frees intersected patches before physics, handing each to
//!   [`sweep_intake::sweep_departure`] with velocity and tick, which routes what stays connected of
//!   it by size ([`crate::debris::crumble::route`]). Unfreed contact transfers on a placed
//!   construction enter [`read_contacts`] as forces for the structure check. The solver already
//!   exchanged their momentum; intake never launches or refunds a second body blow. Sleeping
//!   presses remain in [`RestingLoads`]. A moving group's contacts load no law here: its landings
//!   crumble it ([`crate::debris::crumble`]).
//! * **The order.** Tick intake reads one geometry snapshot. Each owner runs removal and
//!   cavity cracks ([`fracture::strike`]), the part-scale verdict
//!   ([`fracture::parts::Assessment`]), then connectivity on change. Independent owners
//!   evaluate together; effects commit by entity, respecting [`owner_batches`]. Owners whose
//!   topology changed or whose pulse ended, and changed spin, continue next tick without requiring
//!   another deposit.
//! * **Holes** ([`Hole`], drawn only): a line cuts a channel in each chunk it crosses, the cylinder
//!   about its chord, open across each face it crosses ([`open_ends`]), and a blast a dish in each
//!   chunk its rays stop on ([`holes_of`]); a chunk still standing after the deposit
//!   keeps its hole in a per-chunk list ([`Live::holes`]), for the view to draw. No law, collider
//!   or mass reads a hole: they see the whole chunk, and [`Shape`] holds none. A hole holds no more
//!   than the volume its load crushed, the volume it took from the chunk's pool, so one the size
//!   of its chunk is a spent pool, recorded as dust. Only the pool decides dust: a chunk holding
//!   [`HOLES_PER_CHUNK`] holes keeps standing and draws no later one.
//! * **The record** ([`Record`]): wear, dust, holes, cohesion cracks and contact separations, in authority order,
//!   with its tick. The authority appends; the network carries it ([`crate::net`]'s ledger); a
//!   replica rebuilds the same state from it ([`derive_from_record`]); the view presents it at the
//!   presentation clock ([`super::view`]).
//! * **Loose groups** fall on the authority as one dynamic body each ([`Piece`]): each chunk's
//!   occupied hull is brick in ballistics on [`Layer::Armor`], and its contact hull, set back
//!   [`LOOSE_GAP_M`] on its joint faces, collides as [`Layer::Debris`], settled by the shared
//!   rule ([`crate::debris`]). A field departure leaves with
//!   its impulse less the work its opening spent ([`fracture::debit_work`]); its spin is the angular
//!   momentum over its own inertia tensor; gravity, contact and friction decide whether a group
//!   a cracked joint freed rocks back or topples. A later blast's impulse on it is a write to
//!   its velocity and sleep timer, which wakes it asleep (avian's `wake_on_changed`).
//! * **A moving group is a chunk group too**: its body carries its own [`Breakage`], its owned
//!   chunks standing and the rest dust, so the deposits and contacts that reach it run the same
//!   laws with its rigid acceleration removed and its spin read from the body.
//!   A seam that fails there splits it at that tick: [`PendingSplit`] stages the new groups until
//!   the law stage finishes, `split_groups` spawns them with the parent's
//!   rigid velocity field and despawns it, and [`RetirePiece`] keeps the parent's final pose and
//!   retirement tick on the wire, so replicas hand its chunks over at the shared cursor.
//!
//! THE BODY IN THE PHYSICS WORLD. A state change reaches avian's world in the frame it is made
//! ([`reconcile`], then avian's refit of moved colliders), so the next tick's march reads it.

use crate::cost::stages::{Stage, Stages, Work};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use avian3d::collider_tree::{ColliderTrees, update_moved_collider_aabbs};
use avian3d::prelude::{
    ActiveCollisionHooks, AngularVelocity, Collider, ColliderAabb, ColliderOf, CollisionLayers,
    Forces, LayerMask, LinearVelocity, Position, ReadRigidBodyForces, RigidBody, Rotation, Sensor,
    SpatialQuery, SpatialQueryFilter, WriteRigidBodyForces,
};

use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::{ComputeTaskPool, TaskPool};
use serde::{Deserialize, Serialize};

use super::attachments::Attachments;
use super::fracture::{
    self, Blast, Block, Bond, Chunk, Fate, Joint, Load, Material, Origin, Side, State, Structure,
};
use super::schema::{self, ConstructionAsset};
use super::{ConstructionKind, Element};
use crate::ballistics::{
    ArmourMark, BallisticVolume, BurstField, DeliveredLine, PenetrationMarks, ShellMarched,
    SpallMarks,
};
use crate::contact_transfer::{ContactGroup, ContactTransfer, ContactTransfers};
use crate::debris::launch::{Launch, launch_mass};
use crate::{ClientReplica, Layer, ShotClock};

/// Resolution: face-stop tolerance, m, for repeated f32 intersection rounding.
const STOP_TOLERANCE_M: f32 = 0.01;

/// DERIVED from the construction frame: grade is its zero-height plane, m, the plane the construction stands on (a construction spawns with its
/// frame's origin on the ground, its foundation below). A chunk wholly below it is buried.
const GRADE_M: f32 = 0.0;
/// Resolution: on-grade tolerance, m, for baked corners rounded off the cutting plane.
const ON_GRADE_M: f32 = 1e-4;

/// The most holes one chunk draws and the record carries (module doc), the earliest cut kept; a
/// later one still wears the pool (Resolution: bounds the view's per-chunk table and the shared
/// record, no law reads it). ASSUMED: no measured cost or look sets its size yet.
pub(crate) const HOLES_PER_CHUNK: usize = 16;

/// The law's view of one construction asset (module doc), at authored size in the construction frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Layout {
    /// Per law chunk: its asset chunk.
    pub(crate) chunks: Vec<u32>,
    /// Per law chunk: its element.
    pub(crate) element_of: Vec<u16>,
    /// Per element: its law chunks `first..end`, `None` for an element that never breaks.
    pub(crate) elements: Vec<Option<(u16, u16)>>,
    /// Per law chunk: its material's values.
    materials: Vec<Material>,
    /// The law's joints, chunk pairs first in asset order, then the anchors in asset order.
    pub(crate) joints: Vec<Joint>,
    patches: Vec<fracture::bondsolve::Patch>,
    /// Per law chunk: its hull.
    pub(crate) blocks: Vec<Block>,
    /// Per element: its hull.
    pub(crate) element_blocks: Vec<Block>,
}

impl Layout {
    /// The layout of `asset`, its materials' values `law_materials` (per asset material, `None` for a
    /// substance that never breaks), its attachment kinds read in `attachments`. Refuses an asset
    /// naming a kind the list does not hold, and one whose law chunks or joints overrun the wire's
    /// 16-bit numbers.
    pub(crate) fn of(
        asset: &ConstructionAsset,
        law_materials: &[Option<Material>],
        attachments: &Attachments,
    ) -> Result<Self, String> {
        for (k, kind) in asset.attachments.iter().enumerate() {
            attachments
                .holds_kind(kind)
                .map_err(|err| format!("attachments[{k}]: {err}"))?;
        }
        let anchors = attachments.anchors(asset)?;
        let mut chunks = Vec::new();
        let mut element_of = Vec::new();
        let mut materials = Vec::new();
        let mut elements = Vec::with_capacity(asset.elements.len());
        let mut law_of = vec![None; asset.chunks.len()];
        for (index, element) in asset.elements.iter().enumerate() {
            let Some(material) = law_materials[usize::from(element.material)] else {
                elements.push(None);
                continue;
            };
            let material = material.laid(element.surface_frame).map_err(|err| {
                format!(
                    "elements[{index}] (substance {:?}): {err}",
                    asset.materials[usize::from(element.material)]
                )
            })?;
            let first = chunks.len();
            for chunk in element.chunks[0]..element.chunks[1] {
                law_of[chunk as usize] = Some(chunks.len());
                chunks.push(chunk);
                element_of.push(index as u16);
                materials.push(material);
            }
            elements.push(Some((first, chunks.len())));
        }
        let mut joints = Vec::new();
        for joint in &asset.joints {
            let attached = match joint.bond {
                schema::Bond::Seam => None,
                schema::Bond::Attached(k) => Some(
                    attachments
                        .bond(&asset.attachments[usize::from(k)])
                        .map_err(|err| format!("joint ({}, {}): {err}", joint.a, joint.b))?,
                ),
            };
            if let (Some(a), Some(b)) = (law_of[joint.a as usize], law_of[joint.b as usize]) {
                let bond = match attached {
                    Some(bond) => bond,
                    // An intrinsic joint takes its component material's seam.
                    None => Bond::Bonded(materials[a].seam),
                };
                joints.push(Joint {
                    a,
                    b: Side::Chunk(b),
                    area: joint.area_m2,
                    centroid: Vec3::from_array(joint.centroid),
                    normal: Vec3::from_array(joint.normal),
                    bond,
                });
            }
        }
        for (anchor, bond) in asset.anchors.iter().zip(anchors) {
            if let Some(a) = law_of[anchor.chunk as usize] {
                joints.push(Joint {
                    a,
                    b: Side::Ground,
                    area: anchor.area_m2,
                    centroid: Vec3::from_array(anchor.centroid),
                    normal: Vec3::Y,
                    bond,
                });
            }
        }
        let wire = usize::from(u16::MAX);
        if chunks.len() > wire || joints.len() > wire {
            return Err(format!(
                "{} breakable chunks and {} joints, past the wire's {wire}",
                chunks.len(),
                joints.len()
            ));
        }
        // Every hull bounds a closed solid law block: refused here, before play.
        let solid = |points: Vec<Vec3>, what: String| {
            Block::hull(points).ok_or_else(|| {
                format!(
                    "{what} cannot form a closed solid law block thicker than {} m across; rebake a finite, thicker or better-separated hull",
                    fracture::ON_PLANE
                )
            })
        };
        let blocks = chunks
            .iter()
            .map(|&c| {
                let points = asset.chunks[c as usize]
                    .hull
                    .iter()
                    .map(|p| Vec3::from_array(*p))
                    .collect();
                solid(points, format!("chunks[{c}].hull"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let element_blocks = asset
            .elements
            .iter()
            .enumerate()
            .map(|(e, element)| solid(element.points(), format!("elements[{e}].hull")))
            .collect::<Result<Vec<_>, _>>()?;
        let patches = joints
            .iter()
            .enumerate()
            .map(|(j, joint)| {
                let a = blocks[joint.a].face_on(joint.centroid, joint.normal);
                let face = match joint.b {
                    Side::Chunk(b) => fracture::clip_polygon(
                        &a,
                        &blocks[b].face_on(joint.centroid, joint.normal),
                        joint.normal,
                    ),
                    Side::Ground => a,
                };
                fracture::bondsolve::Patch::face(joint, &face)
                    .map_err(|error| format!("joints[{j}].{error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            elements: elements
                .into_iter()
                .map(|range| range.map(|(a, b)| (a as u16, b as u16)))
                .collect(),
            chunks,
            element_of,
            materials,
            joints,
            patches,
            blocks,
            element_blocks,
        })
    }

    /// Certify authored hulls as convex colliders with finite positive mass and inertia,
    /// then retain the structure all instances share. No placement changes its dimensions.
    pub(crate) fn checked_structure(&self, asset: &ConstructionAsset) -> Result<Structure, String> {
        let elements = asset.elements.iter().enumerate().map(|(e, element)| {
            (
                element.points(),
                format!("elements[{e}].hull"),
                element.material,
            )
        });
        let chunks = self
            .blocks
            .iter()
            .zip(&self.chunks)
            .zip(&self.element_of)
            .map(|((block, c), &e)| {
                (
                    block.points.clone(),
                    format!("chunks[{c}].hull"),
                    asset.elements[usize::from(e)].material,
                )
            });
        for (points, field, material) in elements.chain(chunks) {
            let density =
                schema::substance_density(&asset.materials[usize::from(material)])? as f32;
            if !solid_collider(points, density) {
                return Err(format!(
                    "construction {:?}: {field} cannot form a convex collider of finite positive mass and invertible inertia at its material density; rebake a nondegenerate hull at representable authored dimensions",
                    asset.id
                ));
            }
        }
        Ok(Structure {
            geometry: fracture::bondsolve::Geometry::Faces(self.patches.clone()),
            part_of: self.element_of.iter().map(|&e| usize::from(e)).collect(),
            chunks: self
                .chunks
                .iter()
                .zip(&self.materials)
                .map(|(&c, &material)| {
                    let chunk = &asset.chunks[c as usize];
                    Chunk {
                        volume: chunk.volume_m3,
                        centroid: Vec3::from_array(chunk.centroid),
                        mass: chunk.mass_kg,
                        material,
                        buried: chunk.hull.iter().all(|p| p[1] <= GRADE_M + ON_GRADE_M),
                    }
                })
                .collect(),
            joints: self.joints.clone(),
        })
    }

    /// Instruments use the same authored structure as gameplay.
    #[cfg(any(test, feature = "dev_ui"))]
    pub(crate) fn structure(&self, asset: &ConstructionAsset) -> Structure {
        self.checked_structure(asset)
            .expect("instrument data names an admitted construction")
    }

    /// Whether `event` names an element, law chunk or joint of this layout: what a record entry
    /// for this construction must name ([`Shape::apply`]).
    pub(crate) fn admits(&self, event: Event) -> bool {
        let within = |len: usize, i: u16| usize::from(i) < len;
        match event {
            Event::Wear(c) | Event::Dust(c) | Event::Hole(c, _) => within(self.chunks.len(), c),
            Event::Crack(j) | Event::Separate(j) => within(self.joints.len(), j),
        }
    }

    /// The law chunks of `element`, empty for one that never breaks.
    pub(crate) fn chunks_of(&self, element: usize) -> std::ops::Range<usize> {
        self.elements[element].map_or(0..0, |(a, b)| usize::from(a)..usize::from(b))
    }
}

/// One hole in one chunk (module doc): the chunk's matter within `radius` of the segment
/// `from`–`to`. A round's channel runs along its line, each end where the line crosses a face
/// moved out past the chunk ([`open_ends`]), so it is the cylinder the round crossed; a blast's
/// dish has `from == to`, a ball about its centre. In millimetres of the asset frame at authored size,
/// about the baked chunk's centroid ([`super::schema::Chunk::centroid`]); each coordinate saturates
/// at the `i16` range, ±32.767 m from the centroid, which a channel's ends, within two of its chunk's widths of the centroid, keep for
/// every chunk under 16 m across.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hole {
    pub(crate) from: [i16; 3],
    pub(crate) to: [i16; 3],
    pub(crate) radius: u16,
}

impl Hole {
    /// The hole within `radius` of `from`–`to` (m, asset frame at authored size) in a chunk whose
    /// centroid is `centroid`, to the millimetre; `None` for one whose radius rounds to nothing.
    pub(crate) fn cut(centroid: Vec3, from: Vec3, to: Vec3, radius: f32) -> Option<Self> {
        let mm = |p: Vec3| {
            ((p - centroid) * 1000.0)
                .round()
                .to_array()
                .map(|x| x as i16)
        };
        let radius = (radius * 1000.0).round();
        (radius >= 1.0).then(|| Self {
            from: mm(from),
            to: mm(to),
            radius: radius as u16,
        })
    }
}

/// The game's environment destruction causes, with stable [`Layout`] addresses. Wear disturbs
/// occupied matter; dust removes it; cracks remove cohesion; separation ends contact; holes carry presentation geometry.
/// Peers derive element replacement and departure from these causes and the admitted graph.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    Wear(u16),
    Dust(u16),
    Crack(u16),
    Hole(u16, Hole),
    /// Loss of compression contact; cohesion alone may already have cracked.
    Separate(u16),
}

/// One record entry: which construction (its map ordinal), what changed, and the authority
/// tick it changed at (`ShotClock` units).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) construction: u16,
    pub(crate) event: Event,
    pub(crate) tick: u32,
}

/// Why a peer's construction entry is refused where it arrives (`net::construction`), before any record holds it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum EntryError {
    /// The entry names a construction ordinal this map does not place.
    UnknownConstruction(u16),
    /// The entry's event names an element, chunk or joint past its construction's layout.
    OutsideLayout(u16, Event),
}

impl std::fmt::Display for EntryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownConstruction(construction) => {
                write!(f, "construction {construction} is not placed by this map")
            }
            Self::OutsideLayout(construction, event) => {
                write!(
                    f,
                    "{event:?} lies outside construction {construction}'s layout"
                )
            }
        }
    }
}

impl std::error::Error for EntryError {}

impl Entry {
    /// The entry checked against the map's constructions by ordinal: the one gate a peer's entry
    /// passes before a record holds it.
    pub(crate) fn admitted<'a>(
        self,
        layout_of: impl Fn(u16) -> Option<&'a Layout>,
    ) -> Result<Self, EntryError> {
        let layout = layout_of(self.construction)
            .ok_or(EntryError::UnknownConstruction(self.construction))?;
        if layout.admits(self.event) {
            Ok(self)
        } else {
            Err(EntryError::OutsideLayout(self.construction, self.event))
        }
    }
}

/// Every change to every construction in the current world, in authority order (module doc).
/// `generation` moves whenever the record is replaced (a new world, on either peer, or a replica's
/// next ledger): whoever derives from it starts over.
#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub(crate) struct Record {
    pub(crate) entries: Vec<Entry>,
    pub(crate) generation: u32,
}

/// A construction's fates and cracks and which elements are swapped: what the record says of it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shape {
    pub(crate) swapped: Vec<bool>,
    pub(crate) fate: Vec<Fate>,
    pub(crate) cracked: Vec<bool>,
    pub(crate) separated: Vec<bool>,
}

impl Shape {
    /// Every element whole, every chunk standing, every joint intact.
    pub(crate) fn intact(layout: &Layout) -> Self {
        Self {
            swapped: vec![false; layout.elements.len()],
            fate: vec![Fate::Standing; layout.chunks.len()],
            cracked: vec![false; layout.joints.len()],
            separated: vec![false; layout.joints.len()],
        }
    }

    /// Apply an admitted cause. Replacement follows the affected source elements; departure
    /// is derived after the batch by `derive_departures` using the same support law as the sim.
    pub(crate) fn apply(&mut self, event: Event, layout: &Layout) {
        assert!(
            layout.admits(event),
            "a construction record holds only admitted causes: {event:?}"
        );
        let mut touch = |c: usize| self.swapped[usize::from(layout.element_of[c])] = true;
        match event {
            Event::Wear(c) => touch(usize::from(c)),
            Event::Hole(..) => {}
            Event::Dust(c) => {
                touch(usize::from(c));
                self.fate[usize::from(c)] = Fate::Dust;
            }
            Event::Crack(j) | Event::Separate(j) => {
                if matches!(event, Event::Separate(_)) {
                    self.separated[usize::from(j)] = true;
                }
                self.cracked[usize::from(j)] = true;
                let joint = &layout.joints[usize::from(j)];
                touch(joint.a);
                if let Side::Chunk(b) = joint.b {
                    touch(b);
                }
            }
        }
    }

    /// Connectivity is integer-only. Reuse the owner's admitted topology and burial facts;
    /// the authority's wear pools never enter this derivation. Already departed chunks stay out
    /// of the placed owner even when later causes split their moving carriers.
    pub(crate) fn derive_departures(&mut self, layout: &Layout, live: &Live) {
        let mut state = live.state.clone();
        state.fate.clone_from(&self.fate);
        state.cracked.clone_from(&self.cracked);
        state.separated.clone_from(&self.separated);
        for c in fracture::fall(&live.structure, &mut state)
            .into_iter()
            .flatten()
        {
            self.swapped[usize::from(layout.element_of[c])] = true;
        }
        self.fate = state.fate;
    }

    /// Whether law chunk `c` stands as a collider of its own: its element swapped, it standing.
    pub(crate) fn chunk_present(&self, layout: &Layout, c: usize) -> bool {
        self.swapped[usize::from(layout.element_of[c])] && self.fate[c] == Fate::Standing
    }
}

/// One owner's authored construction, law state, drawing holes and reusable assessment storage.
/// Source addresses survive splits; ownership narrows the graph work.
#[derive(Clone)]
pub(crate) struct Live {
    pub(crate) structure: Arc<Structure>,
    pub(crate) state: State,
    pub(crate) swapped: Vec<bool>,
    pub(crate) assessment: Arc<fracture::parts::Assessment>,
    pub(crate) gravity: fracture::parts::Gravity,
    /// Per law chunk: its holes, in record order (module doc).
    pub(crate) holes: Vec<Vec<Hole>>,
    /// Parted joints retained only as carrier grouping at rubble resolution, never as strength.
    pub(crate) rubble_joints: Vec<usize>,
}

impl Live {
    fn new(kind: &ConstructionKind) -> Self {
        let mut live = Self::from_shared(kind.structure.clone(), kind.layout.elements.len());
        live.assessment = kind.equilibrium.clone();
        live
    }

    /// Complete live state built synchronously from a structure and its authored element count.
    #[cfg(any(test, feature = "dev_ui"))]
    pub(crate) fn from_structure(structure: Structure, elements: usize) -> Self {
        Self::from_shared(Arc::new(structure), elements)
    }

    fn from_shared(structure: Arc<Structure>, elements: usize) -> Self {
        Self {
            state: State::new(&structure),
            swapped: vec![false; elements],
            assessment: Arc::new(fracture::parts::Assessment::default()),
            gravity: fracture::parts::Gravity::default(),
            holes: vec![Vec::new(); structure.chunks.len()],
            rubble_joints: Vec::new(),
            structure,
        }
    }

    /// A replacement ledger starts intact without rebuilding immutable construction geometry.
    fn reset(&mut self) {
        *self = Self::from_shared(self.structure.clone(), self.swapped.len());
    }

    /// State for one owned group, retaining the data and wear it had before separation.
    fn group(&self, group: &[usize]) -> Self {
        let mut child = Self {
            structure: self.structure.clone(),
            state: self.state.clone(),
            swapped: vec![true; self.swapped.len()],
            assessment: Arc::new(fracture::parts::Assessment::default()),
            gravity: fracture::parts::Gravity::default(),
            holes: self.holes.clone(),
            rubble_joints: self
                .rubble_joints
                .iter()
                .copied()
                .filter(|&j| {
                    let joint = &self.structure.joints[j];
                    group.binary_search(&joint.a).is_ok()
                        && matches!(joint.b, Side::Chunk(b) if group.binary_search(&b).is_ok())
                })
                .collect(),
        };
        child.state.retain_group(group);
        child
    }

    /// Its fates, cracks and swaps as a [`Shape`].
    pub(crate) fn shape(&self) -> Shape {
        Shape {
            swapped: self.swapped.clone(),
            fate: self.state.fate.clone(),
            cracked: self.state.cracked.clone(),
            separated: self.state.separated.clone(),
        }
    }
}

/// The game's environment destruction state, complete at spawn from admitted construction data.
/// Every group retains its source construction's ordinal and chunk/joint addresses; splitting
/// changes ownership and body origin, never the indices the record and presentation read.
#[derive(Component)]
#[require(crate::contact_transfer::StructureBody)]
pub(crate) struct Breakage {
    pub(crate) kind: Arc<ConstructionKind>,
    /// Stable source identity, shared by placed matter and every group it releases.
    pub(crate) construction: u16,
    pub(crate) live: Box<Live>,
    /// The body's origin in the construction frame; zero for a placed construction.
    origin: Vec3,
}

impl Breakage {
    pub(crate) fn new(kind: Arc<ConstructionKind>, construction: u16) -> Self {
        let live = Box::new(Live::new(&kind));
        Self {
            kind,
            construction,
            live,
            origin: Vec3::ZERO,
        }
    }

    /// The construction frame's world pose, from the body's pose and its data-space origin.
    fn frame(&self, body: &Transform) -> Transform {
        Transform::from_translation(body.translation - body.rotation * self.origin)
            .with_rotation(body.rotation)
    }

    /// Laws use the solver pose, never the eased display transform of a moving body.
    pub(crate) fn physics_frame(&self, (position, rotation): (&Position, &Rotation)) -> Transform {
        self.frame(&Transform::from_translation(position.0).with_rotation(rotation.0))
    }

    pub(crate) fn shape(&self) -> Shape {
        self.live.shape()
    }
}

/// A chunk collider's source law index, under its current standing or moving owner.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[component(immutable)]
pub(crate) struct ChunkCollider(pub(crate) u16);

/// A loose group of a construction's chunks: the construction's ordinal and the group's law chunks,
/// ascending. On the authority it rides the group's dynamic body; the network carries it to
/// replicas on the body's pose carrier, with its birth and optional retirement ticks.
#[derive(Component, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct Piece {
    pub(crate) construction: u16,
    pub(crate) chunks: Vec<u16>,
    /// This group replaces its parent at this authoritative tick (the shared view cursor).
    pub(crate) born: u32,
    /// This group leaves simulation at this tick; its historical pose remains available
    /// until the replica's shared cursor has presented the retirement.
    pub(crate) retired: Option<u32>,
}

/// Geometry admitted once against the selected map before a peer piece reaches the view.
#[derive(Component, Debug, Clone)]
pub(crate) struct AdmittedPiece {
    pub(crate) construction: Entity,
    pub(crate) kind: Arc<ConstructionKind>,
    pub(crate) chunks: Vec<u16>,
    pub(crate) origin: Vec3,
    born: u32,
    ordinal: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PieceError {
    UnknownConstruction(u16),
    Empty(u16),
    OutsideLayout(u16, u16),
    Unordered(u16),
    Changed(u16),
    DuplicateOwnership {
        construction: u16,
        chunk: u16,
        born: u32,
    },
}

impl std::fmt::Display for PieceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownConstruction(c) => write!(
                f,
                "piece names construction {c}, which this map does not place"
            ),
            Self::Empty(c) => write!(f, "construction {c}'s piece has no chunks"),
            Self::OutsideLayout(c, k) => {
                write!(f, "piece chunk {k} lies outside construction {c}'s layout")
            }
            Self::Unordered(c) => write!(
                f,
                "construction {c}'s piece chunks must be unique and ascending"
            ),
            Self::DuplicateOwnership {
                construction,
                chunk,
                born,
            } => write!(
                f,
                "construction {construction}'s chunk {chunk} has multiple carriers born at tick {born}"
            ),
            Self::Changed(c) => write!(
                f,
                "construction {c}'s piece changed its admitted identity (construction, chunks or birth tick)"
            ),
        }
    }
}

impl std::error::Error for PieceError {}

impl Piece {
    pub(crate) fn admitted(
        &self,
        construction: Option<(Entity, &Breakage)>,
    ) -> Result<AdmittedPiece, PieceError> {
        let (root, breakage) = construction
            .filter(|(_, b)| b.construction == self.construction)
            .ok_or(PieceError::UnknownConstruction(self.construction))?;
        if self.chunks.is_empty() {
            return Err(PieceError::Empty(self.construction));
        }
        if let Some(&c) = self
            .chunks
            .iter()
            .find(|&&c| usize::from(c) >= breakage.kind.layout.chunks.len())
        {
            return Err(PieceError::OutsideLayout(self.construction, c));
        }
        if self.chunks.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(PieceError::Unordered(self.construction));
        }
        let kind = &breakage.kind;
        let (mut weighted, mut mass) = (DVec3::ZERO, 0.0);
        for &c in &self.chunks {
            let chunk = &kind.asset.chunks[kind.layout.chunks[usize::from(c)] as usize];
            weighted += DVec3::from_array(chunk.centroid.map(f64::from)) * f64::from(chunk.mass_kg);
            mass += f64::from(chunk.mass_kg);
        }
        Ok(AdmittedPiece {
            construction: root,
            kind: kind.clone(),
            chunks: self.chunks.clone(),
            origin: (weighted / mass).as_vec3(),
            born: self.born,
            ordinal: self.construction,
        })
    }
}

impl AdmittedPiece {
    /// A carrier's group and birth identity are fixed; only retirement may update on the wire.
    pub(crate) fn admits_update(&self, piece: &Piece) -> Result<(), PieceError> {
        if self.ordinal == piece.construction
            && self.chunks == piece.chunks
            && self.born == piece.born
        {
            Ok(())
        } else {
            Err(PieceError::Changed(piece.construction))
        }
    }
}

/// What one shell's march step deposited, in world terms, waiting for [`apply_deposits`].
#[derive(Debug, Default)]
struct RawDeposit {
    /// The game's damage model: resolved line work and momentum, preserving entrance-only shares.
    crossings: Vec<DeliveredLine>,
    /// Each field ray's physical energy, direction, stop and blow.
    rays: Vec<RayDeposit>,
    /// A burst's blast: where, and its energy and charge.
    blast: Option<(Vec3, Blast)>,
    /// A physics step's body contacts, each its construction's root and its load ([`contact_load`]).
    contacts: Vec<(Entity, Load)>,
    /// Owners whose swept topology changed before physics.
    changed: Vec<Entity>,
}

/// The game's environment destruction model: a field ray's payload at its stop. A spall ray
/// carries its share of the energy its cone carries, `strength / N` joules (no HP unit enters a
/// field's energy), and no sourced fragment mass; a casing ray carries its share of the wall's
/// driven energy and its ejection momentum.
#[derive(Debug)]
struct RayDeposit {
    end: Vec3,
    direction: Dir3,
    energy: f32,
    blow: Vec3,
}

impl RayDeposit {
    /// Normalize the recorded angular profile to the casing's one kinetic budget. DERIVED:
    /// `p = 2E / v_s`, so the summed scalar ejection momentum is `M v_s`, and none of this
    /// chemical energy returns to the airblast. The existing ring is the angular sampling
    /// approximation; carried flight energy is not part of the ejection budget.
    fn casing(field: &BurstField, burst: &crate::ballistics::burst::Burst) -> Vec<Self> {
        let total: f64 = field.rays.iter().map(|ray| f64::from(ray.strength)).sum();
        assert!(
            total.is_finite() && total > 0.0,
            "a recorded burst field has a positive finite angular profile"
        );
        field
            .rays
            .iter()
            .map(|ray| {
                let energy = (f64::from(burst.energy) * f64::from(ray.strength) / total) as f32;
                Self {
                    end: ray.end,
                    direction: ray.direction,
                    energy,
                    blow: ray.direction
                        * (2.0 * f64::from(energy) / f64::from(burst.sideways_speed)) as f32,
                }
            })
            .collect()
    }
}

/// The deposits the march and the contacts raised this tick.
#[derive(Resource, Default)]
struct Queue(Vec<RawDeposit>);

/// Per construction root, as of the last physics step: the presses of the bodies asleep against it
/// ([`crate::contact_transfer::Resting`]), each a load with no blow ([`contact_load`]). Every run
/// of the laws on the construction carries them; none starts one.
#[derive(Resource, Default)]
struct RestingLoads(BTreeMap<Entity, Vec<Load>>);

/// The colliders a contact point can lie on: a construction's element or chunk, under its root.
type Colliders<'w, 's> = Query<
    'w,
    's,
    (
        Option<&'static Element>,
        Option<&'static ChunkCollider>,
        &'static ChildOf,
    ),
    Or<(With<Element>, With<ChunkCollider>)>,
>;

mod sweep_intake;
#[cfg(test)]
pub(crate) use sweep_intake::pushes;
pub(crate) use sweep_intake::{ReleasedMotion, SweepAccount, SweepCast, SweepContacts};

/// AUTHORITY: each physics step's body contacts on the constructions as loads, read once a step
/// (module doc): every point on a standing breakable element or chunk queued, the step's as one deposit,
/// with the whole solver transfer divided by the step; and every resting point's press as its
/// construction's ([`RestingLoads`]).
fn read_contacts(
    transfers: Res<ContactTransfers>,
    mut read: Local<u64>,
    colliders: Colliders,
    constructions: Query<((&Position, &Rotation), &Breakage, Option<&Piece>)>,
    mut queue: ResMut<Queue>,
    mut resting: ResMut<RestingLoads>,
    stages: Option<Res<Stages>>,
) {
    let _intake = Stages::start(stages.as_deref(), Stage::Intake);
    if transfers.steps() == *read {
        return;
    }
    *read = transfers.steps();
    let mut inputs: Vec<_> = transfers.groups().map(ContactSource::Group).collect();
    inputs.extend(transfers.resting().iter().map(|rest| {
        ContactSource::Point(ContactInput {
            point: rest.transfer,
            press: rest.press(),
            step: rest.dt,
        })
    }));
    // Only standing construction receives structural loads; moving groups' landings crumble
    // through debris. Reject those roots before grouping or converting their points.
    inputs.retain(|input| {
        constructions
            .get(input.root())
            .is_ok_and(|(_, _, moving)| moving.is_none())
    });
    let awake = inputs
        .iter()
        .map(|input| match input {
            ContactSource::Group(points) => points.len(),
            ContactSource::Point(_) => 0,
        })
        .sum::<usize>();
    let converted = prepare_contacts(
        ComputeTaskPool::get(),
        &inputs,
        &transfers,
        &colliders,
        &constructions,
        stages.as_deref(),
    );
    resting.0.clear();
    let mut contacts = Vec::new();
    for (index, load) in converted.into_iter().enumerate() {
        if let Some((root, load)) = load {
            if index < awake {
                contacts.push((root, load));
            } else {
                resting.0.entry(root).or_default().push(load);
            }
        }
    }
    if !contacts.is_empty() {
        queue.0.push(RawDeposit {
            contacts,
            ..default()
        });
    }
}

/// Test instrumentation at production contact intake, before the laws.
#[cfg(test)]
#[derive(Resource)]
struct ContactIntakeWitness {
    construction: u16,
    source: Entity,
    seen: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(test)]
fn witness_construction_contact(
    queue: Res<Queue>,
    owners: Query<&Breakage>,
    witness: Res<ContactIntakeWitness>,
) {
    if queue
        .0
        .iter()
        .flat_map(|d| &d.contacts)
        .any(|(root, load)| {
            matches!(load.origin, Origin::Contact { other, .. } if other == witness.source)
                && (load.blow != Vec3::ZERO || load.press != Vec3::ZERO)
                && owners
                    .get(*root)
                    .is_ok_and(|owner| owner.construction == witness.construction)
        })
    {
        witness
            .seen
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Require this source's nonzero queued contact; fracture remains observational.
#[cfg(test)]
pub(crate) fn watch_contact_intake(
    app: &mut App,
    construction: u16,
    source: Entity,
) -> Arc<std::sync::atomic::AtomicBool> {
    let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    app.insert_resource(ContactIntakeWitness {
        construction,
        source,
        seen: seen.clone(),
    })
    .add_systems(
        FixedPostUpdate,
        witness_construction_contact
            .after(read_contacts)
            .before(apply_deposits),
    );
    seen
}

/// An awake constraint retains its complete distribution until its owner's worker splits and
/// converts it. A sleeping point already has a known press and needs only frame conversion.
enum ContactSource<'a> {
    Group(ContactGroup<'a>),
    Point(ContactInput),
}

impl ContactSource<'_> {
    fn root(&self) -> Entity {
        match self {
            Self::Group(points) => {
                points
                    .first()
                    .expect("retained contact groups are nonempty")
                    .reference
            }
            Self::Point(input) => input.point.reference,
        }
    }
}

#[derive(Clone, Copy)]
struct ContactInput {
    point: ContactTransfer,
    press: Vec3,
    step: f32,
}

/// Convert complete contact groups on their owner's task. Source-index collection
/// preserves awake and retained-point order independently of task completion.
fn prepare_contacts(
    pool: &TaskPool,
    inputs: &[ContactSource<'_>],
    transfers: &ContactTransfers,
    colliders: &Colliders,
    constructions: &Query<((&Position, &Rotation), &Breakage, Option<&Piece>)>,
    stages: Option<&Stages>,
) -> Vec<Option<(Entity, Load)>> {
    let mut owners: BTreeMap<Entity, Vec<(usize, &ContactSource<'_>)>> = BTreeMap::new();
    for (index, input) in inputs.iter().enumerate() {
        let root = input.root();
        owners.entry(root).or_default().push((index, input));
    }
    if let Some(stages) = stages {
        stages.add(Work::ContactReadRoots, owners.len() as u64);
    }
    let mut converted: Vec<_> = pool
        .scope(|scope| {
            for inputs in owners.into_values() {
                scope.spawn(async move {
                    let convert = |input: ContactInput| {
                        contact_load(
                            colliders,
                            constructions,
                            &input.point,
                            input.press,
                            input.step,
                        )
                    };
                    inputs
                        .into_iter()
                        .map(|(index, input)| {
                            let loads = match input {
                                ContactSource::Group(points) => points
                                    .iter()
                                    .map(|point| {
                                        convert(ContactInput {
                                            point,
                                            press: -point.impulse / transfers.dt(),
                                            step: transfers.dt(),
                                        })
                                    })
                                    .collect(),
                                ContactSource::Point(input) => vec![convert(*input)],
                            };
                            (index, loads)
                        })
                        .collect::<Vec<_>>()
                });
            }
        })
        .into_iter()
        .flatten()
        .collect();
    converted.sort_unstable_by_key(|(index, _)| *index);
    converted.into_iter().flat_map(|(_, loads)| loads).collect()
}

/// A solver contact's world force becomes a load at its occupied chunk, in construction space.
/// An unbreakable element, a body without construction state or a moving group (its landings
/// crumble it, [`crate::debris::crumble`]) has no structural receiver here.
fn contact_load(
    colliders: &Colliders,
    constructions: &Query<((&Position, &Rotation), &Breakage, Option<&Piece>)>,
    transfer: &ContactTransfer,
    press: Vec3,
    step: f32,
) -> Option<(Entity, Load)> {
    let (transform, breakage, moving) = constructions.get(transfer.reference).ok()?;
    if moving.is_some() {
        return None; // A moving group's landings crumble it (`crate::debris::crumble`).
    }
    let (element, chunk, parent) = colliders.get(transfer.reference_collider).expect(
        "a construction collider the last physics step touched stands until the laws' next refit",
    );
    assert_eq!(
        parent.parent(),
        transfer.reference,
        "a construction's collider is its root's child"
    );
    let layout = &breakage.kind.layout;
    let frame = Frame::new(&breakage.physics_frame(transform), breakage.kind.bounds());
    let local = frame.local(transfer.point);
    let chunk = match (element, chunk) {
        (_, Some(&ChunkCollider(c))) => usize::from(c),
        (Some(&Element(e)), None) => nearest_chunk(layout, usize::from(e), local)?,
        (None, None) => unreachable!("Colliders admits elements and chunks only"),
    };
    Some((
        transfer.reference,
        Load {
            chunk,
            at: local,
            energy: 0.0,
            crushed: 0.0,
            blow: Vec3::ZERO,
            press: frame.turn(press),
            peak: None,
            origin: Origin::Contact {
                other: transfer.other,
                step,
            },
        },
    ))
}

/// AUTHORITY: a charge set off at `at` (world), its blast fed to the blast field at the next
/// deposits, as a burst's is.
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct Charge {
    pub(crate) at: Vec3,
    pub(crate) blast: Blast,
}

fn set_off(charge: On<Charge>, mut queue: ResMut<Queue>, stages: Option<Res<Stages>>) {
    let _intake = Stages::start(stages.as_deref(), Stage::Intake);
    queue.0.push(RawDeposit {
        blast: Some((charge.at, charge.blast)),
        ..default()
    });
}

/// The authority's read of one march step ([`ShellMarched`]): its crossings' lines, its new
/// cones' and its burst field's rays, and its burst's blast, queued for [`apply_deposits`]. Runs
/// at the command flush after the authority's march, before a spent shell's despawn.
/// Replica walls change only from the authority's record.
fn read_shell(
    marched: On<ShellMarched>,
    replica: Option<Res<ClientReplica>>,
    shells: Query<(&PenetrationMarks, Option<&SpallMarks>)>,
    mut queue: ResMut<Queue>,
    stages: Option<Res<Stages>>,
) {
    let _intake = Stages::start(stages.as_deref(), Stage::Intake);
    if replica.is_some() {
        return;
    }
    let (marks, spall) = shells.get(marched.shell).expect(
        "ShellMarched is observed before its shell despawns, with its penetration marks intact",
    );
    let mut deposit = RawDeposit::default();
    let marks = marks
        .log
        .get(marched.marks_from..)
        .expect("ShellMarched's mark cursor is its log's length before the step; logs only grow");
    for mark in marks {
        if let ArmourMark::Burst { field_origin, .. } = mark
            && let Some(burst) = marched.burst
        {
            deposit.blast = Some((
                *field_origin,
                Blast::of(burst.blast, burst.charge_radius, burst.products_momentum()),
            ));
        }
    }
    // The game's damage model exposes only lines whose path moments fell in this tick. A
    // crossing mark is immutable metadata and can exist before any of its doses arrive.
    deposit.crossings.extend_from_slice(&marched.lines);
    if let Some(spall) = spall {
        let cones = spall.cones.get(marched.cones_from..).expect(
            "ShellMarched's cone cursor is its cone list's length before the step; lists only grow",
        );
        for cone in cones {
            let share = cone.ray_share();
            deposit.rays.extend(cone.rays.iter().map(|ray| RayDeposit {
                end: ray.end,
                direction: ray.direction,
                energy: ray.strength * share,
                // The game's damage model keeps the debris launch impulse on its emitting
                // construction (walk::Exchange); the field transports work without free bodies.
                blow: Vec3::ZERO,
            }));
        }
        // A source carrying no field energy emits no recorded field.
        if let (Some(burst), Some(field)) = (marched.burst, spall.burst.as_ref()) {
            deposit.rays.extend(RayDeposit::casing(field, &burst));
        }
    }
    if !deposit.crossings.is_empty() || !deposit.rays.is_empty() || deposit.blast.is_some() {
        queue.0.push(deposit);
    }
}

/// A construction's pose: world from its frame (the root's translation and rotation; the root is at
/// authored size), and a ball in world that holds
/// its elements' bounds with room to spare ([`Frame::may_meet`]).
#[derive(Clone, Copy)]
struct Frame {
    translation: Vec3,
    rotation: Quat,
    /// The ball's centre and radius, m, in f64.
    centre: DVec3,
    radius: f64,
}

impl Frame {
    fn new(transform: &Transform, (lo, hi): (Vec3, Vec3)) -> Self {
        let middle = (lo + hi) / 2.0;
        let centre = transform.translation + transform.rotation * middle;
        // Resolution: the bounds' half-diagonal, grown 5 % and half a metre, exceeds rounding in every
        // test the deposits run in the construction frame (DERIVED: f32 rounding, ~1e-4 m at a
        // kilometre).
        let radius = f64::from((hi - lo).length() / 2.0) * 1.05 + 0.5;
        Self {
            translation: transform.translation,
            rotation: transform.rotation,
            centre: centre.as_dvec3(),
            radius,
        }
    }

    /// Whether the ray from `origin` along `dir` (world) can meet the construction's bounds: false only
    /// when it passes clear of the ball, which holds the bounds with room to spare, so every ray
    /// the bounds' own test ([`Block::span`] in the construction frame) accepts is kept.
    fn may_meet(&self, origin: Vec3, dir: Vec3) -> bool {
        let to = self.centre - origin.as_dvec3();
        let far = to.length_squared() - self.radius * self.radius;
        if far <= 0.0 {
            return true;
        }
        let along = to.dot(dir.as_dvec3().normalize_or_zero());
        along > 0.0 && far <= along * along
    }

    /// Whether the world point `at` can lie within the construction's bounds grown by `slack` m.
    fn may_hold(&self, at: Vec3, slack: f32) -> bool {
        let reach = self.radius + f64::from(slack);
        self.centre.distance_squared(at.as_dvec3()) <= reach * reach
    }

    /// A world point in the asset's own frame, at authored size.
    fn local(&self, p: Vec3) -> Vec3 {
        self.rotation.inverse() * (p - self.translation)
    }

    /// A world direction in the construction frame.
    fn turn(&self, d: Vec3) -> Vec3 {
        self.rotation.inverse() * d
    }
}

/// The present blockers along a ray in the asset frame: the first standing chunk of a
/// breakable element it enters, or an unbreakable element entered first, which stops it with no
/// deposit. Returns the law chunk and the distance. `placed` says whether the body is the placed
/// construction, which holds the unbreakable elements; a moving group ([`Piece`]) holds only its
/// own chunks, and an unbreakable element stays with the placed construction.
fn first_chunk(
    layout: &Layout,
    shape: &Shape,
    placed: bool,
    origin: Vec3,
    dir: Vec3,
) -> Option<(Option<usize>, f32)> {
    let mut best: Option<(Option<usize>, f32)> = None;
    for (e, block) in layout.element_blocks.iter().enumerate() {
        if block.span(origin, dir).is_none() {
            continue;
        }
        let hit = match layout.elements[e] {
            None if !placed => None,
            None => block.entry(origin, dir).map(|t| (None, t)),
            Some(_) => layout
                .chunks_of(e)
                .filter(|&c| shape.fate[c] == Fate::Standing)
                .filter_map(|c| layout.blocks[c].entry(origin, dir).map(|t| (Some(c), t)))
                .min_by(|a, b| a.1.total_cmp(&b.1)),
        };
        if let Some(hit) = hit
            && best.is_none_or(|b| hit.1 < b.1)
        {
            best = Some(hit);
        }
    }
    best
}

/// What a construction took of one deposit, in its frame: lines and their loads, field
/// stops, blast hits and contacts.
#[derive(Default)]
struct Taken {
    /// Each line's share of a chunk `(chunk, entry, end, energy J)`, which its hole reads.
    lines: Vec<(usize, Vec3, Vec3, f32)>,
    /// Those lines' loads, each with its share of the round's lost momentum ([`Load::line`]).
    line_loads: Vec<Load>,
    field: Vec<(usize, Vec3, f32, Vec3)>,
    blast: Vec<(usize, fracture::RayHit)>,
    contacts: Vec<Load>,
}

#[derive(bevy::ecs::system::SystemParam)]
struct DepositParam<'w, 's> {
    stages: Option<Res<'w, Stages>>,
    queue: ResMut<'w, Queue>,
    resting: Res<'w, RestingLoads>,
    volumes: Colliders<'w, 's>,
    constructions: Query<
        'w,
        's,
        (
            Entity,
            (&'static Position, &'static Rotation),
            &'static mut Breakage,
            Option<&'static Piece>,
        ),
    >,
    clock: Option<Res<'w, ShotClock>>,
    fixed: Res<'w, Time<Fixed>>,
    record: ResMut<'w, Record>,
    shed: MessageWriter<'w, crate::debris::crumble::Shed>,
    commands: Commands<'w, 's>,
    /// The moving bodies a blast's rays meet ([`moving_near`]) and push.
    spatial: SpatialQuery<'w, 's>,
    collider_of: Query<'w, 's, &'static ColliderOf>,
    children: Query<'w, 's, &'static Children>,
    chunk_entities: ChunkChildren<'w, 's>,
    aabbs: Query<'w, 's, &'static ColliderAabb>,
    bodies: Query<'w, 's, (Forces, &'static RigidBody)>,
    centres: Query<'w, 's, &'static avian3d::prelude::ComputedCenterOfMass>,
}

/// The layers of the surfaces a blast's rays meet through avian: static surfaces
/// ([`Layer::Terrain`]) and a moving body's collision proxy. A tank's
/// armour and witness volumes ([`Layer`]) are its ballistics, not its surface.
const SURFACES: [Layer; 4] = [
    Layer::Terrain,
    Layer::Vehicle,
    Layer::Debris,
    Layer::TrackSide,
];

/// The colliders of every dynamic body whose bounds meet the cube of half-side `reach` m about
/// `origin`, ascending: the moving bodies a blast's rays push. Every other collider a ray meets
/// on [`SURFACES`] stops it with nothing, but a construction's, which its own walk reads.
fn moving_near(p: &DepositParam, origin: Vec3, reach: f32) -> Vec<Entity> {
    let mut near = Vec::new();
    p.spatial.aabb_intersections_with_aabb_callback(
        ColliderAabb::new(origin, Vec3::splat(reach)),
        |collider| {
            if p.collider_of.get(collider).is_ok_and(|of| {
                p.bodies
                    .get(of.body)
                    .is_ok_and(|(_, body)| body.is_dynamic())
                    && !p.constructions.contains(of.body)
            }) {
                near.push(collider);
            }
            true
        },
    );
    near.sort_unstable();
    near
}

/// The rigid field of the owner at `root` in world terms, its construction frame at `origin`: a
/// moving group's body's; a placed construction's is at rest, so its groups come loose at rest.
fn owner_motion(
    bodies: &Query<(Forces, &RigidBody)>,
    centres: &Query<&avian3d::prelude::ComputedCenterOfMass>,
    root: Entity,
    origin: Vec3,
    moving: bool,
) -> ReleasedMotion {
    if !moving {
        return ReleasedMotion::at_rest(origin);
    }
    let (body, _) = bodies
        .get(root)
        .expect("a moving chunk group is a physics body");
    let centre = centres
        .get(root)
        .expect("a physics body has its computed COM");
    ReleasedMotion {
        velocity: body.linear_velocity(),
        spin: body.angular_velocity(),
        centre: body.position().0 + body.rotation().0 * centre.0,
    }
}

/// The spin `run_laws` reads of `motion`, in its construction frame turned by `rotation` (rad/s).
fn construction_spin(motion: &ReleasedMotion, rotation: Quat) -> Vec3 {
    rotation.inverse() * motion.spin
}

/// The field loads whose blow moves a moving group's body once the laws have run on `state`: a
/// field's blow on a chunk it turned to dust went into crushing it and moves nothing.
fn field_blows<'a>(loads: &'a [Load], state: &'a State) -> impl Iterator<Item = &'a Load> {
    loads.iter().filter(|load| {
        matches!(load.origin, Origin::Field)
            && load.blow != Vec3::ZERO
            && state.fate[load.chunk] != Fate::Dust
    })
}

/// The chunk of `element` whose hull lies nearest `at`; `None` for an element that never breaks.
pub(crate) fn nearest_chunk(layout: &Layout, element: usize, at: Vec3) -> Option<usize> {
    layout.chunks_of(element).min_by(|&a, &b| {
        layout.blocks[a]
            .outside(at)
            .total_cmp(&layout.blocks[b].outside(at))
    })
}

/// The game's damage-model intake resolves source ownership before dispatch. Each owner
/// evaluates its crossings in source order; line/impulse effects assemble in entity order.
struct LineInput<'a> {
    line: &'a DeliveredLine,
    element: Option<Element>,
    chunk: Option<ChunkCollider>,
}

fn take_lines(
    pool: &TaskPool,
    p: &DepositParam,
    frames: &BTreeMap<Entity, Frame>,
    deposit: &RawDeposit,
    taken: &mut BTreeMap<Entity, Taken>,
) {
    let mut owners = BTreeMap::<Entity, Vec<LineInput<'_>>>::new();
    for line in &deposit.crossings {
        let Ok((element, chunk, parent)) = p.volumes.get(line.crossing.volume) else {
            continue; // A nonconstruction volume belongs to another law.
        };
        owners.entry(parent.parent()).or_default().push(LineInput {
            line,
            element: element.copied(),
            chunk: chunk.copied(),
        });
    }
    let mut evaluated = pool.scope(|scope| {
        for (root, lines) in owners {
            let (_, _, breakage, _) = p
                .constructions
                .get(root)
                .expect("every construction collider belongs to its live owner");
            let layout = &breakage.kind.layout;
            let frame = frames[&root];
            scope.spawn(async move { (root, take_owner_lines(layout, frame, &lines)) });
        }
    });
    evaluated.sort_unstable_by_key(|(root, _)| *root);
    for (root, mut result) in evaluated {
        if result.lines.is_empty() {
            continue; // These crossings belong only to unbreakable elements, with no law input.
        }
        let into = taken.entry(root).or_default();
        into.lines.append(&mut result.lines);
        into.line_loads.append(&mut result.line_loads);
    }
}

/// Geometry and work/momentum partition for one owner's resolved crossings, before shared effects.
fn take_owner_lines(layout: &Layout, frame: Frame, lines: &[LineInput<'_>]) -> Taken {
    let mut output = Taken::default();
    for source in lines {
        let line = source.line;
        let crate::ballistics::VolumeCrossing {
            entry, end, energy, ..
        } = line.crossing;
        let entrance_energy = line.entrance_energy;
        let impulse = line.momentum;
        let entrance_impulse = line.entrance_momentum;
        let (a, b) = (frame.local(entry), frame.local(end));
        let length = a.distance(b);
        let dir = (b - a).normalize_or_zero();
        let shares: Vec<(usize, f32, f32, f32)> = match (source.element, source.chunk) {
            (_, Some(ChunkCollider(c))) => vec![(usize::from(c), 0.0, length, length)],
            (Some(Element(e)), None) => {
                let chunks: Vec<_> = layout.chunks_of(usize::from(e)).collect();
                if chunks.is_empty() {
                    continue; // An element that never breaks has no chunk to wear.
                }
                let mut crossed = BTreeMap::<usize, (f32, f32, f32)>::new();
                for &(from, to) in &line.contacts {
                    let (from, to) = (frame.local(from), frame.local(to));
                    // World f32 rounding can close a resolved chord to a point.
                    let Some(ray) = (to - from).try_normalize() else {
                        continue;
                    };
                    for &c in &chunks {
                        let Some((t0, t1)) = layout.blocks[c].span(from, ray) else {
                            continue; // This sample missed the chunk.
                        };
                        let (t0, t1) = (t0.max(0.0), t1.min(from.distance(to)));
                        if t1 <= t0 {
                            continue; // The sample met this chunk outside its exposed chord.
                        }
                        let (start, end) = (
                            (from + ray * t0 - a).dot(dir),
                            (from + ray * t1 - a).dot(dir),
                        );
                        let span = crossed.entry(c).or_insert((start, end, 0.0));
                        span.0 = span.0.min(start);
                        span.1 = span.1.max(end);
                        span.2 += t1 - t0;
                    }
                }
                if crossed.is_empty() {
                    // A skid's line has no chord, and rounding can close a short one or carry it
                    // off every hull: the chunk at the line's own point takes it whole.
                    let owner = nearest_chunk(layout, usize::from(e), a)
                        .expect("this element's chunks were found above");
                    crossed.insert(owner, (0.0, length, length));
                }
                crossed
                    .into_iter()
                    .map(|(c, (a, b, weight))| (c, a, b, weight))
                    .collect()
            }
            (None, None) => unreachable!("Colliders admits elements and chunks only"),
        };
        let &(first, _, _, _) = shares
            .iter()
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .expect("a breakable element or chunk owns the resolved contact");
        let chord: f32 = shares.iter().map(|(_, _, _, weight)| weight).sum();
        let turned = frame.turn(impulse - entrance_impulse);
        for (c, t0, t1, weight) in shares {
            // Rounding can close a single owner's line; it still takes the line whole.
            let share = if chord > 0.0 { weight / chord } else { 1.0 };
            let entry_energy = if c == first { entrance_energy } else { 0.0 };
            let entry_blow = if c == first {
                frame.turn(entrance_impulse)
            } else {
                Vec3::ZERO
            };
            let (from, to) = ((a + dir * t0), (a + dir * t1));
            let part = DeliveredLine {
                contacts: line.contacts.clone(),
                crossing: crate::ballistics::VolumeCrossing {
                    entry: from,
                    end: to,
                    energy: (energy - entrance_energy) * share + entry_energy,
                    ..line.crossing
                },
                momentum: turned * share + entry_blow,
                entrance_energy: entry_energy,
                entrance_momentum: entry_blow,
            };
            output.lines.push((c, from, to, part.crossing.energy));
            output
                .line_loads
                .extend(resolved_line_loads(c, &layout.materials[c], &part));
        }
    }
    output
}

/// The game's damage-model construction loads: entrance work and momentum stay at entry;
/// the remaining work and momentum follow the chord. The construction and wall inspector read this
/// same owner. The line's total energy sizes its chunk's visual channel independently of how
/// the solver samples its loads.
pub(crate) fn resolved_line_loads(
    chunk: usize,
    material: &Material,
    line: &DeliveredLine,
) -> Vec<Load> {
    let crossing = line.crossing;
    Load::field(
        &[(
            chunk,
            crossing.entry,
            line.entrance_energy,
            line.entrance_momentum,
        )],
        |_| material.crush,
    )
    .into_iter()
    .chain(Load::line(
        chunk,
        material,
        crossing.entry,
        crossing.end,
        crossing.energy - line.entrance_energy,
        line.momentum - line.entrance_momentum,
    ))
    .collect()
}

/// Effects evaluated from one owner's state; shared bodies and the record change only at commit.
struct OwnerEffects {
    root: Entity,
    construction: u16,
    fields: Vec<Load>,
    events: Vec<Event>,
    split: Option<PendingSplit>,
    departures: Vec<PreparedDeparture>,
    sheds: Vec<crate::debris::crumble::Shed>,
    solves: usize,
}

/// Immutable inputs for one owner's law run; shared-body effects are staged in `OwnerEffects`.
struct OwnerInputs<'a> {
    pool: &'a TaskPool,
    reusable: &'a ReusableColliders,
    taken: &'a Taken,
    pressing: &'a [Load],
    resting: &'a [Load],
    /// The owner body's rigid field in world terms; placed matter is at rest.
    motion: ReleasedMotion,
    tick: u32,
    step: f32,
}

fn evaluate_owner(
    root: Entity,
    breakage: &mut Breakage,
    moving: Option<&[u16]>,
    construction_pose: Transform,
    inputs: OwnerInputs,
) -> OwnerEffects {
    let OwnerInputs {
        pool,
        reusable,
        taken,
        pressing,
        resting,
        motion,
        tick,
        step,
    } = inputs;
    let spin = construction_spin(&motion, construction_pose.rotation);
    let kind = breakage.kind.clone();
    let construction = breakage.construction;
    let live = breakage.live.as_mut();
    let blast = blast_loads(&kind, &taken.blast, live);
    let holes = holes_of(&kind, &taken.lines, &blast);
    let loads: Vec<Load> = taken
        .line_loads
        .iter()
        .copied()
        .chain(Load::field(&taken.field, |c| {
            kind.layout.materials[c].crush
        }))
        .chain(blast)
        .chain(taken.contacts.iter().map(|load| Load {
            press: Vec3::ZERO,
            ..*load
        }))
        .chain(pressing.iter().copied())
        .chain(resting.iter().copied())
        .collect();
    let before = live.state.separated.clone();
    let mut changes = run_laws(&kind.layout.element_of, live, &loads, holes, spin, step);
    let parted: Vec<_> = live
        .state
        .separated
        .iter()
        .enumerate()
        .filter_map(|(j, &now)| (now && !before[j]).then_some(j))
        .collect();
    let (groups, sources): (Vec<_>, Vec<_>) =
        std::mem::take(&mut changes.groups).into_iter().unzip();
    let groups = crate::debris::crumble::merge_fragments(
        &live.structure,
        groups,
        &parted,
        &mut live.rubble_joints,
    );
    let owners: HashMap<_, _> = groups
        .iter()
        .enumerate()
        .flat_map(|(g, chunks)| chunks.iter().map(move |&c| (c, g)))
        .collect();
    let mut regrouped = vec![Vec::new(); groups.len()];
    for load in sources.into_iter().flatten() {
        regrouped[*owners
            .get(&load.chunk)
            .expect("each departing load belongs to one departing group")]
        .push(load);
    }
    // A carrier which did not split releases no new fragment. Its existing matter stays with it:
    // the size split routed it when it formed.
    let unchanged = moving.is_some_and(|owned| {
        groups.len() == 1
            && groups[0]
                .iter()
                .copied()
                .eq(owned.iter().map(|&c| usize::from(c)))
    });
    let sheds = if unchanged {
        changes.groups = groups.into_iter().zip(regrouped).collect();
        Vec::new()
    } else {
        // Groups partition matter, so each one's first chunk names its loads.
        let mut loads: HashMap<usize, Vec<Load>> =
            groups.iter().map(|group| group[0]).zip(regrouped).collect();
        let mut record = Record::default();
        let (kept, sheds) = crate::debris::crumble::route(
            &live.structure,
            (construction, construction_pose),
            groups,
            |light| {
                // One task per decor group: each launch reads only its own chunks and loads.
                pool.scope(|scope| {
                    for group in light {
                        let (kind, structure, loads) = (&kind, &live.structure, &loads[&group[0]]);
                        scope.spawn(async move {
                            let launch = launch(kind, structure, group, loads);
                            let at = construction_pose.transform_point(launch.centre);
                            ReleasedMotion {
                                velocity: motion.velocity
                                    + motion.spin.cross(at - motion.centre)
                                    + construction_pose.rotation * launch.velocity,
                                spin: motion.spin + construction_pose.rotation * launch.spin,
                                centre: at,
                            }
                        });
                    }
                })
            },
            tick,
            &mut record,
        );
        crate::debris::crumble::mark_dust(&mut live.state, &sheds);
        changes
            .events
            .extend(record.entries.into_iter().map(|entry| entry.event));
        changes.groups = kept
            .into_iter()
            .map(|group| {
                let loads = loads
                    .remove(&group[0])
                    .expect("each kept group is one the laws released");
                (group, loads)
            })
            .collect();
        sheds
    };
    // Moving matter already has a carrier; only surviving field loads push it.
    let fields = if moving.is_some() {
        field_blows(&loads, &live.state).copied().collect()
    } else {
        Vec::new()
    };
    let mut departures = Vec::new();
    let split = if moving.is_some() {
        let groups: Vec<_> = changes.groups.into_iter().map(|(group, _)| group).collect();
        if unchanged {
            for &c in &groups[0] {
                live.state.fate[c] = Fate::Standing;
            }
            None
        } else {
            let children: Vec<_> = groups
                .into_iter()
                .map(|group| {
                    let state = live.group(&group);
                    (group, state)
                })
                .collect();
            for (group, _) in &children {
                for &c in group {
                    live.state.fate[c] = Fate::Standing;
                }
            }
            Some(PendingSplit { children, tick })
        }
    } else {
        // Direct scoped submissions return in group order; all geometry is ready before commit.
        departures = pool.scope(|scope| {
            let kind = &kind;
            let live = &*live;
            for (group, loads) in changes.groups {
                scope.spawn(async move {
                    let launch = launch(kind, &live.structure, &group, &loads);
                    prepare_group(
                        Departure {
                            kind: kind.clone(),
                            construction,
                            root: construction_pose,
                            live: live.group(&group),
                            group,
                            launch,
                            tick,
                        },
                        reusable,
                    )
                });
            }
        });
        None
    };
    OwnerEffects {
        root,
        construction,
        fields,
        events: changes.events,
        solves: changes.solves,
        split,
        departures,
        sheds,
    }
}

/// AUTHORITY: run the tick's deposits through the laws (module doc), record every change and spawn
/// a body per loose group.
fn apply_deposits(mut p: DepositParam) {
    if p.queue.0.is_empty()
        && !p.constructions.iter().any(|(root, pose, breakage, piece)| {
            let frame = breakage.physics_frame(pose);
            breakage.live.assessment.needs_tick()
                || (piece.is_some()
                    && breakage.live.assessment.spin_changed(construction_spin(
                        &owner_motion(&p.bodies, &p.centres, root, frame.translation, true),
                        frame.rotation,
                    )))
        })
    {
        return;
    } // No changed load, topology, unfinished solve or moving-frame acceleration.
    let intake = Stages::start(p.stages.as_deref(), Stage::Intake);
    // A press lasts through the physics step, including every other deposit in this pass.
    // The contact deposit retains its blow; its press joins every run exactly once here.
    let mut presses: BTreeMap<Entity, Vec<Load>> = BTreeMap::new();
    for &(root, load) in p.queue.0.iter().flat_map(|deposit| &deposit.contacts) {
        if load.press != Vec3::ZERO {
            presses.entry(root).or_default().push(Load {
                blow: Vec3::ZERO,
                ..load
            });
        }
    }
    let tick = p.clock.as_deref().map_or(0, |clock| clock.0.raw());
    let frames: BTreeMap<Entity, Frame> = p
        .constructions
        .iter()
        .map(|(root, transform, breakage, _)| {
            (
                root,
                Frame::new(&breakage.physics_frame(transform), breakage.kind.bounds()),
            )
        })
        .collect();
    drop(intake);
    let step = p.fixed.timestep().as_secs_f32();
    let mut taken: BTreeMap<Entity, Taken> = BTreeMap::new();
    for deposit in std::mem::take(&mut p.queue.0) {
        let intake = Stages::start(p.stages.as_deref(), Stage::Intake);
        take_lines(ComputeTaskPool::get(), &p, &frames, &deposit, &mut taken);
        // All deposits read the tick's intake geometry. Each owner keeps ray order;
        // nearest-stop reduction keeps source-entity order, independent of worker completion.
        if !deposit.rays.is_empty() || deposit.blast.is_some() {
            let mut geometry = bevy::utils::Parallel::<Vec<OwnerGeometry>>::default();
            p.constructions
                .par_iter()
                .for_each(|(root, _, breakage, piece)| {
                    let result =
                        owner_geometry(root, frames[&root], breakage, piece.is_none(), &deposit);
                    geometry.scope(|results| results.push(result));
                });
            let (fields, stops) =
                reduce_geometry(geometry.drain().collect(), deposit.blast.is_some());
            for (root, field) in fields {
                taken.entry(root).or_default().field.extend(field);
            }
            if let Some((origin, blast)) = deposit.blast {
                cast_blast(&mut p, stops, &mut taken, origin, blast);
            }
        }
        for root in deposit.changed {
            taken.entry(root).or_default();
        }
        for &(root, load) in &deposit.contacts {
            taken.entry(root).or_default().contacts.push(load);
        }
        drop(intake);
    }
    for (root, _, breakage, piece) in p.constructions.iter() {
        let frame = frames[&root];
        let motion = owner_motion(
            &p.bodies,
            &p.centres,
            root,
            frame.translation,
            piece.is_some(),
        );
        let spin = construction_spin(&motion, frame.rotation);
        if breakage.live.assessment.needs_tick() || breakage.live.assessment.spin_changed(spin) {
            taken.entry(root).or_default();
        }
    }
    if taken.is_empty() {
        return;
    } // Every owner is equilibrated and received no changed loads.
    {
        let _structure = Stages::start(p.stages.as_deref(), Stage::Structure);
        let mut departures = Vec::new();
        {
            let batch = taken;
            let mut effects = bevy::utils::Parallel::<Vec<OwnerEffects>>::default();
            let children = &p.children;
            let chunk_entities = &p.chunk_entities;
            let (bodies, centres) = (&p.bodies, &p.centres);
            let resting = &p.resting.0;
            let stages = p.stages.as_deref();
            // One owner per task: an owner's laws scale with the chunks its loads reach, from one
            // to hundreds, so contiguous batches of owners cannot balance them.
            p.constructions
                .par_iter_many_unique_mut(batch.keys())
                .batching_strategy(bevy::ecs::batching::BatchingStrategy::fixed(1))
                .for_each(|(root, transform, mut breakage, piece)| {
                    let taken = &batch[&root];
                    let construction = breakage.physics_frame(transform);
                    let motion = owner_motion(
                        bodies,
                        centres,
                        root,
                        construction.translation,
                        piece.is_some(),
                    );
                    let reusable = match piece {
                        // Moving owners transfer their colliders in split_groups; their laws
                        // prepare no departure that would read these.
                        Some(_) => ReusableColliders::Standing(BTreeMap::new()),
                        // A construction without children has no chunk collider to rehome.
                        None => reusable_colliders(children.get(root).ok(), chunk_entities),
                    };
                    let effect = evaluate_owner(
                        root,
                        breakage.bypass_change_detection(),
                        piece.map(|piece| piece.chunks.as_slice()),
                        construction,
                        OwnerInputs {
                            pool: ComputeTaskPool::get(),
                            reusable: &reusable,
                            taken,
                            pressing: presses.get(&root).map_or(&[][..], Vec::as_slice),
                            resting: resting.get(&root).map_or(&[][..], Vec::as_slice),
                            motion,
                            tick,
                            step,
                        },
                    );
                    if let Some(stages) = stages {
                        stages.add(Work::AssessedOwners, 1);
                        stages.add(Work::StructureSolveColumns, effect.solves as u64);
                    }
                    count_prepared(stages, &effect.departures);
                    // Only recorded changes require a collider refit.
                    if !effect.events.is_empty() {
                        breakage.set_changed();
                    }
                    effects.scope(|effects| effects.push(effect));
                });
            let mut effects: Vec<_> = effects.drain().collect();
            assert_eq!(
                effects.len(),
                batch.len(),
                "every deposit owner has complete construction state and a physics pose"
            );
            effects.sort_unstable_by_key(|effect| effect.root);
            // Records, source impulses and carrier creation keep source-entity order,
            // independent of worker completion. Every owner commits once after tick intake.
            for effect in effects {
                p.shed.write_batch(effect.sheds);
                let frame = frames[&effect.root];
                if !effect.fields.is_empty() {
                    let (mut body, _) = p
                        .bodies
                        .get_mut(effect.root)
                        .expect("a moving chunk group is a physics body");
                    for load in effect.fields {
                        body.apply_linear_impulse_at_point(
                            frame.rotation * load.blow,
                            frame.translation + frame.rotation * load.at,
                        );
                    }
                }
                p.record
                    .entries
                    .extend(effect.events.into_iter().map(|event| Entry {
                        construction: effect.construction,
                        event,
                        tick,
                    }));
                if let Some(split) = effect.split {
                    p.commands.entity(effect.root).insert(split);
                }
                departures.extend(effect.departures);
            }
        }
        for departure in departures {
            spawn_prepared(&mut p.commands, departure);
        }
    }
}

/// A blast's ray hits on one construction as its loads ([`Load::blast`]), each face's Law 2 read on
/// its chunk's hull at authored size.
fn blast_loads(
    kind: &ConstructionKind,
    hits: &[(usize, fracture::RayHit)],
    live: &Live,
) -> Vec<Load> {
    if hits.is_empty() {
        return Vec::new(); // This deposit has no blast surface stops.
    }
    let contacts: Vec<Joint> = live.state.contacts(&live.structure).collect();
    let (loads, _) = Load::blast(hits, |c| fracture::Backing {
        block: &kind.layout.blocks[c],
        material: kind.layout.materials[c],
        contacts: &contacts,
    });
    loads
}

type FieldStop = (usize, Vec3, f32, Vec3);

struct BlastStop {
    distance: f32,
    /// Unbreakable elements shade the ray but receive no game-state load.
    surface: Option<fracture::BlastSurface<Stop>>,
}

struct OwnerGeometry {
    root: Entity,
    field: Vec<FieldStop>,
    blast: Vec<(usize, BlastStop)>,
}

fn owner_geometry(
    root: Entity,
    frame: Frame,
    breakage: &Breakage,
    placed: bool,
    deposit: &RawDeposit,
) -> OwnerGeometry {
    if !deposit
        .rays
        .iter()
        .any(|ray| ray.energy > 0.0 && frame.may_hold(ray.end, STOP_TOLERANCE_M))
        && !deposit
            .blast
            .is_some_and(|(origin, blast)| frame.may_hold(origin, blast.reach()))
    {
        return OwnerGeometry {
            root,
            field: Vec::new(),
            blast: Vec::new(),
        };
    }
    let kind = &breakage.kind;
    let shape = breakage.shape();
    let (lo, hi) = kind.bounds();
    let mut field = Vec::new();
    for ray in &deposit.rays {
        let (end, dir, energy) = (ray.end, Vec3::from(ray.direction), ray.energy);
        if energy == 0.0 || !frame.may_hold(end, STOP_TOLERANCE_M) {
            continue; // A zero-profile angular stratum emits nothing, or this owner is out of reach.
        }
        let local = frame.local(end);
        let slack = Vec3::splat(STOP_TOLERANCE_M);
        if local.cmplt(lo - slack).any() || local.cmpgt(hi + slack).any() {
            continue;
        }
        let turned = frame.turn(dir);
        let back = 2.0 * STOP_TOLERANCE_M;
        if let Some((Some(c), t)) =
            first_chunk(&kind.layout, &shape, placed, local - turned * back, turned)
            && (t - back).abs() <= STOP_TOLERANCE_M
        {
            field.push((c, local, energy, frame.turn(ray.blow)));
        }
    }
    let mut faces = BTreeMap::new();
    let blast = deposit.blast.map_or_else(Vec::new, |(origin, blast)| {
        let reach = blast.reach();
        let local = frame.local(origin);
        let bounds = Block::cuboid(lo, hi);
        fracture::blast_directions()
            .iter()
            .enumerate()
            .filter_map(|(ray, &(dir, _))| {
                if !frame.may_meet(origin, dir) {
                    return None;
                }
                let turned = frame.turn(dir);
                bounds.span(local, turned)?;
                let (chunk, t) = first_chunk(&kind.layout, &shape, placed, local, turned)?;
                let distance = t;
                if distance > reach {
                    return None;
                }
                let surface = chunk.map(|c| {
                    let block = &kind.layout.blocks[c];
                    let (_, normal) = block
                        .entry_face(local, turned)
                        .expect("the ray's stop is where it enters the chunk");
                    let shown = fracture::silhouette(
                        faces.entry(c).or_insert_with(|| block.face_areas()),
                        turned,
                    );
                    fracture::BlastSurface {
                        target: Stop::Chunk(root, c),
                        at: (local + turned * t),
                        normal,
                        direction: turned,
                        distance,
                        silhouette: shown,
                        material: Some(kind.layout.materials[c]),
                    }
                });
                Some((ray, BlastStop { distance, surface }))
            })
            .collect()
    });
    OwnerGeometry { root, field, blast }
}

/// Strict distance comparison retains the first source entity on a tie, matching deposit order.
fn reduce_geometry(
    mut owners: Vec<OwnerGeometry>,
    blast: bool,
) -> (Vec<(Entity, Vec<FieldStop>)>, Vec<Option<BlastStop>>) {
    owners.sort_unstable_by_key(|owner| owner.root);
    let mut stops: Vec<Option<BlastStop>> = if blast {
        std::iter::repeat_with(|| None)
            .take(fracture::blast_directions().len())
            .collect()
    } else {
        Vec::new()
    };
    let mut fields = Vec::new();
    for owner in owners {
        if !owner.field.is_empty() {
            fields.push((owner.root, owner.field));
        }
        for (ray, candidate) in owner.blast {
            let nearest = &mut stops[ray];
            if nearest
                .as_ref()
                .is_none_or(|stop| candidate.distance < stop.distance)
            {
                *nearest = Some(candidate);
            }
        }
    }
    (fields, stops)
}

/// Each chunk and body collider takes the share of its rays' cones its silhouette intercepts
/// ([`fracture::intercepted`]): a chunk's from its faces, a collider's from its bounds, the
/// silhouette of the box that holds it. A collider holding `origin` shades nothing and takes
/// nothing: what a burst inside a body does to it is no law here.
fn cast_blast(
    p: &mut DepositParam,
    stops: Vec<Option<BlastStop>>,
    taken: &mut BTreeMap<Entity, Taken>,
    origin: Vec3,
    blast: Blast,
) {
    let reach = blast.reach();
    let near = moving_near(p, origin, reach);
    let surfaces = SpatialQueryFilter::from_mask(SURFACES);
    let inside = p.spatial.point_intersections(origin, &surfaces);
    let surfaces = surfaces.with_excluded_entities(inside);
    // A construction's colliders are its walk's ([`first_chunk`]); every other surface stops a ray.
    let meets = |collider: Entity| {
        near.binary_search(&collider).is_ok()
            || !p
                .collider_of
                .get(collider)
                .is_ok_and(|of| p.constructions.contains(of.body))
    };
    let mut stops = stops.into_iter();
    let hits = fracture::trace_blast(blast, fracture::blast_directions(), |dir| {
        let stop = stops
            .next()
            .expect("geometry supplies one stop per declared blast ray");
        let surface = p.spatial.cast_ray_predicate(
            origin,
            Dir3::new(dir).expect("the blast field's directions are unit"),
            stop.as_ref().map_or(reach, |stop| stop.distance),
            true,
            &surfaces,
            &meets,
        );
        if let Some(hit) = surface {
            let aabb = p
                .aabbs
                .get(hit.entity)
                .expect("a world-ray target has collider bounds");
            let size = aabb.max - aabb.min;
            let shown = dir.x.abs() * size.y * size.z
                + dir.y.abs() * size.x * size.z
                + dir.z.abs() * size.x * size.y;
            if near.binary_search(&hit.entity).is_err() {
                return None; // Static surfaces shade the ray; constructions use their chunk walk.
            }
            return Some(fracture::BlastSurface {
                target: Stop::Body(hit.entity),
                at: origin + dir * hit.distance,
                normal: hit.normal,
                direction: dir,
                distance: hit.distance,
                silhouette: shown,
                material: None,
            });
        }
        stop?.surface
    });
    for (target, hit) in hits {
        match target {
            Stop::Chunk(root, c) => taken.entry(root).or_default().blast.push((c, hit)),
            Stop::Body(collider) => {
                let of = p
                    .collider_of
                    .get(collider)
                    .expect("a collider in reach has a body");
                let (mut forces, _) = p
                    .bodies
                    .get_mut(of.body)
                    .expect("a body in reach is dynamic");
                forces.apply_linear_impulse_at_point(hit.impulse, hit.at);
            }
        }
    }
}

/// What a blast ray stopped on ([`cast_blast`]): a construction's law chunk, or a moving body's
/// collider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stop {
    Chunk(Entity, usize),
    Body(Entity),
}

/// The holes one deposit cuts (module doc), each with its law chunk, in the baked chunk's frame
/// ([`Hole`]); `lines` and the blast's loads `blast` ([`Load::blast`]) in the construction frame at
/// authored size. Each is sized by the volume it crushes, the volume its load takes from
/// the chunk's pool: a line's, its energy over the crush energy ([`Material::crush`]); a blast's,
/// its load's Law 1 volume ([`Load::crushed`]). A line's channel runs along its chord, its radius
/// the narrower of the cylinder about the chord and the ball that hold that volume (so a chord
/// grazing a corner never cuts a disc wider than the ball); the blast's dish in a chunk is a ball
/// about the chunk's load (its face's impulse-weighted centroid) whose half holds it. ASSUMED: a
/// channel's section is a circle and its width the energy's alone, not the round's diameter; a
/// dish's centre lies on the chunk's face, so half its ball is in the chunk.
fn holes_of(
    kind: &ConstructionKind,
    lines: &[(usize, Vec3, Vec3, f32)],
    blast: &[Load],
) -> Vec<(usize, Hole)> {
    let layout = &kind.layout;
    cut_holes(
        &layout.blocks,
        |c| Vec3::from_array(kind.asset.chunks[layout.chunks[c] as usize].centroid),
        |c| layout.materials[c].crush,
        lines,
        blast,
    )
}

/// [`holes_of`] on any chunk table: each law chunk's hull `blocks`, its baked `centroid` (the
/// hole's frame) and its material's crush energy `crush`, all at authored size. The sandbox's wall cuts
/// its holes here too, so its deposits reach [`HOLES_PER_CHUNK`] as the game's do.
pub(crate) fn cut_holes(
    blocks: &[Block],
    centroid: impl Fn(usize) -> Vec3,
    crush: impl Fn(usize) -> f32,
    lines: &[(usize, Vec3, Vec3, f32)],
    blast: &[Load],
) -> Vec<(usize, Hole)> {
    use std::f32::consts::PI;
    let crushed = |c: usize, energy: f32| energy / crush(c);
    let channels = lines.iter().filter_map(|&(c, from, to, energy)| {
        let volume = crushed(c, energy);
        let ball = (0.75 * volume / PI).cbrt();
        let radius = (volume / (PI * from.distance(to))).sqrt().min(ball);
        let (from, to) = open_ends(&blocks[c], from, to);
        Hole::cut(centroid(c), from, to, radius).map(|hole| (c, hole))
    });
    let dishes = blast.iter().filter_map(|load| {
        let radius = (1.5 * load.crushed / PI).cbrt();
        let at = load.at;
        Hole::cut(centroid(load.chunk), at, at, radius).map(|hole| (load.chunk, hole))
    });
    channels.chain(dishes).collect()
}

/// Resolution: endpoints within a millimetre of a rounded hull face use the full along-face
/// opening. This bounds the f32 baked-plane tolerance, never a material or hole-size choice.
const ON_FACE_M: f32 = 1e-3;

/// A channel's ends `from`–`to` in its chunk's hull `block` (asset frame), each surface end moved
/// out along the line to the hull's farthest corner. Inside the chunk the capsule a [`Hole`] draws is then the cylinder about the line:
/// every chunk point projects between the ends, so its distance to the segment is its distance to
/// the line, and each face the line crosses opens across the cylinder's whole ellipse, with no rim
/// closed by the capsule's rounded end. An end inside the hull (a round at rest there) stays, so
/// its channel stays closed there. The ends lie within the hull's width of the hull.
fn open_ends(block: &Block, from: Vec3, to: Vec3) -> (Vec3, Vec3) {
    let Some(along) = (to - from).try_normalize() else {
        return (from, to);
    };
    let on_face = |p: Vec3| block.planes.iter().any(|&(n, o)| n.dot(p) >= o - ON_FACE_M);
    let (low, high) = block
        .points
        .iter()
        .map(|p| (*p - from).dot(along))
        .fold((0.0_f32, from.distance(to)), |(low, high), s| {
            (low.min(s), high.max(s))
        });
    (
        if on_face(from) {
            from + along * low
        } else {
            from
        },
        if on_face(to) { from + along * high } else { to },
    )
}

/// One owner's recorded causes and released groups in this tick.
/// `solves` counts the part solve's load columns; zero means the last field was reused.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Changes {
    pub(crate) events: Vec<Event>,
    pub(crate) groups: Vec<(Vec<usize>, Vec<Load>)>,
    pub(crate) failures: Vec<fracture::Failure>,
    pub(crate) solves: usize,
}

/// Apply one tick's loads and holes, judge every part interface once, and record simultaneous
/// failures. Holes are view-only; removal consumes their volume pool. Connectivity follows
/// topology changes; redistribution continues on subsequent ticks. The construction design's "Parts decide
/// what stands" owns pulse and failure approximations.
pub(crate) fn run_laws(
    element_of: &[u16],
    live: &mut Live,
    loads: &[Load],
    mut holes: Vec<(usize, Hole)>,
    spin: Vec3,
    step: f32,
) -> Changes {
    let struck = fracture::strike(&live.structure, &mut live.state, loads);
    holes.retain(|&(c, hole)| {
        let kept = live.state.standing(c) && live.holes[c].len() < HOLES_PER_CHUNK;
        if kept {
            live.holes[c].push(hole);
        }
        kept
    });
    let loads: Vec<Load> = loads
        .iter()
        .filter(|load| live.state.standing(load.chunk))
        .copied()
        .collect();
    let changed = !struck.dusted.is_empty() || !struck.cracked.is_empty();
    let (failures, solves) = Arc::make_mut(&mut live.assessment).advance(
        &live.structure,
        &mut live.state,
        &loads,
        spin,
        step,
        changed,
    );
    let structural: Vec<usize> = failures.iter().map(|f| f.joint).collect();
    let groups: Vec<_> = live
        .assessment
        .support
        .iter()
        .filter(|group| live.state.standing(group[0]))
        .cloned()
        .collect();
    for &c in groups.iter().flatten() {
        live.state.fate[c] = Fate::Fallen;
    }
    let joints = &live.structure.joints;
    let cracked_sides = struck
        .cracked
        .iter()
        .chain(&structural)
        .flat_map(|&j| match joints[j].b {
            Side::Chunk(b) => vec![joints[j].a, b],
            Side::Ground => vec![joints[j].a],
        });
    let mut swaps: Vec<usize> = struck
        .worn
        .iter()
        .chain(&struck.dusted)
        .chain(groups.iter().flatten())
        .copied()
        .chain(cracked_sides)
        .map(|c| usize::from(element_of[c]))
        .filter(|&e| !live.swapped[e])
        .collect();
    swaps.sort_unstable();
    swaps.dedup();
    for &e in &swaps {
        live.swapped[e] = true;
    }
    // Only first wear disturbs an intact element. Later wear stays in the authority's pool;
    // dust, holes and cracks carry every further presentation change.
    let mut worn_elements = BTreeSet::new();
    let mut events: Vec<_> = struck
        .worn
        .iter()
        .copied()
        .filter(|&c| {
            swaps.binary_search(&usize::from(element_of[c])).is_ok()
                && worn_elements.insert(element_of[c])
        })
        .map(|c| Event::Wear(c as u16))
        .collect();
    events.extend(struck.dusted.iter().map(|&c| Event::Dust(c as u16)));
    events.extend(holes.iter().map(|&(c, hole)| Event::Hole(c as u16, hole)));
    events.extend(struck.cracked.iter().chain(&structural).map(|&j| {
        if live.state.separated[j] {
            Event::Separate(j as u16)
        } else {
            Event::Crack(j as u16)
        }
    }));
    let groups = departure_loads(&live.structure, groups, &loads, &failures);
    Changes {
        events,
        groups,
        failures,
        solves,
    }
}

/// Partition source loads once by departing ownership. One failed joint's work is shared
/// between its departing sides, or assigned whole to its only departing side. No source is
/// copied onto unrelated groups, and no opening is charged twice.
fn departure_loads(
    structure: &Structure,
    groups: Vec<Vec<usize>>,
    loads: &[Load],
    failures: &[fracture::Failure],
) -> Vec<(Vec<usize>, Vec<Load>)> {
    let owner: HashMap<_, _> = groups
        .iter()
        .enumerate()
        .flat_map(|(g, chunks)| chunks.iter().map(move |&c| (c, g)))
        .collect();
    let mut sources = vec![Vec::new(); groups.len()];
    let mut work = vec![0.0; groups.len()];
    for &load in loads {
        if let Some(&g) = owner.get(&load.chunk) {
            sources[g].push(load);
        }
    }
    for failure in failures {
        let joint = &structure.joints[failure.joint];
        let a = owner.get(&joint.a).copied();
        let b = match joint.b {
            Side::Chunk(b) => owner.get(&b).copied(),
            Side::Ground => None,
        };
        match (a, b) {
            (Some(a), Some(b)) if a != b => {
                work[a] += failure.work / 2.0;
                work[b] += failure.work / 2.0;
            }
            (Some(g), _) | (_, Some(g)) => work[g] += failure.work,
            (None, None) => {} // Both sides remain supported; no body is launched by this opening.
        }
    }
    groups
        .into_iter()
        .zip(sources)
        .zip(work)
        .map(|((group, sources), work)| {
            let loads = fracture::debit_work(structure, &group, work, sources);
            (group, loads)
        })
        .collect()
}

/// Whether `points` at their material `density` form a body the solver can weigh and turn.
fn solid_collider(points: Vec<Vec3>, density: f32) -> bool {
    use avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::{
        AngularInertiaTensor, ComputeMassProperties3d,
    };
    Collider::convex_hull(points).is_some_and(|collider| {
        let properties = collider.mass_properties(density);
        // bevy_heavy 0.5 (avian3d 0.7) returns zero for an underflowed inertia determinant.
        let inverse = properties.angular_inertia_tensor().inverse();
        properties.mass.is_finite()
            && properties.mass > 0.0
            && properties.mass.recip().is_finite()
            && properties.principal_angular_inertia.is_finite()
            && properties.principal_angular_inertia.min_element() > 0.0
            && properties.principal_angular_inertia.recip().is_finite()
            && properties.center_of_mass.is_finite()
            && properties.local_inertial_frame.is_finite()
            && properties.local_inertial_frame.is_normalized()
            && inverse.is_finite()
            && inverse != AngularInertiaTensor::ZERO
    })
}

/// A law chunk's authored hull as a collider about `origin` (construction
/// frame): its occupied volume, so its mass and inertia at its material's density are the
/// construction's. Admission certified the hull as a solid convex collider at authored dimensions
/// ([`Layout::checked_structure`]).
#[cfg(any(test, feature = "dev_ui"))]
pub(crate) fn chunk_collider(kind: &ConstructionKind, c: usize, origin: Vec3) -> Collider {
    Collider::convex_hull(
        kind.layout.blocks[c]
            .points
            .iter()
            .map(|p| *p - origin)
            .collect(),
    )
    .expect("a load-certified authored hull stays a solid convex hull")
}

/// Resolution: how far a freed chunk's contact face stands behind a face it shares with a
/// neighbour or the ground, m. A chunk freed in place touches those faces, and at zero separation
/// the solver's contact normal is rounding noise (Parry's GJK/EPA): MEASURED on the shipped house,
/// with no gap 3 of 11 sampled releases took kicks of up to 5.5 cm and 0.22 J/kg from nothing. At
/// this gap a chunk freed in place gains no more energy than gravity's work, at the origin and
/// 1 km out (`a_chunk_freed_in_place_takes_no_energy_from_its_neighbours`). Only contacts read
/// it ([`OccupiedCollider`]). Set back on both sides, a joint leaves 2 mm of void, which is all
/// the walk bridges in one run (`WalkLaws::weld_perp` and `weld_run_gap_budget`, 2 mm each). So
/// a round crossing two such joints would read two plates where the intact element reads one. An
/// oblique round can cross two: the shipped house's 0.25 m walls are cut into chunks 0.225 to
/// 0.26 m across (DERIVED from its construction file).
/// Avian's minimum speculative margin retains a resting pair; support after release still needs
/// the gravity cast to cross this gap: DERIVED 1.40 mm at 64 Hz with 6 substeps, against 1 mm.
/// Tick-rate or substep changes must recheck that reach.
pub(crate) const LOOSE_GAP_M: f32 = 1e-3;

/// Resolution: how far a contact hull's corner may pass one of its planes and still bound it, m:
/// the f64 rounding of a three-plane intersection at construction-frame coordinates, far below
/// [`fracture::ON_PLANE`].
const CORNER_TOLERANCE_M: f64 = 1e-6;

/// The face of `block` that `joint` lies on, if any: its plane through the joint's centroid,
/// with the joint's normal.
pub(crate) fn joint_face(block: &Block, joint: &Joint) -> Option<usize> {
    block.planes.iter().position(|&(normal, offset)| {
        // Resolution: a joint's unit normal is its face's, within the bake's f32 rounding.
        normal.dot(joint.normal).abs() >= 1.0 - 1e-4
            && (normal.dot(joint.centroid) - offset).abs() <= fracture::ON_PLANE
    })
}

/// Which faces of `block` carry one of its `joints` (each with its index): where another body's
/// coplanar face can meet it, a neighbour's once the two ride different bodies, or the anchor
/// plane's. `Err` names a chunk-to-chunk joint on none of the faces: freed, its chunk would meet
/// that neighbour with no contact gap. A ground anchor on no face (soil bearing a chunk's side)
/// sets none back: the terrain a freed chunk meets is a collider of its own, not that plane.
pub(crate) fn joint_faces<'a>(
    block: &Block,
    joints: impl Iterator<Item = (usize, &'a Joint)>,
) -> Result<Vec<bool>, usize> {
    let mut faces = vec![false; block.planes.len()];
    for (j, joint) in joints {
        match (joint_face(block, joint), joint.b) {
            (Some(face), _) => faces[face] = true,
            (None, Side::Ground) => {}
            (None, Side::Chunk(_)) => return Err(j),
        }
    }
    Ok(faces)
}

/// A freed chunk's contact hull about `origin`: `block`'s occupied planes with each of its joint
/// `faces` ([`joint_faces`]) set back [`LOOSE_GAP_M`] along its normal, every other face the
/// occupied one. `None` when the setback leaves no solid.
pub(crate) fn contact_collider(block: &Block, faces: &[bool], origin: Vec3) -> Option<Collider> {
    let planes: Vec<(DVec3, f64)> = block
        .planes
        .iter()
        .zip(faces)
        .map(|(&(normal, offset), &set)| {
            let gap = if set { f64::from(LOOSE_GAP_M) } else { 0.0 };
            (normal.as_dvec3(), f64::from(offset) - gap)
        })
        .collect();
    let inside = |p: DVec3| {
        planes
            .iter()
            .all(|&(normal, offset)| normal.dot(p) <= offset + CORNER_TOLERANCE_M)
    };
    let mut corners: Vec<DVec3> = Vec::new();
    // Every corner of a convex polytope lies on three of its planes with independent normals.
    for (i, a) in planes.iter().enumerate() {
        for (j, b) in planes.iter().enumerate().skip(i + 1) {
            for c in &planes[j + 1..] {
                let rows = bevy::math::DMat3::from_cols(a.0, b.0, c.0).transpose();
                // Resolution: unit normals this close to dependent meet in no single corner.
                if rows.determinant().abs() < 1e-9 {
                    continue;
                }
                let corner = rows.inverse() * DVec3::new(a.1, b.1, c.1);
                if inside(corner)
                    && corners
                        .iter()
                        .all(|p| p.distance(corner) > CORNER_TOLERANCE_M)
                {
                    corners.push(corner);
                }
            }
        }
    }
    let points: Vec<Vec3> = corners
        .into_iter()
        .map(|p| (p - origin.as_dvec3()).as_vec3())
        .collect();
    Block::hull(points.clone())?;
    Collider::convex_hull(points)
}

/// The ballistic volume of law chunk `c`: its element's substance.
fn chunk_volume(kind: &ConstructionKind, c: usize) -> BallisticVolume {
    let element = &kind.asset.elements[usize::from(kind.layout.element_of[c])];
    let material = usize::from(element.material);
    BallisticVolume {
        material: kind.materials[material],
        substance: kind.asset.materials[material].clone(),
    }
}

/// A freed carrier staged until every construction in the deposit has answered its contacts.
/// One source can free groups in several constructions; their combined exchange is bounded once.
struct Departure {
    live: Live,
    tick: u32,
    kind: Arc<ConstructionKind>,
    construction: u16,
    root: Transform,
    group: Vec<usize>,
    launch: Launch,
}

/// What the game's damage-model rounds meet of a chunk, standing or freed, and a standing chunk's
/// contact surface: its occupied hull in its centroid frame, which it keeps through standing,
/// departure and later splits. A freed chunk's is a sensor on [`Layer::Armor`] that meets no
/// contact. The body owns mass (`spawn_prepared`).
type OccupiedCollider = (
    Collider,
    avian3d::prelude::Friction,
    BallisticVolume,
    ChunkCollider,
);

/// A freed chunk's contact surface in the same frame: its contact hull ([`contact_collider`]).
type ContactCollider = (Collider, avian3d::prelude::Friction, ChunkCollider);

fn occupied_collider(kind: &ConstructionKind, c: usize, rotation: Quat) -> OccupiedCollider {
    let hull = kind.sweep.occupied(c).0.clone();
    (
        super::sweep::posed(hull, rotation),
        kind.chunk_friction(c),
        chunk_volume(kind, c),
        ChunkCollider(c as u16),
    )
}

fn freed_contact(kind: &ConstructionKind, c: usize, rotation: Quat) -> ContactCollider {
    (
        with_insertion_mass(super::sweep::posed(
            kind.sweep.contact_hull(c).clone(),
            rotation,
        )),
        kind.chunk_friction(c),
        ChunkCollider(c as u16),
    )
}

/// `collider` carrying the mass Avian's insertion hook integrates for a collider that is no
/// sensor, prepared off the system thread. A sensor installs zero mass and never reads it.
fn with_insertion_mass(collider: Collider) -> Collider {
    collider.with_mass_properties_cache(avian3d::prelude::ColliderDensity::default().0)
}

/// A departing owner's existing chunk colliders, by chunk, indexed once. A standing owner holds
/// the occupied collider of each chunk whose element has cracked, and its departing chunks build
/// their contact hulls fresh; a piece holds both colliders of every chunk, `[occupied, contact]`,
/// since each has ridden both since it first left.
enum ReusableColliders {
    Standing(BTreeMap<u16, Entity>),
    Piece(BTreeMap<u16, [Entity; 2]>),
}

type ChunkChildren<'w, 's> = Query<'w, 's, (&'static ChunkCollider, Has<BallisticVolume>)>;

/// A standing owner's chunk colliders among its `children`.
fn reusable_colliders(children: Option<&Children>, chunks: &ChunkChildren) -> ReusableColliders {
    let mut occupied = BTreeMap::new();
    for &entity in children.into_iter().flatten() {
        if let Ok((chunk, ballistic)) = chunks.get(entity) {
            assert!(ballistic, "a standing chunk carries no contact hull");
            assert!(
                occupied.insert(chunk.0, entity).is_none(),
                "a standing chunk has one collider"
            );
        } // Element and view children have no chunk address.
    }
    ReusableColliders::Standing(occupied)
}

/// A piece's chunk colliders among its `children`: both of every chunk it carries.
fn piece_colliders(children: &Children, chunks: &ChunkChildren) -> ReusableColliders {
    let mut both = BTreeMap::<u16, [Option<Entity>; 2]>::new();
    for &entity in children {
        if let Ok((chunk, ballistic)) = chunks.get(entity) {
            let side = &mut both.entry(chunk.0).or_default()[usize::from(!ballistic)];
            assert!(
                side.replace(entity).is_none(),
                "a piece's chunk has one occupied and one contact collider"
            );
        } // View children have no chunk address.
    }
    ReusableColliders::Piece(
        both.into_iter()
            .map(|(chunk, [occupied, contact])| {
                let occupied = occupied.expect("a piece's chunk keeps its occupied collider");
                let contact = contact.expect("a piece's chunk rides its contact hull");
                (chunk, [occupied, contact])
            })
            .collect(),
    )
}

/// Where a departing chunk's collider comes from: its existing entity, rehomed, or prepared fresh.
enum ColliderSource<T> {
    Rehome(Entity),
    Fresh(T),
}

impl<T: Bundle> ColliderSource<T> {
    fn place(
        self,
        commands: &mut Commands,
        placed: impl Bundle,
        pose: (Position, Rotation, GlobalTransform),
    ) {
        match self {
            // ColliderOf's relationship hook reads the supplied world pose. No collider insertion
            // runs: its shape and mass cache stay in the chunk's own frame.
            Self::Rehome(entity) => {
                commands.entity(entity).insert((placed, pose));
            }
            // Fresh colliders run Avian's local-pose initialization hook once.
            Self::Fresh(bundle) => {
                commands.spawn((
                    bundle,
                    placed,
                    (Position::PLACEHOLDER, Rotation::PLACEHOLDER, pose.2),
                ));
            }
        }
    }
}

struct PreparedChunk {
    local: Transform,
    occupied: ColliderSource<OccupiedCollider>,
    contact: ColliderSource<ContactCollider>,
}

struct PreparedDeparture {
    departure: Departure,
    chunks: Vec<PreparedChunk>,
    source: super::sweep::SourceShape,
}

/// Count the departures' bodies and the colliders they build fresh.
fn count_prepared(stages: Option<&Stages>, departures: &[PreparedDeparture]) {
    let Some(stages) = stages else {
        return; // No cost instrument is mounted.
    };
    stages.add(Work::PreparedBodies, departures.len() as u64);
    let fresh = departures
        .iter()
        .flat_map(|d| &d.chunks)
        .map(|chunk| {
            u64::from(matches!(chunk.occupied, ColliderSource::Fresh(_)))
                + u64::from(matches!(chunk.contact, ColliderSource::Fresh(_)))
        })
        .sum();
    stages.add(Work::PreparedColliders, fresh);
}

/// Rehoming retains each collider and its mass cache; query geometry is prepared before spawn.
fn prepare_group(departure: Departure, reusable: &ReusableColliders) -> PreparedDeparture {
    let kind = &departure.kind;
    let rotation = departure.root.rotation;
    let chunks = departure
        .group
        .iter()
        .map(|&c| {
            let (occupied, contact) = match reusable {
                ReusableColliders::Standing(occupied) => (
                    match occupied.get(&(c as u16)) {
                        Some(&entity) => ColliderSource::Rehome(entity),
                        None => ColliderSource::Fresh(occupied_collider(kind, c, rotation)),
                    },
                    ColliderSource::Fresh(freed_contact(kind, c, rotation)),
                ),
                ReusableColliders::Piece(both) => {
                    let [occupied, contact] = *both
                        .get(&(c as u16))
                        .expect("a split child's chunks are its parent's");
                    (
                        ColliderSource::Rehome(occupied),
                        ColliderSource::Rehome(contact),
                    )
                }
            };
            PreparedChunk {
                local: Transform::from_translation(
                    kind.structure.chunks[c].centroid - departure.launch.centroid,
                ),
                occupied,
                contact,
            }
        })
        .collect();
    let source = kind
        .sweep
        .source(&departure.group, departure.launch.centroid, rotation);
    PreparedDeparture {
        departure,
        chunks,
        source,
    }
}

/// Occupied free-group geometry and motion from its field impulses, in construction coordinates.
pub(crate) fn launch(
    kind: &ConstructionKind,
    structure: &Structure,
    group: &[usize],
    loads: &[Load],
) -> Launch {
    use avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::MassProperties3d;
    let chunks = &structure.chunks;
    let mass: f32 = group.iter().map(|&i| chunks[i].mass).sum();
    let centroid = group
        .iter()
        .map(|&i| chunks[i].centroid * chunks[i].mass)
        .sum::<Vec3>()
        / mass;
    // Each chunk's mass was built with its hull at load: a collapse builds no hull.
    let total = group
        .iter()
        .map(|&c| {
            let (_, props) = kind.sweep.occupied(c);
            props.transformed_by(Isometry3d::from_translation(chunks[c].centroid - centroid))
        })
        .fold(MassProperties3d::default(), |sum, part| sum + part);
    launch_mass(centroid, total, loads)
}

/// Spawn one loose group's body (module doc): at its chunks' centre of mass, turned as its
/// construction `(ordinal, root pose)`, moving as its bounded plastic exchange leaves it ([`launch`]).
#[cfg(test)]
fn spawn_piece(
    commands: &mut Commands,
    kind: &Arc<ConstructionKind>,
    (construction, root): (u16, Transform),
    group: &[usize],
    launch: Launch,
) {
    let live = Live::new(kind).group(group);
    spawn_group(
        commands,
        Departure {
            kind: kind.clone(),
            construction,
            root,
            group: group.to_vec(),
            launch,
            live,
            tick: 0,
        },
    );
}

#[cfg(test)]
fn spawn_group(commands: &mut Commands, departure: Departure) {
    spawn_prepared(
        commands,
        prepare_group(departure, &ReusableColliders::Standing(BTreeMap::new())),
    );
}

/// Commit a group from its synchronous, independently prepared construction state.
fn spawn_prepared(commands: &mut Commands, prepared: PreparedDeparture) {
    let piece = commands.spawn_empty().id();
    spawn_prepared_as(commands, piece, prepared);
}

/// [`spawn_prepared`] onto `piece`, an entity reserved for it: the sweep names a departure's body
/// before it spawns.
fn spawn_prepared_as(commands: &mut Commands, piece: Entity, prepared: PreparedDeparture) {
    let PreparedDeparture {
        departure,
        chunks,
        source,
    } = prepared;
    let Departure {
        kind,
        construction,
        root,
        group,
        launch,
        live,
        tick,
    } = departure;
    let Launch {
        centroid,
        velocity,
        spin,
        ..
    } = &launch;
    let origin = *centroid;
    let at = root.translation + root.rotation * origin;
    // `Position`/`Rotation` before the body: it never flushes with avian's placeholder pose.
    commands.entity(piece).insert((
        Name::new("Piece"),
        source,
        Piece {
            construction,
            chunks: group.iter().map(|&c| c as u16).collect(),
            born: tick,
            retired: None,
        },
        Breakage {
            kind,
            construction,
            live: Box::new(live),
            origin,
        },
        Transform::from_translation(at).with_rotation(root.rotation),
        Position(at),
        Rotation(root.rotation),
        Visibility::default(),
        crate::debris::DebrisPiece,
        crate::eased_dynamic_body(),
        // The whole group owns its occupied properties at spawn. Child rehoming cannot
        // expose a partial aggregate to driving or sweeps before Avian's prepare phase.
        (
            avian3d::prelude::Mass(launch.mass_properties.mass),
            avian3d::prelude::AngularInertia::new_with_local_frame(
                launch.mass_properties.principal_angular_inertia,
                launch.mass_properties.local_inertial_frame,
            ),
            avian3d::prelude::CenterOfMass(launch.mass_properties.center_of_mass),
            avian3d::prelude::NoAutoMass,
            avian3d::prelude::NoAutoAngularInertia,
            avian3d::prelude::NoAutoCenterOfMass,
        ),
        LinearVelocity(root.rotation * *velocity),
        AngularVelocity(root.rotation * *spin),
    ));
    for PreparedChunk {
        local,
        occupied,
        contact,
    } in chunks
    {
        let world = Transform::from_translation(at + root.rotation * local.translation)
            .with_rotation(root.rotation);
        let pose = (
            Position(world.translation),
            Rotation(world.rotation),
            GlobalTransform::from(world),
        );
        let ballistic = CollisionLayers::new([Layer::Armor], LayerMask::NONE);
        occupied.place(commands, (ChildOf(piece), local, Sensor, ballistic), pose);
        contact.place(commands, (ChildOf(piece), local, piece_layers()), pose);
    }
}

/// A construction group retiring at `tick`: its carrier keeps the final pose and retirement
/// tick, so replicas draw it until the shared cursor reaches its replacement or dust entries.
#[derive(Event)]
pub(crate) struct RetirePiece {
    pub(crate) entity: Entity,
    pub(crate) tick: u32,
}

/// A seam failure's new groups, each with its law state, staged on the parent until all
/// the law stage finishes. `split_groups` spawns them at `tick` with the parent's rigid motion.
#[derive(Component)]
pub(crate) struct PendingSplit {
    children: Vec<(Vec<usize>, Live)>,
    tick: u32,
}

impl PendingSplit {
    /// A crumble's split of moving group `breakage` into `groups` at `tick`
    /// ([`crate::debris::crumble`]): each keeps its share of the parent's state, and an owned
    /// chunk in no group leaves the simulation with the parent's retirement.
    pub(crate) fn of(breakage: &Breakage, groups: Vec<Vec<usize>>, tick: u32) -> Self {
        let children = groups
            .into_iter()
            .map(|group| {
                let live = breakage.live.group(&group);
                (group, live)
            })
            .collect();
        Self { children, tick }
    }

    fn prepare(
        &self,
        pool: &TaskPool,
        breakage: &Breakage,
        construction: u16,
        root: Transform,
        motion: ReleasedMotion,
        reusable: &ReusableColliders,
    ) -> Vec<PreparedDeparture> {
        pool.scope(|scope| {
            for (group, live) in &self.children {
                scope.spawn(async move {
                    let mut launch = launch(&breakage.kind, &live.structure, group, &[]);
                    // DERIVED rigid velocity field: every new COM gets v + omega cross r; every
                    // child retains omega. Occupied hulls partition the parent's mass and inertia.
                    launch.velocity = motion.velocity
                        + motion
                            .spin
                            .cross(launch.centre - breakage.origin - motion.centre);
                    launch.spin = motion.spin;
                    prepare_group(
                        Departure {
                            kind: breakage.kind.clone(),
                            construction,
                            root,
                            group: group.clone(),
                            launch,
                            live: live.clone(),
                            tick: self.tick,
                        },
                        reusable,
                    )
                });
            }
        })
    }
}

fn split_groups(
    parents: Query<(
        Entity,
        &Piece,
        (&Position, &Rotation),
        &Breakage,
        &LinearVelocity,
        &AngularVelocity,
        &avian3d::prelude::ComputedCenterOfMass,
        &PendingSplit,
        Option<&Children>,
    )>,
    chunks: ChunkChildren,
    mut commands: Commands,
    stages: Option<Res<Stages>>,
) {
    let _refit = Stages::start(stages.as_deref(), Stage::Refit);
    let mut effects =
        bevy::utils::Parallel::<Vec<(Entity, u32, Vec<PreparedDeparture>)>>::default();
    parents.par_iter().for_each(
        |(entity, piece, body, breakage, velocity, spin, centre, split, colliders)| {
            let root = breakage.physics_frame(body);
            let inverse = root.rotation.inverse();
            let colliders = colliders.expect("a piece owns its chunk colliders");
            let reusable = piece_colliders(colliders, &chunks);
            let children = split.prepare(
                ComputeTaskPool::get(),
                breakage,
                piece.construction,
                root,
                // The parent's velocities in the construction frame; its COM about its body origin.
                ReleasedMotion {
                    velocity: inverse * velocity.0,
                    spin: inverse * spin.0,
                    centre: centre.0,
                },
                &reusable,
            );
            count_prepared(stages.as_deref(), &children);
            effects.scope(|effects| effects.push((entity, split.tick, children)));
        },
    );
    let mut effects: Vec<_> = effects.drain().collect();
    effects.sort_unstable_by_key(|(entity, ..)| *entity);
    for (entity, tick, children) in effects {
        for child in children {
            spawn_prepared(&mut commands, child);
        }
        commands.trigger(RetirePiece { entity, tick });
        commands.entity(entity).despawn();
    }
}

/// A loose chunk's contact filter: debris meets ground, constructions, hulls, track belts and
/// other pieces, never a tank's armour volumes.
pub(crate) fn piece_layers() -> CollisionLayers {
    CollisionLayers::new(
        [Layer::Debris],
        [
            Layer::Terrain,
            Layer::Vehicle,
            Layer::Debris,
            Layer::TrackSide,
            Layer::TrackBelt,
        ],
    )
}

/// REPLICA: rebuild every construction's state from the record, from the entry it has not applied; a
/// record of another generation first stands every construction whole again.
fn derive_from_record(
    record: Res<Record>,
    mut applied: Local<(u32, usize)>,
    mut constructions: Query<&mut Breakage, Without<Piece>>,
) {
    if applied.0 != record.generation {
        constructions
            .par_iter_mut()
            .for_each(|mut breakage| breakage.live.reset());
        *applied = (record.generation, 0);
    }
    assert!(
        record.entries.len() >= applied.1,
        "a construction record never shrinks within its generation"
    );
    if record.entries.len() == applied.1 {
        return;
    }
    let mut by_ordinal: BTreeMap<u16, Vec<Event>> = BTreeMap::new();
    for entry in &record.entries[applied.1..] {
        by_ordinal
            .entry(entry.construction)
            .or_default()
            .push(entry.event);
    }
    for mut breakage in &mut constructions {
        let Some(events) = by_ordinal.remove(&breakage.construction) else {
            // This construction has no new entries in the ledger.
            continue;
        };
        let kind = breakage.kind.clone();
        let live = breakage.live.as_mut();
        let mut shape = live.shape();
        for event in events {
            shape.apply(event, &kind.layout);
            if let Event::Hole(c, hole) = event {
                live.holes[usize::from(c)].push(hole);
            }
        }
        shape.derive_departures(&kind.layout, live);
        live.swapped = shape.swapped;
        live.state.fate = shape.fate;
        live.state.cracked = shape.cracked;
        live.state.separated = shape.separated;
    }
    assert!(
        by_ordinal.is_empty(),
        "a construction record names only constructions this map places: {:?}",
        by_ordinal.keys().collect::<Vec<_>>()
    );
    applied.1 = record.entries.len();
}

/// Prepared structural effects in the owner's original element-then-chunk order.
enum ColliderChange<E, C> {
    Despawn(Entity),
    Element(E),
    Chunk(usize, C),
}

impl<E: Bundle, C: Bundle> ColliderChange<E, C> {
    fn apply(self, commands: &mut Commands) -> Option<(usize, Entity)> {
        match self {
            Self::Despawn(entity) => {
                commands.entity(entity).despawn();
            }
            Self::Element(bundle) => {
                commands.spawn(bundle);
            }
            Self::Chunk(c, bundle) => return Some((c, commands.spawn(bundle).id())),
        }
        None
    }
}

fn prepare_reconcile(
    root: Entity,
    transform: &Transform,
    breakage: &Breakage,
    children: Option<&Children>,
    elements: &Query<&Element>,
    chunks: &Query<&ChunkCollider>,
) -> Vec<ColliderChange<impl Bundle, impl Bundle>> {
    let mut effects = Vec::new();
    let kind = &breakage.kind;
    let shape = breakage.shape();
    let (mut have_elements, mut have_chunks) = (BTreeMap::new(), BTreeMap::new());
    for child in children.into_iter().flatten() {
        if let Ok(&Element(e)) = elements.get(*child) {
            have_elements.insert(usize::from(e), *child);
        } else if let Ok(&ChunkCollider(c)) = chunks.get(*child) {
            have_chunks.insert(usize::from(c), *child);
        }
    }
    let pose = (
        Position(transform.translation),
        Rotation(transform.rotation),
    );
    for (e, swapped) in shape.swapped.iter().enumerate() {
        match (swapped, have_elements.get(&e)) {
            (true, Some(entity)) => effects.push(ColliderChange::Despawn(*entity)),
            (false, None) => {
                effects.push(ColliderChange::Element((
                    ChildOf(root),
                    super::element_bundle(kind, e),
                    pose,
                )));
            }
            _ => {}
        }
    }
    for c in 0..kind.layout.chunks.len() {
        match (shape.chunk_present(&kind.layout, c), have_chunks.get(&c)) {
            (false, Some(entity)) => effects.push(ColliderChange::Despawn(*entity)),
            (true, None) => {
                let (hull, friction, volume, chunk) =
                    occupied_collider(kind, c, transform.rotation);
                effects.push(ColliderChange::Chunk(
                    c,
                    (
                        ChildOf(root),
                        Transform::from_translation(kind.structure.chunks[c].centroid),
                        (with_insertion_mass(hull), friction, volume, chunk),
                        (
                            super::element_layers(),
                            ActiveCollisionHooks::MODIFY_CONTACTS,
                        ),
                        (
                            Position::PLACEHOLDER,
                            Rotation::PLACEHOLDER,
                            GlobalTransform::from(
                                Transform::from_translation(
                                    transform.transform_point(kind.structure.chunks[c].centroid),
                                )
                                .with_rotation(transform.rotation),
                            ),
                        ),
                    ),
                ));
            }
            _ => {}
        }
    }
    effects
}

/// Scoped results retain query order; only collider preparation runs on workers.
fn prepare_reconciliations(
    pool: &TaskPool,
    constructions: &Query<
        (Entity, &Transform, &Breakage, Option<&Children>),
        (Changed<Breakage>, Without<Piece>),
    >,
    elements: &Query<&Element>,
    chunks: &Query<&ChunkCollider>,
) -> Vec<(Entity, Vec<ColliderChange<impl Bundle, impl Bundle>>)> {
    pool.scope(|scope| {
        for (root, transform, breakage, children) in constructions {
            scope.spawn(async move {
                let effects =
                    prepare_reconcile(root, transform, breakage, children, elements, chunks);
                (root, effects)
            });
        }
    })
}

/// Every peer: prepare changed owners together, then commit colliders before the same-update refit.
/// A belt reaction staged on an element collider that retires for its chunks' moves to the chunk
/// collider the sweep found at its point ([`sweep_intake::SwappedBelts`]).
fn reconcile(
    constructions: Query<
        (Entity, &Transform, &Breakage, Option<&Children>),
        (Changed<Breakage>, Without<Piece>),
    >,
    elements: Query<&Element>,
    chunks: Query<&ChunkCollider>,
    mut commands: Commands,
    mut transfers: ResMut<ContactTransfers>,
    mut swapped: ResMut<sweep_intake::SwappedBelts>,
    stages: Option<Res<Stages>>,
) {
    let _refit = Stages::start(stages.as_deref(), Stage::Refit);
    let effects =
        prepare_reconciliations(ComputeTaskPool::get(), &constructions, &elements, &chunks);
    let mut built = BTreeMap::new();
    // Direct scoped submissions collect in query order. All hulls are ready before this flush,
    // and refit_systems exposes every new collider to spatial queries in the same update.
    for (root, effects) in effects {
        for effect in effects {
            if let Some((c, entity)) = effect.apply(&mut commands) {
                built.insert((root, c), entity);
            }
        }
    }
    let moves = std::mem::take(&mut swapped.0).into_iter();
    transfers.readdress(moves.map(|(index, root, chunk)| {
        let collider = built
            .get(&(root, chunk))
            .expect("a staged reaction outlives its swapped element only on a chunk that stands");
        (index, *collider)
    }));
}

/// Avian's refit of moved colliders in a frame a construction changed: the spawned chunks' bounds
/// and the collider trees a spatial query walks, so the next march reads them.
fn refit_systems() -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (
        |swapped: Res<sweep_intake::SwappedBelts>| {
            assert!(
                swapped.0.is_empty(),
                "sweep reconciliation consumes every swapped belt before physics clears staged contacts"
            );
        },
        reconcile,
        update_moved_collider_aabbs::<Collider>
            .run_if(resource_exists::<ColliderTrees>)
            .run_if(any_match_filter::<Changed<Breakage>>),
    )
        .chain()
}

/// The authority's set: deposits, then the colliders, after the march.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LawSet;

/// Hull sweep and its reconciliation, before physics consumes the changed colliders.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SweepSet;

/// Carrier splits after the laws, flushed before publication reads the resulting groups.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SplitSet;

/// Inside [`LawSet`], after the deposits and before the refit: the step's contacts still name
/// live colliders here, since the refit despawns the ones a deposit retired.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LandingSet;

/// A replica's set, in `Update`: the record's new entries into the constructions, then the colliders.
/// Whatever fills the record runs before it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ReplicaSet;

/// AUTHORITY: the hull sweep ([`sweep_intake::run`]), timed as its own stage.
fn sweep(intake: sweep_intake::Intake, stages: Option<Res<Stages>>) {
    let _sweep = Stages::start(stages.as_deref(), Stage::Sweep);
    sweep_intake::run(intake);
}

pub(crate) fn sim_plugin(app: &mut App) {
    crate::collider_rehome::plugin(app);
    app.init_resource::<SweepAccount>()
        .init_resource::<SweepCast>()
        .init_resource::<sweep_intake::SwappedBelts>()
        .add_message::<crate::debris::crumble::Shed>()
        .add_systems(
            FixedPostUpdate,
            (sweep, reconcile.run_if(sweep_intake::changed))
                .chain()
                .in_set(SweepSet)
                .before(avian3d::prelude::PhysicsSystems::Prepare)
                .run_if(not(resource_exists::<ClientReplica>)),
        )
        .init_resource::<Queue>()
        .init_resource::<RestingLoads>()
        .init_resource::<Record>()
        .add_observer(read_shell)
        .add_observer(set_off)
        .configure_sets(
            FixedPostUpdate,
            LawSet.after(crate::state::SimPhase::ProjectileMarch),
        )
        .configure_sets(FixedPostUpdate, LandingSet.in_set(LawSet))
        .add_systems(
            FixedPostUpdate,
            (
                (read_contacts, apply_deposits).chain().before(LandingSet),
                refit_systems().after(LandingSet),
            )
                .in_set(LawSet)
                .run_if(not(resource_exists::<ClientReplica>)),
        )
        .add_systems(
            FixedPostUpdate,
            split_groups
                .in_set(SplitSet)
                .after(LawSet)
                .before(crate::state::SimPhase::Publish)
                .run_if(not(resource_exists::<ClientReplica>)),
        )
        .add_systems(
            Update,
            (derive_from_record, refit_systems())
                .chain()
                .in_set(ReplicaSet)
                .run_if(resource_exists::<ClientReplica>),
        );
}

#[cfg(test)]
pub(crate) mod tests;
