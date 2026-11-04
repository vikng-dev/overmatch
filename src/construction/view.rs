//! A construction's view: one mesh per construction asset, built from its RON's elements, merged and
//! shared by every instance, drawn with the constructions' one material ([`crate::construction_surface`]).
//!
//! VIEW ONLY (ADR-0014). The mesh is every element's hull, face by face: the drawn silhouette is
//! exactly the colliders' union, and an opening, which is no element, is a hole the camera sees
//! through. Faces two elements share stay in the mesh; they lie inside the wall and are never seen.
//!
//! # Metres and declared surface frames
//!
//! Each face carries the frame [`crate::construction_surface::unit_box`] uses: outward normal `n`,
//! tangent `t`, image-up `b = n × t`, `uv = (p·t, −p·b)` at every corner `p`, here in metres of
//! the construction frame. The material's vertex stage multiplies them by the instance's own scale
//! along `t` and `b`, so a scaled house keeps real-size bricks. A wall face stands its image
//! upright (`t = Y × n`) when its surface frame declares Upright; a Grain frame projects its
//! authored direction onto the face; a level face runs along its element's longest level edge
//! ([`tangent`]). The second UV channel selects the visual's declared palette lane, resolved through
//! its substance and the visual library.
//!
//! # A broken construction ([`plugin`])
//!
//! The view presents the construction record ([`Record`]) up to the presentation clock, so a break
//! shows when the round that made it is drawn arriving. A construction with a swapped element draws its
//! own mesh ([`broken_mesh`]): whole elements as before, a swapped element's standing chunks as
//! their hulls, framed exactly as the element's faces, so a swapped wall looks unchanged until a
//! chunk goes; one with nothing left standing draws no mesh at all, never an empty one. Cracks are
//! dark strips where an opened seam meets its element's surface ([`crack_mesh`]). A fallen group
//! draws its chunks' hulls on its body or carrier ([`piece_mesh`]), holes and all.
//!
//! # Holes
//!
//! A hole never changes a mesh: a construction whose standing chunks hold one wears its own copy of the
//! material ([`crate::construction_surface::holed`]) with its [`hole_table`] bound, and the shader skips
//! the surface inside a hole and draws its inner wall, in the main pass and the shadow pass alike
//! (`assets/shaders/construction_holes.wgsl`). A swapped chunk's faces carry its index + 1 in the second
//! UV's `y`, a whole element's 0, so a fragment finds its chunk's holes. A hole is matter missing
//! from its chunk, so a chunk that falls keeps it: a fallen piece's faces carry their chunk's place
//! in the piece + 1, and a piece whose chunks hold a hole wears its own copy with its
//! [`piece_hole_table`] bound.
//!
//! # Dressing: skin and cards
//!
//! A construction's [`Dressing`] (`<id>.dressing.ron`, view only) adds what its law geometry does
//! not hold. A skinned part's ORIGINAL EXTERIOR — a face on one of its element's planes that meets
//! no other element and no ground — wears the skin tint over its substance's own surface (bark on a
//! trunk); every exposed face, a cut inside an element or an element plane another solid covered,
//! wears its substance's surface untinted (the wood). A card (a branch) belongs to the one law chunk
//! holding its base ([`Dressing::resolve`]) and is drawn exactly where that chunk's faces are: on
//! the construction while the chunk stands, on the fallen group holding it, nowhere once it is dust.
//! Both follow the shown shape and piece visibility, so they move at the shared presentation cursor
//! with the matter and never on a packet's arrival. Tints ride in the vertex colour
//! ([`crate::vfx::construction_dressing`]); an undressed face's is white.

use std::collections::{BTreeMap, BTreeSet};

use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use bevy::render::storage::ShaderBuffer;

use super::ConstructionKind;
use super::breakage::{
    Breakage, Event, HOLES_PER_CHUNK, Hole, Layout, Piece, Record, ReplicaSet, Shape,
};
use super::fracture::{Block, Fate, Side, clip_polygon};
use super::schema::{ConstructionAsset, PLANE_TOLERANCE_M, SurfaceFrame};
use crate::construction_surface::{ConstructionMaterial, holed};
use crate::debris::PresentationClock;
use crate::scatter::MapInstance;

/// `<id>.dressing.ron` as authored or generated (module doc).
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DressingFile {
    /// The parts whose original exterior wears the skin.
    skin: Vec<String>,
    /// Each card: where it grows from, then its flat polygon's corners in order, construction
    /// metres.
    cards: Vec<CardFile>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CardFile {
    base: [f32; 3],
    corners: Vec<[f32; 3]>,
}

/// One branch card: its owning law chunk and its polygon's corners, construction frame at scale 1.
#[derive(Debug, Clone, PartialEq)]
struct Card {
    owner: usize,
    corners: Vec<Vec3>,
}

/// A construction's dressing, resolved against its layout (module doc).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Dressing {
    /// Per element: the planes of its hull that are original exterior of a skinned part.
    skin: Vec<Vec<(Vec3, f32)>>,
    cards: Vec<Card>,
}

/// Resolution: a card's corners lie within this distance (m) of their mean plane; the generator
/// writes flat kites to `plan.CORNER_DECIMALS`.
const CARD_PLANE_TOLERANCE_M: f32 = 1e-4;
/// Resolution: least cosine between an element plane and a face or joint read as lying on it. A
/// chunk's faces and joints are refit from f32 corners, so a small one tilts off its element plane
/// by more than [`PLANE_TOLERANCE_M`]; corners within [`CRACK_FACE_TOLERANCE_M`] of the plane
/// decide, and this only keeps a crossing face out (about 8 degrees).
const ON_PLANE_MIN_COS: f32 = 0.99;

impl Dressing {
    /// A construction drawn bare: no skin and no card. The bake's admission and the inline fixtures,
    /// which hold no dressing file.
    pub(crate) fn bare(layout: &Layout) -> Self {
        Self {
            skin: vec![Vec::new(); layout.element_blocks.len()],
            cards: Vec::new(),
        }
    }

    /// The dressing `text` declares for the construction `asset` laid out as `layout`. Refuses a
    /// skin naming no part of the asset, a card of fewer than three corners, off its plane or not
    /// finite, and a card whose base lies in no law chunk: the owner is the chunk holding the base
    /// deepest, the lowest on a tie, so a base on a shared face has one owner.
    pub(crate) fn resolve(
        text: &str,
        asset: &ConstructionAsset,
        layout: &Layout,
    ) -> Result<Self, String> {
        let file: DressingFile = ron::from_str(text).map_err(|error| error.to_string())?;
        for part in &file.skin {
            if !asset.elements.iter().any(|element| &element.part == part) {
                return Err(format!(
                    "skin names part {part:?}, which no element declares; name a part of the construction"
                ));
            }
        }
        let skin = (0..layout.element_blocks.len())
            .map(|e| {
                if !file.skin.contains(&asset.elements[e].part) {
                    return Vec::new();
                }
                layout.element_blocks[e]
                    .planes
                    .iter()
                    .copied()
                    .filter(|&plane| !covered(layout, e, plane))
                    .collect()
            })
            .collect();
        let cards = file
            .cards
            .iter()
            .enumerate()
            .map(|(k, card)| {
                let corners: Vec<Vec3> = card.corners.iter().map(|p| Vec3::from_array(*p)).collect();
                let base = Vec3::from_array(card.base);
                let flat = corners.len() >= 3
                    && base.is_finite()
                    && corners.iter().all(|p| p.is_finite())
                    && card_normal(&corners).is_some_and(|(n, o)| {
                        corners.iter().all(|p| (n.dot(*p) - o).abs() < CARD_PLANE_TOLERANCE_M)
                    });
                if !flat {
                    return Err(format!(
                        "card {k}: author three or more finite corners on one plane"
                    ));
                }
                let depth = |c: usize| {
                    layout.blocks[c]
                        .planes
                        .iter()
                        .map(|(n, o)| n.dot(base) - o)
                        .fold(f32::NEG_INFINITY, f32::max)
                };
                let owner = (0..layout.blocks.len())
                    .map(|c| (c, depth(c)))
                    .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
                    .filter(|(_, depth)| *depth <= PLANE_TOLERANCE_M)
                    .ok_or_else(|| {
                        format!("card {k}: its base {base} lies in no chunk; grow it from the construction")
                    })?
                    .0;
                Ok(Card { owner, corners })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { skin, cards })
    }

    /// Whether a face of element `e`, outward `n` through `corners`, is original exterior skin.
    fn skins(&self, e: usize, n: Vec3, corners: &[Vec3]) -> bool {
        self.skin[e].iter().any(|(out, offset)| {
            out.dot(n) > ON_PLANE_MIN_COS
                && corners
                    .iter()
                    .all(|p| (out.dot(*p) - offset).abs() < CRACK_FACE_TOLERANCE_M)
        })
    }
}

/// Whether element `e`'s hull plane meets another element or the ground: a joint from one of its
/// chunks to a chunk of another element, or to an anchor, lies on it.
fn covered(layout: &Layout, e: usize, (out, offset): (Vec3, f32)) -> bool {
    layout.joints.iter().any(|joint| {
        let a = usize::from(layout.element_of[joint.a]);
        let b = match joint.b {
            Side::Chunk(b) => Some(usize::from(layout.element_of[b])),
            Side::Ground => None,
        };
        let between = (a == e && b != Some(e)) || (b == Some(e) && a != e);
        between
            && joint.normal.dot(out).abs() > ON_PLANE_MIN_COS
            && (out.dot(joint.centroid) - offset).abs() < CRACK_FACE_TOLERANCE_M
    })
}

/// A polygon's unit normal by Newell's sum and its plane offset; `None` for a degenerate one.
fn card_normal(corners: &[Vec3]) -> Option<(Vec3, f32)> {
    let mut sum = Vec3::ZERO;
    for k in 0..corners.len() {
        sum += corners[k].cross(corners[(k + 1) % corners.len()]);
    }
    let n = sum.try_normalize()?;
    let mean = corners.iter().sum::<Vec3>() / corners.len() as f32;
    Some((n, n.dot(mean)))
}

/// Convex boundary faces, excluding redundant coplanar and interior points: the law block's
/// ([`Block::hull`]), so the view draws the faces the collider and the laws read.
fn hull_faces(points: &[Vec3]) -> Vec<(Vec3, Vec<Vec3>)> {
    Block::hull(points.to_vec())
        .expect("an admitted hull is a law block")
        .faces()
}

/// Resolution: squared projection floor for unit surface-frame axes; smaller projections
/// amplify f32 noise when normalized.
const FRAME_PROJECTION_MIN_SQUARED: f32 = 1e-6;

/// A face's tangent: its element's authored grain projected into its plane; on an end face the
/// grain crosses, and on every face of an Upright element, the upright image (`Y × n`). A level
/// face has no upright, so its image runs along its element's level `run` ([`run`]), which lies
/// in the face within the upright test's tolerance: no level face is left without a tangent.
fn tangent(n: Vec3, frame: SurfaceFrame, run: Vec3) -> Vec3 {
    let in_face = |axis: Vec3| axis - n * axis.dot(n);
    if let SurfaceFrame::Grain(grain) = frame {
        let projected = in_face(Vec3::from_array(grain));
        if projected.length_squared() > FRAME_PROJECTION_MIN_SQUARED {
            return projected.normalize();
        }
    }
    let upright = Vec3::Y.cross(n);
    if upright.length_squared() > FRAME_PROJECTION_MIN_SQUARED {
        return upright.normalize();
    }
    in_face(run).normalize()
}

/// An element's run: the level direction of its hull's edge longest seen from above, unit. A
/// bounded solid has an edge off the vertical, so every element has one; a wall's runs along the
/// wall wherever it is longer than thick.
fn run(points: &[Vec3]) -> Vec3 {
    let mut longest = Vec3::ZERO;
    for (_, corners) in hull_faces(points) {
        for k in 0..corners.len() {
            let edge = corners[(k + 1) % corners.len()] - corners[k];
            let level = Vec3::new(edge.x, 0.0, edge.z);
            if level.length_squared() > longest.length_squared() {
                longest = level;
            }
        }
    }
    longest.normalize()
}

struct ElementLook {
    frame: SurfaceFrame,
    run: Vec3,
    secondary: bool,
}

impl ElementLook {
    fn of(asset: &ConstructionAsset, e: usize) -> Self {
        let element = &asset.elements[e];
        Self {
            frame: element.surface_frame,
            run: run(&element.points()),
            secondary: crate::construction_surface::lane_for_substance(
                &asset.materials[usize::from(element.material)],
            ) == crate::construction_surface::SurfaceLane::Secondary,
        }
    }
}

/// A mesh's faces as they are added, each framed in metres (module doc).
#[derive(Default)]
struct Faces {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    tangents: Vec<[f32; 4]>,
    uvs: Vec<[f32; 2]>,
    packs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

/// An untinted face's vertex colour: its surface as its substance draws it.
const UNTINTED: [f32; 4] = [1.0; 4];

/// A presentation tint as the vertex colour carries it: linear RGB, opaque.
fn tint(srgb: Vec3) -> [f32; 4] {
    Color::srgb_from_array(srgb.to_array())
        .to_linear()
        .to_f32_array()
}

impl Faces {
    /// One face of outward normal `n`, its corners counter-clockwise seen from outside, drawn with
    /// its element's `look` ([`tangent`]) under `tint`; drawn `offset` from where it is (asset
    /// frame) while framed where it is, so a piece keeps the wall's bricks. `tag` is the hole
    /// lookup's (module doc).
    fn face(
        &mut self,
        n: Vec3,
        corners: &[Vec3],
        look: &ElementLook,
        offset: Vec3,
        (tag, tint): (f32, [f32; 4]),
    ) {
        if corners.len() < 3 {
            return;
        }
        let t = tangent(n, look.frame, look.run);
        let b = n.cross(t);
        let first = u32::try_from(self.positions.len()).expect("a construction's mesh fits u32");
        for p in corners {
            self.positions.push((*p - offset).to_array());
            self.normals.push(n.to_array());
            self.tangents.push(t.extend(1.0).to_array());
            self.uvs.push([p.dot(t), -p.dot(b)]);
            self.packs
                .push([if look.secondary { 1.0 } else { 0.0 }, tag]);
            self.colors.push(tint);
        }
        // Corners run counter-clockwise seen from outside: a fan from the first.
        for k in 1..corners.len() as u32 - 1 {
            self.indices.extend([first, first + k, first + k + 1]);
        }
    }

