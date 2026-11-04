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
//! * **Contacts** (the authority's, each physics step, [`read_contacts`]): every point the solver
//!   moved momentum through between a chunk group (a placed construction's breakable element or chunk,
//!   or a moving group's chunk) and another body ([`ContactTransfers`]) loads the chunk there (an
//!   intact element's: the chunk whose hull holds the point, [`contact_load`]) with its blow and
//!   press, each contact split at its body's rest speed ([`ContactTransfers::split`]), all of a
//!   step's points one deposit. On a placed construction the source exchanges its blow plastically
//!   ([`Origin::Body`]); on a moving group the solver already moved both bodies
//!   ([`Origin::Contact`]). A point on an unbreakable element loads nothing. A body asleep against a construction presses it with what its
//!   contact last held ([`RestingLoads`]): every run of the laws on that construction carries the
//!   press once, and it starts none. Awake contacts' presses likewise join every deposit in
//!   the physics step, once per point. No body is read by its kind.
//! * **The order.** Deposits commit in order. Independent owners evaluate together; source
//!   effects commit by entity, respecting [`owner_batches`]. Each owner runs removal and the
//!   push rule ([`fracture::strike`]), the structural check ([`fracture::structural`]), the weight
//!   check on what those changed ([`fracture::weigh`]), support ([`fracture::fall`]).
//! * **Holes** ([`Hole`], drawn only): a line cuts a channel in each chunk it crosses, the cylinder
//!   about its chord, open across each face it crosses ([`open_ends`]), and a blast a dish in each
//!   chunk its rays stop on ([`holes_of`]); a chunk still standing after the deposit
//!   keeps its hole in a per-chunk list ([`Live::holes`]), for the view to draw. No law, collider
//!   or mass reads a hole: they see the whole chunk, and [`Shape`] holds none. A hole holds no more
//!   than the volume its load crushed, the volume it took from the chunk's pool, so one the size
//!   of its chunk is a spent pool, recorded as dust. Only the pool decides dust: a chunk holding
//!   [`HOLES_PER_CHUNK`] holes keeps standing and draws no later one.
//! * **The record** ([`Record`]): wear, dust, holes and cracks, in authority order,
//!   with its tick. The authority appends; the network carries it ([`crate::net`]'s ledger); a
//!   replica rebuilds the same state from it ([`derive_from_record`]); the view presents it at the
//!   presentation clock ([`super::view`]).
//! * **Loose groups** fall on the authority as one dynamic body each ([`Piece`]): its chunks
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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use avian3d::collider_tree::{ColliderTrees, update_moved_collider_aabbs};
use avian3d::prelude::{
    AngularVelocity, Collider, ColliderAabb, ColliderOf, CollisionLayers, Forces, LayerMask,
    LinearVelocity, Position, ReadRigidBodyForces, RigidBody, Rotation, Sensor, SpatialQuery,
    SpatialQueryFilter, WriteRigidBodyForces,
};

use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::{ComputeTaskPool, TaskPool};
use serde::{Deserialize, Serialize};

use super::attachments::{Attachments, GroundAttachment};
use super::fracture::{
    self, Blast, Block, Bond, Chunk, Fate, GroupLoads, Joint, Load, Material, Origin, SectionCache,
    Side, State, Structure,
};
use super::schema::{self, ConstructionAsset};
use super::{ConstructionKind, Element};
use crate::ballistics::{
    ArmourMark, BallisticVolume, BurstField, DeliveredLine, PenetrationMarks, ShellMarched,
    SpallMarks,
};
use crate::contact_transfer::{ContactTransfer, ContactTransfers};
use crate::debris::launch::{
    BodyReturn, Launch, LaunchPlugin, PendingLaunch, launch_colliders, occupied_mass,
};
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

