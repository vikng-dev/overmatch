//! A building's view: one mesh per building asset, built from its RON's elements, merged and
//! shared by every instance, drawn with the buildings' one material ([`crate::building_surface`]).
//!
//! VIEW ONLY (ADR-0014). The mesh is every element's prism, face by face: the drawn silhouette is
//! exactly the colliders' union, and an opening, which is no element, is a hole the camera sees
//! through. Faces two elements share stay in the mesh; they lie inside the wall and are never seen.
//!
//! # Metres on every face, timber by the mesh's word
//!
//! Each face carries the frame [`crate::building_surface::unit_box`] uses: outward normal `n`,
//! tangent `t`, image-up `b = n × t`, `uv = (p·t, −p·b)` at every corner `p`, here in metres of
//! the building frame. The material's vertex stage multiplies them by the instance's own scale
//! along `t` and `b`, so a scaled house keeps real-size bricks. A wall face stands its image
//! upright (`t = Y × n`); a timber face runs its boards along its element's grain, the extrusion
//! axis, where that lies in the face. The second UV channel's `x` says which pack a face wears:
//! 1 on timber, 0 on masonry, so a sill's top stays brick and a board's edge stays timber.
//!
//! # A broken building ([`plugin`])
//!
//! The view presents the building record ([`WallRecord`]) up to the presentation clock, so a break
//! shows when the round that made it is drawn arriving. A building with a swapped element draws its
//! own mesh ([`broken_mesh`]): whole elements as before, a swapped element's standing chunks as
//! their hulls, framed exactly as the element's faces, so a swapped wall looks unchanged until a
//! chunk goes; one with nothing left standing draws no mesh at all, never an empty one. Cracks are
//! dark strips where an opened seam meets a wall face ([`crack_mesh`]). A fallen group draws its
//! chunks' hulls on its body or carrier ([`piece_mesh`]).
//!
//! # Holes
//!
//! A hole never changes a mesh: a building whose standing chunks hold one wears its own copy of the
//! material ([`crate::building_surface::holed`]) with its [`hole_table`] bound, and the shader skips
//! the surface inside a hole and draws its inner wall, in the main pass and the shadow pass alike
//! (`assets/shaders/building_holes.wgsl`). A swapped chunk's faces carry its index + 1 in the second
//! UV's `y`, a whole element's 0, so a fragment finds its chunk's holes.

use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use bevy::render::storage::ShaderBuffer;

use super::breakage::{
    Breakage, HOLES_PER_CHUNK, Hole, Layout, Shape, WallEvent, WallPiece, WallRecord,
    WallReplicaSet,
};
use super::fracture::{Fate, Side, clip_polygon};
use super::schema::BuildingAsset;
use crate::building_surface::{BuildingMaterial, holed};
use crate::scatter::MapInstance;
use crate::trees::FallClock;

/// The substance key whose faces wear the timber pack; every other wears the masonry pack.
const TIMBER: &str = "Wood";