    /// Element `e`'s hull faces, its original exterior skinned (module doc).
    fn element(&mut self, kind: &ConstructionKind, e: usize) {
        let look = ElementLook::of(&kind.asset, e);
        for (n, corners) in hull_faces(&kind.asset.elements[e].points()) {
            let tint = skin_tint(kind, e, n, &corners);
            self.face(n, &corners, &look, Vec3::ZERO, (0.0, tint));
        }
    }

    /// Law chunk `c`'s hull with its element's look, drawn `offset` back, its faces tagged `tag`
    /// (module doc), its original exterior skinned, and the cards it owns.
    fn chunk(&mut self, kind: &ConstructionKind, c: usize, offset: Vec3, tag: f32) {
        let e = usize::from(kind.layout.element_of[c]);
        let look = ElementLook::of(&kind.asset, e);
        for (n, corners) in kind.layout.blocks[c].faces() {
            let tint = skin_tint(kind, e, n, &corners);
            self.face(n, &corners, &look, offset, (tag, tint));
        }
        self.cards(kind, &look, |owner| owner == c, offset);
    }

    /// The cards whose owner `owns` accepts, each drawn on both sides under the card tint, tagged
    /// to read no hole.
    fn cards(
        &mut self,
        kind: &ConstructionKind,
        look: &ElementLook,
        owns: impl Fn(usize) -> bool,
        offset: Vec3,
    ) {
        let card = tint(crate::vfx::construction_dressing().card_srgb);
        for drawn in kind.dressing.cards.iter().filter(|card| owns(card.owner)) {
            let (n, _) = card_normal(&drawn.corners).expect("an admitted card is flat");
            self.face(n, &drawn.corners, look, offset, (0.0, card));
            let back: Vec<Vec3> = drawn.corners.iter().rev().copied().collect();
            self.face(-n, &back, look, offset, (0.0, card));
        }
    }

