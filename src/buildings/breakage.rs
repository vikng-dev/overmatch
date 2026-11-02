//! A building breaking in a match, about the game's environment destruction (game state only):
//! the authority runs the fracture laws ([`super::fracture`]) on what the rounds deposit, records
//! every change, and every peer derives the wall's colliders from that record.
//!
//! * **The law's view of a building** ([`Layout`], once per asset): the chunks of the elements
//!   whose substance states bulk facts ([`Masonry::of`]), numbered in asset order, and the joints
//!   and anchors among them, each holding by the bond the asset gives it ([`Bond`]: a seam inside
//!   a component; between two, what the attachment kind it names holds by
//!   ([`super::attachments`]: a seam, or a rest)). A chunk and a joint keep that number on the
//!   wire.
//! * **First touch swaps an element.** An element stays one collider until a deposit, a crack, a
//!   fall or dust touches one of its chunks; from then on each of its standing chunks is a static
//!   collider of its substance ([`BuildingChunk`]) and the element's own collider is gone. A
//!   building's [`Breakage`] holds its state, built from data on the first touch.
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
//!   building's or a moving group's) and a placed building's unbreakable elements, the other moving
//!   bodies (a hull, a launched turret) and every other surface (the ground, a standing tree); one
//!   that stops on such a body pushes it the same way, through avian at the point it stops. A
//!   tree stop loads that segment through the tree laws; terrain and other nonstructural surfaces
//!   only shade the field.
//! * **Contacts** (the authority's, each physics step, [`read_contacts`]): every point the solver
//!   moved momentum through between a chunk group (a placed building's breakable element or chunk,
//!   or a moving group's chunk) and another body ([`ContactTransfers`]) loads the chunk there (an
//!   intact element's: the chunk whose hull holds the point, [`contact_load`]) with its blow and
//!   press, each contact split at its body's rest speed ([`ContactTransfers::split`]), all of a
//!   step's points one deposit. On a placed building the source exchanges its blow plastically
//!   ([`Origin::Body`]); on a moving group the solver already moved both bodies
//!   ([`Origin::Contact`]). A point on an unbreakable element loads nothing. A body asleep against a building presses it with what its
//!   contact last held ([`RestingLoads`]): every run of the laws on that building carries the
//!   press once, and it starts none. Awake contacts' presses likewise join every deposit in
//!   the physics step, once per point. No body is read by its kind.
//! * **The order.** The laws run each deposit, building by building, in order: removal and the
//!   push rule ([`fracture::strike`]), the structural check ([`fracture::structural`]), the weight
//!   check on what those changed ([`fracture::weigh`]), support ([`fracture::fall`]).
//! * **Holes** ([`Hole`], drawn only): a line cuts a channel in each chunk it crosses, the cylinder
//!   about its chord, open across each face it crosses ([`open_ends`]), and a blast a dish in each
//!   chunk its rays stop on ([`holes_of`]); a chunk still standing after the deposit
//!   keeps its hole in a per-chunk list ([`Live::holes`]), for the view to draw. No law, collider
//!   or mass reads a hole: they see the whole chunk, and [`Shape`] holds none. A hole holds no more
//!   than the volume its load crushed, the volume it took from the chunk's pool, so one the size
//!   of its chunk is a spent pool, recorded as dust. A deposit that would leave a chunk holding
//!   more than [`HOLES_PER_CHUNK`] crumbles it to dust first, before the laws run.
//! * **The record** ([`WallRecord`]): every swap, dust, hole, crack and fall, in authority order,
//!   with its tick. The authority appends; the network carries it ([`crate::net`]'s ledger); a
//!   replica rebuilds the same state from it ([`derive_from_record`]); the view presents it at the
//!   presentation clock ([`super::view`]).
//! * **Loose groups** fall on the authority as one dynamic body each ([`WallPiece`]): its chunks
//!   brick in ballistics on [`Layer::Armor`], colliding as [`Layer::Debris`], settled by the shared
//!   rule ([`crate::debris`]). It leaves with the fields' blows and each body's plastic share of
//!   its blow less the work its opening spent ([`fracture::debit_work`]), returning the rest at
//!   the contact point ([`give_back`]); its spin is the exchanged angular
//!   momentum over its own inertia tensor; gravity, contact and friction decide whether a group
//!   a cracked section freed rocks back or topples. A later blast's impulse on it is a write to
//!   its velocity and sleep timer, which wakes it asleep (avian's `wake_on_changed`).
//! * **A moving group is a chunk group too**: its body carries its own [`Breakage`], its owned
//!   chunks standing and the rest dust, so the deposits and contacts that reach it run the same
//!   laws in its accelerating frame ([`fracture::in_group_frames`], its spin read from the body).
//!   A seam that fails there splits it at that tick: [`PendingSplit`] stages the new groups until
//!   the tick's plastic returns reach the parent, `split_groups` spawns them with the parent's
//!   rigid velocity field and despawns it, and [`RetirePiece`] keeps the parent's final pose and
//!   retirement tick on the wire, so replicas hand its chunks over at the shared cursor.
//!
//! THE BODY IN THE PHYSICS WORLD. A state change reaches avian's world in the frame it is made
//! ([`reconcile`], then avian's refit of moved colliders), so the next tick's march reads it.

use std::collections::BTreeMap;
use std::sync::Arc;

use avian3d::collider_tree::{ColliderTrees, update_moved_collider_aabbs};
use avian3d::prelude::{
    AngularVelocity, Collider, ColliderAabb, ColliderDensity, ColliderOf, CollisionLayers, Forces,
    LinearVelocity, Position, ReadRigidBodyForces, RigidBody, Rotation, SpatialQuery,
    SpatialQueryFilter, WriteRigidBodyForces,
};

use bevy::math::DVec3;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::attachments::Attachments;
use super::fracture::{
    self, Blast, Block, Bond, Chunk, Fate, GroupLoads, Joint, Load, Masonry, Origin, SectionCache,
    Side, Structure, WallState,
};
use super::schema::{self, BuildingAsset};
use super::{BuildingElement, BuildingKind};
use crate::ballistics::{
    ArmourMark, BallisticVolume, BurstField, DeliveredLine, PenetrationMarks, ShellMarched,
    SpallMarks,
};
use crate::contact_transfer::{ContactTransfer, ContactTransfers};
use crate::debris::launch::{BodyReturn, Launch, LaunchPlugin, PendingLaunch, launch_colliders};
use crate::scatter::MapInstance;
use crate::{ClientReplica, Layer, ShotClock};

/// How far a field ray's stop may sit from the chunk face that stops it and still be that chunk's,
/// m: the f32 rounding of a stop the walk computed against the same face, many times over.
const STOP_TOLERANCE_M: f32 = 0.01;

/// How far a loose chunk's collider falls short of its hull on every side, m, so a group freed in
/// place does not start inside its standing neighbours (Resolution).
const LOOSE_GAP_M: f32 = 0.005;

/// Grade in the building frame, m: the plane the building stands on (a building spawns with its
/// frame's origin on the ground, its foundation below). A chunk wholly below it is buried.
const GRADE_M: f32 = 0.0;
/// How far above grade a buried chunk's corner may sit, m: the bake's rounding of a corner it cut
/// on the plane.
const ON_GRADE_M: f32 = 1e-4;

/// The most holes one chunk holds (module doc); the deposit that would add past it crumbles the
/// chunk. ASSUMED: the bound on the list the view reads per chunk, and on the record's holes per
/// chunk; no measured cost or look sets it yet.
pub(crate) const HOLES_PER_CHUNK: usize = 16;

/// The law's view of one building asset (module doc), at scale 1 in the building frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Layout {
    /// Per law chunk: its asset chunk.
    pub(crate) chunks: Vec<u32>,
    /// Per law chunk: its element.
    pub(crate) element_of: Vec<u16>,
    /// Per element: its law chunks `first..end`, `None` for an element that never breaks.
    pub(crate) elements: Vec<Option<(u16, u16)>>,
    /// Per law chunk: its material's values.
    materials: Vec<Masonry>,
    /// The law's joints, chunk pairs first in asset order, then the anchors in asset order.
    pub(crate) joints: Vec<Joint>,
    /// Per law chunk: its hull.
    pub(crate) blocks: Vec<Block>,
    /// Per element: its prism.
    pub(crate) element_blocks: Vec<Block>,
}

