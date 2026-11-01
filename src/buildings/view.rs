//! A building's view: one mesh per building asset, built from its RON's elements, merged and
//! shared by every instance, drawn with the buildings' one material ([`crate::building_surface`]).
//!
//! VIEW ONLY (ADR-0014). The mesh is every element's prism, face by face: the drawn silhouette is
//! exactly the colliders' union, and an opening, which is no element, is a hole the camera sees
//! through. Faces two elements share stay in the mesh; they lie inside the wall and are never seen.
//!
//! # Metres and declared surface frames
//!
//! Each face carries the frame [`crate::building_surface::unit_box`] uses: outward normal `n`,
//! tangent `t`, image-up `b = n × t`, `uv = (p·t, −p·b)` at every corner `p`, here in metres of
//! the building frame. The material's vertex stage multiplies them by the instance's own scale
//! along `t` and `b`, so a scaled house keeps real-size bricks. A wall face stands its image
//! upright (`t = Y × n`) when its surface frame declares Upright; a Grain frame projects its
//! authored direction onto the face; a level face runs along its element's construction
//! ([`tangent`]). The second UV channel selects the visual's declared
//! palette lane, resolved through its substance and the visual library.
//!
//! # A broken building ([`plugin`])
//!
//! The view presents the building record ([`WallRecord`]) up to the presentation clock, so a break
//! shows when the round that made it is drawn arriving. A building with a swapped element draws its
//! own mesh ([`broken_mesh`]): whole elements as before, a swapped element's standing chunks as
//! their hulls, framed exactly as the element's faces, so a swapped wall looks unchanged until a
//! chunk goes; one with nothing left standing draws no mesh at all, never an empty one. Cracks are
//! dark strips where an opened seam meets a wall face ([`crack_mesh`]). A fallen group draws its
//! chunks' hulls on its body or carrier ([`piece_mesh`]), holes and all.
//!
//! # Holes
//!
//! A hole never changes a mesh: a building whose standing chunks hold one wears its own copy of the
//! material ([`crate::building_surface::holed`]) with its [`hole_table`] bound, and the shader skips
//! the surface inside a hole and draws its inner wall, in the main pass and the shadow pass alike
//! (`assets/shaders/building_holes.wgsl`). A swapped chunk's faces carry its index + 1 in the second
//! UV's `y`, a whole element's 0, so a fragment finds its chunk's holes. A hole is matter missing
//! from its chunk, so a chunk that falls keeps it: a fallen piece's faces carry their chunk's place
//! in the piece + 1, and a piece whose chunks hold a hole wears its own copy with its
//! [`piece_hole_table`] bound.

use std::collections::{BTreeMap, BTreeSet};

use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use bevy::render::storage::ShaderBuffer;

use super::breakage::{
    Breakage, HOLES_PER_CHUNK, Hole, Layout, Shape, WallEvent, WallPiece, WallRecord,
    WallReplicaSet,
};
use super::fracture::{Fate, Side, clip_polygon};
use super::schema::{BuildingAsset, SurfaceFrame};
use crate::building_surface::{BuildingMaterial, holed};
use crate::debris::PresentationClock;
use crate::scatter::MapInstance;

/// A face's tangent: its element's authored grain projected into its plane; on an end face the
/// grain crosses, and on every face of an Upright element, the upright image (`Y × n`). A level
/// face has no upright, so its image runs along the element's own construction: its profile's
/// run `u` projected into the face, or its depth axis `w` on a face `u` crosses at over 45°.
/// The frame's axes are orthonormal (the load gate), so `(u·n)² + (w·n)² ≤ 1` and the axis taken
/// lies at least half in the face: no level face is left without a tangent.
fn tangent(n: Vec3, frame: SurfaceFrame, [u, w]: [Vec3; 2]) -> Vec3 {
    let in_face = |axis: Vec3| axis - n * axis.dot(n);
    if let SurfaceFrame::Grain(grain) = frame {
        let projected = in_face(Vec3::from_array(grain));
        if projected.length_squared() > 1e-6 {
            return projected.normalize();
        }
    }
    let upright = Vec3::Y.cross(n);
    if upright.length_squared() > 1e-6 {
        return upright.normalize();
    }
    let run = in_face(u);
    if run.length_squared() >= 0.5 {
        run.normalize()
    } else {
        in_face(w).normalize()
    }
}