    fn mesh(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, self.tangents)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, self.packs)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// A face's vertex colour: the skin tint on a skinned element's original exterior, else untinted.
fn skin_tint(kind: &ConstructionKind, e: usize, n: Vec3, corners: &[Vec3]) -> [f32; 4] {
    if kind.dressing.skins(e, n, corners) {
        tint(crate::vfx::construction_dressing().skin_srgb)
    } else {
        UNTINTED
    }
}

/// The construction's mesh in its own frame at scale 1 (module doc): every element and every card.
pub(crate) fn mesh(kind: &ConstructionKind) -> Mesh {
    let mut faces = Faces::default();
    for e in 0..kind.layout.element_blocks.len() {
        faces.element(kind, e);
        let look = ElementLook::of(&kind.asset, e);
        faces.cards(kind, &look, |c| chunk_of(kind, c) == e, Vec3::ZERO);
    }
    faces.mesh()
}

/// The element law chunk `c` tiles.
fn chunk_of(kind: &ConstructionKind, c: usize) -> usize {
    usize::from(kind.layout.element_of[c])
}

/// A broken construction's mesh in its own frame at scale 1: every whole element's hull, and every
/// standing chunk of each swapped element its hull, so a chunk turned to dust or fallen is a hole;
/// each card drawn with the matter that owns it.
pub(crate) fn broken_mesh(kind: &ConstructionKind, shape: &Shape) -> Mesh {
    let mut faces = Faces::default();
    for (e, swapped) in shape.swapped.iter().enumerate() {
        if *swapped {
            for c in kind.layout.chunks_of(e) {
                if shape.fate[c] == Fate::Standing {
                    faces.chunk(kind, c, Vec3::ZERO, (c + 1) as f32);
                }
            }
        } else {
            faces.element(kind, e);
            let look = ElementLook::of(&kind.asset, e);
            faces.cards(kind, &look, |c| chunk_of(kind, c) == e, Vec3::ZERO);
        }
    }
    faces.mesh()
}

/// A fallen group's mesh, its chunks' hulls and cards about `origin` (asset frame, scale 1),
/// framed where they stood, the `k`-th chunk's faces tagged `k + 1` (module doc).
pub(crate) fn piece_mesh(kind: &ConstructionKind, chunks: &[u16], origin: Vec3) -> Mesh {
    let mut faces = Faces::default();
    for (k, &c) in chunks.iter().enumerate() {
        faces.chunk(kind, usize::from(c), origin, (k + 1) as f32);
    }
    faces.mesh()
}

/// The hole table a construction's material binds (module doc): one header per law chunk, the
/// standing ones' holes and hulls. `None` when no standing chunk holds a hole.
pub(crate) fn hole_table(
    asset: &ConstructionAsset,
    layout: &Layout,
    shape: &Shape,
    holes: &[Vec<Hole>],
) -> Option<Vec<Vec4>> {
    let standing =
        (0..layout.chunks.len()).map(|c| (shape.fate.get(c) == Some(&Fate::Standing)).then_some(c));
    table(asset, layout, standing, holes, Vec3::ZERO)
}

/// The hole table a fallen piece's material binds (module doc): one header per chunk of the piece,
/// in its order, their holes and hulls drawn `origin` back as [`piece_mesh`] draws them. `None`
/// when none of its chunks holds a hole.
pub(crate) fn piece_hole_table(
    asset: &ConstructionAsset,
    layout: &Layout,
    chunks: &[u16],
    holes: &[Vec<Hole>],
    origin: Vec3,
) -> Option<Vec<Vec4>> {
    let chunks = chunks.iter().map(|&c| Some(usize::from(c)));
    table(asset, layout, chunks, holes, origin)
}

/// A hole table (the layout is `assets/shaders/construction_holes.wgsl`'s), mesh-local metres: one
/// header per slot, a slot being a face tag − 1 that names the law chunk it draws, or none; then
/// each named chunk's holes, at most [`HOLES_PER_CHUNK`], and its hull's planes, all drawn
/// `offset` back from the asset frame. `None` when no named chunk holds a hole.
fn table(
    asset: &ConstructionAsset,
    layout: &Layout,
    slots: impl ExactSizeIterator<Item = Option<usize>>,
    holes: &[Vec<Hole>],
    offset: Vec3,
) -> Option<Vec<Vec4>> {
    let headers = slots.len();
    let mut table = vec![Vec4::ZERO; headers];
    for (slot, c) in slots.enumerate() {
        let Some((c, holes)) = c.and_then(|c| Some((c, holes.get(c)?))) else {
            continue;
        };
        if holes.is_empty() {
            continue;
        }
        let centroid = Vec3::from_array(asset.chunks[layout.chunks[c] as usize].centroid) - offset;
        let at = |mm: [i16; 3]| centroid + Vec3::from_array(mm.map(f32::from)) / 1000.0;
        let kept = &holes[..holes.len().min(HOLES_PER_CHUNK)];
        let planes = &layout.blocks[c].planes;
        table[slot] = Vec4::new(
            table.len() as f32,
            kept.len() as f32,
            planes.len() as f32,
            0.0,
        );
        for hole in kept {
            table.push(at(hole.from).extend(f32::from(hole.radius) / 1000.0));
            table.push(at(hole.to).extend(0.0));
        }
        table.extend(planes.iter().map(|(n, o)| n.extend(*o - n.dot(offset))));
    }
    (table.len() > headers).then_some(table)
}

/// Resolution: skip numerically unresolved contact edges (m) before normalizing their direction.
const CRACK_EDGE_MIN_M: f32 = 1e-4;
/// Resolution: plane-membership allowance (m) for independently reconstructed f32 element and
/// joint faces.
const CRACK_FACE_TOLERANCE_M: f32 = 1e-3;

/// The cracks a broken construction shows, in its frame at scale 1: for every cracked joint between
/// two standing chunks, a dark strip along each edge of their shared face that lies on a bounding
/// plane of the first chunk's element, where the crack opens to the element's surface. `None` when
/// no crack shows.
pub(crate) fn crack_mesh(
    layout: &Layout,
    shape: &Shape,
    look: &crate::vfx::ConstructionCracks,
) -> Option<Mesh> {
    let (mut positions, mut normals, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    for (j, joint) in layout.joints.iter().enumerate() {
        let Side::Chunk(b) = joint.b else {
            continue;
        };
        if !shape.cracked[j]
            || shape.fate[joint.a] != Fate::Standing
            || shape.fate[b] != Fate::Standing
        {
            continue;
        }
        let face_a = layout.blocks[joint.a].face_on(joint.centroid, joint.normal);
        let face_b = layout.blocks[b].face_on(joint.centroid, joint.normal);
        let polygon = clip_polygon(&face_a, &face_b, joint.normal);
        let element = &layout.element_blocks[usize::from(layout.element_of[joint.a])];
        for k in 0..polygon.len() {
            let (p, q) = (polygon[k], polygon[(k + 1) % polygon.len()]);
            let along = q - p;
            if along.length() < CRACK_EDGE_MIN_M {
                continue;
            }
            for &(out, offset) in &element.planes {
                let on = |x: Vec3| (out.dot(x) - offset).abs() < CRACK_FACE_TOLERANCE_M;
                if !(on(p) && on(q)) {
                    continue;
                }
                let side = out.cross(along.normalize()) * look.half_width_m;
                let lift = out * look.standoff_m;
                let first = positions.len() as u32;
                for corner in [p - side, q - side, q + side, p + side] {
                    positions.push((corner + lift).to_array());
                    normals.push(out.to_array());
                }
                // `side` is `out × along`: p − side → q − side → q + side turns about `out`.
                indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
            }
        }
    }
    (!indices.is_empty()).then(|| {
        Mesh::new(PrimitiveTopology::TriangleList, default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
            .with_inserted_indices(Indices::U32(indices))
    })
}

/// A construction's drawn child: the entity the scatter gave the construction's mesh and material, at the
/// instance's scale under its root.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct View;

/// The one material every construction wears ([`crate::construction_surface`]), for the meshes the breaks
/// add; and the cracks' dark material.
#[derive(Resource, Clone)]
pub(crate) struct Look {
    pub(crate) surface: Handle<crate::construction_surface::ConstructionMaterial>,
}

/// The cracks' material: unlit near-black, the shadow inside an opened seam.
#[derive(Resource, Clone)]
struct CrackLook(Handle<StandardMaterial>);

/// What a construction shows: the record presented up to the presentation clock.
#[derive(Component, Clone, Debug, PartialEq)]
pub(crate) struct ShownShape(pub(crate) Shape);

/// The holes a construction shows, per law chunk in record order, presented with its [`ShownShape`].
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub(crate) struct ShownHoles(pub(crate) Vec<Vec<Hole>>);

/// A broken construction view's cracks' child. The view's own mesh lives in its `Mesh3d` alone: a
/// replaced or removed handle frees the old asset.
#[derive(Component)]
struct OwnMesh {
    cracks: Option<Entity>,
}

/// Present the record up to the presentation clock ([`PresentationClock`]: the interpolation cursor on a
/// replica, the sim tick elsewhere), so a break appears when the round that made it is drawn
/// arriving (ADR-0037), and redraw every construction whose shown shape moved.
fn present_record(
    record: Res<Record>,
    clock: Res<PresentationClock>,
    mut presented: Local<(u32, usize, usize)>,
    mut constructions: Query<
        (
            Entity,
            &MapInstance,
            &Breakage,
            Option<&mut ShownShape>,
            Option<&mut ShownHoles>,
        ),
        Without<Piece>,
    >,
    mut commands: Commands,
) {
    let reset = presented.0 != record.generation;
    if reset {
        for (_, _, breakage, shown, holes) in &mut constructions {
            if let Some(mut shown) = shown {
                shown.0 = Shape::intact(&breakage.kind.layout);
            }
            if let Some(mut holes) = holes {
                holes.set_if_neq(ShownHoles::default());
            }
        }
        *presented = (record.generation, 0, 0);
    }
    assert!(
        record.entries.len() >= presented.2,
        "a construction record never shrinks within its generation"
    );
    presented.2 = record.entries.len();
    let due = record.entries[presented.1..]
        .iter()
        .take_while(|entry| {
            clock
                .since(entry.tick)
                .is_some_and(|elapsed| elapsed >= 0.0)
        })
        .count();
    if due == 0 {
        return;
    }
    let mut events: std::collections::BTreeMap<u16, Vec<Event>> = Default::default();
    for entry in &record.entries[presented.1..presented.1 + due] {
        events
            .entry(entry.construction)
            .or_default()
            .push(entry.event);
    }
    presented.1 += due;
    for (root, identity, breakage, shown, shown_holes) in &mut constructions {
        let Some(events) = events.remove(&identity.ordinal) else {
            continue;
        };
        let mut shape = shown.as_ref().map_or_else(
            || Shape::intact(&breakage.kind.layout),
            |shown| shown.0.clone(),
        );
        let mut holes = shown_holes.as_ref().map_or_else(
            || ShownHoles(vec![Vec::new(); shape.fate.len()]),
            |holes| (*holes).clone(),
        );
        holes.0.resize(shape.fate.len(), Vec::new());
        for event in events {
            shape.apply(event);
            if let Event::Hole(c, hole) = event {
                holes.0[usize::from(c)].push(hole);
            }
        }
        match shown {
            Some(mut shown) => {
                shown.set_if_neq(ShownShape(shape));
            }
            None => {
                commands.entity(root).insert(ShownShape(shape));
            }
        }
        match shown_holes {
            Some(mut shown) => {
                shown.set_if_neq(holes);
            }
            None => {
                commands.entity(root).insert(holes);
            }
        }
    }
    assert!(
        events.is_empty(),
        "a construction record names only constructions this map places: {:?}",
        events.keys().collect::<Vec<_>>()
    );
}

/// The stores a construction's own material is made in ([`crate::construction_surface::holed`]): absent
/// where nothing draws.
#[derive(bevy::ecs::system::SystemParam)]
struct SurfaceStores<'w> {
    look: Option<Res<'w, Look>>,
    materials: Option<ResMut<'w, Assets<ConstructionMaterial>>>,
    buffers: Option<ResMut<'w, Assets<ShaderBuffer>>>,
}

impl SurfaceStores<'_> {
    /// The material a construction whose standing chunks hold `table` wears: its own holed copy, or the
    /// shared one when there is no table. `None` where nothing draws.
    fn wear(&mut self, table: Option<Vec<Vec4>>) -> Option<Handle<ConstructionMaterial>> {
        let look = self.look.as_ref()?;
        let Some(table) = table else {
            return Some(look.surface.clone());
        };
        let (materials, buffers) = (self.materials.as_mut()?, self.buffers.as_mut()?);
        let material = holed(
            materials.get(&look.surface)?,
            buffers.add(ShaderBuffer::from(table)),
        );
        Some(materials.add(material))
    }
}

/// Redraw a construction whose shown shape or holes moved: its view's own mesh, its material (its own
/// while a hole shows, module doc) and its cracks. A view holds a `Mesh3d` exactly while its
/// construction has a face to draw: a zero-vertex mesh never reaches the renderer (see
/// [`crate::track::shadow_proxy::ProxySide::mesh`]).
fn redraw(
    constructions: Query<
        (&Breakage, &ShownShape, Option<&ShownHoles>, &Children),
        Or<(Changed<ShownShape>, Changed<ShownHoles>)>,
    >,
    mut views: Query<
        (
            Entity,
            Option<&mut Mesh3d>,
            Option<&mut MeshMaterial3d<ConstructionMaterial>>,
            Option<&OwnMesh>,
        ),
        With<View>,
    >,
    look: Option<Res<CrackLook>>,
    mut surfaces: SurfaceStores,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut commands: Commands,
) {
    for (breakage, shown, holes, children) in &constructions {
        let Some(view) = children.iter().find(|child| views.contains(*child)) else {
            continue; // A construction spawned with no view attached has nothing to redraw.
        };
        let (view, drawn, worn, own) = views
            .get_mut(view)
            .expect("the view child was found in this same query");
        let kind = &breakage.kind;
        let table =
            holes.and_then(|holes| hole_table(&kind.asset, &kind.layout, &shown.0, &holes.0));
        match (surfaces.wear(table), worn) {
            (Some(surface), Some(mut worn)) => worn.0 = surface,
            (Some(surface), None) => {
                commands.entity(view).insert(MeshMaterial3d(surface));
            }
            (None, _) => {}
        }
        let broken = broken_mesh(kind, &shown.0);
        let mesh = (broken.count_vertices() > 0).then(|| meshes.add(broken));
        let cracks = crack_mesh(&kind.layout, &shown.0, crate::vfx::construction_cracks())
            .map(|m| meshes.add(m));
        match (&mesh, drawn) {
            (Some(mesh), Some(mut drawn)) => drawn.0 = mesh.clone(),
            (Some(mesh), None) => {
                commands.entity(view).insert(Mesh3d(mesh.clone()));
            }
            (None, Some(_)) => {
                commands.entity(view).remove::<Mesh3d>();
            }
            (None, None) => {}
        }
        match own {
            Some(OwnMesh {
                cracks: Some(child),
            }) => commands.entity(*child).despawn(),
            Some(_) => {}
            None => {
                commands.entity(view).insert(OwnMesh { cracks: None });
            }
        }
        let material = match (&look, materials.as_deref_mut()) {
            (Some(look), _) => Some(look.0.clone()),
            (None, Some(materials)) => {
                let handle = materials.add(StandardMaterial {
                    base_color: Color::srgb_from_array(
                        crate::vfx::construction_cracks().color_srgb.to_array(),
                    ),
                    unlit: true,
                    ..default()
                });
                commands.insert_resource(CrackLook(handle.clone()));
                Some(handle)
            }
            (None, None) => None,
        };
        let child = match (cracks, material) {
            (Some(cracks), Some(material)) => Some(
                commands
                    .spawn((
                        ChildOf(view),
                        Transform::IDENTITY,
                        Mesh3d(cracks),
                        MeshMaterial3d(material),
                    ))
                    .id(),
            ),
            _ => None,
        };
        commands.queue(move |world: &mut World| {
            if let Some(mut own) = world.get_mut::<OwnMesh>(view) {
                own.cracks = child;
            }
        });
    }
}

/// Dress a fallen group's entity (the body offline, its carrier on a replica) with its chunks'
/// mesh at its construction's scale, hidden until the record is presented to its fall. A carrier is
/// dressed once it holds a pose: an interpolated one takes its first when the cursor reaches it.
fn dress_pieces(
    pieces: Query<
        (
            Entity,
            &Piece,
            Option<&crate::world::WorldGeneration>,
            Option<&super::breakage::AdmittedPiece>,
        ),
        (With<Transform>, Without<PieceDressed>),
    >,
    generation: Option<Res<crate::world::CurrentWorld>>,
    replica: Option<Res<crate::ClientReplica>>,
    constructions: Query<(Entity, &MapInstance, &Breakage), Without<Piece>>,
    look: Option<Res<Look>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    let Some(look) = look else {
        return;
    };
    for (entity, piece, belongs, admitted) in &pieces {
        // Offline pieces are local bodies. A streamed piece waits for its own map construction
        // before resolving the construction root it will retain for its entire presented lifetime.
        if belongs.is_some_and(|belongs| {
            generation.as_deref().and_then(|current| current.0.as_ref()) != Some(belongs)
        }) {
            continue;
        }
        let local;
        let admitted = if replica.is_some() || belongs.is_some() {
            // Admission precedes dressing; a peer carrier can await its selected world or flush.
            let Some(admitted) = admitted else {
                continue;
            };
            admitted
        } else {
            local = piece
                .admitted(
                    constructions
                        .iter()
                        .find(|(_, identity, _)| identity.ordinal == piece.construction)
                        .map(|(root, _, breakage)| (root, breakage)),
                )
                .expect(
                    "authority pieces are built from the placed construction's own sorted chunks",
                );
            &local
        };
        let kind = &admitted.kind;
        let construction = admitted.construction;
        let origin = admitted.origin;
        let mesh = meshes.add(piece_mesh(kind, &admitted.chunks, origin));
        let view = commands
            .spawn((
                ChildOf(entity),
                Transform::from_scale(Vec3::splat(admitted.scale)),
                Mesh3d(mesh),
                MeshMaterial3d(look.surface.clone()),
                crate::render_cost::CasterClass::House,
            ))
            .id();
        commands.entity(entity).insert((
            PieceDressed {
                construction,
                view,
                origin,
            },
            Visibility::Hidden,
        ));
    }
}

/// A dressed piece: its construction's root, resolved from the piece's ordinal once, at dressing; its
/// mesh child; and the point (asset frame, scale 1) its mesh is drawn about.
#[derive(Component)]
struct PieceDressed {
    construction: Entity,
    view: Entity,
    origin: Vec3,
}

/// A shown piece whose mesh wears its chunks' holes.
#[derive(Component)]
struct PieceHoled(usize);

/// A piece shows once its construction's shown shape has its chunks fallen, and from then on wears its
/// chunks' holes (module doc). New deposits can cut a moving group too, so each changed hole
/// count refreshes its material at the same shared cursor as the ledger.
///
/// A split hands its chunks over at its tick on the shared cursor, all at once: the retired group
/// stays shown until every chunk it held that still stands has a replacement born at its
/// retirement tick. Until then (and while the retirement itself has not arrived) a replacement
/// waits hidden behind the older piece still drawing its chunks, so each chunk is drawn by exactly
/// one piece. The arrivals are indexed once per frame by construction, birth tick and chunk, and every
/// handover check is a lookup there.
fn show_pieces(
    clock: Res<PresentationClock>,
    arrived: Query<(&Piece, &PieceDressed)>,
    mut pieces: Query<(
        Entity,
        &Piece,
        &PieceDressed,
        &mut Visibility,
        Option<&PieceHoled>,
    )>,
    constructions: Query<(&Breakage, &ShownShape, Option<&ShownHoles>)>,
    mut worn: Query<&mut MeshMaterial3d<ConstructionMaterial>>,
    mut surfaces: SurfaceStores,
    mut commands: Commands,
) {
    // The shared cursor owns visibility as well as streamed poses. The net-motion contract says
    // "Genuine remote-clock resyncs remain discontinuities" (`.agents/docs/design/net-motion-contract.md`).
    // A backward resync can reverse a handover; a local monotone cursor would detach it from poses.
    let due = |tick: u32| clock.since(tick).is_some_and(|elapsed| elapsed >= 0.0);
    let born_with: BTreeSet<(Entity, u32, u16)> = arrived
        .iter()
        .flat_map(|(piece, dressed)| {
            piece
                .chunks
                .iter()
                .map(|&c| (dressed.construction, piece.born, c))
        })
        .collect();
    let handed_over = |piece: &Piece, dressed: &PieceDressed| {
        piece.retired.is_some_and(|tick| {
            due(tick)
                && constructions
                    .get(dressed.construction)
                    .is_ok_and(|(_, shown, _)| {
                        piece.chunks.iter().all(|&c| {
                            shown.0.fate[usize::from(c)] == Fate::Dust
                                || born_with.contains(&(dressed.construction, tick, c))
                        })
                    })
        })
    };
    // Per construction and chunk, the oldest birth tick among the pieces not yet handed over that
    // hold it: the piece drawing that chunk. Births share the presentation clock's signed
    // half-range ordering, so ancestry survives tick wrap.
    let mut drawing: BTreeMap<(Entity, u16), u32> = BTreeMap::new();
    for (piece, dressed) in &arrived {
        if !handed_over(piece, dressed) {
            for &c in &piece.chunks {
                let oldest = drawing
                    .entry((dressed.construction, c))
                    .or_insert(piece.born);
                if (piece.born.wrapping_sub(*oldest) as i32) < 0 {
                    *oldest = piece.born;
                }
            }
        }
    }
    for (entity, piece, dressed, mut visibility, holed) in &mut pieces {
        // A piece whose construction has despawned is hidden.
        let construction = constructions.get(dressed.construction).ok();
        let born = due(piece.born)
            && piece.chunks.iter().all(|&c| {
                drawing
                    .get(&(dressed.construction, c))
                    .is_none_or(|&oldest| oldest == piece.born)
            });
        let retired = handed_over(piece, dressed);
        let fallen = born
            && !retired
            && construction.is_some_and(|(_, shown, _)| {
                piece
                    .chunks
                    .iter()
                    .any(|&c| shown.0.fate.get(usize::from(c)) == Some(&Fate::Fallen))
            });
        visibility.set_if_neq(if fallen {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        let hole_count = construction
            .and_then(|(_, _, holes)| holes)
            .map_or(0, |holes| {
                piece
                    .chunks
                    .iter()
                    .map(|&c| holes.0[usize::from(c)].len())
                    .sum()
            });
        if !fallen || holed.is_some_and(|shown| shown.0 == hole_count) {
            continue;
        }
        let table = construction.and_then(|(breakage, _, holes)| {
            let kind = &breakage.kind;
            piece_hole_table(
                &kind.asset,
                &kind.layout,
                &piece.chunks,
                &holes?.0,
                dressed.origin,
            )
        });
        if table.is_some() {
            let Ok(mut worn) = worn.get_mut(dressed.view) else {
                continue;
            };
            let Some(surface) = surfaces.wear(table) else {
                continue;
            };
            worn.0 = surface;
        }
        commands.entity(entity).insert(PieceHoled(hole_count));
    }
}

/// Off a replica the presentation clock is the sim's own tick and the fixed step's overstep.
fn publish_sim_clock(
    tick: Option<Res<crate::ShotClock>>,
    step: Res<Time<Fixed>>,
    mut clock: ResMut<PresentationClock>,
) {
    let Some(tick) = tick else {
        return; // Tank-less view fixtures keep their explicit cursor.
    };
    *clock = PresentationClock::At {
        tick: tick.0.raw(),
        overstep: step.overstep_fraction_f64(),
    };
}

/// The breaks' view, on every root that draws: the record presented at the presentation clock,
/// the broken constructions redrawn, the fallen groups dressed and shown.
pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<PresentationClock>()
        .add_systems(
            Update,
            publish_sim_clock
                .before(present_record)
                .run_if(not(resource_exists::<crate::ClientReplica>)),
        )
        .add_systems(
            Update,
            (present_record, redraw, dress_pieces, show_pieces)
                .chain()
                .after(ReplicaSet),
        );
}

#[cfg(test)]
mod tests {
    use bevy::mesh::VertexAttributeValues;

    use super::*;
    use crate::construction::fracture::Block;
    use crate::construction::tests::shipped_house;

    /// Every triangle of `mesh`, as its three corners.
    fn triangles(mesh: &Mesh) -> Vec<[Vec3; 3]> {
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions");
        };
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("u32 indices");
        };
        indices
            .chunks(3)
            .map(|t| [0, 1, 2].map(|k| Vec3::from_array(positions[t[k] as usize])))
            .collect()
    }