impl Layout {
    /// The layout of `asset`, its materials' values `masonry` (per asset material, `None` for a
    /// substance that never breaks), its attachment kinds read in `attachments`. Refuses an asset
    /// naming a kind the list does not hold, and one whose law chunks or joints overrun the wire's
    /// 16-bit numbers.
    pub(crate) fn of(
        asset: &BuildingAsset,
        masonry: &[Option<Masonry>],
        attachments: &Attachments,
    ) -> Result<Self, String> {
        let attached = asset
            .attachments
            .iter()
            .enumerate()
            .map(|(k, kind)| {
                attachments
                    .bond(kind)
                    .map_err(|err| format!("attachments[{k}]: {err}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut chunks = Vec::new();
        let mut element_of = Vec::new();
        let mut materials = Vec::new();
        let mut elements = Vec::with_capacity(asset.elements.len());
        let mut law_of = vec![None; asset.chunks.len()];
        for (index, element) in asset.elements.iter().enumerate() {
            let Some(material) = masonry[usize::from(element.material)] else {
                elements.push(None);
                continue;
            };
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
            if let (Some(a), Some(b)) = (law_of[joint.a as usize], law_of[joint.b as usize]) {
                let bond = match joint.bond {
                    schema::Bond::Seam => Bond::Bonded(materials[a].seam),
                    schema::Bond::Attached(k) => attached[usize::from(k)],
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
        for anchor in &asset.anchors {
            if let Some(a) = law_of[anchor.chunk as usize] {
                joints.push(Joint {
                    a,
                    b: Side::Ground,
                    area: anchor.area_m2,
                    centroid: Vec3::from_array(anchor.centroid),
                    normal: Vec3::Y,
                    bond: Bond::Bonded(materials[a].seam),
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
        use avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::ComputeMassProperties3d;
        let blocks = chunks
            .iter()
            .map(|&c| {
                let points: Vec<Vec3> = asset.chunks[c as usize]
                    .hull
                    .iter()
                    .map(|p| Vec3::from_array(*p))
                    .collect();
                if points.iter().any(|p| !p.is_finite())
                    || !Collider::convex_hull(points.clone()).is_some_and(|collider| {
                        let properties = collider.mass_properties(1.0);
                        properties.mass.is_finite()
                            && properties.mass > 0.0
                            && properties.angular_inertia_tensor().inverse().is_finite()
                    })
                {
                    return Err(format!(
                        "chunks[{c}].hull cannot form a convex collider; rebake a finite, nondegenerate solid hull"
                    ));
                }
                Ok(Block::hull(points))
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
            blocks,
            element_blocks: asset
                .elements
                .iter()
                .map(|element| Block::hull(element.points()))
                .collect(),
        })
    }

    /// The structure the laws read for an instance at uniform `scale`: lengths × s, areas × s²,
    /// volumes and masses × s³.
    pub(crate) fn structure(&self, asset: &BuildingAsset, scale: f32) -> Structure {
        let (s2, s3) = (scale * scale, scale * scale * scale);
        Structure {
            chunks: self
                .chunks
                .iter()
                .zip(&self.materials)
                .map(|(&c, &material)| {
                    let chunk = &asset.chunks[c as usize];
                    Chunk {
                        volume: chunk.volume_m3 * s3,
                        centroid: Vec3::from_array(chunk.centroid) * scale,
                        mass: chunk.mass_kg * s3,
                        material,
                        buried: chunk.hull.iter().all(|p| p[1] <= GRADE_M + ON_GRADE_M),
                    }
                })
                .collect(),
            joints: self
                .joints
                .iter()
                .map(|joint| Joint {
                    area: joint.area * s2,
                    centroid: joint.centroid * scale,
                    ..*joint
                })
                .collect(),
        }
    }

    /// Whether `event` names an element, law chunk or joint of this layout: what a record entry
    /// for this building must name ([`Shape::apply`]).
    pub(crate) fn admits(&self, event: WallEvent) -> bool {
        let within = |len: usize, i: u16| usize::from(i) < len;
        match event {
            WallEvent::Swap(e) => within(self.elements.len(), e),
            WallEvent::Dust(c) | WallEvent::Fall(c) | WallEvent::Hole(c, _) => {
                within(self.chunks.len(), c)
            }
            WallEvent::Crack(j) => within(self.joints.len(), j),
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
/// dish has `from == to`, a ball about its centre. In millimetres of the asset frame at scale 1,
/// about the baked chunk's centroid ([`super::schema::Chunk::centroid`]), so it is the same hole
/// at every instance scale; each coordinate saturates at the `i16` range, ±32.767 m from the
/// centroid, which a channel's ends, within two of its chunk's widths of the centroid, keep for
/// every chunk under 16 m across.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hole {
    pub(crate) from: [i16; 3],
    pub(crate) to: [i16; 3],
    pub(crate) radius: u16,
}

impl Hole {
    /// The hole within `radius` of `from`–`to` (m, asset frame at scale 1) in a chunk whose
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

/// One change to a building, as the record carries it: the element swapped to its chunks, the
/// chunk turned to dust, a hole cut in the chunk, the joint cracked, the chunk fallen with a loose
/// group. Indices are the [`Layout`]'s.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WallEvent {
    Swap(u16),
    Dust(u16),
    Crack(u16),
    Fall(u16),
    Hole(u16, Hole),
}

/// One record entry: which building (its [`MapInstance`] ordinal), what changed, and the authority
/// tick it changed at (`ShotClock` units).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WallEntry {
    pub(crate) building: u16,
    pub(crate) event: WallEvent,
    pub(crate) tick: u32,
}

/// Why a peer's wall entry is refused where it arrives (`net::walls`), before any record holds it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WallEntryError {
    /// The entry names a building ordinal this map does not place.
    UnknownBuilding(u16),
    /// The entry's event names an element, chunk or joint past its building's layout.
    OutsideLayout(u16, WallEvent),
}

impl std::fmt::Display for WallEntryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownBuilding(building) => {
                write!(f, "building {building} is not placed by this map")
            }
            Self::OutsideLayout(building, event) => {
                write!(f, "{event:?} lies outside building {building}'s layout")
            }
        }
    }
}

impl std::error::Error for WallEntryError {}

impl WallEntry {
    /// The entry checked against the map's buildings by ordinal: the one gate a peer's entry
    /// passes before a record holds it.
    pub(crate) fn admitted<'a>(
        self,
        layout_of: impl Fn(u16) -> Option<&'a Layout>,
    ) -> Result<Self, WallEntryError> {
        let layout =
            layout_of(self.building).ok_or(WallEntryError::UnknownBuilding(self.building))?;
        if layout.admits(self.event) {
            Ok(self)
        } else {
            Err(WallEntryError::OutsideLayout(self.building, self.event))
        }
    }
}

/// Every change to every building in the current world, in authority order (module doc).
/// `generation` moves whenever the record is replaced (a new world, on either peer, or a replica's
/// next ledger): whoever derives from it starts over.
#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub(crate) struct WallRecord {
    pub(crate) entries: Vec<WallEntry>,
    pub(crate) generation: u32,
}

/// A building's fates and cracks and which elements are swapped: what the record says of it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shape {
    pub(crate) swapped: Vec<bool>,
    pub(crate) fate: Vec<Fate>,
    pub(crate) cracked: Vec<bool>,
}

impl Shape {
    /// Every element whole, every chunk standing, every joint intact.
    pub(crate) fn intact(layout: &Layout) -> Self {
        Self {
            swapped: vec![false; layout.elements.len()],
            fate: vec![Fate::Standing; layout.chunks.len()],
            cracked: vec![false; layout.joints.len()],
        }
    }

    /// Apply one recorded change. A record holds only events its building's layout admits
    /// ([`Layout::admits`]): the authority's laws write them, and a replica refuses a peer's
    /// others where they arrive ([`WallEntryError`]). A hole leaves the shape as it is (module
    /// doc).
    pub(crate) fn apply(&mut self, event: WallEvent) {
        let slot = |len: usize, i: u16| {
            assert!(
                usize::from(i) < len,
                "a wall record holds only events inside its building's layout: {event:?}"
            );
            usize::from(i)
        };
        match event {
            WallEvent::Swap(e) => {
                let e = slot(self.swapped.len(), e);
                self.swapped[e] = true;
            }
            WallEvent::Dust(c) => {
                let c = slot(self.fate.len(), c);
                self.fate[c] = Fate::Dust;
            }
            WallEvent::Fall(c) => {
                let c = slot(self.fate.len(), c);
                self.fate[c] = Fate::Fallen;
            }
            WallEvent::Crack(j) => {
                let j = slot(self.cracked.len(), j);
                self.cracked[j] = true;
            }
            WallEvent::Hole(c, _) => {
                slot(self.fate.len(), c);
            }
        }
    }

    /// Whether law chunk `c` stands as a collider of its own: its element swapped, it standing.
    pub(crate) fn chunk_present(&self, layout: &Layout, c: usize) -> bool {
        self.swapped[usize::from(layout.element_of[c])] && self.fate[c] == Fate::Standing
    }
}

/// A building's live state: the structure the laws read at its instance scale, the laws' state,
/// its shape, the structural check's sections of the structure, and each chunk's holes. The
/// shape's fates and cracks are the state's, mirrored as the laws move them.
#[derive(Clone)]
pub(crate) struct Live {
    pub(crate) structure: Arc<Structure>,
    pub(crate) state: WallState,
    pub(crate) swapped: Vec<bool>,
    pub(crate) sections: SectionCache,
    /// Presses already checked against the current topology: chunk, point (m), force (N).
    /// Decreases are rechecked too: removing a stabilizing force can expose another's demand.
    held_presses: Vec<(usize, Vec3, Vec3)>,
    /// Per law chunk: its holes, in record order (module doc).
    pub(crate) holes: Vec<Vec<Hole>>,
}

impl Live {
    fn new(kind: &BuildingKind, scale: f32) -> Self {
        Self::from_structure(
            kind.layout.structure(&kind.asset, scale),
            kind.layout.elements.len(),
        )
    }

    /// Complete live state built synchronously from a structure and its authored element count.
    pub(crate) fn from_structure(structure: Structure, elements: usize) -> Self {
        Self {
            state: WallState::new(&structure),
            swapped: vec![false; elements],
            sections: SectionCache::default(),
            held_presses: Vec::new(),
            holes: vec![Vec::new(); structure.chunks.len()],
            structure: Arc::new(structure),
        }
    }

    /// State for one owned group, retaining the data and wear it had before separation.
    fn group(&self, group: &[usize]) -> Self {
        let mut child = self.clone();
        child.sections = SectionCache::default();
        child.held_presses.clear();
        child.swapped.fill(true);
        for (c, fate) in child.state.fate.iter_mut().enumerate() {
            *fate = if group.binary_search(&c).is_ok() {
                Fate::Standing
            } else {
                Fate::Dust
            };
        }
        child
    }

    /// Its fates, cracks and swaps as a [`Shape`].
    pub(crate) fn shape(&self) -> Shape {
        Shape {
            swapped: self.swapped.clone(),
            fate: self.state.fate.clone(),
            cracked: self.state.cracked.clone(),
        }
    }
}

/// A building root's breakable state (module doc): its kind and instance scale, and its live
/// state once anything has touched it. Inserted at spawn; the live state is built from the kind's
/// data, synchronously, on the first touch.
#[derive(Component)]
#[require(crate::contact_transfer::StructureBody)]
pub(crate) struct Breakage {
    pub(crate) kind: Arc<BuildingKind>,
    pub(crate) scale: f32,
    pub(crate) live: Option<Box<Live>>,
    /// The body's origin in the scaled construction frame; zero for a placed building.
    origin: Vec3,
}

impl Breakage {
    pub(crate) fn new(kind: Arc<BuildingKind>, scale: f32) -> Self {
        Self {
            kind,
            scale,
            live: None,
            origin: Vec3::ZERO,
        }
    }

    /// The construction frame's world pose, from the body's pose and its data-space origin.
    fn frame(&self, body: &Transform) -> Transform {
        Transform::from_translation(body.translation - body.rotation * self.origin)
            .with_rotation(body.rotation)
    }

    /// Laws use the solver pose, never the eased display transform of a moving body.
    fn physics_frame(&self, (position, rotation): (&Position, &Rotation)) -> Transform {
        self.frame(&Transform::from_translation(position.0).with_rotation(rotation.0))
    }

    /// The live state, built on first use.
    pub(crate) fn live(&mut self) -> &mut Live {
        self.live
            .get_or_insert_with(|| Box::new(Live::new(&self.kind, self.scale)))
    }

    /// Its shape: the live state's, or intact.
    pub(crate) fn shape(&self) -> Shape {
        self.live
            .as_ref()
            .map_or_else(|| Shape::intact(&self.kind.layout), |live| live.shape())
    }
}

/// One law chunk's static collider entity, a child of its building's root: its law index.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[component(immutable)]
pub(crate) struct BuildingChunk(pub(crate) u16);

/// A loose group of a building's chunks: the building's ordinal and the group's law chunks,
/// ascending. On the authority it rides the group's dynamic body; the network carries it to
/// replicas on the body's pose carrier, with its birth and optional retirement ticks.
#[derive(Component, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct WallPiece {
    pub(crate) building: u16,
    pub(crate) chunks: Vec<u16>,
    /// This group replaces its parent at this authoritative tick (the shared view cursor).
    pub(crate) born: u32,
    /// A split retires its old carrier at this tick; the historical pose remains available
    /// until the replica's shared cursor has presented the replacement.
    pub(crate) retired: Option<u32>,
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
    /// A physics step's body contacts, each its building's root and its load ([`contact_load`]).
    contacts: Vec<(Entity, Load)>,
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
struct WallQueue(Vec<RawDeposit>);

/// Per building root, as of the last physics step: the presses of the bodies asleep against it
/// ([`crate::contact_transfer::Resting`]), each a load with no blow ([`contact_load`]). Every run
/// of the laws on the building carries them; none starts one.
#[derive(Resource, Default)]
struct RestingLoads(BTreeMap<Entity, Vec<Load>>);

/// The colliders a contact point can lie on: a building's element or chunk, under its root.
type BuildingColliders<'w, 's> = Query<
    'w,
    's,
    (
        Option<&'static BuildingElement>,
        Option<&'static BuildingChunk>,
        &'static ChildOf,
    ),
    Or<(With<BuildingElement>, With<BuildingChunk>)>,
>;

/// AUTHORITY: each physics step's body contacts on the buildings as loads, read once a step
/// (module doc): every point on a breakable element or chunk queued, the step's as one deposit,
/// with the blow and press its transfer splits into; and every resting point's press as its
/// building's ([`RestingLoads`]). A structure that is no building (a tree) reads its own.
fn read_contacts(
    transfers: Res<ContactTransfers>,
    mut read: Local<u64>,
    colliders: BuildingColliders,
    buildings: Query<((&Position, &Rotation), &Breakage, Option<&WallPiece>)>,
    mut queue: ResMut<WallQueue>,
    mut resting: ResMut<RestingLoads>,
) {
    if transfers.steps() == *read {
        return;
    }
    *read = transfers.steps();
    let contacts: Vec<(Entity, Load)> = transfers
        .split()
        .filter_map(|(point, blow, press, mass)| {
            contact_load(
                &colliders,
                &buildings,
                point,
                blow,
                press,
                mass,
                transfers.dt(),
            )
        })
        .collect();
    resting.0.clear();
    for rest in transfers.resting() {
        // A resting press has no blow, so no law reads its source mass; the point's own stands.
        if let Some((root, load)) = contact_load(
            &colliders,
            &buildings,
            &rest.transfer,
            Vec3::ZERO,
            rest.press(),
            rest.transfer.effective_mass,
            rest.dt,
        ) {
            resting.0.entry(root).or_default().push(load);
        }
    }
    if !contacts.is_empty() {
        queue.0.push(RawDeposit {
            contacts,
            ..default()
        });
    }
}

/// The load a body's contact point `transfer` puts on a building, with `blow` (N·s) and `press`
/// (N) in world terms, its contact's source mass `mass` (kg, [`ContactTransfers::split`]) and the
/// physics step `step` (s) the solver moved them within:
/// on the chunk whose collider the point lies on, or for an intact element
/// the element's chunk whose hull holds the point (the hull nearest it, by [`Block::outside`],
/// ties to the lower index), turned into the building's frame; with the building's root. `None`
/// when the structure is no building (a tree) and on an element that never breaks, which the laws
/// do not read. Reads the body's entity only to name the load's origin.
fn contact_load(
    colliders: &BuildingColliders,
    buildings: &Query<((&Position, &Rotation), &Breakage, Option<&WallPiece>)>,
    transfer: &ContactTransfer,
    blow: Vec3,
    press: Vec3,
    mass: f32,
    step: f32,
) -> Option<(Entity, Load)> {
    let (transform, breakage, moving) = buildings.get(transfer.structure).ok()?;
    let (element, chunk, parent) = colliders.get(transfer.structure_collider).expect(
        "a building collider the last physics step touched stands until the laws' next refit",
    );
    assert_eq!(
        parent.parent(),
        transfer.structure,
        "a building's collider is its root's child"
    );
    let layout = &breakage.kind.layout;
    let frame = Frame::new(
        &breakage.physics_frame(transform),
        breakage.scale,
        breakage.kind.bounds(),
    );
    let local = frame.unscaled(transfer.point);
    let chunk = match (element, chunk) {
        (_, Some(&BuildingChunk(c))) => usize::from(c),
        (Some(&BuildingElement(e)), None) => {
            layout.chunks_of(usize::from(e)).min_by(|&a, &b| {
                layout.blocks[a]
                    .outside(local)
                    .total_cmp(&layout.blocks[b].outside(local))
            })?
        }
        (None, None) => unreachable!("BuildingColliders admits elements and chunks only"),
    };
    Some((
        transfer.structure,
        Load {
            chunk,
            at: local * frame.scale,
            energy: 0.0,
            crushed: 0.0,
            blow: frame.turn(blow),
            press: frame.turn(press),
            peak: None,
            origin: if moving.is_some() {
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
        },
    ))
}

/// AUTHORITY: a charge set off at `at` (world), its blast fed to the blast field at the next
/// deposits, as a burst's is.
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct WallCharge {
    pub(crate) at: Vec3,
    pub(crate) blast: Blast,
}

fn set_off(charge: On<WallCharge>, mut queue: ResMut<WallQueue>) {
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
    mut queue: ResMut<WallQueue>,
) {
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
            let share = 1.0 / cone.rays.len().max(1) as f32;
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

/// A building's pose: world from its frame (the root's translation and rotation; the root is at
/// unit scale and the instance's scale is in the frame's lengths), and a ball in world that holds
/// its elements' bounds with room to spare ([`Frame::may_meet`]).
#[derive(Clone, Copy)]
struct Frame {
    translation: Vec3,
    rotation: Quat,
    scale: f32,
    /// The ball's centre and radius, m, in f64.
    centre: DVec3,
    radius: f64,
}

impl Frame {
    fn new(transform: &Transform, scale: f32, (lo, hi): (Vec3, Vec3)) -> Self {
        let middle = (lo + hi) / 2.0;
        let centre = transform.translation + transform.rotation * (middle * scale);
        // The bounds' half-diagonal, grown 5 % and half a metre: far beyond the rounding of every
        // test the deposits run in the building frame (DERIVED: f32 rounding, ~1e-4 m at a
        // kilometre).
        let radius = f64::from((hi - lo).length() / 2.0 * scale) * 1.05 + 0.5;
        Self {
            translation: transform.translation,
            rotation: transform.rotation,
            scale,
            centre: centre.as_dvec3(),
            radius,
        }
    }

    /// Whether the ray from `origin` along `dir` (world) can meet the building's bounds: false only
    /// when it passes clear of the ball, which holds the bounds with room to spare, so every ray
    /// the bounds' own test ([`Block::span`] in the building frame) accepts is kept.
    fn may_meet(&self, origin: Vec3, dir: Vec3) -> bool {
        let to = self.centre - origin.as_dvec3();
        let far = to.length_squared() - self.radius * self.radius;
        if far <= 0.0 {
            return true;
        }
        let along = to.dot(dir.as_dvec3().normalize_or_zero());
        along > 0.0 && far <= along * along
    }

    /// Whether the world point `at` can lie within the building's bounds grown by `slack` m.
    fn may_hold(&self, at: Vec3, slack: f32) -> bool {
        let reach = self.radius + f64::from(slack);
        self.centre.distance_squared(at.as_dvec3()) <= reach * reach
    }

    /// A world point in the asset's own frame, at scale 1.
    fn unscaled(&self, p: Vec3) -> Vec3 {
        self.rotation.inverse() * (p - self.translation) / self.scale
    }

    /// A world direction in the building frame.
    fn turn(&self, d: Vec3) -> Vec3 {
        self.rotation.inverse() * d
    }
}

/// The present blockers along a ray in the asset frame (unscaled): the first standing chunk of a
/// breakable element it enters, or an unbreakable element entered first, which stops it with no
/// deposit. Returns the law chunk and the distance. `placed` says whether the body is the placed
/// building, which holds the unbreakable elements; a moving group ([`WallPiece`]) holds only its
/// own chunks, and an unbreakable element stays with the placed building.
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

/// What a building took of one deposit, in its frame (scaled): lines and their loads, field
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
    queue: ResMut<'w, WallQueue>,
    resting: Res<'w, RestingLoads>,
    volumes: BuildingColliders<'w, 's>,
    tree_segments: Query<'w, 's, (&'static crate::trees::TreeSegment, &'static ChildOf)>,
    trunks: Query<
        'w,
        's,
        (
            &'static crate::trees::TreeState,
            &'static Transform,
            &'static mut crate::trees::TrunkLoads,
        ),
    >,
    buildings: Query<
        'w,
        's,
        (
            Entity,
            Option<&'static MapInstance>,
            (&'static Position, &'static Rotation),
            &'static mut Breakage,
            Option<&'static WallPiece>,
        ),
    >,
    clock: Option<Res<'w, ShotClock>>,
    record: ResMut<'w, WallRecord>,
    commands: Commands<'w, 's>,
    /// The moving bodies a blast's rays meet ([`moving_near`]) and push.
    spatial: SpatialQuery<'w, 's>,
    collider_of: Query<'w, 's, &'static ColliderOf>,
    aabbs: Query<'w, 's, &'static ColliderAabb>,
    bodies: Query<'w, 's, (Forces, &'static RigidBody)>,
}

/// The layers of the surfaces a blast's rays meet through avian: the ground, a standing tree and
/// every other static surface ([`Layer::Terrain`]), and a moving body's collision proxy. A tank's
/// armour and witness volumes ([`Layer`]) are its ballistics, not its surface.
const SURFACES: [Layer; 4] = [
    Layer::Terrain,
    Layer::Vehicle,
    Layer::Debris,
    Layer::TrackSide,
];

/// The colliders of every dynamic body whose bounds meet the cube of half-side `reach` m about
/// `origin`, ascending: the moving bodies a blast's rays push. Every other collider a ray meets
/// on [`SURFACES`] stops it with nothing, but a building's, which its own walk reads.
fn moving_near(p: &DepositParam, origin: Vec3, reach: f32) -> Vec<Entity> {
    let mut near = Vec::new();
    p.spatial.aabb_intersections_with_aabb_callback(
        ColliderAabb::new(origin, Vec3::splat(reach)),
        |collider| {
            if p.collider_of.get(collider).is_ok_and(|of| {
                p.bodies
                    .get(of.body)
                    .is_ok_and(|(_, body)| body.is_dynamic())
                    && !p.buildings.contains(of.body)
            }) {
                near.push(collider);
            }
            true
        },
    );
    near.sort_unstable();
    near
}

/// The spin `run_laws` reads for the body at `root`, in its construction `frame` (rad/s): a
/// moving group's body's angular velocity; zero on a placed building, whose groups come loose at
/// rest.
fn construction_spin(
    bodies: &Query<(Forces, &RigidBody)>,
    root: Entity,
    frame: Frame,
    moving: bool,
) -> Vec3 {
    if !moving {
        return Vec3::ZERO;
    }
    let (body, _) = bodies
        .get(root)
        .expect("a moving chunk group is a physics body");
    frame.rotation.inverse() * body.angular_velocity()
}

/// The field loads whose blow moves a moving group's body once the laws have run on `state`: a
/// field's blow on a chunk it turned to dust went into crushing it and moves nothing.
fn field_blows<'a>(loads: &'a [Load], state: &'a WallState) -> impl Iterator<Item = &'a Load> {
    loads.iter().filter(|load| {
        matches!(load.origin, Origin::Field)
            && load.blow != Vec3::ZERO
            && state.fate[load.chunk] != Fate::Dust
    })
}

/// Map resolved crossings to owned construction chunks, independent of the group's motion.
fn take_lines(
    p: &DepositParam,
    frames: &BTreeMap<Entity, Frame>,
    deposit: &RawDeposit,
    taken: &mut BTreeMap<Entity, Taken>,
) {
    for line in &deposit.crossings {
        let crate::ballistics::VolumeCrossing {
            volume,
            entry,
            end,
            energy,
            ..
        } = line.crossing;
        let entrance_energy = line.entrance_energy;
        let impulse = line.momentum;
        let entrance_impulse = line.entrance_momentum;
        let Ok((element, chunk, parent)) = p.volumes.get(volume) else {
            continue; // Another body's volume (a hull's armour, a trunk) is another law's.
        };
        let root = parent.parent();
        let frame = &frames[&root];
        let (_, _, _, breakage, _) = p
            .buildings
            .get(root)
            .expect("a building element or chunk is a child of its building's root");
        let layout = &breakage.kind.layout;
        let (a, b) = (frame.unscaled(entry), frame.unscaled(end));
        let length = a.distance(b);
        let dir = (b - a).normalize_or_zero();
        let shares: Vec<(usize, f32, f32)> = match (element, chunk) {
            (_, Some(&BuildingChunk(c))) => vec![(usize::from(c), 0.0, length)],
            (Some(&BuildingElement(e)), None) => layout
                .chunks_of(usize::from(e))
                .filter_map(|c| {
                    let (t0, t1) = layout.blocks[c].span(a, dir)?;
                    let (t0, t1) = (t0.max(0.0), t1.min(length));
                    (t1 > t0).then_some((c, t0, t1))
                })
                .collect(),
            (None, None) => {
                unreachable!("BuildingColliders admits elements and chunks only")
            }
        };
        let chord: f32 = shares.iter().map(|(_, t0, t1)| t1 - t0).sum();
        assert!(
            chord > 0.0,
            "a resolved building line crosses at least one owned chunk; its work and momentum cannot disappear"
        );
        let into = taken.entry(root).or_default();
        let turned = frame.turn(impulse - entrance_impulse);
        let &(first, _, _) = shares
            .iter()
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .expect("a positive chunk chord has an entrance");
        for (c, t0, t1) in shares {
            let share = (t1 - t0) / chord;
            let entry_energy = if c == first { entrance_energy } else { 0.0 };
            let entry_blow = if c == first {
                frame.turn(entrance_impulse)
            } else {
                Vec3::ZERO
            };
            let (from, to) = ((a + dir * t0) * frame.scale, (a + dir * t1) * frame.scale);
            let part = DeliveredLine {
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
            into.lines.push((c, from, to, part.crossing.energy));
            into.line_loads
                .extend(resolved_line_loads(c, &layout.materials[c], &part));
        }
    }
}

/// The game's damage-model construction loads: entrance work and momentum stay at entry;
/// the remaining work and momentum follow the chord. The building and wall inspector read this
/// same owner. The line's total energy sizes its chunk's visual channel independently of how
/// the solver samples its loads.
pub(crate) fn resolved_line_loads(
    chunk: usize,
    material: &Masonry,
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

/// AUTHORITY: run the tick's deposits through the laws (module doc), record every change and spawn
/// a body per loose group.
fn apply_deposits(mut p: DepositParam) {
    if p.queue.0.is_empty() {
        return;
    }
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
        .buildings
        .iter()
        .map(|(root, _, transform, breakage, _)| {
            (
                root,
                Frame::new(
                    &breakage.physics_frame(transform),
                    breakage.scale,
                    breakage.kind.bounds(),
                ),
            )
        })
        .collect();
    for deposit in std::mem::take(&mut p.queue.0) {
        let mut taken: BTreeMap<Entity, Taken> = BTreeMap::new();
        take_lines(&p, &frames, &deposit, &mut taken);
        // Each building's shape as the deposit found it, read once per deposit.
        let mut shapes: BTreeMap<Entity, Shape> = BTreeMap::new();
        for ray in &deposit.rays {
            let (end, dir, energy) = (ray.end, Vec3::from(ray.direction), ray.energy);
            // An angular stratum with zero profile emits no energy or momentum.
            if energy == 0.0 {
                continue;
            }
            for (root, frame) in &frames {
                if !frame.may_hold(end, STOP_TOLERANCE_M) {
                    continue;
                }
                let (_, _, _, breakage, piece) = p
                    .buildings
                    .get(*root)
                    .expect("frames enumerates this same building query");
                let placed = piece.is_none();
                let kind = &breakage.kind;
                let local = frame.unscaled(end);
                let (lo, hi) = kind.bounds();
                let slack = Vec3::splat(STOP_TOLERANCE_M / frame.scale);
                if local.cmplt(lo - slack).any() || local.cmpgt(hi + slack).any() {
                    continue;
                }
                let turned = frame.turn(dir);
                let back = 2.0 * STOP_TOLERANCE_M / frame.scale;
                let shape = shapes.entry(*root).or_insert_with(|| breakage.shape());
                if let Some((Some(c), t)) =
                    first_chunk(&kind.layout, shape, placed, local - turned * back, turned)
                    && (t - back).abs() * frame.scale <= STOP_TOLERANCE_M
                {
                    taken.entry(*root).or_default().field.push((
                        c,
                        local * frame.scale,
                        energy,
                        frame.turn(ray.blow),
                    ));
                }
            }
        }
        if let Some((origin, blast)) = deposit.blast {
            cast_blast(&mut p, &frames, &mut shapes, &mut taken, origin, blast);
        }
        for &(root, load) in &deposit.contacts {
            taken.entry(root).or_default().contacts.push(load);
        }
        let mut departures = Vec::new();
        for (root, taken) in taken {
            let (_, identity, transform, mut breakage, piece) = p.buildings.get_mut(root).expect(
                "a deposit's take is keyed by building roots, which a match never despawns",
            );
            let frame = frames[&root];
            let kind = breakage.kind.clone();
            let (building, label) = match (identity, piece) {
                (Some(identity), None) => (identity.ordinal, identity.id.as_str()),
                (None, Some(piece)) => (piece.building, kind.asset.id.as_str()),
                _ => panic!("a chunk group is either a placed map instance or one moving owner"),
            };
            let scale = breakage.scale;
            let construction = breakage.physics_frame(transform);
            let moving = piece.map(|piece| piece.chunks.clone());
            let live = breakage.bypass_change_detection().live();
            let blast = blast_loads(label, &kind, frame.scale, &taken.blast, live);
            let holes = holes_of(&kind, frame.scale, &taken.lines, &blast);
            // A run that changes nothing leaves the colliders as they stand: only a recorded
            // change marks the building for the refit (`reconcile`).
            let resting = p.resting.0.get(&root).map_or(&[][..], Vec::as_slice);
            let pressing = presses.get(&root).map_or(&[][..], Vec::as_slice);
            let loads: Vec<Load> = taken
                .line_loads
                .into_iter()
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
            let spin = construction_spin(&p.bodies, root, frame, moving.is_some());
            let mut changes = run_laws(&kind.layout.element_of, live, &loads, holes, spin);
            // A moving group is already a body: a field's blow moves it directly, at the point
            // it struck, except where it turned its chunk to dust and went into crushing it
            // (`run_laws`), as on a standing building.
            if moving.is_some() {
                let (mut body, _) = p
                    .bodies
                    .get_mut(root)
                    .expect("a moving chunk group is a physics body");
                for load in field_blows(&loads, &live.state) {
                    body.apply_linear_impulse_at_point(
                        frame.rotation * load.blow,
                        frame.translation + frame.rotation * load.at,
                    );
                }
            }
            if moving.is_some() {
                changes
                    .events
                    .retain(|event| !matches!(event, WallEvent::Fall(_)));
            }
            let counts = [
                taken.lines.len(),
                taken.field.len(),
                taken.blast.len(),
                taken.contacts.len().max(pressing.len()) + resting.len(),
            ];
            if !changes.events.is_empty() || loads.iter().any(|load| load.energy > 0.0) {
                log_changes(label, &loads, counts, &changes);
            }
            p.record
                .entries
                .extend(changes.events.iter().map(|&event| WallEntry {
                    building,
                    event,
                    tick,
                }));
            give_back(&mut p.bodies, frame, &changes.returns);
            if let Some(owned) = moving {
                let groups: Vec<_> = changes.groups.into_iter().map(|(group, _)| group).collect();
                let unchanged = groups.len() == 1
                    && groups[0]
                        .iter()
                        .copied()
                        .eq(owned.iter().map(|&c| usize::from(c)));
                if unchanged {
                    for &c in &groups[0] {
                        live.state.fate[c] = Fate::Standing;
                    }
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
                    p.commands
                        .entity(root)
                        .insert(PendingSplit { children, tick });
                }
            } else {
                for (group, loads) in changes.groups {
                    let launch = launch(&kind, &live.structure, frame.scale, &group, &loads);
                    departures.push(Departure {
                        kind: kind.clone(),
                        building,
                        root: construction,
                        scale,
                        live: live.group(&group),
                        group,
                        launch,
                        tick,
                    });
                }
            }
            if !changes.events.is_empty() {
                breakage.set_changed();
            }
        }
        for departure in departures {
            spawn_group(&mut p.commands, departure, true);
        }
    }
}

/// A blast's ray hits on one building as its loads ([`Load::blast`]), each face's Law 2 read on
/// its chunk's hull at the instance's `scale`; logs the rear scabs thrown (debug).
fn blast_loads(
    id: &str,
    kind: &BuildingKind,
    scale: f32,
    hits: &[(usize, fracture::RayHit)],
    live: &Live,
) -> Vec<Load> {
    if hits.is_empty() {
        return Vec::new(); // This deposit has no blast surface stops.
    }
    let contacts: Vec<Joint> = live.state.contacts(&live.structure).collect();
    let (loads, scabs) = Load::blast(hits, |c| fracture::Backing {
        block: &kind.layout.blocks[c],
        scale,
        material: kind.layout.materials[c],
        contacts: &contacts,
    });
    if !scabs.is_empty() {
        debug!(
            "buildings: {id} took {} rear scabs, {:.2} kg carrying {:.1} N s",
            scabs.len(),
            scabs.iter().map(|scab| scab.mass).sum::<f32>(),
            scabs
                .iter()
                .map(|scab| scab.momentum().length())
                .sum::<f32>(),
        );
    }
    loads
}

/// Log what one deposit did to a building (debug): `id` the building's, `loads` its deposit's,
/// `counts` its lines, field stops, blast rays and contact points.
fn log_changes(id: &str, loads: &[Load], counts: [usize; 4], changes: &Changes) {
    let count = |f: fn(&WallEvent) -> bool| changes.events.iter().filter(|e| f(e)).count();
    debug!(
        "buildings: {id} took {:.0} J in {} chunks ({} lines, {} field stops, {} blast rays, {} \
         contact points from {} bodies pressing {:.0} N): {} swapped, {} dust, {} holes, {} \
         cracks, {} structural sections {:?} (cause, demand, capacity: N s m for a push, Pa for \
         weight), weight check {} steps with {} touched chunks unassessed, {} chunks fell in {} \
         groups",
        loads.iter().map(|load| load.energy).sum::<f32>(),
        loads
            .iter()
            .filter(|load| load.energy > 0.0)
            .map(|load| load.chunk)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        counts[0],
        counts[1],
        counts[2],
        counts[3],
        loads
            .iter()
            .filter_map(|load| match load.origin {
                Origin::Body { body, .. } => Some(body),
                Origin::Contact { other, .. } => Some(other),
                Origin::Field | Origin::Inertia { .. } => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        loads.iter().map(|load| load.press.length()).sum::<f32>(),
        count(|e| matches!(e, WallEvent::Swap(_))),
        count(|e| matches!(e, WallEvent::Dust(_))),
        count(|e| matches!(e, WallEvent::Hole(..))),
        count(|e| matches!(e, WallEvent::Crack(_))),
        changes.failures.len(),
        changes
            .failures
            .iter()
            .map(|f| (f.cause, f.demand, f.capacity))
            .collect::<Vec<_>>(),
        changes.weight_steps,
        changes.unassessed.len(),
        count(|e| matches!(e, WallEvent::Fall(_))),
        changes.groups.len(),
    );
}

/// One blast's rays from `origin` (module doc): each stops, within the blast's reach, on the
/// nearest of every building's present blockers ([`first_chunk`]), the moving bodies
/// ([`moving_near`]) and every other surface on [`SURFACES`] (the ground, a standing tree). A ray
/// on a standing chunk joins its building's take; a ray on a moving body pushes the body through
/// avian at the point it stops. A trunk stop loads its segment through the tree laws; other
/// static surfaces only shade the field.
/// Each chunk and body collider takes the share of its rays' cones its silhouette intercepts
/// ([`fracture::intercepted`]): a chunk's from its faces, a collider's from its bounds, the
/// silhouette of the box that holds it. A collider holding `origin` shades nothing and takes
/// nothing: what a burst inside a body does to it is no law here.
fn cast_blast(
    p: &mut DepositParam,
    frames: &BTreeMap<Entity, Frame>,
    shapes: &mut BTreeMap<Entity, Shape>,
    taken: &mut BTreeMap<Entity, Taken>,
    origin: Vec3,
    blast: Blast,
) {
    let reach = blast.reach();
    let near = moving_near(p, origin, reach);
    // The colliders holding the burst point: a ray from inside one leaves it through no face it
    // meets, so they shade nothing and take no push of the field's.
    let surfaces = SpatialQueryFilter::from_mask(SURFACES);
    let inside = p.spatial.point_intersections(origin, &surfaces);
    let surfaces = surfaces.with_excluded_entities(inside);
    // A building's colliders are its walk's ([`first_chunk`]); every other surface stops a ray.
    let meets = |collider: Entity| {
        near.binary_search(&collider).is_ok()
            || !p
                .collider_of
                .get(collider)
                .is_ok_and(|of| p.buildings.contains(of.body))
    };
    // Per building a ray may meet: the origin in its frame, and its bounds.
    let mut views: BTreeMap<Entity, (Vec3, Block)> = BTreeMap::new();
    let mut faces: BTreeMap<(Entity, usize), Vec<(Vec3, f32)>> = BTreeMap::new();
    let hits = fracture::trace_blast(blast, fracture::blast_directions(), |dir| {
        let mut stop: Option<(Entity, Option<usize>, f32, f32)> = None;
        for (root, frame) in frames {
            if !frame.may_meet(origin, dir) {
                continue;
            }
            let (_, _, _, breakage, piece) = p
                .buildings
                .get(*root)
                .expect("frames enumerates this same building query");
            let placed = piece.is_none();
            let kind = &breakage.kind;
            let (local, bounds) = views.entry(*root).or_insert_with(|| {
                let (lo, hi) = kind.bounds();
                (frame.unscaled(origin), Block::cuboid(lo, hi))
            });
            let turned = frame.turn(dir);
            if bounds.span(*local, turned).is_none() {
                continue;
            }
            let shape = shapes.entry(*root).or_insert_with(|| breakage.shape());
            if let Some((chunk, t)) = first_chunk(&kind.layout, shape, placed, *local, turned)
                && t * frame.scale <= reach
                && stop.is_none_or(|(.., d)| t * frame.scale < d)
            {
                stop = Some((*root, chunk, t, t * frame.scale));
            }
        }
        let surface = p.spatial.cast_ray_predicate(
            origin,
            Dir3::new(dir).expect("the blast field's directions are unit"),
            stop.map_or(reach, |(.., d)| d),
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
                // Terrain shades the ray. A live trunk receives this same visible stop through
                // its own laws; no second cast can pass the wall or ground before it.
                let Ok((&crate::trees::TreeSegment(segment), parent)) =
                    p.tree_segments.get(hit.entity)
                else {
                    return None;
                };
                let (state, root, _) = p
                    .trunks
                    .get(parent.parent())
                    .expect("a standing segment belongs to its tree root");
                if matches!(*state, crate::trees::TreeState::Down { cut, .. } if segment < cut) {
                    return None; // The stump is fixed ground matter.
                }
                let into = root.rotation.inverse();
                return Some(fracture::BlastSurface {
                    target: Stop::Trunk(parent.parent(), segment),
                    at: into * (origin + dir * hit.distance - root.translation),
                    normal: into * hit.normal,
                    direction: into * dir,
                    distance: hit.distance,
                    silhouette: shown,
                    material: None,
                });
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
        let (root, Some(c), t, d) = stop? else {
            return None;
        }; // Unbreakable matter shades it.
        let (_, _, _, breakage, _) = p
            .buildings
            .get(root)
            .expect("the stop names a queried building");
        let (frame, local) = (frames[&root], views[&root].0);
        let turned = frame.turn(dir);
        let block = &breakage.kind.layout.blocks[c];
        let (_, normal) = block
            .entry_face(local, turned)
            .expect("the ray's stop is where it enters the chunk");
        let shown = fracture::silhouette(
            faces.entry((root, c)).or_insert_with(|| block.face_areas()),
            turned,
        ) * frame.scale
            * frame.scale;
        Some(fracture::BlastSurface {
            target: Stop::Chunk(root, c),
            at: (local + turned * t) * frame.scale,
            normal,
            direction: turned,
            distance: d,
            silhouette: shown,
            material: Some(breakage.kind.layout.materials[c]),
        })
    });
    for (target, hit) in hits {
        match target {
            Stop::Chunk(root, c) => taken.entry(root).or_default().blast.push((c, hit)),
            Stop::Trunk(root, segment) => {
                p.trunks
                    .get_mut(root)
                    .expect("the world-ray tree survives this structural pass")
                    .2
                    .0
                    .push(Load {
                        chunk: usize::from(segment),
                        at: hit.at,
                        blow: hit.impulse,
                        peak: Some(hit.peak),
                        press: Vec3::ZERO,
                        energy: 0.0,
                        crushed: 0.0,
                        origin: Origin::Field,
                    });
            }
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

/// What a blast ray stopped on ([`cast_blast`]): a building's law chunk, or a moving body's
/// collider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stop {
    Chunk(Entity, usize),
    Body(Entity),
    Trunk(Entity, u8),
}

/// The holes one deposit cuts (module doc), each with its law chunk, in the baked chunk's frame
/// ([`Hole`]); `lines` and the blast's loads `blast` ([`Load::blast`]) in the building frame at
/// the instance's `scale`. Each is sized by the volume it crushes, the volume its load takes from
/// the chunk's pool: a line's, its energy over the crush energy ([`Masonry::crush`]); a blast's,
/// its load's Law 1 volume ([`Load::crushed`]). A line's channel runs along its chord, its radius
/// the narrower of the cylinder about the chord and the ball that hold that volume (so a chord
/// grazing a corner never cuts a disc wider than the ball); the blast's dish in a chunk is a ball
/// about the chunk's load (its face's impulse-weighted centroid) whose half holds it. ASSUMED: a
/// channel's section is a circle and its width the energy's alone, not the round's diameter; a
/// dish's centre lies on the chunk's face, so half its ball is in the chunk.
fn holes_of(
    kind: &BuildingKind,
    scale: f32,
    lines: &[(usize, Vec3, Vec3, f32)],
    blast: &[Load],
) -> Vec<(usize, Hole)> {
    let layout = &kind.layout;
    cut_holes(
        &layout.blocks,
        |c| Vec3::from_array(kind.asset.chunks[layout.chunks[c] as usize].centroid),
        |c| layout.materials[c].crush,
        scale,
        lines,
        blast,
    )
}

/// [`holes_of`] on any chunk table: each law chunk's hull `blocks`, its baked `centroid` (the
/// hole's frame) and its material's crush energy `crush`, all at scale 1. The sandbox's wall cuts
/// its holes here too, so its deposits reach [`HOLES_PER_CHUNK`] as the game's do.
pub(crate) fn cut_holes(
    blocks: &[Block],
    centroid: impl Fn(usize) -> Vec3,
    crush: impl Fn(usize) -> f32,
    scale: f32,
    lines: &[(usize, Vec3, Vec3, f32)],
    blast: &[Load],
) -> Vec<(usize, Hole)> {
    use std::f32::consts::PI;
    let crushed = |c: usize, energy: f32| energy / crush(c);
    let channels = lines.iter().filter_map(|&(c, from, to, energy)| {
        let volume = crushed(c, energy);
        let ball = (0.75 * volume / PI).cbrt();
        let radius = (volume / (PI * from.distance(to))).sqrt().min(ball);
        let (from, to) = open_ends(&blocks[c], from / scale, to / scale, scale);
        Hole::cut(centroid(c), from, to, radius / scale).map(|hole| (c, hole))
    });
    let dishes = blast.iter().filter_map(|load| {
        let radius = (1.5 * load.crushed / PI).cbrt();
        let at = load.at / scale;
        Hole::cut(centroid(load.chunk), at, at, radius / scale).map(|hole| (load.chunk, hole))
    });
    channels.chain(dishes).collect()
}

/// How far inside its chunk's hull a channel's end may sit and still be where the line crosses a
/// face, m (DERIVED: the walk's crossings meet a face to f32 rounding, far under a millimetre).
const ON_FACE_M: f32 = 1e-3;

/// A channel's ends `from`–`to` in its chunk's hull `block` (asset frame, scale 1; the instance at
/// `scale`), each end on the hull's surface moved out along the line to the hull's farthest corner
/// along it. Inside the chunk the capsule a [`Hole`] draws is then the cylinder about the line:
/// every chunk point projects between the ends, so its distance to the segment is its distance to
/// the line, and each face the line crosses opens across the cylinder's whole ellipse, with no rim
/// closed by the capsule's rounded end. An end inside the hull (a round at rest there) stays, so
/// its channel stays closed there. The ends lie within the hull's width of the hull.
fn open_ends(block: &Block, from: Vec3, to: Vec3, scale: f32) -> (Vec3, Vec3) {
    let Some(along) = (to - from).try_normalize() else {
        return (from, to);
    };
    let on_face = |p: Vec3| {
        block
            .planes
            .iter()
            .any(|&(n, o)| n.dot(p) >= o - ON_FACE_M / scale)
    };
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

/// What the laws changed in one building: its record events in order, each loose group with the
/// loads on it (building frame), the structural failures (the push check's, then the weight
/// check's), and the weight check's unassessed chunks and operation count ([`fracture::Weighed`]).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Changes {
    pub(crate) events: Vec<WallEvent>,
    pub(crate) groups: Vec<(Vec<usize>, Vec<Load>)>,
    pub(crate) failures: Vec<fracture::Failure>,
    pub(crate) unassessed: Vec<usize>,
    pub(crate) weight_steps: usize,
    /// Body blows on chunks with no carrier in this deposit, returned whole in building terms:
    /// dust has none, and a chunk an earlier deposit freed already launched as its own body.
    pub(crate) returns: Vec<BodyReturn>,
}

/// Run one deposit's `loads`, with the `holes` it cuts ([`holes_of`]), through the laws on `live`
/// (module doc) and say what changed: the elements first touched swap, then the dust, the holes
/// kept, the cracks (the push rule's, then the structural check's, then the weight check's) and the
/// falls, each ascending but the holes, in deposit order. A chunk the holes would carry past
/// [`HOLES_PER_CHUNK`] crumbles to dust before the laws run; a hole is kept only in a chunk
/// standing after the removal, and no law reads one. A field load on a chunk turned to dust
/// went into crushing it and moves nothing; a body's blow there is returned whole because dust
/// has no carrier. A queued contact on a chunk an earlier deposit freed is returned whole too:
/// that chunk launched as its own body, which the next physics step's solver meets as a dynamic
/// body, so the momentum the static stop took from the source is neither kept nor spent twice.
/// A held section's energy check reads a body's pair work against its hinge effective mass; a
/// freed group launches against its free effective mass, each body's blow first reduced by the
/// work the openings about the group spent ([`fracture::debit_work`]). Gravity plus presses reach the strength and rest
/// laws from changed supports and pressed chunks. An identical press set on unchanged topology
/// reuses its held verdict; every changed force or point, including a decrease, rechecks.
pub(crate) fn run_laws(
    element_of: &[u16],
    live: &mut Live,
    loads: &[Load],
    holes: Vec<(usize, Hole)>,
    spin: Vec3,
) -> Changes {
    let mut cutting: BTreeMap<usize, usize> = BTreeMap::new();
    for &(c, _) in &holes {
        *cutting.entry(c).or_default() += 1;
    }
    let mut crumbled = Vec::new();
    for (c, n) in cutting {
        let pool = live.state.pool[c];
        if live.holes[c].len() + n > HOLES_PER_CHUNK
            && fracture::wear(&live.structure, &mut live.state, c, pool)
        {
            crumbled.push(c);
        }
    }
    let mut struck = fracture::strike(&live.structure, &mut live.state, loads);
    struck.dusted.extend(crumbled);
    struck.dusted.sort_unstable();
    let holes: Vec<(usize, Hole)> = holes
        .into_iter()
        .filter(|&(c, _)| live.state.standing(c))
        .collect();
    for &(c, hole) in &holes {
        live.holes[c].push(hole);
    }
    let returns = loads
        .iter()
        .filter(|load| !live.state.standing(load.chunk))
        .filter_map(|load| BodyReturn::of(load, Vec3::ZERO))
        .collect();
    let loads: Vec<Load> = loads
        .iter()
        .filter(|load| live.state.standing(load.chunk))
        .copied()
        .collect();
    let inputs = GroupLoads::new(&loads, spin);
    let mut failures =
        fracture::structural(&live.structure, &mut live.state, inputs, &mut live.sections);
    let pushed: Vec<usize> = struck
        .cracked
        .iter()
        .chain(failures.iter().flat_map(|failure| &failure.joints))
        .copied()
        .collect();
    let presses: Vec<_> = loads
        .iter()
        .filter(|load| load.press != Vec3::ZERO)
        .map(|load| (load.chunk, load.at, load.press))
        .collect();
    let unloaded: Vec<_> = live
        .held_presses
        .iter()
        .map(|&(chunk, ..)| chunk)
        .filter(|chunk| !presses.iter().any(|&(current, ..)| current == *chunk))
        .collect();
    // A framed group's frame is this run's own (its motion changes every run), so no held
    // verdict carries over to it.
    let recheck = !struck.dusted.is_empty()
        || !pushed.is_empty()
        || presses != live.held_presses
        || !fracture::loose_groups(&live.structure, &live.state).is_empty();
    let weighed = fracture::weigh(
        &live.structure,
        &mut live.state,
        &struck.dusted,
        &pushed,
        if recheck {
            inputs
        } else {
            GroupLoads::new(&[], spin)
        },
        &unloaded,
        &mut live.sections,
    );
    failures.extend(weighed.failures);
    let structural: Vec<usize> = failures
        .iter()
        .flat_map(|failure| failure.joints.iter().copied())
        .collect();
    let groups = fracture::fall(&live.structure, &mut live.state);
    // The strength loop reached a held verdict on the final topology. A fall removes only
    // groups with no live support path, so it changes no remaining held group's load path.
    live.held_presses = presses;
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
    let mut events = Vec::new();
    for &e in &swaps {
        live.swapped[e] = true;
        events.push(WallEvent::Swap(e as u16));
    }
    events.extend(struck.dusted.iter().map(|&c| WallEvent::Dust(c as u16)));
    events.extend(
        holes
            .iter()
            .map(|&(c, hole)| WallEvent::Hole(c as u16, hole)),
    );
    events.extend(
        struck
            .cracked
            .iter()
            .chain(&structural)
            .map(|&j| WallEvent::Crack(j as u16)),
    );
    events.extend(groups.iter().flatten().map(|&c| WallEvent::Fall(c as u16)));
    let groups = groups
        .into_iter()
        .map(|group| {
            let on_group = loads
                .iter()
                .filter(|load| group.binary_search(&load.chunk).is_ok())
                .copied();
            let on_group = fracture::debit_work(&live.structure, &failures, &group, on_group);
            (group, on_group)
        })
        .collect();
    Changes {
        events,
        groups,
        failures,
        unassessed: weighed.unassessed,
        weight_steps: weighed.steps,
        returns,
    }
}

/// A law chunk's collider shape for an instance at `scale`, about `origin` (building frame,
/// scaled), its faces moved in by `gap` m. The layout certified every law chunk's hull as a
/// solid convex collider at load; a positive uniform scale and a shrink toward its mean of at
/// most 80 % keep it one.
pub(crate) fn chunk_collider(
    kind: &BuildingKind,
    c: usize,
    scale: f32,
    origin: Vec3,
    gap: f32,
) -> Collider {
    let block = &kind.layout.blocks[c];
    let middle = block.points.iter().sum::<Vec3>() / block.points.len().max(1) as f32;
    let shrink = (1.0 - gap / (block.inradius() * scale).max(1e-6)).max(0.2);
    Collider::convex_hull(
        block
            .points
            .iter()
            .map(|p| (middle + (*p - middle) * shrink) * scale - origin)
            .collect(),
    )
    .expect("a load-certified hull, scaled and shrunk toward its mean, stays a solid convex hull")
}

/// The ballistic volume of law chunk `c`: its element's substance.
fn chunk_volume(kind: &BuildingKind, c: usize) -> BallisticVolume {
    let element = &kind.asset.elements[usize::from(kind.layout.element_of[c])];
    let material = usize::from(element.material);
    BallisticVolume {
        material: kind.materials[material],
        substance: kind.asset.materials[material].clone(),
    }
}

/// A freed carrier staged until every building in the deposit has answered its contacts.
/// One source can free groups in several buildings; their combined exchange is bounded once.
struct Departure {
    scale: f32,
    live: Live,
    tick: u32,
    kind: Arc<BuildingKind>,
    building: u16,
    root: Transform,
    group: Vec<usize>,
    launch: Launch,
}

/// One loose group's colliders about its chunks' centre of mass (building frame) and departure:
/// each field blow launches whole, each body blow only its plastic share against the group's free
/// effective mass at the load point. The rest returns to its source at that point. The wall
/// design ("Bodies in contact") owns the momentum and energy derivation.
pub(crate) fn launch(
    kind: &BuildingKind,
    structure: &Structure,
    scale: f32,
    group: &[usize],
    loads: &[Load],
) -> Launch {
    let chunks = &structure.chunks;
    let mass: f32 = group.iter().map(|&i| chunks[i].mass).sum();
    let centroid = group
        .iter()
        .map(|&i| chunks[i].centroid * chunks[i].mass)
        .sum::<Vec3>()
        / mass;
    let colliders: Vec<(usize, Collider)> = group
        .iter()
        .map(|&c| (c, chunk_collider(kind, c, scale, centroid, LOOSE_GAP_M)))
        .collect();
    launch_colliders(centroid, colliders, |c| chunks[c].material.density, loads)
}

/// Return a departing chunk's excess solver impulse immediately, at the original world point.
/// A structure that held supplies no return; presses remain with the standing structure.
fn give_back(bodies: &mut Query<(Forces, &RigidBody)>, frame: Frame, returns: &[BodyReturn]) {
    for returned in returns {
        let (mut body, kind) = bodies
            .get_mut(returned.body)
            .expect("a contact's dynamic source survives the same tick's building laws");
        assert!(kind.is_dynamic(), "a contact's source is a dynamic body");
        body.apply_linear_impulse_at_point(
            frame.rotation * returned.impulse,
            frame.translation + frame.rotation * returned.at,
        );
    }
}

/// Spawn one loose group's body (module doc): at its chunks' centre of mass, turned as its
/// building `(ordinal, root pose)`, moving as its bounded plastic exchange leaves it ([`launch`]).
#[cfg(test)]
fn spawn_piece(
    commands: &mut Commands,
    kind: &Arc<BuildingKind>,
    (building, root): (u16, Transform),
    group: &[usize],
    launch: Launch,
) {
    let scale = root.scale.x;
    let live = Live::new(kind, scale).group(group);
    spawn_group(
        commands,
        Departure {
            kind: kind.clone(),
            building,
            root,
            group: group.to_vec(),
            launch,
            live,
            scale,
            tick: 0,
        },
        true,
    );
}

/// Spawn a group from its synchronous construction state, initial or split.
fn spawn_group(commands: &mut Commands, departure: Departure, finalize: bool) {
    let Departure {
        kind,
        building,
        root,
        group,
        mut launch,
        live,
        scale,
        tick,
    } = departure;
    let colliders = std::mem::take(&mut launch.colliders);
    let Launch {
        centroid,
        velocity,
        spin,
        mass,
        ..
    } = &launch;
    let at = root.translation + root.rotation * *centroid;
    let mut elements: Vec<&str> = group
        .iter()
        .map(|&c| {
            kind.asset.elements[usize::from(kind.layout.element_of[c])]
                .name
                .as_str()
        })
        .collect();
    elements.dedup();
    debug!(
        "buildings: building {building} drops {} chunks ({:.0} kg, of {elements:?}) at {at:?}, \
         leaving at {:.3} m/s spinning {:.3} rad/s",
        group.len(),
        mass,
        velocity.length(),
        spin.length(),
    );
    // `Position`/`Rotation` before the body: it never flushes with avian's placeholder pose.
    let mut piece = commands.spawn((
        Name::new("WallPiece"),
        WallPiece {
            building,
            chunks: group.iter().map(|&c| c as u16).collect(),
            born: tick,
            retired: None,
        },
        Breakage {
            kind: kind.clone(),
            scale,
            live: Some(Box::new(live)),
            origin: *centroid,
        },
        Transform::from_translation(at).with_rotation(root.rotation),
        Position(at),
        Rotation(root.rotation),
        Visibility::default(),
        crate::debris::DebrisPiece,
        crate::eased_dynamic_body(),
        LinearVelocity(root.rotation * *velocity),
        AngularVelocity(root.rotation * *spin),
    ));
    if finalize {
        piece.insert(PendingLaunch::new(root, launch));
    }
    piece.with_children(|parent| {
        for (c, collider) in colliders {
            parent.spawn((
                Transform::IDENTITY,
                collider,
                ColliderDensity(kind.layout.materials[c].density),
                piece_layers(),
                kind.chunk_friction(c),
                chunk_volume(&kind, c),
                BuildingChunk(c as u16),
            ));
        }
    });
}

/// A split retiring its parent group's body at `tick`, triggered as `split_groups` despawns it:
/// the server keeps the parent's carrier with its final pose and this tick
/// (`net::debris_stress`), so replicas draw the parent until its replacements take over.
#[derive(Event)]
pub(crate) struct RetirePiece {
    pub(crate) entity: Entity,
    pub(crate) tick: u32,
}

/// A seam failure's new groups, each with its law state, staged on the parent until all
/// same-tick plastic returns have reached it: no source disappears before a simultaneous launch
/// finishes exchanging with it. `split_groups` spawns them at `tick`.
#[derive(Component)]
struct PendingSplit {
    children: Vec<(Vec<usize>, Live)>,
    tick: u32,
}

fn split_groups(
    parents: Query<(
        Entity,
        &WallPiece,
        (&Position, &Rotation),
        &Breakage,
        &LinearVelocity,
        &AngularVelocity,
        &avian3d::prelude::ComputedCenterOfMass,
        &PendingSplit,
    )>,
    mut commands: Commands,
) {
    for (entity, piece, body, breakage, velocity, spin, centre, split) in &parents {
        let root = breakage.physics_frame(body);
        let inverse = root.rotation.inverse();
        for (group, live) in &split.children {
            let mut launch = launch(&breakage.kind, &live.structure, breakage.scale, group, &[]);
            // DERIVED rigid velocity field: every new COM gets v + omega cross r; every child
            // retains omega. Exact child colliders partition the parent's occupied geometry.
            launch.velocity = inverse * velocity.0
                + (inverse * spin.0).cross(launch.centre - breakage.origin - centre.0);
            launch.spin = inverse * spin.0;
            spawn_group(
                &mut commands,
                Departure {
                    kind: breakage.kind.clone(),
                    building: piece.building,
                    root,
                    group: group.clone(),
                    launch,
                    scale: breakage.scale,
                    live: live.clone(),
                    tick: split.tick,
                },
                false,
            );
        }
        debug!(
            "buildings: group of {} chunks splits into {:?}",
            piece.chunks.len(),
            split
                .children
                .iter()
                .map(|(group, _)| group.len())
                .collect::<Vec<_>>()
        );
        commands.trigger(RetirePiece {
            entity,
            tick: split.tick,
        });
        commands.entity(entity).despawn();
    }
}

/// A loose chunk's filter: brick in ballistics ([`Layer::Armor`]), colliding as debris with the
/// ground, buildings, hulls and other pieces, never with a tank's armour volumes.
pub(crate) fn piece_layers() -> CollisionLayers {
    CollisionLayers::new(
        [Layer::Debris, Layer::Armor],
        [
            Layer::Terrain,
            Layer::Vehicle,
            Layer::Debris,
            Layer::TrackSide,
        ],
    )
}

/// REPLICA: rebuild every building's state from the record, from the entry it has not applied; a
/// record of another generation first stands every building whole again.
fn derive_from_record(
    record: Res<WallRecord>,
    mut applied: Local<(u32, usize)>,
    mut buildings: Query<(&MapInstance, &mut Breakage), Without<WallPiece>>,
) {
    if applied.0 != record.generation || record.entries.len() < applied.1 {
        for (_, mut breakage) in &mut buildings {
            if breakage.live.is_some() {
                breakage.live = None;
            }
        }
        *applied = (record.generation, 0);
    }
    if record.entries.len() == applied.1 {
        return;
    }
    let mut by_ordinal: BTreeMap<u16, Vec<WallEvent>> = BTreeMap::new();
    for entry in &record.entries[applied.1..] {
        by_ordinal
            .entry(entry.building)
            .or_default()
            .push(entry.event);
    }
    for (identity, mut breakage) in &mut buildings {
        let Some(events) = by_ordinal.remove(&identity.ordinal) else {
            continue;
        };
        let live = breakage.live();
        let mut shape = live.shape();
        for event in events {
            shape.apply(event);
            if let WallEvent::Hole(c, hole) = event {
                live.holes[usize::from(c)].push(hole);
            }
        }
        live.swapped = shape.swapped;
        live.state.fate = shape.fate;
        live.state.cracked = shape.cracked;
    }
    assert!(
        by_ordinal.is_empty(),
        "a wall record names only buildings this map places: {:?}",
        by_ordinal.keys().collect::<Vec<_>>()
    );
    applied.1 = record.entries.len();
}

/// Every peer: make a changed building's colliders its shape's — an intact element its own
/// collider, a swapped element one static collider per standing chunk — spawning and despawning
/// only what differs. A spawned collider carries its pose so the refit that follows places it.
fn reconcile(
    buildings: Query<
        (Entity, &Transform, &Breakage, Option<&Children>),
        (Changed<Breakage>, Without<WallPiece>),
    >,
    elements: Query<&BuildingElement>,
    chunks: Query<&BuildingChunk>,
    mut commands: Commands,
) {
    for (root, transform, breakage, children) in &buildings {
        let kind = &breakage.kind;
        let shape = breakage.shape();
        let (mut have_elements, mut have_chunks) = (BTreeMap::new(), BTreeMap::new());
        for child in children.into_iter().flatten() {
            if let Ok(&BuildingElement(e)) = elements.get(*child) {
                have_elements.insert(usize::from(e), *child);
            } else if let Ok(&BuildingChunk(c)) = chunks.get(*child) {
                have_chunks.insert(usize::from(c), *child);
            }
        }
        let pose = (
            Position(transform.translation),
            Rotation(transform.rotation),
        );
        for (e, swapped) in shape.swapped.iter().enumerate() {
            match (swapped, have_elements.get(&e)) {
                (true, Some(entity)) => commands.entity(*entity).despawn(),
                (false, None) => {
                    commands.spawn((
                        ChildOf(root),
                        super::element_bundle(kind, e, breakage.scale),
                        pose,
                    ));
                }
                _ => {}
            }
        }
        for c in 0..kind.layout.chunks.len() {
            match (shape.chunk_present(&kind.layout, c), have_chunks.get(&c)) {
                (false, Some(entity)) => commands.entity(*entity).despawn(),
                (true, None) => {
                    commands.spawn((
                        ChildOf(root),
                        Transform::IDENTITY,
                        chunk_collider(kind, c, breakage.scale, Vec3::ZERO, 0.0),
                        super::element_layers(),
                        kind.chunk_friction(c),
                        chunk_volume(kind, c),
                        BuildingChunk(c as u16),
                        pose,
                    ));
                }
                _ => {}
            }
        }
    }
}

/// Avian's refit of moved colliders in a frame a building changed: the spawned chunks' bounds
/// and the collider trees a spatial query walks, so the next march reads them.
fn refit_systems() -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (
        reconcile,
        update_moved_collider_aabbs::<Collider>
            .run_if(resource_exists::<ColliderTrees>)
            .run_if(any_match_filter::<Changed<Breakage>>),
    )
        .chain()
}

/// The authority's set: deposits, then the colliders, after the march.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WallLawSet;

/// A replica's set, in `Update`: the record's new entries into the buildings, then the colliders.
/// Whatever fills the record runs before it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WallReplicaSet;

pub(crate) fn sim_plugin(app: &mut App) {
    if !app.is_plugin_added::<LaunchPlugin>() {
        app.add_plugins(LaunchPlugin);
    }
    app.init_resource::<WallQueue>()
        .init_resource::<RestingLoads>()
        .init_resource::<WallRecord>()
        .add_observer(read_shell)
        .add_observer(set_off)
        .configure_sets(
            FixedPostUpdate,
            WallLawSet.after(crate::state::SimPhase::ProjectileMarch),
        )
        .add_systems(
            FixedPostUpdate,
            (read_contacts, apply_deposits, refit_systems())
                .chain()
                .in_set(WallLawSet)
                .run_if(not(resource_exists::<ClientReplica>)),
        )
        .add_systems(
            FixedPostUpdate,
            split_groups
                .after(crate::debris::launch::LaunchSet)
                .before(crate::state::SimPhase::Publish)
                .run_if(not(resource_exists::<ClientReplica>)),
        )
        .add_systems(
            Update,
            (derive_from_record, refit_systems())
                .chain()
                .in_set(WallReplicaSet)
                .run_if(resource_exists::<ClientReplica>),
        );
}

#[cfg(test)]
mod tests;