/// An element's construction axes for [`tangent`]: its profile's run `u` and its depth `u × v`.
fn axes(element: &super::schema::Element) -> [Vec3; 2] {
    let (u, v) = (
        Vec3::from_array(element.frame.u),
        Vec3::from_array(element.frame.v),
    );
    [u, u.cross(v)]
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
    /// `element` (its authored surface frame and construction axes, [`tangent`]) in palette lane
    /// `secondary`; drawn `offset` from where it is (asset frame) while framed where it is, so a
    /// piece keeps the wall's bricks. `tag` is the hole lookup's (module doc).
    fn face(
        &mut self,
        n: Vec3,
        corners: &[Vec3],
        element: &super::schema::Element,
        secondary: bool,
        offset: Vec3,
        tag: f32,
    ) {
        if corners.len() < 3 {
            return;
        }
        let t = tangent(n, element.surface_frame, axes(element));
        let b = n.cross(t);
        let first = u32::try_from(self.positions.len()).expect("a building's mesh fits u32");
        for p in corners {
            self.positions.push((*p - offset).to_array());
            self.normals.push(n.to_array());
            self.tangents.push(t.extend(1.0).to_array());
            self.uvs.push([p.dot(t), -p.dot(b)]);
            self.packs.push([if secondary { 1.0 } else { 0.0 }, tag]);
        }
        // Corners run counter-clockwise seen from outside: a fan from the first.
        for k in 1..corners.len() as u32 - 1 {
            self.indices.extend([first, first + k, first + k + 1]);
        }
    }

    /// Element `e`'s whole prism.
    fn element(&mut self, asset: &BuildingAsset, e: usize) {
        let element = &asset.elements[e];
        let secondary = crate::building_surface::lane_for_substance(
            &asset.materials[usize::from(element.material)],
        ) == crate::building_surface::SurfaceLane::Secondary;
        for (n, corners) in element.faces() {
            self.face(n, &corners, element, secondary, Vec3::ZERO, 0.0);
        }
    }

    /// Law chunk `c`'s hull, drawn `offset` back, its faces tagged `tag` (module doc).
    fn chunk(&mut self, asset: &BuildingAsset, layout: &Layout, c: usize, offset: Vec3, tag: f32) {
        let element = &asset.elements[usize::from(layout.element_of[c])];
        let secondary = crate::building_surface::lane_for_substance(
            &asset.materials[usize::from(element.material)],
        ) == crate::building_surface::SurfaceLane::Secondary;
        for (n, corners) in layout.blocks[c].faces() {
            self.face(n, &corners, element, secondary, offset, tag);
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
                    faces.chunk(asset, layout, c, Vec3::ZERO, (c + 1) as f32);
                }
            }
        } else {
            faces.element(asset, e);
        }
    }
    faces.mesh()
}

/// A fallen group's mesh, its chunks' hulls about `origin` (asset frame, scale 1), framed where
/// they stood, the `k`-th chunk's faces tagged `k + 1` (module doc).
pub(crate) fn piece_mesh(
    asset: &BuildingAsset,
    layout: &Layout,
    chunks: &[u16],
    origin: Vec3,
) -> Mesh {
    let mut faces = Faces::default();
    for (k, &c) in chunks.iter().enumerate() {
        faces.chunk(asset, layout, usize::from(c), origin, (k + 1) as f32);
    }
    faces.mesh()
}