    /// Whether the segment `from → to` crosses a triangle (Möller–Trumbore, either side).
    fn crosses(triangles: &[[Vec3; 3]], from: Vec3, to: Vec3) -> bool {
        let dir = to - from;
        triangles.iter().any(|&[a, b, c]| {
            let (e1, e2) = (b - a, c - a);
            let p = dir.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-9 {
                return false;
            }
            let s = from - a;
            let u = s.dot(p) / det;
            let q = s.cross(e1);
            let v = dir.dot(q) / det;
            let t = e2.dot(q) / det;
            (0.0..=1.0).contains(&u) && v >= 0.0 && u + v <= 1.0 && (0.0..=1.0).contains(&t)
        })
    }

    /// Appearance must follow the construction record without changing geometry. The view
    /// reader relies on frame data even for a material drawn in the primary palette lane.
    #[test]
    fn authored_grain_changes_the_frame_and_never_the_silhouette() {
        use bevy::mesh::VertexAttributeValues;
        let mut kind = (*shipped_house()).clone();
        let before = mesh(&kind);
        kind.asset.elements[0].surface_frame = SurfaceFrame::Grain([0.0, 0.0, 1.0]);
        let after = mesh(&kind);
        let positions = |mesh: &Mesh| match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            Some(VertexAttributeValues::Float32x3(values)) => values.clone(),
            _ => panic!("positions"),
        };
        let tangents = |mesh: &Mesh| match mesh.attribute(Mesh::ATTRIBUTE_TANGENT) {
            Some(VertexAttributeValues::Float32x4(values)) => values.clone(),
            _ => panic!("tangents"),
        };
        assert_eq!(positions(&before), positions(&after));
        assert_ne!(tangents(&before), tangents(&after));
        kind.asset.elements[0].surface_frame = SurfaceFrame::Grain([f32::NAN, 0.0, 0.0]);
        let error = kind.asset.check().unwrap_err();
        assert!(
            error.contains("surface_frame") && error.contains("author"),
            "{error}"
        );
    }

    /// The camera must see every occupied face of an admitted hull. Fixture: a unit cube with
    /// a redundant face-centre point. Sorting that point into the perimeter cuts a triangular
    /// hole; a face-area check and sight lines through each face expose that mutant.
    #[test]
    fn redundant_hull_points_cannot_open_a_solid_face() {
        let mut asset = shipped_house().asset.clone();
        let mut points = Block::cuboid(Vec3::ZERO, Vec3::ONE).points;
        points.extend([Vec3::new(0.5, 0.5, 1.0), Vec3::splat(0.5)]);
        asset.elements[0].hull = points.iter().map(Vec3::to_array).collect();
        asset.check().expect("the fixture meets schema admission");
        let registry = crate::substances::SubstanceRegistry::shipped_static();
        let masonry = asset
            .materials
            .iter()
            .map(|name| super::super::fracture::Material::of(registry, name).unwrap())
            .collect::<Vec<_>>();
        let layout = Layout::of(
            &asset,
            &masonry,
            crate::construction::attachments::Attachments::shipped_static(),
        )
        .expect("the fixture meets collider admission");
        let mut kind = (*shipped_house()).clone();
        kind.dressing = Dressing::bare(&layout);
        (kind.asset, kind.layout) = (asset, layout);
        let mut faces = Faces::default();
        faces.element(&kind, 0);
        let triangles = triangles(&faces.mesh());
        let area: f32 = triangles
            .iter()
            .map(|[a, b, c]| (b - a).cross(c - a).length() / 2.0)
            .sum();
        assert!((area - 6.0).abs() < 1e-5, "unit cube surface: {area}");
        for (normal, u, v) in [
            (Vec3::X, Vec3::Y, Vec3::Z),
            (Vec3::Y, Vec3::Z, Vec3::X),
            (Vec3::Z, Vec3::X, Vec3::Y),
        ] {
            for sign in [-1.0, 1.0] {
                for x in [-0.4, 0.0, 0.4] {
                    for y in [-0.4, 0.0, 0.4] {
                        let centre = Vec3::splat(0.5) + normal * sign * 0.5 + u * x + v * y;
                        assert!(crosses(
                            &triangles,
                            centre + normal * sign * 0.1,
                            centre - normal * sign * 0.1,
                        ));
                    }
                }
            }
        }
    }

