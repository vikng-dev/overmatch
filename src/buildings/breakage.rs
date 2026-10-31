//! A building breaking in a match, about the game's environment destruction (game state only):
//! the authority runs the fracture laws ([`super::fracture`]) on what the rounds deposit, records
//! every change, and every peer derives the wall's colliders from that record.
//!
//! * **The law's view of a building** ([`Layout`], once per asset): the chunks of the elements
//!   whose substance states a seam ([`Masonry::of`]), numbered in asset order, and the joints and
//!   anchors among them. A chunk and a joint keep that number on the wire.
//! * **First touch swaps an element.** An element stays one collider until a deposit, a crack, a
//!   fall or dust touches one of its chunks; from then on each of its standing chunks is a static
//!   collider of its substance ([`BuildingChunk`]) and the element's own collider is gone. A
//!   building's [`Breakage`] holds its state, built from data on the first touch.
//! * **Deposits** (the authority's, [`ShellMarched`]): a round's line through an element is shared
//!   among the chunks it crosses by chord; through a chunk it is that chunk's. Every field ray (a
//!   spall cone's, a burst's fragments) that stops on a chunk deposits its energy there. A burst
//!   feeds the blast field ([`fracture::blast_rays`]) from its burst point, its energy the core's
//!   chemical energy less the casing's share. A line pushes with all the momentum the round lost
//!   in it ([`Projectile::lost_momentum`]: the share the crushed brick carries away is open); a
//!   blast pushes each face its rays stop on with its reflected impulse ([`Blast::push`]). The
//!   laws then run in order: removal and the push rule ([`fracture::strike`]), the structural
//!   check ([`fracture::structural`]), the weight check on what those changed
//!   ([`fracture::weigh`]), support ([`fracture::fall`]).
//! * **Holes** ([`Hole`], drawn only): a line cuts a channel in each chunk it crosses and a blast a
//!   dish in each chunk its rays stop on ([`holes_of`]); a chunk still standing after the deposit
//!   keeps its hole in a per-chunk list ([`Live::holes`]), for the view to draw. No law, collider
//!   or mass reads a hole: they see the whole chunk, and [`Shape`] holds none. A hole holds no more
//!   than the volume its energy crushed (`energy / crush`), so one the size of its chunk is a spent
//!   pool, recorded as dust. A deposit that would leave a chunk holding more than
//!   [`HOLES_PER_CHUNK`] crumbles it to dust first, before the laws run.
//! * **The record** ([`WallRecord`]): every swap, dust, hole, crack and fall, in authority order,
//!   with its tick. The authority appends; the network carries it ([`crate::net`]'s ledger); a
//!   replica rebuilds the same state from it ([`derive_from_record`]); the view presents it at the
//!   presentation clock ([`super::view`]).
//! * **Loose groups** fall on the authority as one dynamic body each ([`WallPiece`]): its chunks
//!   brick in ballistics on [`Layer::Armor`], colliding as [`Layer::Debris`], settled by the shared
//!   rule ([`crate::debris`]). It leaves with the momentum pushed into it, its spin the angular
//!   momentum over its own inertia tensor; gravity, contact and friction decide whether a group
//!   a cracked section freed rocks back or topples.
//!
//! THE BODY IN THE PHYSICS WORLD. A state change reaches avian's world in the frame it is made
//! ([`reconcile`], then avian's refit of moved colliders), so the next tick's march reads it.

use std::collections::BTreeMap;
use std::sync::Arc;