/// The hole table a building's material binds (module doc): one header per law chunk, the
/// standing ones' holes and hulls. `None` when no standing chunk holds a hole.
pub(crate) fn hole_table(
    asset: &BuildingAsset,
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
    asset: &BuildingAsset,
    layout: &Layout,
    chunks: &[u16],
    holes: &[Vec<Hole>],
    origin: Vec3,
) -> Option<Vec<Vec4>> {
    let chunks = chunks.iter().map(|&c| Some(usize::from(c)));
    table(asset, layout, chunks, holes, origin)
}

/// A hole table (the layout is `assets/shaders/building_holes.wgsl`'s), mesh-local metres: one
/// header per slot, a slot being a face tag − 1 that names the law chunk it draws, or none; then
/// each named chunk's holes, at most [`HOLES_PER_CHUNK`], and its hull's planes, all drawn
/// `offset` back from the asset frame. `None` when no named chunk holds a hole.
fn table(
    asset: &BuildingAsset,
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

/// Present the record up to the presentation clock ([`PresentationClock`]: the interpolation cursor on a
/// replica, the sim tick elsewhere), so a break appears when the round that made it is drawn
/// arriving (ADR-0037), and redraw every building whose shown shape moved.
fn present_record(
    record: Res<WallRecord>,
    clock: Res<PresentationClock>,
    mut presented: Local<(u32, usize)>,
    mut buildings: Query<
        (
            Entity,
            &MapInstance,
            &Breakage,
            Option<&mut ShownShape>,
            Option<&mut ShownHoles>,
        ),
        Without<WallPiece>,
    >,
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
        .take_while(|entry| {
            clock
                .since(entry.tick)
                .is_some_and(|elapsed| elapsed >= 0.0)
        })
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
            shape.apply(event);
            if let WallEvent::Hole(c, hole) = event {
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
        "a wall record names only buildings this map places: {:?}",
        events.keys().collect::<Vec<_>>()
    );
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
            continue; // A building spawned with no view attached has nothing to redraw.
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
    pieces: Query<
        (Entity, &WallPiece, Option<&crate::world::WorldGeneration>),
        (With<Transform>, Without<PieceDressed>),
    >,
    generation: Option<Res<crate::world::CurrentWorld>>,
    buildings: Query<(Entity, &MapInstance, &Breakage), Without<WallPiece>>,
    look: Option<Res<BuildingLook>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    let Some(look) = look else {
        return;
    };
    for (entity, piece, belongs) in &pieces {
        // Offline pieces are local bodies. A streamed piece waits for its own map construction
        // before resolving the building root it will retain for its entire presented lifetime.
        if belongs.is_some_and(|belongs| {
            generation.as_deref().and_then(|current| current.0.as_ref()) != Some(belongs)
        }) {
            continue;
        }
        let Some((building, _, breakage)) = buildings
            .iter()
            .find(|(_, identity, _)| identity.ordinal == piece.building)
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
        let view = commands
            .spawn((
                ChildOf(entity),
                Transform::from_scale(Vec3::splat(breakage.scale)),
                Mesh3d(mesh),
                MeshMaterial3d(look.surface.clone()),
                crate::render_cost::CasterClass::House,
            ))
            .id();
        commands.entity(entity).insert((
            PieceDressed {
                building,
                view,
                origin,
            },
            Visibility::Hidden,
        ));
    }
}

/// A dressed piece: its building's root, resolved from the piece's ordinal once, at dressing; its
/// mesh child; and the point (asset frame, scale 1) its mesh is drawn about.
#[derive(Component)]
struct PieceDressed {
    building: Entity,
    view: Entity,
    origin: Vec3,
}

/// A shown piece whose mesh wears its chunks' holes.
#[derive(Component)]
struct PieceHoled(usize);

/// A piece shows once its building's shown shape has its chunks fallen, and from then on wears its
/// chunks' holes (module doc). New deposits can cut a moving group too, so each changed hole
/// count refreshes its material at the same shared cursor as the ledger.
///
/// A split hands its chunks over at its tick on the shared cursor, all at once: the retired group
/// stays shown until every chunk it held that still stands has a replacement born at its
/// retirement tick. Until then (and while the retirement itself has not arrived) a replacement
/// waits hidden behind the older piece still drawing its chunks, so each chunk is drawn by exactly
/// one piece. The arrivals are indexed once per frame by building, birth tick and chunk, and every
/// handover check is a lookup there.
fn show_pieces(
    clock: Res<PresentationClock>,
    arrived: Query<(&WallPiece, &PieceDressed)>,
    mut pieces: Query<(
        Entity,
        &WallPiece,
        &PieceDressed,
        &mut Visibility,
        Option<&PieceHoled>,
    )>,
    buildings: Query<(&Breakage, &ShownShape, Option<&ShownHoles>)>,
    mut worn: Query<&mut MeshMaterial3d<BuildingMaterial>>,
    mut surfaces: SurfaceStores,
    mut commands: Commands,
) {
    let due = |tick: u32| clock.since(tick).is_some_and(|elapsed| elapsed >= 0.0);
    let born_with: BTreeSet<(Entity, u32, u16)> = arrived
        .iter()
        .flat_map(|(piece, dressed)| {
            piece
                .chunks
                .iter()
                .map(|&c| (dressed.building, piece.born, c))
        })
        .collect();
    let handed_over = |piece: &WallPiece, dressed: &PieceDressed| {
        piece.retired.is_some_and(|tick| {
            due(tick)
                && buildings.get(dressed.building).is_ok_and(|(_, shown, _)| {
                    piece.chunks.iter().all(|&c| {
                        shown.0.fate[usize::from(c)] == Fate::Dust
                            || born_with.contains(&(dressed.building, tick, c))
                    })
                })
        })
    };
    // Per building and chunk, the oldest birth tick among the pieces not yet handed over that
    // hold it: the piece drawing that chunk.
    let mut drawing: BTreeMap<(Entity, u16), u32> = BTreeMap::new();
    for (piece, dressed) in &arrived {
        if !handed_over(piece, dressed) {
            for &c in &piece.chunks {
                let oldest = drawing.entry((dressed.building, c)).or_insert(piece.born);
                *oldest = (*oldest).min(piece.born);
            }
        }
    }
    for (entity, piece, dressed, mut visibility, holed) in &mut pieces {
        // A piece whose building has despawned is hidden.
        let building = buildings.get(dressed.building).ok();
        let born = due(piece.born)
            && piece.chunks.iter().all(|&c| {
                drawing
                    .get(&(dressed.building, c))
                    .is_none_or(|&oldest| oldest >= piece.born)
            });
        let retired = handed_over(piece, dressed);
        let fallen = born
            && !retired
            && building.is_some_and(|(_, shown, _)| {
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
        let hole_count = building.and_then(|(_, _, holes)| holes).map_or(0, |holes| {
            piece
                .chunks
                .iter()
                .map(|&c| holes.0[usize::from(c)].len())
                .sum()
        });
        if !fallen || holed.is_some_and(|shown| shown.0 == hole_count) {
            continue;
        }
        let table = building.and_then(|(breakage, _, holes)| {
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
/// the broken buildings redrawn, the fallen groups dressed and shown.
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

    /// Appearance must follow the construction record without changing geometry. The view
    /// reader relies on frame data even for a material drawn in the primary palette lane.
    #[test]
    fn authored_grain_changes_the_frame_and_never_the_silhouette() {
        use bevy::mesh::VertexAttributeValues;
        let mut asset = crate::buildings::tests::shipped_house().asset.clone();
        let before = mesh(&asset);
        asset.elements[0].surface_frame = SurfaceFrame::Grain([0.0, 0.0, 1.0]);
        let after = mesh(&asset);
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
        asset.elements[0].surface_frame = SurfaceFrame::Grain([f32::NAN, 0.0, 0.0]);
        let error = asset.check().unwrap_err();
        assert!(
            error.contains("surface_frame") && error.contains("author"),
            "{error}"
        );
    }

    /// A level face projects a construction axis into its plane. The surface reader relies
    /// on a unit tangent perpendicular to the normal, including sloping and vertical runs.
    /// ASSUMED fixture rotations cover both axis choices without naming an asset.
    #[test]
    fn a_level_face_runs_along_its_element() {
        for yaw in [0.0_f32, 0.7, 1.57] {
            let heading = Quat::from_rotation_y(yaw);
            let horizontal = heading * Vec3::X;
            for degrees in [0.0_f32, 5.0, 44.0, 46.0, 85.0, 90.0] {
                let rotation = heading * Quat::from_rotation_z(degrees.to_radians());
                let axes = [rotation * Vec3::X, rotation * Vec3::Y];
                for n in [Vec3::Y, Vec3::NEG_Y] {
                    let actual = tangent(n, SurfaceFrame::Upright, axes);
                    assert!((actual.length() - 1.0).abs() < 1e-6);
                    assert!(actual.dot(n).abs() < 1e-6);
                    assert!(actual.dot(horizontal).abs() > 0.999);
                }
            }
        }
    }

    /// A level face takes its image from its own element's construction axes on the mesh
    /// path (`Faces::face`), never from a world axis: a mutant passing `[X, Z]` there draws a
    /// yawed element's floorboards along world X. ASSUMED fixture: a shipped element's frame
    /// yawed off every world axis, its surface frame Upright.
    #[test]
    fn a_level_face_on_the_mesh_path_runs_along_its_element() {
        let mut element = shipped_house().asset.elements[0].clone();
        let run = Quat::from_rotation_y(0.7) * Vec3::X;
        element.frame.u = run.to_array();
        element.frame.v = Vec3::Y.to_array();
        element.surface_frame = SurfaceFrame::Upright;
        let mut faces = Faces::default();
        faces.face(
            Vec3::Y,
            &[Vec3::ZERO, Vec3::Z, Vec3::X],
            &element,
            false,
            Vec3::ZERO,
            0.0,
        );
        let tangent = Vec4::from_array(faces.tangents[0]).truncate();
        assert!(tangent.dot(run).abs() > 0.999, "{tangent} along {run}");
    }

    /// An unsynchronized replica must hold actual ledger entries. Checking only the clock's
    /// accessor would let a presentation predicate that treats Waiting as due escape.
    #[test]
    fn a_waiting_replica_presents_no_record_entry() {
        use crate::buildings::breakage::WallEntry;
        let kind = shipped_house();
        let mut world = World::new();
        world.insert_resource(PresentationClock::Waiting);
        world.insert_resource(WallRecord {
            entries: vec![WallEntry {
                building: 3,
                event: WallEvent::Swap(0),
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
                Breakage::new(kind, 1.0),
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
            .filter(|element| {
                crate::building_surface::lane_for_substance(
                    &asset.materials[usize::from(element.material)],
                ) == crate::building_surface::SurfaceLane::Secondary
            })
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
    /// so every vertex of a swapped chunk's hull carries that chunk's index + 1, every vertex of a
    /// fallen piece's `k`-th chunk `k + 1` (its [`piece_hole_table`]'s header), and every whole
    /// element's vertex 0, which reads no hole. A tag off by one draws a hole in the neighbouring
    /// chunk; a tag on a whole element reads a header that is not its own.
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
        let order = [chunks.start + 1, chunks.start];
        let piece = piece_mesh(asset, layout, &order.map(|c| c as u16), Vec3::ZERO);
        let tagged = tags(&piece);
        let first = hull_corners(layout, order[0]);
        assert_eq!(tagged.len(), first + hull_corners(layout, order[1]));
        assert!(
            tagged[..first].iter().all(|&tag| tag == 1.0)
                && tagged[first..].iter().all(|&tag| tag == 2.0),
            "a piece's chunks carry their place in it"
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

    /// A world holding the stores a view draws from and the shared material ([`BuildingLook`]),
    /// whose handle it returns.
    fn shared_look(world: &mut World) -> Handle<BuildingMaterial> {
        world.init_resource::<PresentationClock>();
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
        shared
    }

    /// A pose can precede its Battle snapshot. Dressing it against the old construction would
    /// retain a departed building root forever, hiding valid current debris from the player.
    #[test]
    fn a_streamed_piece_resolves_its_building_only_in_its_own_world() {
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
                    id: "fixture_building".to_owned(),
                    ordinal: 3,
                },
                Breakage::new(kind.clone(), 1.0),
            ))
            .id();
        let carrier = world
            .spawn((
                WallPiece {
                    building: 3,
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
                    id: "fixture_building".to_owned(),
                    ordinal: 3,
                },
                Breakage::new(kind, 1.0),
            ))
            .id();
        world.resource_mut::<CurrentWorld>().0 = Some(WorldGeneration(2));
        schedule.run(&mut world);
        assert_eq!(world.get::<PieceDressed>(carrier).unwrap().building, fresh);
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

    /// A FALLEN PIECE KEEPS ITS CHUNKS' HOLES (a hole is matter missing from its chunk): once its
    /// chunks show fallen, a piece one of whose chunks holds a hole wears its own masked material
    /// binding its [`piece_hole_table`], the capsule and the hull's planes drawn about the piece's
    /// origin as its mesh is, so every vertex of the mesh lies on the hull the shader tests; a
    /// piece of hole-free chunks keeps the shared material. A view that dropped a falling chunk's
    /// holes, or drew them about the building's frame instead of the piece's, fails here. The
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
            Breakage::new(kind.clone(), 1.0),
            ShownShape(shape),
            holes.clone(),
        ));
        let piece = |world: &mut World, c: usize| {
            world
                .spawn((
                    WallPiece {
                        building: 3,
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
            let material = world.get::<MeshMaterial3d<BuildingMaterial>>(view).unwrap();
            let mesh = world.get::<Mesh3d>(view).unwrap();
            (material.0.clone(), mesh.0.clone(), dressed.origin)
        };
        assert_eq!(worn(&world, plain_piece).0, shared, "no hole: shared");
        let (material, mesh, origin) = worn(&world, holed_piece);
        assert_ne!(material, shared, "a holed piece wears its own material");
        let material = world
            .resource::<Assets<BuildingMaterial>>()
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
    /// A split replaces the old carrier at the shared cursor, after all replacement views
    /// have arrived. A late child must not make occupied geometry disappear early.
    #[test]
    fn a_split_waits_for_its_tick_and_every_replacement() {
        let kind = shipped_house();
        let (_, range) = chunked_element(&kind.layout);
        let chunks: Vec<_> = range.take(2).map(|c| c as u16).collect();
        assert_eq!(chunks.len(), 2);
        let mut world = World::new();
        shared_look(&mut world);
        *world.resource_mut::<PresentationClock>() = PresentationClock::At {
            tick: 19,
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
            Breakage::new(kind, 1.0),
            ShownShape(shape),
        ));
        let parent = world
            .spawn((
                WallPiece {
                    building: 3,
                    chunks: chunks.clone(),
                    born: 0,
                    retired: Some(20),
                },
                Transform::IDENTITY,
            ))
            .id();
        let a = world
            .spawn((
                WallPiece {
                    building: 3,
                    chunks: vec![chunks[0]],
                    born: 20,
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
            tick: 20,
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
                WallPiece {
                    building: 3,
                    chunks: vec![chunks[1]],
                    born: 20,
                    retired: None,
                },
                Transform::IDENTITY,
            ))
            .id();
        schedule.run(&mut world);
        assert_eq!(world.get::<Visibility>(parent), Some(&Visibility::Hidden));
        assert_eq!(world.get::<Visibility>(a), Some(&Visibility::Inherited));
        assert_eq!(world.get::<Visibility>(b), Some(&Visibility::Inherited));
    }
}