    /// The game's construction destruction view reads crack dimensions from their presentation
    /// owner. Fixture: two unit cubes meet inside one element; scaling the owner's dimensions
    /// must scale each strip's width and lift. A fixed width or standoff fails either choice.
    #[test]
    fn crack_geometry_reads_its_presentation_owner() {
        let mut layout = shipped_house().layout.clone();
        layout.chunks = vec![0, 1];
        layout.element_of = vec![0, 0];
        layout.blocks = vec![
            Block::cuboid(Vec3::ZERO, Vec3::ONE),
            Block::cuboid(Vec3::X, Vec3::new(2.0, 1.0, 1.0)),
        ];
        layout.element_blocks = vec![Block::cuboid(Vec3::ZERO, Vec3::new(2.0, 1.0, 1.0))];
        let mut joint = layout.joints[0];
        joint.a = 0;
        joint.b = Side::Chunk(1);
        joint.area = 1.0;
        joint.centroid = Vec3::new(1.0, 0.5, 0.5);
        joint.normal = Vec3::X;
        layout.joints = vec![joint];
        let mut shape = Shape::intact(&layout);
        shape.cracked[0] = true;
        let owner = crate::vfx::construction_cracks();
        for scale in [0.5, 2.0] {
            let look = crate::vfx::ConstructionCracks {
                half_width_m: owner.half_width_m * scale,
                standoff_m: owner.standoff_m * scale,
                color_srgb: owner.color_srgb,
            };
            let mesh =
                crack_mesh(&layout, &shape, &look).expect("the fixture opens surface cracks");
            let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("crack positions");
            };
            let Some(VertexAttributeValues::Float32x3(normals)) =
                mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
            else {
                panic!("crack normals");
            };
            let (quads, remainder) = positions.as_chunks::<4>();
            assert!(remainder.is_empty(), "each strip has four corners");
            for (k, quad) in quads.iter().enumerate() {
                let points: [Vec3; 4] = std::array::from_fn(|i| Vec3::from_array(quad[i]));
                let width = points[3].distance(points[0]);
                assert!(
                    (width - 2.0 * look.half_width_m).abs() < 1e-6,
                    "width {width}"
                );
                let normal = Vec3::from_array(normals[k * 4]);
                let (_, offset) = layout.element_blocks[0]
                    .planes
                    .iter()
                    .find(|(n, _)| n.dot(normal) > 0.99)
                    .expect("a strip lies over an element face");
                let centre = points.iter().sum::<Vec3>() / 4.0;
                let lift = normal.dot(centre) - offset;
                assert!((lift - look.standoff_m).abs() < 1e-6, "lift {lift}");
            }
        }
    }

    /// A LEVEL FACE RUNS ALONG ITS ELEMENT, NEVER ALONG A WORLD AXIS: an element's run is its
    /// hull's edge longest seen from above, so a box 2 m long, 1 m tall and 0.25 m thick (fixture
    /// data), yawed off every world axis, gives a level face drawn with its look, on the mesh path
    /// (`Faces::face`), a unit tangent in the face along its length. A run read from a world axis,
    /// or from the shortest edge, draws a yawed sill's bricks across it.
    #[test]
    fn a_level_face_runs_along_its_element() {
        for yaw in [0.0_f32, 0.7, 1.57, 2.5] {
            let heading = Quat::from_rotation_y(yaw);
            let length = heading * Vec3::X;
            let mut corners = Vec::new();
            for x in [-1.0, 1.0] {
                for y in [0.0, 1.0] {
                    for z in [-0.125, 0.125] {
                        corners.push(heading * Vec3::new(x, y, z));
                    }
                }
            }
            let look = ElementLook {
                frame: SurfaceFrame::Upright,
                run: run(&corners),
                secondary: false,
            };
            for n in [Vec3::Y, Vec3::NEG_Y] {
                let mut faces = Faces::default();
                faces.face(
                    n,
                    &[Vec3::ZERO, Vec3::Z, Vec3::X],
                    &look,
                    Vec3::ZERO,
                    (0.0, UNTINTED),
                );
                let t = Vec4::from_array(faces.tangents[0]).truncate();
                assert!((t.length() - 1.0).abs() < 1e-6 && t.dot(n).abs() < 1e-6);
                assert!(t.dot(length).abs() > 0.999, "yaw {yaw}: {t} along {length}");
            }
        }
    }

    /// An unsynchronized replica must hold actual ledger entries. Checking only the clock's
    /// accessor would let a presentation predicate that treats Waiting as due escape.
    #[test]
    fn a_waiting_replica_presents_no_record_entry() {
        use crate::construction::breakage::Entry;
        let kind = shipped_house();
        let mut world = World::new();
        world.insert_resource(PresentationClock::Waiting);
        world.insert_resource(Record {
            entries: vec![Entry {
                construction: 3,
                event: Event::Swap(0),
                tick: 0,
            }],
            ..default()
        });
        let root = world
            .spawn((
                MapInstance {
                    id: "fixture".to_owned(),
                    ordinal: 3,
                },
                Breakage::new(kind, 3, 1.0),
            ))
            .id();
        let mut schedule = Schedule::default();
        schedule.add_systems(present_record);
        schedule.run(&mut world);
        assert!(world.get::<ShownShape>(root).is_none());
        *world.resource_mut::<PresentationClock>() = PresentationClock::At {
            tick: 0,
            overstep: 0.0,
        };
        schedule.run(&mut world);
        assert!(
            world
                .get::<ShownShape>(root)
                .expect("the synchronized cursor releases the entry")
                .0
                .swapped[0]
        );
    }

    /// Even unpresented future entries are append-only. Presentation may reset only on a new
    /// generation; treating a shortening as a reset would hide an internal record regression.
    #[test]
    #[should_panic(expected = "a construction record never shrinks within its generation")]
    fn a_same_generation_record_cannot_shrink_before_presentation() {
        use crate::construction::breakage::Entry;
        let mut world = World::new();
        world.insert_resource(PresentationClock::Waiting);
        world.insert_resource(Record {
            entries: vec![Entry {
                construction: 3,
                event: Event::Swap(0),
                tick: 9,
            }],
            ..default()
        });
        let mut schedule = Schedule::default();
        schedule.add_systems(present_record);
        schedule.run(&mut world);
        world.resource_mut::<Record>().entries.clear();
        schedule.run(&mut world);
    }

    /// An opening is absent construction: the camera sees through it and meets the surrounding
    /// solids. Fixture geometry is in metres, independent of shipped window placement.
    #[test]
    fn a_window_is_a_hole_and_a_pier_is_solid() {
        use crate::construction::schema;
        // Fixture: a wall with an off-centre rectangular opening, one chunk per solid.
        let size = Vec3::new(4.0, 3.0, 0.25);
        let opening_low = Vec3::new(1.0, 0.8, 0.0);
        let opening_high = Vec3::new(2.5, 2.2, size.z);
        let solids = [
            (Vec3::ZERO, Vec3::new(opening_low.x, size.y, size.z), "pier"),
            (Vec3::new(opening_high.x, 0.0, 0.0), size, "pier"),
            (
                Vec3::new(opening_low.x, 0.0, 0.0),
                Vec3::new(opening_high.x, opening_low.y, size.z),
                "sill",
            ),
            (
                Vec3::new(opening_low.x, opening_high.y, 0.0),
                Vec3::new(opening_high.x, size.y, size.z),
                "lintel",
            ),
        ];
        let mut asset = ConstructionAsset {
            id: "opening_fixture".into(),
            format: schema::FORMAT,
            foundation_y_m: 0.0,
            materials: vec!["Brick".into()],
            attachments: Vec::new(),
            components: vec!["wall".into()],
            elements: Vec::new(),
            chunks: Vec::new(),
            joints: Vec::new(),
            anchors: Vec::new(),
        };
        let density = schema::substance_density(&asset.materials[0]).unwrap() as f32;
        for (index, (low, high, part)) in solids.into_iter().enumerate() {
            let hull: Vec<_> = Block::cuboid(low, high)
                .points
                .iter()
                .map(Vec3::to_array)
                .collect();
            let span = high - low;
            let volume = span.x * span.y * span.z;
            asset.elements.push(schema::Element {
                name: format!("solid_{index}"),
                part: part.to_owned(),
                component: 0,
                material: 0,
                surface_frame: SurfaceFrame::Upright,
                hull: hull.clone(),
                chunks: [index as u32, index as u32 + 1],
            });
            asset.chunks.push(schema::Chunk {
                element: index as u16,
                material: 0,
                hull,
                volume_m3: volume,
                centroid: ((low + high) / 2.0).to_array(),
                mass_kg: density * volume,
            });
        }
        let kind = crate::construction::ConstructionKind::resolve(
            &asset.id,
            &ron::ser::to_string(&asset).unwrap(),
            crate::substances::SubstanceRegistry::shipped_static(),
            crate::construction::attachments::Attachments::shipped_static(),
        )
        .unwrap();
        let triangles = triangles(&mesh(&kind));
        let centre = (opening_low + opening_high) / 2.0;
        let across = |x, y| {
            crosses(
                &triangles,
                Vec3::new(x, y, -size.z),
                Vec3::new(x, y, 2.0 * size.z),
            )
        };
        assert!(!across(centre.x, centre.y), "the window is open");
        for (x, y) in [
            (opening_low.x / 2.0, centre.y),
            ((opening_high.x + size.x) / 2.0, centre.y),
            (centre.x, opening_low.y / 2.0),
            (centre.x, (opening_high.y + size.y) / 2.0),
        ] {
            assert!(across(x, y), "the surrounding solid is drawn at {x}, {y}");
        }
    }

    /// The material reader requires metre UVs in each element's declared frame and its own
    /// palette assignment. Fixture frame choices exercise grain on either palette lane and
    /// upright mapping without requiring any particular lane to occur in the construction.
    #[test]
    fn every_face_is_framed_in_metres_and_wears_its_element_s_pack() {
        use crate::construction_surface::{SurfaceLane, lane_for_substance};
        let mut kind = (*shipped_house()).clone();
        for e in 0..kind.asset.elements.len() {
            let declared = kind.asset.elements[e].surface_frame;
            let palette = match lane_for_substance(
                &kind.asset.materials[usize::from(kind.asset.elements[e].material)],
            ) {
                SurfaceLane::Primary => 0.0,
                SurfaceLane::Secondary => 1.0,
            };
            for frame in [
                declared,
                SurfaceFrame::Upright,
                SurfaceFrame::Grain([1.0, 0.0, 0.0]),
                SurfaceFrame::Grain([0.0, 0.0, 1.0]),
            ] {
                kind.asset.elements[e].surface_frame = frame;
                let mut faces = Faces::default();
                faces.element(&kind, e);
                let mesh = faces.mesh();
                assert_tagged_hulls(&mesh, &[(0.0, &kind.layout.element_blocks[e])], Vec3::ZERO);
                let attribute = |id| match mesh.attribute(id) {
                    Some(VertexAttributeValues::Float32x3(values)) => values
                        .iter()
                        .copied()
                        .map(Vec3::from_array)
                        .collect::<Vec<_>>(),
                    _ => panic!("Float32x3"),
                };
                let positions = attribute(Mesh::ATTRIBUTE_POSITION);
                let normals = attribute(Mesh::ATTRIBUTE_NORMAL);
                let Some(VertexAttributeValues::Float32x4(tangents)) =
                    mesh.attribute(Mesh::ATTRIBUTE_TANGENT)
                else {
                    panic!("tangents");
                };
                let Some(VertexAttributeValues::Float32x2(uvs)) =
                    mesh.attribute(Mesh::ATTRIBUTE_UV_0)
                else {
                    panic!("uvs");
                };
                let Some(VertexAttributeValues::Float32x2(packs)) =
                    mesh.attribute(Mesh::ATTRIBUTE_UV_1)
                else {
                    panic!("packs");
                };
                assert_eq!(positions.len(), normals.len());
                assert_eq!(positions.len(), tangents.len());
                assert_eq!(positions.len(), uvs.len());
                assert_eq!(positions.len(), packs.len());
                for k in 0..positions.len() {
                    let (p, n, tangent) = (positions[k], normals[k], Vec4::from_array(tangents[k]));
                    let t = tangent.truncate();
                    let b = tangent.w * n.cross(t);
                    let uv = Vec2::from_array(uvs[k]);
                    assert!((uv - Vec2::new(p.dot(t), -p.dot(b))).length() < 1e-4);
                    assert!((t.length() - 1.0).abs() < 1e-4 && t.dot(n).abs() < 1e-4);
                    let grain = match frame {
                        SurfaceFrame::Grain(axis) => {
                            let axis = Vec3::from_array(axis);
                            Some(axis - n * axis.dot(n))
                        }
                        SurfaceFrame::Upright => None,
                    };
                    if let Some(grain) =
                        grain.filter(|g| g.length_squared() > FRAME_PROJECTION_MIN_SQUARED)
                    {
                        assert!(
                            (t - grain.normalize()).length() < 1e-4,
                            "element {e}: authored grain"
                        );
                    } else {
                        let up = Vec3::Y - n * n.y;
                        if up.length_squared() > FRAME_PROJECTION_MIN_SQUARED {
                            assert!(
                                (b - up.normalize()).length() < 1e-4,
                                "element {e}: upright image"
                            );
                        }
                    }
                    assert_eq!(
                        packs[k],
                        [palette, 0.0],
                        "element {e}: palette and whole-element tag"
                    );
                }
            }
        }
    }

    /// Every vertex's second UV `y`, the hole lookup's tag.
    fn tags(mesh: &Mesh) -> Vec<f32> {
        let Some(VertexAttributeValues::Float32x2(packs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_1)
        else {
            panic!("packs");
        };
        packs.iter().map(|pack| pack[1]).collect()
    }

    /// The first element the shipped house's layout splits into two chunks or more, and its chunks.
    fn chunked_element(layout: &Layout) -> (usize, std::ops::Range<usize>) {
        (0..layout.element_of.len())
            .map(|e| (e, layout.chunks_of(e)))
            .find(|(_, chunks)| chunks.len() >= 2)
            .expect("the shipped house has a chunked element")
    }

    /// The shader's tags name authored solids, irrespective of the renderer's vertex topology.
    /// Every triangle must lie on its tagged hull, and every hull plane must remain represented.
    fn assert_tagged_hulls(mesh: &Mesh, hulls: &[(f32, &Block)], origin: Vec3) {
        use crate::construction::schema::PLANE_TOLERANCE_M;
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions");
        };
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("u32 indices");
        };
        let tags = tags(mesh);
        assert_eq!(positions.len(), tags.len());
        let mut seen: Vec<_> = hulls
            .iter()
            .map(|(_, block)| vec![false; block.planes.len()])
            .collect();
        let (triangles, remainder) = indices.as_chunks::<3>();
        assert!(
            remainder.is_empty(),
            "triangle indices form complete triples"
        );
        for triangle in triangles {
            let points: [Vec3; 3] =
                std::array::from_fn(|k| Vec3::from_array(positions[triangle[k] as usize]) + origin);
            let tag = tags[triangle[0] as usize];
            assert!(
                triangle.iter().all(|&i| tags[i as usize] == tag),
                "one tag per triangle"
            );
            let area_vector = (points[1] - points[0]).cross(points[2] - points[0]);
            let mut owned = false;
            for (h, (expected, block)) in hulls.iter().enumerate() {
                if tag != *expected
                    || points.iter().any(|p| {
                        block
                            .planes
                            .iter()
                            .any(|(n, offset)| n.dot(*p) - offset > PLANE_TOLERANCE_M)
                    })
                {
                    continue;
                }
                for (f, (n, offset)) in block.planes.iter().enumerate() {
                    if area_vector.dot(*n) > 0.0
                        && points
                            .iter()
                            .all(|p| (n.dot(*p) - offset).abs() < PLANE_TOLERANCE_M)
                    {
                        seen[h][f] = true;
                        owned = true;
                    }
                }
            }
            assert!(
                owned,
                "triangle {points:?} belongs to no authored hull carrying tag {tag}"
            );
        }
        for (h, faces) in seen.iter().enumerate() {
            assert!(
                !faces.is_empty() && faces.iter().all(|&drawn| drawn),
                "every plane of tagged hull {h} is drawn"
            );
        }
    }

    /// A FRAGMENT FINDS ITS OWN CHUNK'S HOLES. The shader reads a face's hole header at its tag − 1,
    /// so every vertex of a swapped chunk's hull carries that chunk's index + 1, every vertex of a
    /// fallen piece's `k`-th chunk `k + 1` (its [`piece_hole_table`]'s header), and every whole
    /// element's vertex 0, which reads no hole. A tag off by one draws a hole in the neighbouring
    /// chunk; a tag on a whole element reads a header that is not its own.
    #[test]
    fn a_swapped_chunk_s_faces_carry_its_index_and_nothing_else_carries_one() {
        let kind = shipped_house();
        let layout = &kind.layout;
        let (e, chunks) = chunked_element(layout);
        let mut shape = Shape::intact(layout);
        let intact: Vec<_> = layout
            .element_blocks
            .iter()
            .map(|block| (0.0, block))
            .collect();
        assert_tagged_hulls(&mesh(&kind), &intact, Vec3::ZERO);
        shape.swapped[e] = true;
        let mut standing: Vec<_> = layout
            .element_blocks
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != e)
            .map(|(_, block)| (0.0, block))
            .collect();
        standing.extend(chunks.clone().map(|c| ((c + 1) as f32, &layout.blocks[c])));
        assert_tagged_hulls(&broken_mesh(&kind, &shape), &standing, Vec3::ZERO);
        let order = [chunks.start + 1, chunks.start];
        // Fixture: a nonzero origin also checks that ownership uses the authored frame.
        let origin = Vec3::new(0.7, 1.1, -0.4);
        let piece = piece_mesh(&kind, &order.map(|c| c as u16), origin);
        let fallen: Vec<_> = order
            .iter()
            .enumerate()
            .map(|(k, &c)| ((k + 1) as f32, &layout.blocks[c]))
            .collect();
        assert_tagged_hulls(&piece, &fallen, origin);
    }

    /// THE TABLE IS WHAT THE SHADER WALKS (`construction_holes.wgsl`): chunk `c`'s header at record `c`
    /// names where its records start, how many holes and how many planes follow; each hole is its
    /// capsule in mesh-local metres (the chunk's centroid plus the record's millimetres), and its
    /// hull's planes follow. A chunk no longer standing, or holding no hole, has an empty header.
    /// At most [`HOLES_PER_CHUNK`] holes are written and the header counts exactly those, or the
    /// shader reads its planes from the wrong records. The holes are ASSUMED fixture data, set
    /// directly rather than cut by law.
    #[test]
    fn the_hole_table_carries_each_standing_chunk_s_holes_and_hull() {
        let kind = shipped_house();
        let (asset, layout) = (&kind.asset, &kind.layout);
        let (e, chunks) = chunked_element(layout);
        let (c, dust) = (chunks.start, chunks.start + 1);
        let mut shape = Shape::intact(layout);
        shape.swapped[e] = true;
        let mut holes = vec![Vec::new(); layout.chunks.len()];
        assert_eq!(hole_table(asset, layout, &shape, &holes), None, "no hole");

        let channel = Hole {
            from: [-120, 40, 0],
            to: [-120, 40, 250],
            radius: 60,
        };
        let dish = Hole {
            from: [300, -5, 125],
            to: [300, -5, 125],
            radius: 210,
        };
        holes[c] = vec![channel, dish];
        holes[dust] = vec![channel];
        shape.fate[dust] = Fate::Dust;
        let table = hole_table(asset, layout, &shape, &holes).expect("a standing hole");
        let planes = &layout.blocks[c].planes;
        let n = layout.chunks.len();
        assert_eq!(table[c], Vec4::new(n as f32, 2.0, planes.len() as f32, 0.0));
        assert_eq!(
            table[dust],
            Vec4::ZERO,
            "a chunk turned to dust shows no hole"
        );
        assert_eq!(
            table[..n]
                .iter()
                .filter(|header| **header != Vec4::ZERO)
                .count(),
            1
        );
        let centroid = Vec3::from_array(asset.chunks[layout.chunks[c] as usize].centroid);
        let metres = |mm: [i16; 3]| centroid + Vec3::from_array(mm.map(f32::from)) / 1000.0;
        for (k, hole) in [channel, dish].iter().enumerate() {
            let (start, end) = (table[n + 2 * k], table[n + 2 * k + 1]);
            assert!((start.truncate() - metres(hole.from)).length() < 1e-6);
            assert!((end.truncate() - metres(hole.to)).length() < 1e-6);
            assert_eq!(start.w, f32::from(hole.radius) / 1000.0);
        }
        let hull: Vec<Vec4> = planes.iter().map(|(n, o)| n.extend(*o)).collect();
        assert_eq!(
            table[n + 4..],
            hull[..],
            "the hull's planes follow the holes"
        );

        holes[c] = vec![channel; HOLES_PER_CHUNK + 1];
        let table = hole_table(asset, layout, &shape, &holes).expect("a standing hole");
        assert_eq!(table[c].y, HOLES_PER_CHUNK as f32);
        assert_eq!(table.len(), n + 2 * HOLES_PER_CHUNK + planes.len());
    }

    /// THE SHADER'S BOUND IS THE RECORD'S: `construction_holes.wgsl` sizes its per-fragment spans by
    /// its own `HOLES_PER_CHUNK`, so a record cap above it would leave holes undrawn.
    #[test]
    fn the_shader_s_hole_cap_is_the_record_s() {
        let shader = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/shaders/construction_holes.wgsl"),
        )
        .expect("the hole shader");
        assert!(
            shader.contains(&format!("const HOLES_PER_CHUNK: u32 = {HOLES_PER_CHUNK}u;")),
            "the shader's cap is breakage::HOLES_PER_CHUNK"
        );
    }

    /// A world holding the stores a view draws from and the shared material ([`Look`]),
    /// whose handle it returns.
    fn shared_look(world: &mut World) -> Handle<ConstructionMaterial> {
        world.init_resource::<PresentationClock>();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<ConstructionMaterial>>();
        world.init_resource::<Assets<ShaderBuffer>>();
        let shared_buffer = world
            .resource_mut::<Assets<ShaderBuffer>>()
            .add(crate::construction_surface::no_holes());
        let shared =
            world
                .resource_mut::<Assets<ConstructionMaterial>>()
                .add(ConstructionMaterial {
                    base: StandardMaterial::default(),
                    extension: crate::construction_surface::Surface {
                        params: default(),
                        secondary_albedo: default(),
                        secondary_normal: default(),
                        secondary_arm: default(),
                        holes: shared_buffer,
                    },
                });
        world.insert_resource(Look {
            surface: shared.clone(),
        });
        shared
    }

    /// A pose can precede its Battle snapshot. Dressing it against the old construction would
    /// retain a departed construction root forever, hiding valid current debris from the player.
    #[test]
    fn a_streamed_piece_resolves_its_construction_only_in_its_own_world() {
        use crate::world::{CurrentWorld, WorldGeneration};
        // ASSUMED fixture identities and generation numbers; geometry comes from asset data.
        let mut world = World::new();
        shared_look(&mut world);
        world.insert_resource(CurrentWorld(None));
        let kind = shipped_house();
        let (_, chunks) = chunked_element(&kind.layout);
        let old = world
            .spawn((
                MapInstance {
                    id: "fixture_construction".to_owned(),
                    ordinal: 3,
                },
                Breakage::new(kind.clone(), 3, 1.0),
            ))
            .id();
        let carrier = world
            .spawn((
                Piece {
                    construction: 3,
                    chunks: vec![chunks.start as u16],
                    born: 0,
                    retired: None,
                },
                WorldGeneration(2),
                Transform::IDENTITY,
            ))
            .id();
        let mut schedule = Schedule::default();
        schedule.add_systems(dress_pieces);
        schedule.run(&mut world);
        assert!(
            world.get::<PieceDressed>(carrier).is_none(),
            "no authority construction selected"
        );
        world.resource_mut::<CurrentWorld>().0 = Some(WorldGeneration(1));
        schedule.run(&mut world);
        assert!(
            world.get::<PieceDressed>(carrier).is_none(),
            "the future piece waits"
        );
        world.despawn(old);
        let fresh = world
            .spawn((
                MapInstance {
                    id: "fixture_construction".to_owned(),
                    ordinal: 3,
                },
                Breakage::new(kind, 3, 1.0),
            ))
            .id();
        world.resource_mut::<CurrentWorld>().0 = Some(WorldGeneration(2));
        let admitted = world
            .get::<Piece>(carrier)
            .unwrap()
            .admitted(Some((fresh, world.get::<Breakage>(fresh).unwrap())))
            .unwrap();
        world.entity_mut(carrier).insert(admitted);

        schedule.run(&mut world);
        assert_eq!(
            world.get::<PieceDressed>(carrier).unwrap().construction,
            fresh
        );
    }

    /// A HOLE IS DRAWN BY ITS CONSTRUCTION'S OWN MATERIAL, AND ONLY WHILE IT SHOWS: a construction whose
    /// shown holes stand wears a copy of the shared material, alpha-masked (the variant whose
    /// shader and shadow pass test holes) and binding its table; one with none wears the shared
    /// material, so every construction without a hole keeps the opaque path. The hole is ASSUMED
    /// fixture data.
    #[test]
    fn a_construction_with_a_hole_wears_its_own_masked_material_until_the_hole_goes() {
        let kind = shipped_house();
        let mut world = World::new();
        let shared = shared_look(&mut world);
        let (e, chunks) = chunked_element(&kind.layout);
        let mut shape = Shape::intact(&kind.layout);
        shape.swapped[e] = true;
        let mut holes = ShownHoles(vec![Vec::new(); kind.layout.chunks.len()]);
        holes.0[chunks.start].push(Hole {
            from: [0, 0, 0],
            to: [0, 0, 0],
            radius: 100,
        });
        let root = world
            .spawn((
                Breakage::new(kind.clone(), 3, 1.0),
                ShownShape(shape),
                holes,
            ))
            .id();
        let view = world
            .spawn((View, MeshMaterial3d(shared.clone()), ChildOf(root)))
            .id();
        let mut schedule = Schedule::default();
        schedule.add_systems(redraw);
        schedule.run(&mut world);
        let worn = world
            .get::<MeshMaterial3d<ConstructionMaterial>>(view)
            .unwrap()
            .0
            .clone();
        assert_ne!(worn, shared, "a holed construction wears its own material");
        let material = world
            .resource::<Assets<ConstructionMaterial>>()
            .get(&worn)
            .unwrap();
        assert_eq!(material.base.alpha_mode, AlphaMode::Mask(0.0));
        let table = hole_table(
            &kind.asset,
            &kind.layout,
            &world.get::<ShownShape>(root).unwrap().0,
            &world.get::<ShownHoles>(root).unwrap().0,
        )
        .unwrap();
        let bound = world
            .resource::<Assets<ShaderBuffer>>()
            .get(&material.extension.holes)
            .unwrap();
        assert_eq!(
            bound.data,
            ShaderBuffer::from(table).data,
            "it binds the construction's table"
        );

        world.get_mut::<ShownHoles>(root).unwrap().0 = vec![Vec::new(); kind.layout.chunks.len()];
        schedule.run(&mut world);
        let worn = &world
            .get::<MeshMaterial3d<ConstructionMaterial>>(view)
            .unwrap()
            .0;
        assert_eq!(
            *worn, shared,
            "with no hole it wears the shared material again"
        );
    }

    /// A FALLEN PIECE KEEPS ITS CHUNKS' HOLES (a hole is matter missing from its chunk): once its
    /// chunks show fallen, a piece one of whose chunks holds a hole wears its own masked material
    /// binding its [`piece_hole_table`], the capsule and the hull's planes drawn about the piece's
    /// origin as its mesh is, so every vertex of the mesh lies on the hull the shader tests; a
    /// piece of hole-free chunks keeps the shared material. A view that dropped a falling chunk's
    /// holes, or drew them about the construction's frame instead of the piece's, fails here. The
    /// fates and the hole are ASSUMED fixture state, set directly rather than reached by law.
    #[test]
    fn a_fallen_piece_wears_its_chunks_holes() {
        let kind = shipped_house();
        let (asset, layout) = (&kind.asset, &kind.layout);
        let mut world = World::new();
        let shared = shared_look(&mut world);
        let (_, chunks) = chunked_element(layout);
        let (holed, plain) = (chunks.start, chunks.start + 1);
        let mut shape = Shape::intact(layout);
        shape.fate[holed] = Fate::Fallen;
        shape.fate[plain] = Fate::Fallen;
        let hole = Hole {
            from: [0, -60, 0],
            to: [0, 60, 0],
            radius: 40,
        };
        let mut holes = ShownHoles(vec![Vec::new(); layout.chunks.len()]);
        holes.0[holed].push(hole);
        world.spawn((
            MapInstance {
                id: "house_0001".to_owned(),
                ordinal: 3,
            },
            Breakage::new(kind.clone(), 3, 1.0),
            ShownShape(shape),
            holes.clone(),
        ));
        let piece = |world: &mut World, c: usize| {
            world
                .spawn((
                    Piece {
                        construction: 3,
                        chunks: vec![c as u16],
                        born: 0,
                        retired: None,
                    },
                    Transform::IDENTITY,
                ))
                .id()
        };
        let (holed_piece, plain_piece) = (piece(&mut world, holed), piece(&mut world, plain));
        let mut schedule = Schedule::default();
        schedule.add_systems((dress_pieces, show_pieces).chain());
        schedule.run(&mut world);
        schedule.run(&mut world);

        let worn = |world: &World, piece: Entity| {
            let dressed = world.get::<PieceDressed>(piece).expect("dressed");
            assert_eq!(world.get::<Visibility>(piece), Some(&Visibility::Inherited));
            let view = dressed.view;
            let material = world
                .get::<MeshMaterial3d<ConstructionMaterial>>(view)
                .unwrap();
            let mesh = world.get::<Mesh3d>(view).unwrap();
            (material.0.clone(), mesh.0.clone(), dressed.origin)
        };
        assert_eq!(worn(&world, plain_piece).0, shared, "no hole: shared");
        let (material, mesh, origin) = worn(&world, holed_piece);
        assert_ne!(material, shared, "a holed piece wears its own material");
        let material = world
            .resource::<Assets<ConstructionMaterial>>()
            .get(&material)
            .unwrap();
        assert_eq!(material.base.alpha_mode, AlphaMode::Mask(0.0));
        let table = piece_hole_table(asset, layout, &[holed as u16], &holes.0, origin).unwrap();
        let bound = world
            .resource::<Assets<ShaderBuffer>>()
            .get(&material.extension.holes)
            .unwrap();
        assert_eq!(bound.data, ShaderBuffer::from(table.clone()).data);

        let centroid = Vec3::from_array(asset.chunks[layout.chunks[holed] as usize].centroid);
        assert_eq!(
            table[0],
            Vec4::new(1.0, 1.0, layout.blocks[holed].planes.len() as f32, 0.0)
        );
        assert!(
            (table[1] - (centroid - origin + Vec3::new(0.0, -0.06, 0.0)).extend(0.04)).length()
                < 1e-6
        );
        let planes = &table[3..];
        let Some(VertexAttributeValues::Float32x3(positions)) = world
            .resource::<Assets<Mesh>>()
            .get(&mesh)
            .unwrap()
            .attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions");
        };
        for p in positions.iter().map(|p| Vec3::from_array(*p)) {
            let out = planes
                .iter()
                .map(|plane| plane.truncate().dot(p) - plane.w)
                .fold(f32::MIN, f32::max);
            assert!(
                out.abs() < 1e-4,
                "vertex {p} off the hull the shader tests by {out}"
            );
        }
    }

    /// NO EMPTY MESH REACHES A RENDER ENTITY: a construction with nothing left standing holds no
    /// `Mesh3d` and leaves no zero-vertex mesh asset (see
    /// [`crate::track::shadow_proxy::ProxySide::mesh`]), and a record reset that stands it again gives its view a mesh
    /// back. The all-gone shape is ASSUMED fixture state, set directly rather than reached by law.
    #[test]
    fn a_construction_with_nothing_standing_draws_no_mesh_until_it_stands_again() {
        let kind = shipped_house();
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        let shared = world.resource_mut::<Assets<Mesh>>().add(mesh(&kind));
        let intact = Shape::intact(&kind.layout);
        let root = world
            .spawn((Breakage::new(kind, 3, 1.0), ShownShape(intact.clone())))
            .id();
        let view = world.spawn((View, Mesh3d(shared), ChildOf(root))).id();
        let mut schedule = Schedule::default();
        schedule.add_systems(redraw);
        let mut show = |world: &mut World, shape: &Shape| {
            world.get_mut::<ShownShape>(root).unwrap().0 = shape.clone();
            schedule.run(world);
            let meshes = world.resource::<Assets<Mesh>>();
            assert!(
                meshes.iter().all(|(_, mesh)| mesh.count_vertices() > 0),
                "a zero-vertex mesh asset"
            );
            world
                .get::<Mesh3d>(view)
                .map(|drawn| meshes.get(&drawn.0).map_or(0, Mesh::count_vertices))
        };

        assert!(show(&mut world, &intact).is_some_and(|v| v > 0));
        let gone = Shape {
            swapped: vec![true; intact.swapped.len()],
            fate: vec![Fate::Dust; intact.fate.len()],
            cracked: intact.cracked.clone(),
        };
        assert_eq!(show(&mut world, &gone), None, "nothing stands: no mesh");
        assert!(
            show(&mut world, &intact).is_some_and(|v| v > 0),
            "a reset stands the construction again"
        );
    }
    /// A split replaces the old carrier at the shared cursor, after all replacement views
    /// have arrived. A late child must not make occupied geometry disappear early.
    #[test]
    fn a_split_waits_for_its_tick_and_every_replacement() {
        for offset in [0, u32::MAX - 10] {
            split_handover(offset);
        }
    }

    fn split_handover(offset: u32) {
        let kind = shipped_house();
        let (_, range) = chunked_element(&kind.layout);
        let chunks: Vec<_> = range.take(2).map(|c| c as u16).collect();
        assert_eq!(chunks.len(), 2);
        let mut world = World::new();
        shared_look(&mut world);
        *world.resource_mut::<PresentationClock>() = PresentationClock::At {
            tick: offset.wrapping_add(19),
            overstep: 0.0,
        };
        let mut shape = Shape::intact(&kind.layout);
        for &c in &chunks {
            shape.fate[usize::from(c)] = Fate::Fallen;
        }
        world.spawn((
            MapInstance {
                id: "fixture".to_owned(),
                ordinal: 3,
            },
            Breakage::new(kind, 3, 1.0),
            ShownShape(shape),
        ));
        let parent = world
            .spawn((
                Piece {
                    construction: 3,
                    chunks: chunks.clone(),
                    born: offset,
                    retired: Some(offset.wrapping_add(20)),
                },
                Transform::IDENTITY,
            ))
            .id();
        let a = world
            .spawn((
                Piece {
                    construction: 3,
                    chunks: vec![chunks[0]],
                    born: offset.wrapping_add(20),
                    retired: None,
                },
                Transform::IDENTITY,
            ))
            .id();
        let mut schedule = Schedule::default();
        schedule.add_systems((dress_pieces, show_pieces).chain());
        schedule.run(&mut world);
        assert_eq!(
            world.get::<Visibility>(parent),
            Some(&Visibility::Inherited)
        );
        assert_eq!(world.get::<Visibility>(a), Some(&Visibility::Hidden));
        *world.resource_mut::<PresentationClock>() = PresentationClock::At {
            tick: offset.wrapping_add(20),
            overstep: 0.0,
        };
        schedule.run(&mut world);
        assert_eq!(
            world.get::<Visibility>(parent),
            Some(&Visibility::Inherited),
            "the late replacement keeps the parent available"
        );
        assert_eq!(
            world.get::<Visibility>(a),
            Some(&Visibility::Hidden),
            "an arrived replacement waits while its parent still draws its chunks"
        );
        let b = world
            .spawn((
                Piece {
                    construction: 3,
                    chunks: vec![chunks[1]],
                    born: offset.wrapping_add(20),
                    retired: None,
                },
                Transform::IDENTITY,
            ))
            .id();
        schedule.run(&mut world);
        assert_eq!(world.get::<Visibility>(parent), Some(&Visibility::Hidden));
        assert_eq!(world.get::<Visibility>(a), Some(&Visibility::Inherited));
        assert_eq!(world.get::<Visibility>(b), Some(&Visibility::Inherited));
        // Fixture backward resync: visibility follows the same discontinuity as the carrier poses.
        *world.resource_mut::<PresentationClock>() = PresentationClock::At {
            tick: offset.wrapping_add(19),
            overstep: 0.0,
        };
        schedule.run(&mut world);
        assert_eq!(
            world.get::<Visibility>(parent),
            Some(&Visibility::Inherited)
        );
        assert_eq!(world.get::<Visibility>(a), Some(&Visibility::Hidden));
        assert_eq!(world.get::<Visibility>(b), Some(&Visibility::Hidden));
        *world.resource_mut::<PresentationClock>() = PresentationClock::At {
            tick: offset.wrapping_add(20),
            overstep: 0.0,
        };
        schedule.run(&mut world);
        assert_eq!(world.get::<Visibility>(parent), Some(&Visibility::Hidden));
        assert_eq!(world.get::<Visibility>(a), Some(&Visibility::Inherited));
        assert_eq!(world.get::<Visibility>(b), Some(&Visibility::Inherited));
    }

    /// The shipped house with a fixture dressing: one triangular card grown from each of chunks
    /// `owners`' centroids, its corners 0.3 m out along a distinct axis per card (fixture data).
    /// Returns the kind and each card's first corner, asset frame.
    fn carded_house(owners: &[usize]) -> (ConstructionKind, Vec<Vec3>) {
        let mut kind = (*shipped_house()).clone();
        let mut cards = String::new();
        let mut firsts = Vec::new();
        for (k, &c) in owners.iter().enumerate() {
            let base = Vec3::from_array(kind.asset.chunks[kind.layout.chunks[c] as usize].centroid);
            let out = Vec3::new(0.3, 0.05 * k as f32, 0.0);
            let corners = [base, base + out, base + out + Vec3::Y * 0.2];
            firsts.push(corners[0]);
            let point = |p: Vec3| format!("({}, {}, {})", p.x, p.y, p.z);
            cards += &format!(
                "(base: {}, corners: [{}]),",
                point(base),
                corners.map(point).join(", ")
            );
        }
        let text = format!("(skin: [], cards: [{cards}])");
        kind.dressing = Dressing::resolve(&text, &kind.asset, &kind.layout).expect("fixture cards");
        for (card, &c) in kind.dressing.cards.iter().zip(owners) {
            assert_eq!(
                card.owner, c,
                "a card belongs to the chunk holding its base"
            );
        }
        (kind, firsts)
    }

    /// How many times each card in `firsts` is drawn by `mesh`, drawn `origin` back from the
    /// asset frame: card-tinted vertices at its first corner, two per drawn card (both sides).
    fn cards_in(mesh: &Mesh, firsts: &[Vec3], origin: Vec3) -> Vec<usize> {
        let card = tint(crate::vfx::construction_dressing().card_srgb);
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions");
        };
        let Some(VertexAttributeValues::Float32x4(colors)) = mesh.attribute(Mesh::ATTRIBUTE_COLOR)
        else {
            panic!("colours");
        };
        firsts
            .iter()
            .map(|first| {
                let sides = positions
                    .iter()
                    .zip(colors)
                    .filter(|(p, color)| {
                        **color == card && (Vec3::from_array(**p) + origin).distance(*first) < 1e-5
                    })
                    .count();
                assert_eq!(sides % 2, 0, "a card is drawn on both sides");
                sides / 2
            })
            .collect()
    }

    /// PLAYERS SEE EACH CARD EXACTLY ONCE, ON THE MATTER HOLDING ITS BASE, AT THE SHARED CURSOR.
    /// The shipped view plugin over a fixture house whose two chunks each grow a card: a fallen
    /// group's carrier arriving before the cursor reaches its fall leaves both cards on the
    /// standing construction; at the fall's tick the fallen chunk's card leaves with its piece and
    /// the other stays; a chunk turned to dust draws its card nowhere. A view that draws every card
    /// on every mesh duplicates one; one that keeps a fallen chunk's card on the construction
    /// leaves it on departed matter; one that hands a card over when the carrier arrives moves it
    /// before the cursor. Ticks are fixture data.
    #[test]
    fn each_card_is_drawn_once_by_the_matter_holding_its_base() {
        let (e, chunks) = chunked_element(&shipped_house().layout);
        let (fallen, dusted) = (chunks.start, chunks.start + 1);
        let (kind, firsts) = carded_house(&[fallen, dusted]);
        let kind = std::sync::Arc::new(kind);
        let mut app = App::new();
        app.add_plugins(plugin);
        let world = app.world_mut();
        let shared = shared_look(world);
        world.insert_resource(Time::<Fixed>::default());
        world.init_resource::<Assets<StandardMaterial>>();
        world.insert_resource(Record {
            entries: [
                (Event::Swap(e as u16), 10),
                (Event::Fall(fallen as u16), 10),
                (Event::Dust(dusted as u16), 12),
            ]
            .map(|(event, tick)| crate::construction::breakage::Entry {
                construction: 3,
                event,
                tick,
            })
            .to_vec(),
            ..default()
        });
        let intact = world.resource_mut::<Assets<Mesh>>().add(mesh(&kind));
        let root = world
            .spawn((
                MapInstance {
                    id: "fixture".to_owned(),
                    ordinal: 3,
                },
                Breakage::new(kind, 3, 1.0),
                Transform::IDENTITY,
            ))
            .id();
        let view = world
            .spawn((View, Mesh3d(intact), MeshMaterial3d(shared), ChildOf(root)))
            .id();
        let carrier = world
            .spawn((
                Piece {
                    construction: 3,
                    chunks: vec![fallen as u16],
                    born: 10,
                    retired: None,
                },
                Transform::IDENTITY,
            ))
            .id();
        // Per card: draws by the construction, then by the carrier while shown.
        let drawn = |app: &mut App, tick: u32| {
            *app.world_mut().resource_mut::<PresentationClock>() = PresentationClock::At {
                tick,
                overstep: 0.0,
            };
            app.update();
            let world = app.world();
            let meshes = world.resource::<Assets<Mesh>>();
            let standing = world
                .get::<Mesh3d>(view)
                .map_or(vec![0; firsts.len()], |m| {
                    cards_in(meshes.get(&m.0).unwrap(), &firsts, Vec3::ZERO)
                });
            let dressed = world
                .get::<PieceDressed>(carrier)
                .expect("dressed on arrival");
            let shown = world.get::<Visibility>(carrier) == Some(&Visibility::Inherited);
            let piece = world.get::<Mesh3d>(dressed.view).unwrap();
            let carried = cards_in(meshes.get(&piece.0).unwrap(), &firsts, dressed.origin);
            let carried: Vec<usize> = carried.iter().map(|n| if shown { *n } else { 0 }).collect();
            (standing, carried)
        };
        assert_eq!(
            drawn(&mut app, 5),
            (vec![1, 1], vec![0, 0]),
            "before the fall"
        );
        assert_eq!(drawn(&mut app, 10), (vec![0, 1], vec![1, 0]), "at the fall");
        assert_eq!(
            drawn(&mut app, 12),
            (vec![0, 0], vec![1, 0]),
            "after the dust"
        );
    }

    /// A CARD HAS ONE OWNER, OR THE CONSTRUCTION IS REFUSED: its base inside a chunk names that
    /// chunk, a base on a face two chunks share names the lower, and a base outside the
    /// construction, a skin naming no part, or a card off its plane is refused before play
    /// naming the fix. Fixture points are offsets from shipped chunk centroids.
    #[test]
    fn a_card_s_owner_is_unique_and_a_stray_card_is_refused() {
        let kind = shipped_house();
        let joint = kind
            .layout
            .joints
            .iter()
            .find(|joint| matches!(joint.b, Side::Chunk(_)))
            .expect("the house has a chunk-to-chunk joint");
        let Side::Chunk(b) = joint.b else {
            unreachable!("filtered above")
        };
        let resolve = |base: Vec3, skin: &str| {
            let p = |p: Vec3| format!("({}, {}, {})", p.x, p.y, p.z);
            let corners = [base, base + Vec3::X * 0.1, base + Vec3::Y * 0.1].map(p);
            let text = format!(
                "(skin: [{skin}], cards: [(base: {}, corners: [{}])])",
                p(base),
                corners.join(", ")
            );
            Dressing::resolve(&text, &kind.asset, &kind.layout)
        };
        let shared = resolve(joint.centroid, "").expect("a base on a shared face");
        assert_eq!(shared.cards[0].owner, joint.a.min(b));
        let (lo, hi) = kind.bounds();
        let error = resolve(hi + (hi - lo), "").unwrap_err();
        assert!(error.contains("lies in no chunk"), "{error}");
        let error = resolve(joint.centroid, "\"no_such_part\"").unwrap_err();
        assert!(error.contains("no element declares"), "{error}");
        let text = "(skin: [], cards: [(base: (0.0, 0.0, 0.0), corners: [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0)])])";
        let error = Dressing::resolve(text, &kind.asset, &kind.layout).unwrap_err();
        assert!(error.contains("one plane"), "{error}");
    }

    /// BARK FOLLOWS THE ORIGINAL EXTERIOR; A CUT READS AS WOOD. Every face of a skinned element's
    /// standing chunks is tinted exactly when it lies on a plane of its element that no other
    /// element and no ground anchor covers; the oracle is independent geometry: the face lies on
    /// its element's boundary, a point just outside it is inside no element, and it is not the
    /// foundation's underside.
    /// The shipped dressed trunk is the fixture (every chunk of every element shown swapped). A
    /// view that skins every face draws bark on a cut; one that skins none loses the bark.
    #[test]
    fn bark_is_the_original_exterior_and_every_cut_is_wood() {
        let library = crate::construction::ConstructionLibrary::load(
            &crate::map::tests::shipped_assets(),
            ["fir_1"],
        );
        let kind = library.get("fir_1").expect("the shipped trunk");
        let skin = tint(crate::vfx::construction_dressing().skin_srgb);
        let mut shape = Shape::intact(&kind.layout);
        shape.swapped.fill(true);
        let mesh = broken_mesh(kind, &shape);
        let Some(VertexAttributeValues::Float32x4(colors)) = mesh.attribute(Mesh::ATTRIBUTE_COLOR)
        else {
            panic!("colours");
        };
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("u32 indices");
        };
        let card = tint(crate::vfx::construction_dressing().card_srgb);
        // Each vertex's chunk: its hole tag − 1, every element being swapped.
        let tags = tags(&mesh);
        let owner = |k: usize| tags[k] as usize - 1;
        let (mut barked, mut cut) = (0, 0);
        for (triangle, first) in triangles(&mesh).iter().zip(indices.iter().step_by(3)) {
            let k = *first as usize;
            if colors[k] == card {
                continue;
            }
            let [a, b, c] = *triangle;
            let n = (b - a).cross(c - a).normalize();
            let centre = (a + b + c) / 3.0;
            let element =
                &kind.layout.element_blocks[usize::from(kind.layout.element_of[owner(k)])];
            let boundary = element.planes.iter().any(|(out, offset)| {
                out.dot(n) > 0.999
                    && [a, b, c]
                        .iter()
                        .all(|p| (out.dot(*p) - offset).abs() < CRACK_FACE_TOLERANCE_M)
            });
            let probe = centre + n * 0.01;
            let inside = kind.layout.element_blocks.iter().any(|block| {
                block
                    .planes
                    .iter()
                    .all(|(out, offset)| out.dot(probe) - offset < 0.0)
            });
            let underside = n.y < -0.999
                && (centre.y - kind.asset.foundation_y_m).abs() < CRACK_FACE_TOLERANCE_M;
            let exterior = boundary && !inside && !underside;
            assert_eq!(colors[k] == skin, exterior, "face at {centre} facing {n}");
            if exterior {
                barked += 1;
            } else {
                cut += 1;
            }
        }
        assert!(barked > 0 && cut > 0, "{barked} bark, {cut} cut triangles");
    }
}