/// The law's view of one construction asset (module doc), at scale 1 in the construction frame.
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
    ground: Vec<(GroundAttachment, Vec<usize>)>,
    /// The law's joints, chunk pairs first in asset order, then the anchors in asset order.
    pub(crate) joints: Vec<Joint>,
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
        let mut ground = BTreeMap::<u16, (GroundAttachment, Vec<usize>)>::new();
        for (anchor, (hold, bond)) in asset.anchors.iter().zip(anchors) {
            if let Some(a) = law_of[anchor.chunk as usize] {
                let component = asset.elements
                    [usize::from(asset.chunks[anchor.chunk as usize].element)]
                .component;
                ground
                    .entry(component)
                    .or_insert_with(|| (hold, Vec::new()))
                    .1
                    .push(joints.len());
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
        Ok(Self {
            elements: elements
                .into_iter()
                .map(|range| range.map(|(a, b)| (a as u16, b as u16)))
                .collect(),
            chunks,
            element_of,
            materials,
            ground: ground.into_values().collect(),
            joints,
            blocks,
            element_blocks,
        })
    }

    /// The structure the laws read for an instance at uniform `scale`: lengths × s, areas × s²,
    /// volumes and masses × s³. Refuses a scale at which an element's or a law chunk's collider,
    /// as spawn and a swap build it ([`super::spawn`], [`chunk_collider`]), loses its finite
    /// positive mass or inertia.
    pub(crate) fn checked_structure(
        &self,
        asset: &ConstructionAsset,
        scale: f32,
    ) -> Result<Structure, String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(
                "scale must be finite and positive; author a representable instance scale".into(),
            );
        }
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
            if !solid_collider(points.into_iter().map(|p| p * scale).collect(), density) {
                return Err(format!(
                    "construction {:?}: {field} at scale {scale} cannot form a convex collider of finite positive mass and invertible inertia at its material density; author a representable instance scale or rebake a nondegenerate hull",
                    asset.id
                ));
            }
        }
        let (s2, s3) = (scale * scale, scale * scale * scale);
        let mut structure = Structure {
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
        };
        if scale != 1.0 {
            for (hold, indices) in &self.ground {
                let bond = hold.bond(scale).map_err(|error| format!(
                    "foundation attachment at joints {indices:?}: {error}; reduce the instance scale or rebake a representable footprint"
                ))?;
                for &index in indices {
                    structure.joints[index].bond = bond;
                }
            }
        }
        for (index, chunk) in structure.chunks.iter().enumerate() {
            if !(chunk.volume.is_finite()
                && chunk.volume > 0.0
                && chunk.mass.is_finite()
                && chunk.mass > 0.0
                && chunk.centroid.is_finite())
            {
                return Err(format!(
                    "chunks[{}] scaled volume, mass and centroid must remain finite and positive where required; author a representable scale",
                    self.chunks[index]
                ));
            }
        }
        for (index, joint) in structure.joints.iter().enumerate() {
            if !(joint.area.is_finite() && joint.area > 0.0 && joint.centroid.is_finite()) {
                return Err(format!(
                    "joints[{index}] scaled area and centroid must remain finite and positive where required; author a representable scale"
                ));
            }
        }
        Ok(structure)
    }

    /// Instrument reconstruction at an admitted scale (`track_sandbox::rubble` and law tests).
    /// Placed gameplay instances retain their checked structure in `ConstructionKind` at map load.
    pub(crate) fn structure(&self, asset: &ConstructionAsset, scale: f32) -> Structure {
        self.checked_structure(asset, scale)
            .expect("instrument data names an admitted construction scale")
    }

    /// Whether `event` names an element, law chunk or joint of this layout: what a record entry
    /// for this construction must name ([`Shape::apply`]).
    pub(crate) fn admits(&self, event: Event) -> bool {
        let within = |len: usize, i: u16| usize::from(i) < len;
        match event {
            Event::Wear(c) | Event::Dust(c) | Event::Hole(c, _) => within(self.chunks.len(), c),
            Event::Crack(j) => within(self.joints.len(), j),
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

/// The game's environment destruction causes, with stable [`Layout`] addresses. Wear disturbs
/// occupied matter; dust removes it; cracks remove supports; holes carry presentation geometry.
/// Peers derive element replacement and departure from these causes and the admitted graph.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    Wear(u16),
    Dust(u16),
    Crack(u16),
    Hole(u16, Hole),
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
            Event::Crack(j) => {
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

/// One owner's scaled construction, law state, drawing holes and reusable assessment storage.
/// Source addresses survive splits; ownership narrows the graph work.
#[derive(Clone)]
pub(crate) struct Live {
    pub(crate) structure: Arc<Structure>,
    pub(crate) state: State,
    pub(crate) swapped: Vec<bool>,
    pub(crate) sections: SectionCache,
    /// Presses already checked against the current topology: chunk, point (m), force (N).
    /// Decreases are rechecked too: removing a stabilizing force can expose another's demand.
    held_presses: Vec<(usize, Vec3, Vec3)>,
    /// Per law chunk: its holes, in record order (module doc).
    pub(crate) holes: Vec<Vec<Hole>>,
}

impl Live {
    fn new(kind: &ConstructionKind, scale: f32) -> Self {
        Self::from_shared(kind.structure(scale), kind.layout.elements.len())
    }

    /// Complete live state built synchronously from a structure and its authored element count.
    pub(crate) fn from_structure(structure: Structure, elements: usize) -> Self {
        Self::from_shared(Arc::new(structure), elements)
    }

    fn from_shared(structure: Arc<Structure>, elements: usize) -> Self {
        Self {
            state: State::new(&structure),
            swapped: vec![false; elements],
            sections: SectionCache::default(),
            held_presses: Vec::new(),
            holes: vec![Vec::new(); structure.chunks.len()],
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
            sections: SectionCache::default(),
            held_presses: Vec::new(),
            holes: self.holes.clone(),
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
    pub(crate) scale: f32,
    /// Stable source identity, shared by placed matter and every group it releases.
    pub(crate) construction: u16,
    pub(crate) live: Box<Live>,
    /// The body's origin in the scaled construction frame; zero for a placed construction.
    origin: Vec3,
}

impl Breakage {
    pub(crate) fn new(kind: Arc<ConstructionKind>, construction: u16, scale: f32) -> Self {
        assert!(
            scale.is_finite() && scale > 0.0,
            "a construction has positive finite uniform scale"
        );
        let live = Box::new(Live::new(&kind, scale));
        Self {
            kind,
            scale,
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
    fn physics_frame(&self, (position, rotation): (&Position, &Rotation)) -> Transform {
        self.frame(&Transform::from_translation(position.0).with_rotation(rotation.0))
    }

    pub(crate) fn shape(&self) -> Shape {
        self.live.shape()
    }
}

/// One law chunk's static collider entity, a child of its construction's root: its law index.
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
    /// A split retires its old carrier at this tick; the historical pose remains available
    /// until the replica's shared cursor has presented the replacement.
    pub(crate) retired: Option<u32>,
}

/// Geometry admitted once against the selected map before a peer piece reaches the view.
#[derive(Component, Debug, Clone)]
pub(crate) struct AdmittedPiece {
    pub(crate) construction: Entity,
    pub(crate) kind: Arc<ConstructionKind>,
    pub(crate) scale: f32,
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
            scale: breakage.scale,
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

/// AUTHORITY: each physics step's body contacts on the constructions as loads, read once a step
/// (module doc): every point on a breakable element or chunk queued, the step's as one deposit,
/// with the blow and press its transfer splits into; and every resting point's press as its
/// construction's ([`RestingLoads`]).
fn read_contacts(
    transfers: Res<ContactTransfers>,
    mut read: Local<u64>,
    colliders: Colliders,
    constructions: Query<((&Position, &Rotation), &Breakage, Option<&Piece>)>,
    mut queue: ResMut<Queue>,
    mut resting: ResMut<RestingLoads>,
) {
    if transfers.steps() == *read {
        return;
    }
    *read = transfers.steps();
    let mut inputs: Vec<_> = transfers
        .split()
        .map(|(point, blow, press, mass)| ContactInput {
            point: *point,
            blow,
            press,
            mass,
            step: transfers.dt(),
        })
        .collect();
    let awake = inputs.len();
    inputs.extend(transfers.resting().iter().map(|rest| ContactInput {
        point: rest.transfer,
        blow: Vec3::ZERO,
        press: rest.press(),
        // Resting presses carry no blow; no law reads their source mass.
        mass: rest.transfer.effective_mass,
        step: rest.dt,
    }));
    let converted = prepare_contacts(ComputeTaskPool::get(), &inputs, &colliders, &constructions);
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

#[derive(Clone, Copy)]
struct ContactInput {
    point: ContactTransfer,
    blow: Vec3,
    press: Vec3,
    mass: f32,
    step: f32,
}

/// Constraint mass splitting precedes dispatch. Each owner converts its contacts in source order;
/// indexed collection preserves the original awake/resting sequence under any worker assignment.
fn prepare_contacts(
    pool: &TaskPool,
    inputs: &[ContactInput],
    colliders: &Colliders,
    constructions: &Query<((&Position, &Rotation), &Breakage, Option<&Piece>)>,
) -> Vec<Option<(Entity, Load)>> {
    let mut owners: BTreeMap<Entity, Vec<(usize, &ContactInput)>> = BTreeMap::new();
    for (index, input) in inputs.iter().enumerate() {
        owners
            .entry(input.point.structure)
            .or_default()
            .push((index, input));
    }
    let mut converted: Vec<_> = pool
        .scope(|scope| {
            for inputs in owners.into_values() {
                scope.spawn(async move {
                    inputs
                        .into_iter()
                        .map(|(index, input)| {
                            (
                                index,
                                contact_load(
                                    colliders,
                                    constructions,
                                    &input.point,
                                    input.blow,
                                    input.press,
                                    input.mass,
                                    input.step,
                                ),
                            )
                        })
                        .collect::<Vec<_>>()
                });
            }
        })
        .into_iter()
        .flatten()
        .collect();
    converted.sort_unstable_by_key(|(index, _)| *index);
    converted.into_iter().map(|(_, load)| load).collect()
}

/// The load a body's contact point `transfer` puts on a construction, with `blow` (N·s) and `press`
/// (N) in world terms, its contact's source mass `mass` (kg, [`ContactTransfers::split`]) and the
/// physics step `step` (s) the solver moved them within:
/// on the chunk whose collider the point lies on, or for an intact element
/// the element's chunk whose hull holds the point (the hull nearest it, by [`Block::outside`],
/// ties to the lower index), turned into the construction's frame; with the construction's root. `None`
/// for bodies without construction state and elements that never break, which these laws do not
/// read. Reads the body's entity only to name the load's origin.
fn contact_load(
    colliders: &Colliders,
    constructions: &Query<((&Position, &Rotation), &Breakage, Option<&Piece>)>,
    transfer: &ContactTransfer,
    blow: Vec3,
    press: Vec3,
    mass: f32,
    step: f32,
) -> Option<(Entity, Load)> {
    let (transform, breakage, moving) = constructions.get(transfer.structure).ok()?;
    let (element, chunk, parent) = colliders.get(transfer.structure_collider).expect(
        "a construction collider the last physics step touched stands until the laws' next refit",
    );
    assert_eq!(
        parent.parent(),
        transfer.structure,
        "a construction's collider is its root's child"
    );
    let layout = &breakage.kind.layout;
    let frame = Frame::new(
        &breakage.physics_frame(transform),
        breakage.scale,
        breakage.kind.bounds(),
    );
    let local = frame.unscaled(transfer.point);
    let chunk = match (element, chunk) {
        (_, Some(&ChunkCollider(c))) => usize::from(c),
        (Some(&Element(e)), None) => nearest_chunk(layout, usize::from(e), local)?,
        (None, None) => unreachable!("Colliders admits elements and chunks only"),
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
pub(crate) struct Charge {
    pub(crate) at: Vec3,
    pub(crate) blast: Blast,
}

fn set_off(charge: On<Charge>, mut queue: ResMut<Queue>) {
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
        // test the deposits run in the construction frame (DERIVED: f32 rounding, ~1e-4 m at a
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

    /// A world point in the asset's own frame, at scale 1.
    fn unscaled(&self, p: Vec3) -> Vec3 {
        self.rotation.inverse() * (p - self.translation) / self.scale
    }

    /// A world direction in the construction frame.
    fn turn(&self, d: Vec3) -> Vec3 {
        self.rotation.inverse() * d
    }
}

/// The present blockers along a ray in the asset frame (unscaled): the first standing chunk of a
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

/// What a construction took of one deposit, in its frame (scaled): lines and their loads, field
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
    record: ResMut<'w, Record>,
    commands: Commands<'w, 's>,
    /// The moving bodies a blast's rays meet ([`moving_near`]) and push.
    spatial: SpatialQuery<'w, 's>,
    collider_of: Query<'w, 's, &'static ColliderOf>,
    aabbs: Query<'w, 's, &'static ColliderAabb>,
    bodies: Query<'w, 's, (Forces, &'static RigidBody)>,
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

/// The spin `run_laws` reads for the body at `root`, in its construction `frame` (rad/s): a
/// moving group's body's angular velocity; zero on a placed construction, whose groups come loose at
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
fn field_blows<'a>(loads: &'a [Load], state: &'a State) -> impl Iterator<Item = &'a Load> {
    loads.iter().filter(|load| {
        matches!(load.origin, Origin::Field)
            && load.blow != Vec3::ZERO
            && state.fate[load.chunk] != Fate::Dust
    })
}

/// The chunk of `element` whose hull lies nearest `at`; `None` for an element that never breaks.
fn nearest_chunk(layout: &Layout, element: usize, at: Vec3) -> Option<usize> {
    layout.chunks_of(element).min_by(|&a, &b| {
        layout.blocks[a]
            .outside(at)
            .total_cmp(&layout.blocks[b].outside(at))
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
            continue; // A volume without a construction element or chunk belongs to another law.
        };
        let root = parent.parent();
        let frame = &frames[&root];
        let (_, _, breakage, _) = p
            .constructions
            .get(root)
            .expect("a construction element or chunk is a child of its construction's root");
        let layout = &breakage.kind.layout;
        let (a, b) = (frame.unscaled(entry), frame.unscaled(end));
        let length = a.distance(b);
        let dir = (b - a).normalize_or_zero();
        let shares: Vec<(usize, f32, f32, f32)> = match (element, chunk) {
            (_, Some(&ChunkCollider(c))) => vec![(usize::from(c), 0.0, length, length)],
            (Some(&Element(e)), None) => {
                let chunks: Vec<_> = layout.chunks_of(usize::from(e)).collect();
                if chunks.is_empty() {
                    continue; // An element that never breaks has no chunk to wear.
                }
                let mut crossed = BTreeMap::<usize, (f32, f32, f32)>::new();
                for &(from, to) in &line.contacts {
                    let (from, to) = (frame.unscaled(from), frame.unscaled(to));
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
        let into = taken.entry(root).or_default();
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
            let (from, to) = ((a + dir * t0) * frame.scale, (a + dir * t1) * frame.scale);
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
            into.lines.push((c, from, to, part.crossing.energy));
            into.line_loads
                .extend(resolved_line_loads(c, &layout.materials[c], &part));
        }
    }
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
    returns: Vec<BodyReturn>,
    split: Option<PendingSplit>,
    departures: Vec<PreparedDeparture>,
}

/// Immutable inputs for one owner's law run; shared-body effects are staged in `OwnerEffects`.
struct OwnerInputs<'a> {
    pool: &'a TaskPool,
    frame: Frame,
    taken: &'a Taken,
    pressing: &'a [Load],
    resting: &'a [Load],
    spin: Vec3,
    tick: u32,
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
        frame,
        taken,
        pressing,
        resting,
        spin,
        tick,
    } = inputs;
    let kind = breakage.kind.clone();
    let construction = breakage.construction;
    let scale = breakage.scale;
    let live = breakage.live.as_mut();
    let blast = blast_loads(&kind, frame.scale, &taken.blast, live);
    let holes = holes_of(&kind, frame.scale, &taken.lines, &blast);
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
    let changes = run_laws(&kind.layout.element_of, live, &loads, holes, spin);
    // Moving matter already has a carrier; only surviving field loads push it.
    let fields = if moving.is_some() {
        field_blows(&loads, &live.state).copied().collect()
    } else {
        Vec::new()
    };
    let mut departures = Vec::new();
    let split = if let Some(owned) = moving {
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
                    let launch = launch(kind, &live.structure, frame.scale, &group, &loads);
                    prepare_group(Departure {
                        kind: kind.clone(),
                        construction,
                        root: construction_pose,
                        scale,
                        live: live.group(&group),
                        group,
                        launch,
                        tick,
                    })
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
        returns: changes.returns,
        split,
        departures,
    }
}

/// Keep source-entity effect order. An earlier owner's immediate plastic return may change a
/// later moving owner's spin, which its laws read; that owner starts after those returns commit.
/// Other owners evaluate together. Field loads have no source returns, and held presses no blow.
fn owner_batches(taken: BTreeMap<Entity, Taken>) -> impl Iterator<Item = BTreeMap<Entity, Taken>> {
    let mut pending = taken.into_iter().peekable();
    std::iter::from_fn(move || {
        let mut batch = BTreeMap::new();
        let mut returns_to = BTreeSet::new();
        while let Some(&(root, _)) = pending.peek() {
            if returns_to.contains(&root) {
                break;
            }
            let (root, taken) = pending.next().expect("peek found an owner");
            returns_to.extend(
                taken
                    .contacts
                    .iter()
                    .chain(&taken.line_loads)
                    .filter_map(|load| {
                        BodyReturn::of(load, Vec3::ZERO).map(|returned| returned.body)
                    }),
            );
            batch.insert(root, taken);
        }
        (!batch.is_empty()).then_some(batch)
    })
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
        .constructions
        .iter()
        .map(|(root, transform, breakage, _)| {
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
        // Geometry reads the state left by the preceding deposit. Each owner keeps ray order;
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
        for &(root, load) in &deposit.contacts {
            taken.entry(root).or_default().contacts.push(load);
        }
        let mut departures = Vec::new();
        for batch in owner_batches(taken) {
            let mut effects = bevy::utils::Parallel::<Vec<OwnerEffects>>::default();
            let bodies = &p.bodies;
            let resting = &p.resting.0;
            p.constructions
                .par_iter_many_unique_mut(batch.keys())
                .for_each(|(root, transform, mut breakage, piece)| {
                    let taken = &batch[&root];
                    let frame = frames[&root];
                    let construction = breakage.physics_frame(transform);
                    let effect = evaluate_owner(
                        root,
                        breakage.bypass_change_detection(),
                        piece.map(|piece| piece.chunks.as_slice()),
                        construction,
                        OwnerInputs {
                            pool: ComputeTaskPool::get(),
                            frame,
                            taken,
                            pressing: presses.get(&root).map_or(&[][..], Vec::as_slice),
                            resting: resting.get(&root).map_or(&[][..], Vec::as_slice),
                            spin: construction_spin(bodies, root, frame, piece.is_some()),
                            tick,
                        },
                    );
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
            // independent of worker completion. Each deposit commits before the next is cast.
            for effect in effects {
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
                give_back(&mut p.bodies, frame, &effect.returns);
                if let Some(split) = effect.split {
                    p.commands.entity(effect.root).insert(split);
                }
                departures.extend(effect.departures);
            }
        }
        for departure in departures {
            spawn_prepared(&mut p.commands, departure, true);
        }
    }
}

/// A blast's ray hits on one construction as its loads ([`Load::blast`]), each face's Law 2 read on
/// its chunk's hull at the instance's `scale`.
fn blast_loads(
    kind: &ConstructionKind,
    scale: f32,
    hits: &[(usize, fracture::RayHit)],
    live: &Live,
) -> Vec<Load> {
    if hits.is_empty() {
        return Vec::new(); // This deposit has no blast surface stops.
    }
    let contacts: Vec<Joint> = live.state.contacts(&live.structure).collect();
    let (loads, _) = Load::blast(hits, |c| fracture::Backing {
        block: &kind.layout.blocks[c],
        scale,
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
        let local = frame.unscaled(end);
        let slack = Vec3::splat(STOP_TOLERANCE_M / frame.scale);
        if local.cmplt(lo - slack).any() || local.cmpgt(hi + slack).any() {
            continue;
        }
        let turned = frame.turn(dir);
        let back = 2.0 * STOP_TOLERANCE_M / frame.scale;
        if let Some((Some(c), t)) =
            first_chunk(&kind.layout, &shape, placed, local - turned * back, turned)
            && (t - back).abs() * frame.scale <= STOP_TOLERANCE_M
        {
            field.push((c, local * frame.scale, energy, frame.turn(ray.blow)));
        }
    }
    let mut faces = BTreeMap::new();
    let blast = deposit.blast.map_or_else(Vec::new, |(origin, blast)| {
        let reach = blast.reach();
        let local = frame.unscaled(origin);
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
                let distance = t * frame.scale;
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
                    ) * frame.scale
                        * frame.scale;
                    fracture::BlastSurface {
                        target: Stop::Chunk(root, c),
                        at: (local + turned * t) * frame.scale,
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
/// the instance's `scale`. Each is sized by the volume it crushes, the volume its load takes from
/// the chunk's pool: a line's, its energy over the crush energy ([`Material::crush`]); a blast's,
/// its load's Law 1 volume ([`Load::crushed`]). A line's channel runs along its chord, its radius
/// the narrower of the cylinder about the chord and the ball that hold that volume (so a chord
/// grazing a corner never cuts a disc wider than the ball); the blast's dish in a chunk is a ball
/// about the chunk's load (its face's impulse-weighted centroid) whose half holds it. ASSUMED: a
/// channel's section is a circle and its width the energy's alone, not the round's diameter; a
/// dish's centre lies on the chunk's face, so half its ball is in the chunk.
fn holes_of(
    kind: &ConstructionKind,
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

/// What the laws changed in one construction: its record events in order, each loose group with the
/// loads on it (construction frame), the structural failures (the push check's, then the weight
/// check's), and the weight check's unassessed chunks and operation count ([`fracture::Weighed`]).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Changes {
    pub(crate) events: Vec<Event>,
    pub(crate) groups: Vec<(Vec<usize>, Vec<Load>)>,
    pub(crate) failures: Vec<fracture::Failure>,
    pub(crate) unassessed: Vec<usize>,
    pub(crate) weight_steps: usize,
    /// Body blows on chunks with no carrier in this deposit, returned whole in construction terms:
    /// dust has none, and a chunk an earlier deposit freed already launched as its own body.
    pub(crate) returns: Vec<BodyReturn>,
}

/// Run one deposit's `loads`, with the `holes` it cuts ([`holes_of`]), through the laws on `live`
/// (module doc) and record its wear, dust, kept holes and cracks (the push rule's, then the
/// structural and weight checks'), each ascending but the holes, in deposit order. Replacement
/// and departure are derived from these causes. A hole is kept only in a chunk standing
/// after the removal and holding fewer than [`HOLES_PER_CHUNK`], and no law reads one. A field
/// load on a chunk turned to dust went into crushing it and moves nothing; a body's blow there is
/// returned whole because dust has no carrier. A queued contact on a chunk an earlier deposit freed is returned whole too:
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
    mut holes: Vec<(usize, Hole)>,
    spin: Vec3,
) -> Changes {
    let struck = fracture::strike(&live.structure, &mut live.state, loads);
    holes.retain(|&(c, hole)| {
        let kept = live.state.standing(c) && live.holes[c].len() < HOLES_PER_CHUNK;
        if kept {
            live.holes[c].push(hole);
        }
        kept
    });
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
    events.extend(
        struck
            .cracked
            .iter()
            .chain(&structural)
            .map(|&j| Event::Crack(j as u16)),
    );
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

/// A law chunk's whole hull as a collider for an instance at `scale`, about `origin` (construction
/// frame, scaled): its occupied volume, so its mass and inertia at its material's density are the
/// construction's. Admission certified the hull as a solid convex collider at every placed scale
/// ([`Layout::checked_structure`]).
pub(crate) fn chunk_collider(
    kind: &ConstructionKind,
    c: usize,
    scale: f32,
    origin: Vec3,
) -> Collider {
    Collider::convex_hull(
        kind.layout.blocks[c]
            .points
            .iter()
            .map(|p| *p * scale - origin)
            .collect(),
    )
    .expect("a load-certified hull at a positive scale stays a solid convex hull")
}

/// How far a freed chunk's contact hull stands inside its whole hull, m (Resolution). A chunk freed
/// in place touches its neighbours, and at zero separation the solver's contact normal is rounding
/// noise. MEASURED on the shipped house, 11 sampled releases: with no gap 3 took kicks of up to
/// 5.5 cm and 0.22 J/kg from nothing; at 1 mm none gained energy, 1 km from the origin as well.
const LOOSE_GAP_M: f32 = 1e-3;

/// Resolution: preserve a solid, numerically resolved hull when the requested inset would
/// consume a small fragment. This limits contraction, not the fragment's occupied geometry.
const MIN_INSET_SCALE: f32 = 0.2;

/// A certified hull's points contracted about their mean by `gap` m at `scale`. The nearest face
/// moves by the gap unless MIN_INSET_SCALE limits contraction; farther faces move proportionally.
/// Contact separation and visual seams supply independent gaps to this one geometric operation.
pub(crate) fn inset_point(block: &Block, scale: f32, gap: f32) -> impl Fn(Vec3) -> Vec3 {
    assert!(
        block.points.len() >= 4,
        "a certified solid hull has at least four corners"
    );
    assert!(
        scale.is_finite() && scale > 0.0 && gap.is_finite() && gap >= 0.0,
        "a certified hull's scale is positive and its requested inset is nonnegative"
    );
    let middle = block.points.iter().sum::<Vec3>() / block.points.len() as f32;
    let radius = block.inradius() * scale;
    assert!(
        radius.is_finite() && radius > 0.0,
        "a certified convex hull's corner mean is strictly inside its bounding planes"
    );
    let shrink = (1.0 - gap / radius).max(MIN_INSET_SCALE);
    move |point| middle + (point - middle) * shrink
}

/// The hull a freed chunk meets contacts with, inside its whole occupied hull. The nominal
/// nearest-face gap is LOOSE_GAP_M, reduced by the small-hull cap in inset_point. Mass, inertia
/// and ballistics read the whole hull instead.
pub(crate) fn loose_collider(block: &Block, scale: f32, origin: Vec3) -> Collider {
    let inset = inset_point(block, scale, LOOSE_GAP_M);
    Collider::convex_hull(
        block
            .points
            .iter()
            .map(|p| inset(*p) * scale - origin)
            .collect(),
    )
    .expect("a certified hull, scaled and contracted about its mean, stays solid")
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
    scale: f32,
    live: Live,
    tick: u32,
    kind: Arc<ConstructionKind>,
    construction: u16,
    root: Transform,
    group: Vec<usize>,
    launch: Launch,
}

type ContactChunk = (
    Transform,
    Collider,
    (
        avian3d::prelude::Mass,
        avian3d::prelude::AngularInertia,
        avian3d::prelude::CenterOfMass,
    ),
    CollisionLayers,
    avian3d::prelude::Friction,
    ChunkCollider,
);
type OccupiedChunk = (
    Transform,
    Collider,
    Sensor,
    CollisionLayers,
    BallisticVolume,
    ChunkCollider,
);

struct PreparedDeparture {
    departure: Departure,
    chunks: Vec<(ContactChunk, OccupiedChunk)>,
}

/// Prepare occupied mass and inset contact geometry while the group is independently owned.
/// Ordered emission only enqueues these complete bundles.
fn prepare_group(mut departure: Departure) -> PreparedDeparture {
    let kind = &departure.kind;
    let origin = departure.launch.centroid;
    let chunks = std::mem::take(&mut departure.launch.colliders)
        .into_iter()
        .map(|(c, hull)| {
            let contact = (
                Transform::IDENTITY,
                loose_collider(&kind.layout.blocks[c], departure.scale, origin),
                occupied_mass(&hull, kind.layout.materials[c].density),
                piece_layers(),
                kind.chunk_friction(c),
                ChunkCollider(c as u16),
            );
            // The game's damage model reads occupied geometry; the sensor adds no solver mass.
            let occupied = (
                Transform::IDENTITY,
                hull,
                Sensor,
                CollisionLayers::new([Layer::Armor], LayerMask::NONE),
                chunk_volume(kind, c),
                ChunkCollider(c as u16),
            );
            (contact, occupied)
        })
        .collect();
    PreparedDeparture { departure, chunks }
}

/// One loose group's colliders about its chunks' centre of mass (construction frame) and departure:
/// each field blow launches whole, each body blow only its plastic share against the group's free
/// effective mass at the load point. The rest returns to its source at that point. The wall
/// design ("Bodies in contact") owns the momentum and energy derivation.
pub(crate) fn launch(
    kind: &ConstructionKind,
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
        .map(|&c| (c, chunk_collider(kind, c, scale, centroid)))
        .collect();
    launch_colliders(centroid, colliders, |c| chunks[c].material.density, loads)
}

/// Return a departing chunk's excess solver impulse immediately, at the original world point.
/// A structure that held supplies no return; presses remain with the standing structure.
fn give_back(bodies: &mut Query<(Forces, &RigidBody)>, frame: Frame, returns: &[BodyReturn]) {
    for returned in returns {
        let (mut body, kind) = bodies
            .get_mut(returned.body)
            .expect("a contact's dynamic source survives the same tick's construction laws");
        assert!(kind.is_dynamic(), "a contact's source is a dynamic body");
        body.apply_linear_impulse_at_point(
            frame.rotation * returned.impulse,
            frame.translation + frame.rotation * returned.at,
        );
    }
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
    let scale = root.scale.x;
    let live = Live::new(kind, scale).group(group);
    spawn_group(
        commands,
        Departure {
            kind: kind.clone(),
            construction,
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

#[cfg(test)]
fn spawn_group(commands: &mut Commands, departure: Departure, finalize: bool) {
    spawn_prepared(commands, prepare_group(departure), finalize);
}

/// Commit a group from its synchronous, independently prepared construction state.
fn spawn_prepared(commands: &mut Commands, prepared: PreparedDeparture, finalize: bool) {
    let PreparedDeparture { departure, chunks } = prepared;
    let Departure {
        kind,
        construction,
        root,
        group,
        launch,
        live,
        scale,
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
    let mut piece = commands.spawn((
        Name::new("Piece"),
        Piece {
            construction,
            chunks: group.iter().map(|&c| c as u16).collect(),
            born: tick,
            retired: None,
        },
        Breakage {
            kind,
            scale,
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
        LinearVelocity(root.rotation * *velocity),
        AngularVelocity(root.rotation * *spin),
    ));
    if finalize {
        piece.insert(PendingLaunch::new(root, launch));
    }
    piece.with_children(|parent| {
        for (contact, occupied) in chunks {
            parent.spawn(contact);
            parent.spawn(occupied);
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

/// Parent velocities in the construction frame; COM offset from its body origin, metres.
#[derive(Clone, Copy)]
struct SplitMotion {
    velocity: Vec3,
    spin: Vec3,
    centre: Vec3,
}

impl PendingSplit {
    fn prepare(
        &self,
        pool: &TaskPool,
        breakage: &Breakage,
        construction: u16,
        root: Transform,
        motion: SplitMotion,
    ) -> Vec<PreparedDeparture> {
        pool.scope(|scope| {
            for (group, live) in &self.children {
                scope.spawn(async move {
                    let mut launch =
                        launch(&breakage.kind, &live.structure, breakage.scale, group, &[]);
                    // DERIVED rigid velocity field: every new COM gets v + omega cross r; every
                    // child retains omega. Occupied hulls partition the parent's mass and inertia;
                    // contact insets do not enter this calculation.
                    launch.velocity = motion.velocity
                        + motion
                            .spin
                            .cross(launch.centre - breakage.origin - motion.centre);
                    launch.spin = motion.spin;
                    prepare_group(Departure {
                        kind: breakage.kind.clone(),
                        construction,
                        root,
                        group: group.clone(),
                        launch,
                        scale: breakage.scale,
                        live: live.clone(),
                        tick: self.tick,
                    })
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
    )>,
    mut commands: Commands,
) {
    let mut effects =
        bevy::utils::Parallel::<Vec<(Entity, u32, Vec<PreparedDeparture>)>>::default();
    parents.par_iter().for_each(
        |(entity, piece, body, breakage, velocity, spin, centre, split)| {
            let root = breakage.physics_frame(body);
            let inverse = root.rotation.inverse();
            let children = split.prepare(
                ComputeTaskPool::get(),
                breakage,
                piece.construction,
                root,
                SplitMotion {
                    velocity: inverse * velocity.0,
                    spin: inverse * spin.0,
                    centre: centre.0,
                },
            );
            effects.scope(|effects| effects.push((entity, split.tick, children)));
        },
    );
    let mut effects: Vec<_> = effects.drain().collect();
    effects.sort_unstable_by_key(|(entity, ..)| *entity);
    for (entity, tick, children) in effects {
        for child in children {
            spawn_prepared(&mut commands, child, false);
        }
        commands.trigger(RetirePiece { entity, tick });
        commands.entity(entity).despawn();
    }
}

/// A loose chunk's contact filter: debris meets ground, constructions, hulls and other pieces,
/// never a tank's armour volumes. Its separate whole-hull sensor supplies ballistic geometry.
pub(crate) fn piece_layers() -> CollisionLayers {
    CollisionLayers::new(
        [Layer::Debris],
        [
            Layer::Terrain,
            Layer::Vehicle,
            Layer::Debris,
            Layer::TrackSide,
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
    Chunk(C),
}

impl<E: Bundle, C: Bundle> ColliderChange<E, C> {
    fn apply(self, commands: &mut Commands) {
        match self {
            Self::Despawn(entity) => {
                commands.entity(entity).despawn();
            }
            Self::Element(bundle) => {
                commands.spawn(bundle);
            }
            Self::Chunk(bundle) => {
                commands.spawn(bundle);
            }
        }
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
                    super::element_bundle(kind, e, breakage.scale),
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
                effects.push(ColliderChange::Chunk((
                    ChildOf(root),
                    Transform::IDENTITY,
                    chunk_collider(kind, c, breakage.scale, Vec3::ZERO),
                    super::element_layers(),
                    kind.chunk_friction(c),
                    chunk_volume(kind, c),
                    ChunkCollider(c as u16),
                    pose,
                )));
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
) -> Vec<Vec<ColliderChange<impl Bundle, impl Bundle>>> {
    pool.scope(|scope| {
        for (root, transform, breakage, children) in constructions {
            scope.spawn(async move {
                prepare_reconcile(root, transform, breakage, children, elements, chunks)
            });
        }
    })
}

/// Every peer: prepare changed owners together, then commit colliders before the same-update refit.
fn reconcile(
    constructions: Query<
        (Entity, &Transform, &Breakage, Option<&Children>),
        (Changed<Breakage>, Without<Piece>),
    >,
    elements: Query<&Element>,
    chunks: Query<&ChunkCollider>,
    mut commands: Commands,
) {
    let effects =
        prepare_reconciliations(ComputeTaskPool::get(), &constructions, &elements, &chunks);
    // Direct scoped submissions collect in query order. All hulls are ready before this flush,
    // and refit_systems exposes every new collider to spatial queries in the same update.
    for effect in effects.into_iter().flatten() {
        effect.apply(&mut commands);
    }
}

/// Avian's refit of moved colliders in a frame a construction changed: the spawned chunks' bounds
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
pub(crate) struct LawSet;

/// A replica's set, in `Update`: the record's new entries into the constructions, then the colliders.
/// Whatever fills the record runs before it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ReplicaSet;

pub(crate) fn sim_plugin(app: &mut App) {
    if !app.is_plugin_added::<LaunchPlugin>() {
        app.add_plugins(LaunchPlugin);
    }
    app.init_resource::<Queue>()
        .init_resource::<RestingLoads>()
        .init_resource::<Record>()
        .add_observer(read_shell)
        .add_observer(set_off)
        .configure_sets(
            FixedPostUpdate,
            LawSet.after(crate::state::SimPhase::ProjectileMarch),
        )
        .add_systems(
            FixedPostUpdate,
            (read_contacts, apply_deposits, refit_systems())
                .chain()
                .in_set(LawSet)
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
                .in_set(ReplicaSet)
                .run_if(resource_exists::<ClientReplica>),
        );
}

#[cfg(test)]
mod tests;