use avian3d::collider_tree::{ColliderTrees, update_moved_collider_aabbs};
use avian3d::prelude::{
    AngularVelocity, Collider, ColliderDensity, CollisionLayers, LinearVelocity, Position, Rotation,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::fracture::{
    self, Blast, Block, Chunk, Fate, Joint, Loading, Masonry, Push, SectionCache, Side, Structure,
    WallState,
};
use super::schema::BuildingAsset;
use super::{BuildingElement, BuildingKind};
use crate::ballistics::{
    ArmourMark, BallisticVolume, CrossingEnd, Envelope, PenetrationMarks, Projectile, ShellMarched,
    SpallMarks,
};
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
    /// substance that never breaks). Refuses an asset whose law chunks or joints overrun the
    /// wire's 16-bit numbers.
    pub(crate) fn of(asset: &BuildingAsset, masonry: &[Option<Masonry>]) -> Result<Self, String> {
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
                joints.push(Joint {
                    a,
                    b: Side::Chunk(b),
                    area: joint.area_m2,
                    centroid: Vec3::from_array(joint.centroid),
                    normal: Vec3::from_array(joint.normal),
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
        let blocks = chunks
            .iter()
            .map(|&c| {
                Block::hull(
                    asset.chunks[c as usize]
                        .hull
                        .iter()
                        .map(|p| Vec3::from_array(*p))
                        .collect(),
                )
            })
            .collect();
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

    /// The law chunks of `element`, empty for one that never breaks.
    pub(crate) fn chunks_of(&self, element: usize) -> std::ops::Range<usize> {
        self.elements[element].map_or(0..0, |(a, b)| usize::from(a)..usize::from(b))
    }
}

/// One hole in one chunk (module doc): the chunk's matter within `radius` of the segment
/// `from`–`to`. A round's channel runs along its line; a blast's dish has `from == to`, a ball
/// about its centre. In millimetres of the asset frame at scale 1, about the baked chunk's
/// centroid ([`super::schema::Chunk::centroid`]), so it is the same hole at every instance scale;
/// each coordinate saturates at the `i16` range, ±32.767 m from the centroid, which every point of
/// a chunk under 65 m across stays within.
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

/// Every change to every building so far, in authority order (module doc). `generation` moves
/// when the record is replaced (a replica's next link): whoever derives from it starts over.
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

    /// Apply one recorded change. An index past the layout is refused (logged): the map digest
    /// the handshake compares makes it a broken peer, never a state to clamp into. A hole leaves
    /// the shape as it is (module doc).
    pub(crate) fn apply(&mut self, event: WallEvent) -> bool {
        let slot = |len: usize, i: u16| (usize::from(i) < len).then_some(usize::from(i));
        match event {
            WallEvent::Swap(e) => slot(self.swapped.len(), e).map(|e| self.swapped[e] = true),
            WallEvent::Dust(c) => slot(self.fate.len(), c).map(|c| self.fate[c] = Fate::Dust),
            WallEvent::Fall(c) => slot(self.fate.len(), c).map(|c| self.fate[c] = Fate::Fallen),
            WallEvent::Crack(j) => slot(self.cracked.len(), j).map(|j| self.cracked[j] = true),
            WallEvent::Hole(c, _) => slot(self.fate.len(), c).map(|_| ()),
        }
        .is_some()
    }

    /// Whether law chunk `c` stands as a collider of its own: its element swapped, it standing.
    pub(crate) fn chunk_present(&self, layout: &Layout, c: usize) -> bool {
        self.swapped[usize::from(layout.element_of[c])] && self.fate[c] == Fate::Standing
    }
}

/// A building's live state: the structure the laws read at its instance scale, the laws' state,
/// its shape, the structural check's sections of the structure, and each chunk's holes. The
/// shape's fates and cracks are the state's, mirrored as the laws move them.
pub(crate) struct Live {
    pub(crate) structure: Structure,
    pub(crate) state: WallState,
    pub(crate) swapped: Vec<bool>,
    pub(crate) sections: SectionCache,
    /// Per law chunk: its holes, in record order (module doc).
    pub(crate) holes: Vec<Vec<Hole>>,
}

impl Live {
    fn new(kind: &BuildingKind, scale: f32) -> Self {
        let structure = kind.layout.structure(&kind.asset, scale);
        Self {
            state: WallState::new(&structure),
            swapped: vec![false; kind.layout.elements.len()],
            sections: SectionCache::default(),
            holes: vec![Vec::new(); structure.chunks.len()],
            structure,
        }
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
pub(crate) struct Breakage {
    pub(crate) kind: Arc<BuildingKind>,
    pub(crate) scale: f32,
    pub(crate) live: Option<Box<Live>>,
}

impl Breakage {
    pub(crate) fn new(kind: Arc<BuildingKind>, scale: f32) -> Self {
        Self {
            kind,
            scale,
            live: None,
        }
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
/// replicas on the body's pose carrier, which never changes it.
#[derive(Component, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct WallPiece {
    pub(crate) building: u16,
    pub(crate) chunks: Vec<u16>,
}

/// What one shell's march step deposited, in world terms, waiting for [`apply_deposits`].
#[derive(Debug, Default)]
struct RawDeposit {
    /// Each crossing: its lines `(volume, entry, end, energy J)`, its unit travel, and the momentum
    /// its strength term took (N·s).
    crossings: Vec<(Vec<(Entity, Vec3, Vec3, f32)>, Vec3, f32)>,
    /// Each field ray: where it was thrown from, where it stopped, and the energy it carried (J).
    rays: Vec<(Vec3, Vec3, f32)>,
    /// A burst's blast: where, and its energy and charge.
    blast: Option<(Vec3, Blast)>,
}

/// The deposits the march raised this tick.
#[derive(Resource, Default)]
struct WallQueue(Vec<RawDeposit>);

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
/// at the command flush after the march, before a spent shell's despawn. A replica's cosmetic
/// march raises the event too, and is ignored: its walls change only by the record.
fn read_shell(
    marched: On<ShellMarched>,
    replica: Option<Res<ClientReplica>>,
    shells: Query<(&PenetrationMarks, Option<&SpallMarks>, &Projectile)>,
    mut queue: ResMut<WallQueue>,
) {
    if replica.is_some() {
        return;
    }
    let Ok((marks, spall, projectile)) = shells.get(marched.shell) else {
        return;
    };
    let mut deposit = RawDeposit::default();
    for mark in marks.log.get(marched.marks_from..).unwrap_or_default() {
        match mark {
            ArmourMark::Crossing {
                volumes,
                incoming,
                speed_in,
                end,
                ..
            } => {
                let speed_out = match end {
                    CrossingEnd::Exit { speed, .. } => *speed,
                    CrossingEnd::Rest { .. } => 0.0,
                };
                deposit.crossings.push((
                    volumes
                        .iter()
                        .map(|v| (v.volume, v.entry, v.end, v.energy))
                        .collect(),
                    incoming.normalize_or_zero(),
                    projectile.lost_momentum(*speed_in, speed_out),
                ));
            }
            ArmourMark::Burst { at, .. } => {
                if let Some(burst) = marched.burst {
                    deposit.blast = Some((*at, Blast::of(burst.blast)));
                }
            }
            _ => {}
        }
    }
    if let Some(spall) = spall {
        let cones = spall.cones.get(marched.cones_from..).unwrap_or_default();
        let thrown = cones.iter().map(|cone| (cone.origin, &cone.rays)).chain(
            spall
                .burst
                .as_ref()
                .filter(|_| marched.burst.is_some())
                .map(|field| (field.origin, &field.rays)),
        );
        for (origin, rays) in thrown {
            let share = 1.0 / rays.len().max(1) as f32;
            deposit.rays.extend(
                rays.iter()
                    .map(|ray| (origin, ray.end, ray.strength * share)),
            );
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
/// deposit. Returns the law chunk and the distance.
fn first_chunk(
    layout: &Layout,
    shape: &Shape,
    origin: Vec3,
    dir: Vec3,
) -> Option<(Option<usize>, f32)> {
    let mut best: Option<(Option<usize>, f32)> = None;
    for (e, block) in layout.element_blocks.iter().enumerate() {
        if block.span(origin, dir).is_none() {
            continue;
        }
        let hit = match layout.elements[e] {
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

/// What a building took this tick, in its frame (scaled): lines, field stops, blast hits and
/// pushes.
#[derive(Default)]
struct Taken {
    lines: Vec<(usize, Vec3, Vec3, f32)>,
    field: Vec<(usize, Vec3, f32)>,
    blast: Vec<fracture::RayHit>,
    pushes: Vec<Push>,
}

#[derive(bevy::ecs::system::SystemParam)]
struct DepositParam<'w, 's> {
    queue: ResMut<'w, WallQueue>,
    volumes: Query<
        'w,
        's,
        (
            Option<&'static BuildingElement>,
            Option<&'static BuildingChunk>,
            &'static ChildOf,
        ),
    >,
    buildings: Query<
        'w,
        's,
        (
            Entity,
            &'static MapInstance,
            &'static Transform,
            &'static mut Breakage,
        ),
    >,
    clock: Option<Res<'w, ShotClock>>,
    record: ResMut<'w, WallRecord>,
    commands: Commands<'w, 's>,
}

/// AUTHORITY: run the tick's deposits through the laws (module doc), record every change and spawn
/// a body per loose group.
fn apply_deposits(mut p: DepositParam) {
    if p.queue.0.is_empty() {
        return;
    }
    let tick = p.clock.as_deref().map_or(0, |clock| clock.0);
    let frames: BTreeMap<Entity, Frame> = p
        .buildings
        .iter()
        .map(|(root, _, transform, breakage)| {
            (
                root,
                Frame::new(transform, breakage.scale, breakage.kind.bounds()),
            )
        })
        .collect();
    for deposit in std::mem::take(&mut p.queue.0) {
        let mut taken: BTreeMap<Entity, Taken> = BTreeMap::new();
        for (lines, along, impulse) in &deposit.crossings {
            let total: f32 = lines.iter().map(|line| line.3).sum();
            for &(volume, entry, end, energy) in lines {
                let Ok((element, chunk, parent)) = p.volumes.get(volume) else {
                    continue;
                };
                let root = parent.parent();
                let (Some(frame), Ok((_, _, _, breakage))) =
                    (frames.get(&root), p.buildings.get(root))
                else {
                    continue;
                };
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
                    (None, None) => continue,
                };
                let chord: f32 = shares.iter().map(|(_, t0, t1)| t1 - t0).sum();
                if chord <= 0.0 {
                    continue;
                }
                let into = taken.entry(root).or_default();
                let turned = frame.turn(*along);
                for (c, t0, t1) in shares {
                    let part = energy * (t1 - t0) / chord;
                    let (from, to) = ((a + dir * t0) * frame.scale, (a + dir * t1) * frame.scale);
                    into.lines.push((c, from, to, part));
                    if total > 0.0 {
                        into.pushes.push(Push {
                            chunk: c,
                            at: (from + to) / 2.0,
                            impulse: turned * (impulse * part / total),
                        });
                    }
                }
            }
        }
        // Each building's shape as the deposit found it, read once per deposit.
        let mut shapes: BTreeMap<Entity, Shape> = BTreeMap::new();
        for &(origin, end, energy) in &deposit.rays {
            let dir = (end - origin).normalize_or_zero();
            if dir == Vec3::ZERO || energy <= 0.0 {
                continue;
            }
            for (root, frame) in &frames {
                if !frame.may_hold(end, STOP_TOLERANCE_M) {
                    continue;
                }
                let Ok((_, _, _, breakage)) = p.buildings.get(*root) else {
                    continue;
                };
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
                    first_chunk(&kind.layout, shape, local - turned * back, turned)
                    && (t - back).abs() * frame.scale <= STOP_TOLERANCE_M
                {
                    taken
                        .entry(*root)
                        .or_default()
                        .field
                        .push((c, local * frame.scale, energy));
                }
            }
        }
        if let Some((origin, blast)) = deposit.blast {
            let directions = fracture::blast_directions();
            for (root, frame) in &frames {
                let Ok((_, _, _, breakage)) = p.buildings.get(*root) else {
                    continue;
                };
                let kind = &breakage.kind;
                let (lo, hi) = kind.bounds();
                let local = frame.unscaled(origin);
                let mut bounds = None;
                let mut hits = Vec::new();
                for &(dir, share) in directions {
                    if !frame.may_meet(origin, dir) {
                        continue;
                    }
                    let turned = frame.turn(dir);
                    let bounds = bounds.get_or_insert_with(|| Block::cuboid(lo, hi));
                    if bounds.span(local, turned).is_none() {
                        continue;
                    }
                    let shape = shapes.entry(*root).or_insert_with(|| breakage.shape());
                    if let Some((Some(c), t)) = first_chunk(&kind.layout, shape, local, turned) {
                        let normal = kind.layout.blocks[c]
                            .entry_face(local, turned)
                            .map_or(-turned, |(_, normal)| normal);
                        hits.push(fracture::RayHit {
                            chunk: c,
                            at: (local + turned * t) * frame.scale,
                            energy: blast.energy * share,
                            impulse: blast.push(share, t * frame.scale, turned, normal),
                        });
                    }
                }
                if !hits.is_empty() {
                    taken.entry(*root).or_default().blast.extend(hits);
                }
            }
        }
        for (root, taken) in taken {
            let Ok((_, identity, transform, mut breakage)) = p.buildings.get_mut(root) else {
                continue;
            };
            let frame = frames[&root];
            let kind = breakage.kind.clone();
            let (blast, blast_pushes) = Loading::blast(&taken.blast);
            let holes = holes_of(&kind, frame.scale, &taken.lines, &blast);
            let loading = Loading::lines(&taken.lines)
                .and(Loading::field(&taken.field))
                .and(blast);
            let live = breakage.live();
            let changes = run_laws(
                &kind.layout,
                live,
                &loading,
                taken.pushes.into_iter().chain(blast_pushes).collect(),
                holes,
            );
            let building = identity.ordinal;
            let counts = [taken.lines.len(), taken.field.len(), taken.blast.len()];
            log_changes(&identity.id, &loading, counts, &changes);
            p.record
                .entries
                .extend(changes.events.iter().map(|&event| WallEntry {
                    building,
                    event,
                    tick,
                }));
            for (group, pushes) in changes.groups {
                spawn_piece(
                    &mut p.commands,
                    &kind,
                    &live.structure,
                    (building, frame.scale, *transform),
                    &group,
                    &pushes,
                );
            }
        }
    }
}

/// Log what one deposit did to a building (debug): `id` the building's, `counts` its deposit's
/// lines, field stops and blast rays.
fn log_changes(id: &str, loading: &Loading, counts: [usize; 3], changes: &Changes) {
    let count = |f: fn(&WallEvent) -> bool| changes.events.iter().filter(|e| f(e)).count();
    debug!(
        "buildings: {id} took {:.0} J in {} chunks ({} lines, {} field stops, {} blast rays): {} \
         swapped, {} dust, {} holes, {} cracks, {} structural sections {:?} (load, demand, \
         capacity: N s m for a push, Pa for weight), weight check {} steps with {} touched chunks \
         unassessed, {} chunks fell in {} groups",
        loading.energies.iter().map(|(_, e)| e).sum::<f32>(),
        loading.energies.len(),
        counts[0],
        counts[1],
        counts[2],
        count(|e| matches!(e, WallEvent::Swap(_))),
        count(|e| matches!(e, WallEvent::Dust(_))),
        count(|e| matches!(e, WallEvent::Hole(..))),
        count(|e| matches!(e, WallEvent::Crack(_))),
        changes.failures.len(),
        changes
            .failures
            .iter()
            .map(|f| (f.load, f.demand, f.capacity))
            .collect::<Vec<_>>(),
        changes.weight_steps,
        changes.unassessed.len(),
        count(|e| matches!(e, WallEvent::Fall(_))),
        changes.groups.len(),
    );
}

/// The holes one deposit cuts (module doc), each with its law chunk, in the baked chunk's frame
/// ([`Hole`]); `lines` and `blast` in the building frame at the instance's `scale`. Each is sized
/// by the volume its energy crushes, `energy / crush` ([`Masonry::crush`]): a line's channel runs
/// along its chord, its radius the narrower of the cylinder about the chord and the ball that hold
/// that volume (so a chord grazing a corner never cuts a disc wider than the ball); the blast's
/// dish in a chunk is a ball about the chunk's source (its rays' energy-weighted mean stop) whose
/// half holds it. ASSUMED: a channel's section is a circle and its width the energy's alone, not the
/// round's diameter; a dish's centre lies on the chunk's face, so half its ball is in the chunk.
fn holes_of(
    kind: &BuildingKind,
    scale: f32,
    lines: &[(usize, Vec3, Vec3, f32)],
    blast: &Loading,
) -> Vec<(usize, Hole)> {
    use std::f32::consts::PI;
    let layout = &kind.layout;
    let centroid =
        |c: usize| Vec3::from_array(kind.asset.chunks[layout.chunks[c] as usize].centroid);
    let crushed = |c: usize, energy: f32| energy / layout.materials[c].crush;
    let channels = lines.iter().filter_map(|&(c, from, to, energy)| {
        let volume = crushed(c, energy);
        let ball = (0.75 * volume / PI).cbrt();
        let radius = (volume / (PI * from.distance(to))).sqrt().min(ball);
        Hole::cut(centroid(c), from / scale, to / scale, radius / scale).map(|hole| (c, hole))
    });
    let dishes = blast
        .sources
        .iter()
        .zip(&blast.energies)
        .filter_map(|(source, &(c, energy))| {
            let radius = (1.5 * crushed(c, energy) / PI).cbrt();
            let at = source.at / scale;
            Hole::cut(centroid(c), at, at, radius / scale).map(|hole| (c, hole))
        });
    channels.chain(dishes).collect()
}

/// What the laws changed in one building: its record events in order, each loose group with the
/// pushes on it (building frame), the structural failures (the push check's, then the weight
/// check's), and the weight check's unassessed chunks and operation count ([`fracture::Weighed`]).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Changes {
    pub(crate) events: Vec<WallEvent>,
    pub(crate) groups: Vec<(Vec<usize>, Vec<Push>)>,
    pub(crate) failures: Vec<fracture::Failure>,
    pub(crate) unassessed: Vec<usize>,
    pub(crate) weight_steps: usize,
}

/// Run one deposit, with the `holes` it cuts ([`holes_of`]), through the laws on `live` (module
/// doc) and say what changed: the elements first touched swap, then the dust, the holes kept, the
/// cracks (the push rule's, then the structural check's, then the weight check's) and the falls,
/// each ascending but the holes, in deposit order. A chunk the holes would carry past
/// [`HOLES_PER_CHUNK`] crumbles to dust before the laws run; a hole is kept only in a chunk
/// standing after the removal, and no law reads one. A push on a chunk turned to dust went into
/// crushing it and moves nothing.
pub(crate) fn run_laws(
    layout: &Layout,
    live: &mut Live,
    loading: &Loading,
    pushes: Vec<Push>,
    holes: Vec<(usize, Hole)>,
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
    let mut struck = fracture::strike(&live.structure, &mut live.state, loading);
    struck.dusted.extend(crumbled);
    struck.dusted.sort_unstable();
    let holes: Vec<(usize, Hole)> = holes
        .into_iter()
        .filter(|&(c, _)| live.state.standing(c))
        .collect();
    for &(c, hole) in &holes {
        live.holes[c].push(hole);
    }
    let pushes: Vec<Push> = pushes
        .into_iter()
        .filter(|push| live.state.standing(push.chunk))
        .collect();
    let mut failures = fracture::structural(
        &live.structure,
        &mut live.state,
        &pushes,
        &mut live.sections,
    );
    let pushed: Vec<usize> = struck
        .cracked
        .iter()
        .chain(failures.iter().flat_map(|failure| &failure.joints))
        .copied()
        .collect();
    let weighed = fracture::weigh(
        &live.structure,
        &mut live.state,
        &struck.dusted,
        &pushed,
        &mut live.sections,
    );
    failures.extend(weighed.failures);
    let structural: Vec<usize> = failures
        .iter()
        .flat_map(|failure| failure.joints.iter().copied())
        .collect();
    let groups = fracture::fall(&live.structure, &mut live.state);
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
        .map(|c| usize::from(layout.element_of[c]))
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
            let on_group = pushes
                .iter()
                .filter(|push| group.binary_search(&push.chunk).is_ok())
                .copied()
                .collect();
            (group, on_group)
        })
        .collect();
    Changes {
        events,
        groups,
        failures,
        unassessed: weighed.unassessed,
        weight_steps: weighed.steps,
    }
}

/// A law chunk's collider shape for an instance at `scale`, about `origin` (building frame,
/// scaled), its faces moved in by `gap` m.
pub(crate) fn chunk_collider(
    kind: &BuildingKind,
    c: usize,
    scale: f32,
    origin: Vec3,
    gap: f32,
) -> Option<Collider> {
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

/// One loose group's colliders about its chunks' centre of mass (building frame), and the linear
/// and angular velocity (building frame) its pushes give it: the pushes' impulse over its mass,
/// their angular momentum about its centre of mass through its own inertia tensor, both from the
/// colliders it falls with at their substance's density. Momentum conserved: a group leaves with
/// the impulse pushed into it, no more.
pub(crate) fn launch(
    kind: &BuildingKind,
    structure: &Structure,
    scale: f32,
    group: &[usize],
    pushes: &[Push],
) -> (Vec3, Vec<(usize, Collider)>, Vec3, Vec3) {
    use avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::{
        ComputeMassProperties3d, MassProperties3d,
    };
    let chunks = &structure.chunks;
    let mass: f32 = group.iter().map(|&i| chunks[i].mass).sum();
    let centroid = group
        .iter()
        .map(|&i| chunks[i].centroid * chunks[i].mass)
        .sum::<Vec3>()
        / mass;
    let colliders: Vec<(usize, Collider)> = group
        .iter()
        .filter_map(|&c| chunk_collider(kind, c, scale, centroid, LOOSE_GAP_M).map(|s| (c, s)))
        .collect();
    let total = colliders
        .iter()
        .map(|(c, shape)| shape.mass_properties(chunks[*c].material.density))
        .fold(MassProperties3d::default(), |sum, part| sum + part);
    let impulse: Vec3 = pushes.iter().map(|push| push.impulse).sum();
    let com = centroid + total.center_of_mass;
    let angular: Vec3 = pushes
        .iter()
        .map(|push| (push.at - com).cross(push.impulse))
        .sum();
    let spin = if total.mass > 0.0 {
        total.angular_inertia_tensor().inverse() * angular
    } else {
        Vec3::ZERO
    };
    (
        centroid,
        colliders,
        impulse / total.mass.max(f32::EPSILON),
        spin,
    )
}

/// Spawn one loose group's body (module doc): at its chunks' centre of mass, turned as its
/// building `(ordinal, scale, root pose)`, moving as its pushes leave it ([`launch`]).
fn spawn_piece(
    commands: &mut Commands,
    kind: &Arc<BuildingKind>,
    structure: &Structure,
    (building, scale, root): (u16, f32, Transform),
    group: &[usize],
    pushes: &[Push],
) {
    let (centroid, colliders, velocity, spin) = launch(kind, structure, scale, group, pushes);
    let at = root.translation + root.rotation * centroid;
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
        group.iter().map(|&c| structure.chunks[c].mass).sum::<f32>(),
        velocity.length(),
        spin.length(),
    );
    // Its envelope from its colliders, which never move on it (`ballistics::Envelope`).
    let envelope = Envelope::holding(
        colliders
            .iter()
            .map(|(_, collider)| (collider, Vec3::ZERO, Quat::IDENTITY)),
    );
    // `Position`/`Rotation` before the body: it never flushes with avian's placeholder pose.
    let mut piece = commands.spawn((
        Name::new("WallPiece"),
        WallPiece {
            building,
            chunks: group.iter().map(|&c| c as u16).collect(),
        },
        Transform::from_translation(at).with_rotation(root.rotation),
        Position(at),
        Rotation(root.rotation),
        Visibility::default(),
        crate::debris::DebrisPiece,
        crate::eased_dynamic_body(),
        LinearVelocity(root.rotation * velocity),
        AngularVelocity(root.rotation * spin),
    ));
    if let Some(envelope) = envelope {
        piece.insert(envelope);
    }
    piece.with_children(|parent| {
        for (c, collider) in colliders {
            parent.spawn((
                Transform::IDENTITY,
                collider,
                ColliderDensity(structure.chunks[c].material.density),
                piece_layers(),
                kind.piece_friction(c),
                chunk_volume(kind, c),
            ));
        }
    });
}

/// A loose chunk's filter: brick in ballistics ([`Layer::Armor`]), colliding as debris with the
/// ground, buildings, hulls and other pieces, never with a tank's armour volumes.
pub(crate) fn piece_layers() -> CollisionLayers {
    CollisionLayers::new(
        [Layer::Debris, Layer::Armor],
        [Layer::Terrain, Layer::Vehicle, Layer::Debris],
    )
}

/// REPLICA: rebuild every building's state from the record, from the entry it has not applied; a
/// record of another generation first stands every building whole again.
fn derive_from_record(
    record: Res<WallRecord>,
    mut applied: Local<(u32, usize)>,
    mut buildings: Query<(&MapInstance, &mut Breakage)>,
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
            if !shape.apply(event) {
                error!("buildings: record event {event:?} names nothing of building {identity:?}");
            } else if let WallEvent::Hole(c, hole) = event {
                live.holes[usize::from(c)].push(hole);
            }
        }
        live.swapped = shape.swapped;
        live.state.fate = shape.fate;
        live.state.cracked = shape.cracked;
    }
    for ordinal in by_ordinal.keys() {
        error!("buildings: the record names building {ordinal}, which this map does not place");
    }
    applied.1 = record.entries.len();
}

/// Every peer: make a changed building's colliders its shape's — an intact element its own
/// collider, a swapped element one static collider per standing chunk — spawning and despawning
/// only what differs. A spawned collider carries its pose so the refit that follows places it.
fn reconcile(
    buildings: Query<(Entity, &Transform, &Breakage, Option<&Children>), Changed<Breakage>>,
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
                    let Some(collider) = chunk_collider(kind, c, breakage.scale, Vec3::ZERO, 0.0)
                    else {
                        continue;
                    };
                    commands.spawn((
                        ChildOf(root),
                        Transform::IDENTITY,
                        collider,
                        super::element_layers(),
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
    let authority = not(resource_exists::<ClientReplica>);
    app.init_resource::<WallQueue>()
        .init_resource::<WallRecord>()
        .add_observer(read_shell)
        .add_observer(set_off)
        .configure_sets(
            FixedUpdate,
            WallLawSet.after(crate::state::SimPhase::ProjectileMarch),
        )
        .add_systems(
            FixedUpdate,
            (apply_deposits, refit_systems())
                .chain()
                .in_set(WallLawSet)
                .run_if(authority),
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