/// The tangent frame of a face with outward normal `n` on an element extruded along `grain`:
/// the grain where it lies in the face and the face is timber, else `Y × n` (the image upright),
/// else `X` on a face that is level.
fn tangent(n: Vec3, grain: Vec3, timber: bool) -> Vec3 {
    if timber && grain.dot(n).abs() < 1e-3 {
        return grain.normalize();
    }
    let upright = Vec3::Y.cross(n);
    if upright.length_squared() > 1e-6 {
        upright.normalize()
    } else {
        Vec3::X
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
    indices: Vec<u32>,
}

impl Faces {
    /// One face of outward normal `n`, its corners counter-clockwise seen from outside, on an
    /// element extruded along `grain`, of timber or not; drawn `offset` from where it is (asset
    /// frame) while framed where it is, so a piece keeps the wall's bricks. `tag` is the hole
    /// lookup's (module doc).
    fn face(
        &mut self,
        n: Vec3,
        corners: &[Vec3],
        grain: Vec3,
        timber: bool,
        offset: Vec3,
        tag: f32,
    ) {
        if corners.len() < 3 {
            return;
        }
        let t = tangent(n, grain, timber);
        let b = n.cross(t);
        let first = u32::try_from(self.positions.len()).expect("a building's mesh fits u32");
        for p in corners {
            self.positions.push((*p - offset).to_array());
            self.normals.push(n.to_array());
            self.tangents.push(t.extend(1.0).to_array());
            self.uvs.push([p.dot(t), -p.dot(b)]);
            self.packs.push([if timber { 1.0 } else { 0.0 }, tag]);
        }
        // Corners run counter-clockwise seen from outside: a fan from the first.
        for k in 1..corners.len() as u32 - 1 {
            self.indices.extend([first, first + k, first + k + 1]);
        }
    }

    /// Element `e`'s whole prism.
    fn element(&mut self, asset: &BuildingAsset, e: usize) {
        let element = &asset.elements[e];
        let timber = asset.materials[usize::from(element.material)] == TIMBER;
        for (n, corners) in element.faces() {
            self.face(n, &corners, element.frame.w(), timber, Vec3::ZERO, 0.0);
        }
    }

    /// Law chunk `c`'s hull, drawn `offset` back, tagged with its holes (module doc) or not.
    fn chunk(&mut self, asset: &BuildingAsset, layout: &Layout, c: usize, offset: Vec3, tag: bool) {
        let element = &asset.elements[usize::from(layout.element_of[c])];
        let timber = asset.materials[usize::from(element.material)] == TIMBER;
        let tag = if tag { (c + 1) as f32 } else { 0.0 };
        for (n, corners) in layout.blocks[c].faces() {
            self.face(n, &corners, element.frame.w(), timber, offset, tag);
        }
    }

    fn mesh(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, self.tangents)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, self.packs)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// The building's mesh in its own frame at scale 1 (module doc).
pub(crate) fn mesh(asset: &BuildingAsset) -> Mesh {
    let mut faces = Faces::default();
    for e in 0..asset.elements.len() {
        faces.element(asset, e);
    }
    faces.mesh()
}

/// A broken building's mesh in its own frame at scale 1: every whole element's prism, and every
/// standing chunk of each swapped element its hull, so a chunk turned to dust or fallen is a hole.
pub(crate) fn broken_mesh(asset: &BuildingAsset, layout: &Layout, shape: &Shape) -> Mesh {
    let mut faces = Faces::default();
    for (e, swapped) in shape.swapped.iter().enumerate() {
        if *swapped {
            for c in layout.chunks_of(e) {
                if shape.fate[c] == Fate::Standing {
                    faces.chunk(asset, layout, c, Vec3::ZERO, true);
                }
            }
        } else {
            faces.element(asset, e);
        }
    }
    faces.mesh()
}

/// A fallen group's mesh, its chunks' hulls about `origin` (asset frame, scale 1), framed where
/// they stood.
pub(crate) fn piece_mesh(
    asset: &BuildingAsset,
    layout: &Layout,
    chunks: &[u16],
    origin: Vec3,
) -> Mesh {
    let mut faces = Faces::default();
    for &c in chunks {
        faces.chunk(asset, layout, usize::from(c), origin, false);
    }
    faces.mesh()
}

/// The hole table a building's material binds (module doc; the layout is
/// `assets/shaders/building_holes.wgsl`'s), mesh-local metres: one header per law chunk, then each
/// standing chunk's holes, at most [`HOLES_PER_CHUNK`], and its hull's planes. `None` when no
/// standing chunk holds a hole.
pub(crate) fn hole_table(
    asset: &BuildingAsset,
    layout: &Layout,
    shape: &Shape,
    holes: &[Vec<Hole>],
) -> Option<Vec<Vec4>> {
    let mut table = vec![Vec4::ZERO; layout.chunks.len()];
    let at = |centroid: Vec3, mm: [i16; 3]| centroid + Vec3::from_array(mm.map(f32::from)) / 1000.0;
    for (c, holes) in holes.iter().enumerate() {
        if holes.is_empty() || shape.fate.get(c) != Some(&Fate::Standing) {
            continue;
        }
        let centroid = Vec3::from_array(asset.chunks[layout.chunks[c] as usize].centroid);
        let kept = &holes[..holes.len().min(HOLES_PER_CHUNK)];
        let planes = &layout.blocks[c].planes;
        table[c] = Vec4::new(
            table.len() as f32,
            kept.len() as f32,
            planes.len() as f32,
            0.0,
        );
        for hole in kept {
            table.push(at(centroid, hole.from).extend(f32::from(hole.radius) / 1000.0));
            table.push(at(centroid, hole.to).extend(0.0));
        }
        table.extend(planes.iter().map(|(n, o)| n.extend(*o)));
    }
    (table.len() > layout.chunks.len()).then_some(table)
}

/// A crack line's half width on a wall face, m (Resolution: about a mortar joint's, so a crack
/// reads at a few tens of metres).
const CRACK_HALF_WIDTH_M: f32 = 0.006;
/// How far a crack line stands off the face it marks, m: clear of the face's depth, under any
/// brick's relief.
const CRACK_STANDOFF_M: f32 = 0.0015;

/// The cracks a broken building shows, in its frame at scale 1: for every cracked joint between
/// two standing chunks, a dark strip along each edge of their shared face that lies on one of
/// their element's two faces (its extrusion's near and far planes), where the crack opens to the
/// eye. `None` when no crack shows.
pub(crate) fn crack_mesh(asset: &BuildingAsset, layout: &Layout, shape: &Shape) -> Option<Mesh> {
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
        let element = &asset.elements[usize::from(layout.element_of[joint.a])];
        let w = element.frame.w();
        let origin = Vec3::from_array(element.frame.origin);
        for k in 0..polygon.len() {
            let (p, q) = (polygon[k], polygon[(k + 1) % polygon.len()]);
            let along = q - p;
            if along.length() < 1e-4 {
                continue;
            }
            for (depth, out) in [(element.depth[0], -w), (element.depth[1], w)] {
                let on = |x: Vec3| ((x - origin).dot(w) - depth).abs() < 1e-3;
                if !(on(p) && on(q)) {
                    continue;
                }
                let side = out.cross(along.normalize()) * CRACK_HALF_WIDTH_M;
                let lift = out * CRACK_STANDOFF_M;
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

/// A building's drawn child: the entity the scatter gave the building's mesh and material, at the
/// instance's scale under its root.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct BuildingView;

/// The one material every building wears ([`crate::building_surface`]), for the meshes the breaks
/// add; and the cracks' dark material.
#[derive(Resource, Clone)]
pub(crate) struct BuildingLook {
    pub(crate) surface: Handle<crate::building_surface::BuildingMaterial>,
}

/// The cracks' material: unlit near-black, the shadow inside an opened seam.
#[derive(Resource, Clone)]
struct CrackLook(Handle<StandardMaterial>);

/// What a building shows: the record presented up to the presentation clock.
#[derive(Component, Clone, Debug, PartialEq)]
pub(crate) struct ShownShape(pub(crate) Shape);

/// The holes a building shows, per law chunk in record order, presented with its [`ShownShape`].
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub(crate) struct ShownHoles(pub(crate) Vec<Vec<Hole>>);

/// A broken building view's cracks' child. The view's own mesh lives in its `Mesh3d` alone: a
/// replaced or removed handle frees the old asset.
#[derive(Component)]
struct OwnMesh {
    cracks: Option<Entity>,
}

/// Present the record up to the presentation clock ([`FallClock`]: the interpolation cursor on a
/// replica, the sim tick elsewhere), so a break appears when the round that made it is drawn
/// arriving (ADR-0037), and redraw every building whose shown shape moved.
fn present_record(
    record: Res<WallRecord>,
    clock: Res<FallClock>,
    mut presented: Local<(u32, usize)>,
    mut buildings: Query<(
        Entity,
        &MapInstance,
        &Breakage,
        Option<&mut ShownShape>,
        Option<&mut ShownHoles>,
    )>,
    mut commands: Commands,
) {
    let reset = presented.0 != record.generation || record.entries.len() < presented.1;
    if reset {
        for (_, _, breakage, shown, holes) in &mut buildings {
            if let Some(mut shown) = shown {
                shown.0 = Shape::intact(&breakage.kind.layout);
            }
            if let Some(mut holes) = holes {
                holes.set_if_neq(ShownHoles::default());
            }
        }
        *presented = (record.generation, 0);
    }
    let due = record.entries[presented.1..]
        .iter()
        .take_while(|entry| clock.since(entry.tick) >= 0.0)
        .count();
    if due == 0 {
        return;
    }
    let mut events: std::collections::BTreeMap<u16, Vec<WallEvent>> = Default::default();
    for entry in &record.entries[presented.1..presented.1 + due] {
        events.entry(entry.building).or_default().push(entry.event);
    }
    presented.1 += due;
    for (root, identity, breakage, shown, shown_holes) in &mut buildings {
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
            if shape.apply(event)
                && let WallEvent::Hole(c, hole) = event
            {
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
}

/// The stores a building's own material is made in ([`crate::building_surface::holed`]): absent
/// where nothing draws.
#[derive(bevy::ecs::system::SystemParam)]
struct SurfaceStores<'w> {
    look: Option<Res<'w, BuildingLook>>,
    materials: Option<ResMut<'w, Assets<BuildingMaterial>>>,
    buffers: Option<ResMut<'w, Assets<ShaderBuffer>>>,
}

impl SurfaceStores<'_> {
    /// The material a building whose standing chunks hold `table` wears: its own holed copy, or the
    /// shared one when there is no table. `None` where nothing draws.
    fn wear(&mut self, table: Option<Vec<Vec4>>) -> Option<Handle<BuildingMaterial>> {
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

/// Redraw a building whose shown shape or holes moved: its view's own mesh, its material (its own
/// while a hole shows, module doc) and its cracks. A view holds a `Mesh3d` exactly while its
/// building has a face to draw: a zero-vertex mesh never reaches the renderer (see
/// [`crate::track::shadow_proxy::ProxySide::mesh`]).
fn redraw(
    buildings: Query<
        (&Breakage, &ShownShape, Option<&ShownHoles>, &Children),
        Or<(Changed<ShownShape>, Changed<ShownHoles>)>,
    >,
    mut views: Query<
        (
            Entity,
            Option<&mut Mesh3d>,
            Option<&mut MeshMaterial3d<BuildingMaterial>>,
            Option<&OwnMesh>,
        ),
        With<BuildingView>,
    >,
    look: Option<Res<CrackLook>>,
    mut surfaces: SurfaceStores,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut commands: Commands,
) {
    for (breakage, shown, holes, children) in &buildings {
        let Some(view) = children.iter().find(|child| views.contains(*child)) else {
            continue;
        };
        let Ok((view, drawn, worn, own)) = views.get_mut(view) else {
            continue;
        };
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
        let broken = broken_mesh(&kind.asset, &kind.layout, &shown.0);
        let mesh = (broken.count_vertices() > 0).then(|| meshes.add(broken));
        let cracks = crack_mesh(&kind.asset, &kind.layout, &shown.0).map(|m| meshes.add(m));
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
                    base_color: Color::srgb(0.05, 0.04, 0.035),
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
/// mesh at its building's scale, hidden until the record is presented to its fall. A carrier is
/// dressed once it holds a pose: an interpolated one takes its first when the cursor reaches it.
fn dress_pieces(
    pieces: Query<(Entity, &WallPiece), (With<Transform>, Without<PieceDressed>)>,
    buildings: Query<(&MapInstance, &Breakage)>,
    look: Option<Res<BuildingLook>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    let Some(look) = look else {
        return;
    };
    for (entity, piece) in &pieces {
        let Some((_, breakage)) = buildings
            .iter()
            .find(|(identity, _)| identity.ordinal == piece.building)
        else {
            continue;
        };
        let kind = &breakage.kind;
        let (mut weighted, mut mass) = (Vec3::ZERO, 0.0);
        for &c in &piece.chunks {
            let chunk = &kind.asset.chunks[kind.layout.chunks[usize::from(c)] as usize];
            weighted += Vec3::from_array(chunk.centroid) * chunk.mass_kg;
            mass += chunk.mass_kg;
        }
        let origin = weighted / mass.max(f32::EPSILON);
        let mesh = meshes.add(piece_mesh(&kind.asset, &kind.layout, &piece.chunks, origin));
        commands
            .entity(entity)
            .insert((PieceDressed, Visibility::Hidden));
        commands.spawn((
            ChildOf(entity),
            Transform::from_scale(Vec3::splat(breakage.scale)),
            Mesh3d(mesh),
            MeshMaterial3d(look.surface.clone()),
            crate::render_cost::CasterClass::House,
        ));
    }
}

/// A dressed piece: its mesh child is spawned.
#[derive(Component)]
struct PieceDressed;

/// A piece shows once its building's shown shape has its chunks fallen.
fn show_pieces(
    mut pieces: Query<(&WallPiece, &mut Visibility), With<PieceDressed>>,
    buildings: Query<(&MapInstance, &ShownShape)>,
) {
    for (piece, mut visibility) in &mut pieces {
        let fallen = buildings
            .iter()
            .find(|(identity, _)| identity.ordinal == piece.building)
            .is_some_and(|(_, shown)| {
                piece
                    .chunks
                    .first()
                    .is_some_and(|&c| shown.0.fate.get(usize::from(c)) == Some(&Fate::Fallen))
            });
        visibility.set_if_neq(if fallen {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

/// The breaks' view, on every root that draws: the record presented at the presentation clock,
/// the broken buildings redrawn, the fallen groups dressed and shown.
pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (present_record, redraw, dress_pieces, show_pieces)
            .chain()
            .after(WallReplicaSet),
    );
}

#[cfg(test)]
mod tests {
    use bevy::mesh::VertexAttributeValues;

    use super::*;
    use crate::buildings::tests::shipped_house;

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

    /// AN OPENING IS A HOLE THE CAMERA SEES THROUGH, A PIER IS NOT: across the shipped house's
    /// east wall (outer face x = 2 m, inner 1.75 m), a sight line through its first window's
    /// middle (z 1.5 m, 1.5 m up; house_1's spec) meets no triangle, and one through the pier
    /// beside it (z 2.5 m) does. A view that drew the wall as its graybox box, or filled the
    /// openings, fails the first; one that dropped an element fails the second.
    #[test]
    fn a_window_is_a_hole_and_a_pier_is_solid() {
        let triangles = triangles(&mesh(&shipped_house().asset));
        let across = |z: f32| crosses(&triangles, Vec3::new(2.1, 1.5, z), Vec3::new(1.6, 1.5, z));
        assert!(!across(1.5), "the window is open");
        assert!(across(2.5), "the pier is drawn");
    }

    /// EVERY FACE IS FRAMED IN METRES AND WEARS ITS ELEMENT'S PACK: each corner's UV is its
    /// position on the face's tangent and image-up axes (the frame the material's vertex stage
    /// rescales by the instance's scale), wall faces stand their image upright, and the second
    /// UV marks exactly the timber elements' faces. A UV off the frame stretches the bricks; a
    /// mark on the wrong faces puts boards on a wall or bricks on the roof.
    #[test]
    fn every_face_is_framed_in_metres_and_wears_its_element_s_pack() {
        let asset = shipped_house().asset.clone();
        let mesh = mesh(&asset);
        let attribute = |id| match mesh.attribute(id) {
            Some(VertexAttributeValues::Float32x3(values)) => values
                .iter()
                .copied()
                .map(Vec3::from_array)
                .collect::<Vec<_>>(),
            _ => panic!("Float32x3"),
        };
        let (positions, normals) = (
            attribute(Mesh::ATTRIBUTE_POSITION),
            attribute(Mesh::ATTRIBUTE_NORMAL),
        );
        let Some(VertexAttributeValues::Float32x4(tangents)) =
            mesh.attribute(Mesh::ATTRIBUTE_TANGENT)
        else {
            panic!("tangents");
        };
        let Some(VertexAttributeValues::Float32x2(uvs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("uvs");
        };
        let Some(VertexAttributeValues::Float32x2(packs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_1)
        else {
            panic!("packs");
        };
        let mut timber_vertices = 0;
        for k in 0..positions.len() {
            let (p, n, t) = (positions[k], normals[k], Vec4::from_array(tangents[k]));
            let b = t.w * n.cross(t.truncate());
            let uv = Vec2::from_array(uvs[k]);
            assert!((uv - Vec2::new(p.dot(t.truncate()), -p.dot(b))).length() < 1e-4);
            assert!(
                t.truncate().dot(n).abs() < 1e-4,
                "the tangent lies in the face"
            );
            if packs[k][0] == 1.0 {
                timber_vertices += 1;
            } else {
                assert_eq!(packs[k][0], 0.0);
                if n.y.abs() < 1e-3 {
                    assert!(
                        (b - Vec3::Y).length() < 1e-4,
                        "a wall stands its bricks upright"
                    );
                }
            }
        }
        let expected: usize = asset
            .elements
            .iter()
            .filter(|element| asset.materials[usize::from(element.material)] == TIMBER)
            .map(|element| element.faces().iter().map(|(_, c)| c.len()).sum::<usize>())
            .sum();
        assert!(expected > 0);
        assert_eq!(timber_vertices, expected, "exactly the timber faces");
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

    /// The corners of law chunk `c`'s hull, as the mesh draws them.
    fn hull_corners(layout: &Layout, c: usize) -> usize {
        layout.blocks[c]
            .faces()
            .iter()
            .map(|(_, corners)| corners.len())
            .sum()
    }

    /// A FRAGMENT FINDS ITS OWN CHUNK'S HOLES. The shader reads a face's hole header at its tag − 1,
    /// so every vertex of a swapped chunk's hull carries that chunk's index + 1, and every whole
    /// element's and fallen piece's vertex 0, which reads no hole. A tag off by one draws a hole in
    /// the neighbouring chunk; a tag on a whole element reads a header that is not its own.
    #[test]
    fn a_swapped_chunk_s_faces_carry_its_index_and_nothing_else_carries_one() {
        let kind = shipped_house();
        let (asset, layout) = (&kind.asset, &kind.layout);
        let (e, chunks) = chunked_element(layout);
        let mut shape = Shape::intact(layout);
        assert!(tags(&mesh(asset)).iter().all(|&tag| tag == 0.0));
        shape.swapped[e] = true;
        let tagged = tags(&broken_mesh(asset, layout, &shape));
        for c in chunks.clone() {
            let count = tagged.iter().filter(|&&tag| tag == (c + 1) as f32).count();
            assert_eq!(
                count,
                hull_corners(layout, c),
                "chunk {c}'s hull carries its tag"
            );
        }
        let chunk_corners: usize = chunks.clone().map(|c| hull_corners(layout, c)).sum();
        assert_eq!(
            tagged.iter().filter(|&&tag| tag != 0.0).count(),
            chunk_corners,
            "only the swapped chunks carry a tag"
        );
        let piece = piece_mesh(asset, layout, &[chunks.start as u16], Vec3::ZERO);
        assert!(
            tags(&piece).iter().all(|&tag| tag == 0.0),
            "a piece reads no hole"
        );
    }

    /// THE TABLE IS WHAT THE SHADER WALKS (`building_holes.wgsl`): chunk `c`'s header at record `c`
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

    /// THE SHADER'S BOUND IS THE RECORD'S: `building_holes.wgsl` sizes its per-fragment spans by
    /// its own `HOLES_PER_CHUNK`, so a record cap above it would leave holes undrawn.
    #[test]
    fn the_shader_s_hole_cap_is_the_record_s() {
        let shader = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/shaders/building_holes.wgsl"),
        )
        .expect("the hole shader");
        assert!(
            shader.contains(&format!("const HOLES_PER_CHUNK: u32 = {HOLES_PER_CHUNK}u;")),
            "the shader's cap is breakage::HOLES_PER_CHUNK"
        );
    }

    /// A HOLE IS DRAWN BY ITS BUILDING'S OWN MATERIAL, AND ONLY WHILE IT SHOWS: a building whose
    /// shown holes stand wears a copy of the shared material, alpha-masked (the variant whose
    /// shader and shadow pass test holes) and binding its table; one with none wears the shared
    /// material, so every building without a hole keeps the opaque path. The hole is ASSUMED
    /// fixture data.
    #[test]
    fn a_building_with_a_hole_wears_its_own_masked_material_until_the_hole_goes() {
        let kind = shipped_house();
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<BuildingMaterial>>();
        world.init_resource::<Assets<ShaderBuffer>>();
        let shared_buffer = world
            .resource_mut::<Assets<ShaderBuffer>>()
            .add(crate::building_surface::no_holes());
        let shared = world
            .resource_mut::<Assets<BuildingMaterial>>()
            .add(BuildingMaterial {
                base: StandardMaterial::default(),
                extension: crate::building_surface::BuildingSurface {
                    params: default(),
                    roof_albedo: default(),
                    roof_normal: default(),
                    roof_arm: default(),
                    holes: shared_buffer,
                },
            });
        world.insert_resource(BuildingLook {
            surface: shared.clone(),
        });
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
            .spawn((Breakage::new(kind.clone(), 1.0), ShownShape(shape), holes))
            .id();
        let view = world
            .spawn((BuildingView, MeshMaterial3d(shared.clone()), ChildOf(root)))
            .id();
        let mut schedule = Schedule::default();
        schedule.add_systems(redraw);
        schedule.run(&mut world);
        let worn = world
            .get::<MeshMaterial3d<BuildingMaterial>>(view)
            .unwrap()
            .0
            .clone();
        assert_ne!(worn, shared, "a holed building wears its own material");
        let material = world
            .resource::<Assets<BuildingMaterial>>()
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
            "it binds the building's table"
        );

        world.get_mut::<ShownHoles>(root).unwrap().0 = vec![Vec::new(); kind.layout.chunks.len()];
        schedule.run(&mut world);
        let worn = &world
            .get::<MeshMaterial3d<BuildingMaterial>>(view)
            .unwrap()
            .0;
        assert_eq!(
            *worn, shared,
            "with no hole it wears the shared material again"
        );
    }

    /// NO EMPTY MESH REACHES A RENDER ENTITY: a building with nothing left standing holds no
    /// `Mesh3d` and leaves no zero-vertex mesh asset (see
    /// [`crate::track::shadow_proxy::ProxySide::mesh`]), and a record reset that stands it again gives its view a mesh
    /// back. The all-gone shape is ASSUMED fixture state, set directly rather than reached by law.
    #[test]
    fn a_building_with_nothing_standing_draws_no_mesh_until_it_stands_again() {
        let kind = shipped_house();
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        let shared = world.resource_mut::<Assets<Mesh>>().add(mesh(&kind.asset));
        let intact = Shape::intact(&kind.layout);
        let root = world
            .spawn((Breakage::new(kind, 1.0), ShownShape(intact.clone())))
            .id();
        let view = world
            .spawn((BuildingView, Mesh3d(shared), ChildOf(root)))
            .id();
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
            "a reset stands the building again"
        );
    }
}
