//! The belt's GPU DRAW: every shoe of one side drawn by ONE mesh entity from that belt's frame
//! table ([`BeltFrames`]).
//!
//! # The adapter: the shoe repeated, placed in the vertex shader
//!
//! A belt's mesh is its rung's shoe repeated once per material link ([`repeated_shoe`]): the
//! authored geometry untouched, every vertex tagged with the link it belongs to
//! ([`ATTRIBUTE_BELT_LINK`]). The vertex shader (`shaders/track_belt_mesh.wgsl`, placing through
//! `shaders/track_belt.wgsl`) reads that link's span from the belt's storage buffer and places the
//! vertex rigidly, as [`BeltFrames::pose`] places the shoe on the CPU: the template correction, the
//! swing about the belt's lateral axis onto the chord, the anchor. A shoe is never stretched or bent.
//!
//! The material is the shoe's own `StandardMaterial` extended by [`BeltShoes`], so the belt rides
//! every phase that material rides: opaque, depth prepass and shadow. The extension supplies a
//! prepass vertex shader, which is also what makes bevy bind the material in its depth-only
//! prepass and shadow paths (`prepass_reads_material`); without it those paths would draw the
//! undeformed stack.
//!
//! The rejected adapter is custom instancing (bevy 0.19's `custom_shader_instancing` example):
//! it queues its own phase item with its own pipeline, so it would have to rebuild, for the shoe,
//! what the material pipeline already gives every mesh — the bindless `StandardMaterial` bind
//! group, the opaque, prepass and shadow phases, visibility, and the x-ray's look and mask
//! materials. The repeated mesh costs the shoe's vertices once per link in GPU memory instead.
//!
//! # What the repeated meshes cost, and when they are built
//!
//! A rung's repeated mesh is its shoe's vertices and indices times the link count: DERIVED ~71 MB
//! for all five rungs of both sides of a 97-link belt (the shipped shoe's MEASURED vertex and
//! index counts, an ASSUMED 52-byte vertex), ~26 MB of it rung 0. Every belt of one count shares
//! them ([`BeltMeshes`]). They are built on the main thread, one rung and side at a time, on the
//! first frame a belt of that count draws that rung, and dropped with the last table of that
//! count; the build logs each mesh's size.
//!
//! Vertex pulling (the link read as `vertex_index / V` off one copy of the shoe) does not remove
//! the cost on bevy 0.19's material path: `DrawMesh` draws exactly the mesh's own index range
//! (`bevy_pbr` `render/mesh.rs`, direct and indirect alike), and the instance range belongs to
//! batching, so a belt still needs a mesh with one index per drawn corner — DERIVED ~7 MB per count
//! at 4 bytes a corner — and every shoe attribute moved to a storage buffer, with the vertex
//! shader defs (`VERTEX_UVS_A`, `VERTEX_TANGENTS`) set by hand in every pass the layout no longer
//! implies. Drawing one shoe `count` times without a count-sized mesh needs a draw command of its
//! own, which is the custom-instancing adapter above.
//!
//! # Per-belt assets
//!
//! A belt owns its storage buffer and its material ([`BeltDraw`]): `count` records of 16 bytes, one
//! per material link, rewritten whenever the table changes. `ShaderBuffer` writes an unchanged size
//! into the same GPU buffer, so the material's bind group stays valid. Both assets go with the belt
//! entity. The repeated meshes are shared by every belt of one link count, side and rung
//! ([`BeltMeshes`]).
//!
//! # Bounds
//!
//! The mesh's own bounds are the undeformed stack of shoes about the shoe origin, so the belt
//! carries [`NoAutoAabb`] and an [`Aabb`] written from its table ([`BeltFrames::bounds`]): every
//! anchor grown by the shoe's canonical reach.

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::{NoAutoAabb, VisibilitySystems};
use bevy::ecs::system::SystemParam;
use bevy::mesh::{
    Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology,
    VertexAttributeValues, VertexFormat,
};
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;

use super::link_view::{BeltFrames, LinkTemplate};
use super::side::{PerSide, Side};

/// The material link a belt-mesh vertex belongs to: its record in the belt's table.
pub(crate) const ATTRIBUTE_BELT_LINK: MeshVertexAttribute =
    MeshVertexAttribute::new("BeltLink", 0x0B31_7172, VertexFormat::Uint32);

/// The vertex shader location [`ATTRIBUTE_BELT_LINK`] is bound at. Above every location bevy's
/// forward (0–7) and prepass (0–7) layouts use, and inside wgpu's default 16 vertex attributes.
pub(crate) const LINK_LOCATION: u32 = 15;

/// The belt's vertex shader, for the main pass and the prepass alike (`PREPASS_PIPELINE` selects).
const MESH_SHADER: &str = "shaders/track_belt_mesh.wgsl";

/// The per-side constants of the placement, `shaders/track_belt.wgsl`'s `BeltParams`.
#[derive(ShaderType, Reflect, Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct BeltParams {
    /// Mesh → canonical rotation ([`super::link_view::LinkFrame`]).
    correction: Mat3,
    /// The canonical origin in mesh space.
    origin: Vec3,
    /// The shoe's lateral centre in the belt's frame.
    lateral_x: f32,
}

impl BeltParams {
    /// The constants `frames`' shoes are placed with.
    pub(crate) fn of(frames: &BeltFrames) -> Self {
        let (correction, origin) = frames.frame().correction_and_origin();
        Self {
            correction,
            origin,
            lateral_x: frames.lateral_x(),
        }
    }
}

/// The belt's half of its material: the placement constants and the table's GPU copy.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub(crate) struct BeltShoes {
    #[uniform(100)]
    pub(crate) params: BeltParams,
    /// One `vec4` per material link: the span's joints `(a.z, a.y, b.z, b.y)`.
    #[storage(101, read_only)]
    pub(crate) frames: Handle<ShaderBuffer>,
}

impl MaterialExtension for BeltShoes {
    fn vertex_shader() -> ShaderRef {
        MESH_SHADER.into()
    }

    fn prepass_vertex_shader() -> ShaderRef {
        MESH_SHADER.into()
    }

    fn deferred_vertex_shader() -> ShaderRef {
        MESH_SHADER.into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        bind_link_attribute(descriptor, layout)
    }
}

/// The belt's material: the shoe's own look, placed by [`BeltShoes`].
pub(crate) type BeltMaterial = ExtendedMaterial<StandardMaterial, BeltShoes>;

/// Add [`ATTRIBUTE_BELT_LINK`] at [`LINK_LOCATION`] to a pipeline's vertex buffer: every material
/// that draws a belt mesh calls this from its `specialize`, after the base layout is built.
pub(crate) fn bind_link_attribute(
    descriptor: &mut RenderPipelineDescriptor,
    layout: &MeshVertexBufferLayoutRef,
) -> Result<(), SpecializedMeshPipelineError> {
    let link = layout
        .0
        .get_layout(&[ATTRIBUTE_BELT_LINK.at_shader_location(LINK_LOCATION)])?;
    let Some(buffer) = descriptor.vertex.buffers.first_mut() else {
        return Ok(());
    };
    buffer.attributes.extend(link.attributes);
    Ok(())
}

/// A belt's table buffer, released with the belt (its material goes with its
/// `MeshMaterial3d<BeltMaterial>`, or with whatever dressed it in another look).
#[derive(Component)]
pub(crate) struct BeltDraw {
    frames: Handle<ShaderBuffer>,
    /// The farthest any vertex of any rung of this side's shoe stands from the canonical origin, m.
    reach: f32,
}

/// The repeated meshes, per link count: each count's verdict, and the rungs its belts have drawn.
///
/// A count's entry is made the first time a belt of that count asks ([`Self::ready`]) and holds
/// one of two answers for as long as it lives. `Refused`: some rung's shoe cannot be repeated, the
/// refusal was logged once, and the count is never examined again. `Ready`: every rung's shoe was
/// checked before anything was built, so a rung built later cannot fail; a rung is built on the
/// first frame a belt of that count draws it ([`Self::rung_or_build`]). While a shoe has not loaded
/// there is no entry, and the next ask checks again.
///
/// A `Ready` count no live belt table uses is evicted ([`evict_unused_counts`]), which drops its
/// meshes; a `Refused` count is kept, so a refusal is never re-examined.
#[derive(Resource, Default)]
pub(crate) struct BeltMeshes {
    by_count: HashMap<usize, CountMeshes>,
}

/// One link count's verdict ([`BeltMeshes`]).
enum CountMeshes {
    Refused,
    Ready {
        /// The farthest any vertex of any rung of each side's shoe stands from the canonical
        /// origin, m: the bounds' growth ([`BeltFrames::bounds`]).
        reach: PerSide<f32>,
        /// Per rung, nearest first, each side's repeated mesh once built.
        rungs: Vec<PerSide<Option<Handle<Mesh>>>>,
    },
}

impl BeltMeshes {
    /// The mesh a belt of `count` links draws at `rung` on `side`, if it has been built; a rung
    /// past the floor draws the floor ([`LinkTemplate::rung_mesh`]).
    #[cfg(test)]
    pub(crate) fn rung(&self, count: usize, rung: usize, side: Side) -> Option<&Handle<Mesh>> {
        let Some(CountMeshes::Ready { rungs, .. }) = self.by_count.get(&count) else {
            return None;
        };
        rungs[rung.min(rungs.len().checked_sub(1)?)]
            .get(side)
            .as_ref()
    }

    /// The mesh a belt of `count` links draws at `rung` on `side`, built now if no belt of that
    /// count has drawn it yet. `None` for a count that is not [`Self::ready`].
    pub(crate) fn rung_or_build(
        &mut self,
        count: usize,
        rung: usize,
        side: Side,
        template: &LinkTemplate,
        meshes: &mut Assets<Mesh>,
    ) -> Option<Handle<Mesh>> {
        let Some(CountMeshes::Ready { rungs, .. }) = self.by_count.get_mut(&count) else {
            return None;
        };
        let rung = rung.min(rungs.len().checked_sub(1)?);
        let slot = rungs[rung].get_mut(side);
        if let Some(built) = slot {
            return Some(built.clone());
        }
        let mesh = match repeated_shoe(meshes.get(&template.rung_mesh(rung, side))?, count) {
            Ok(mesh) => mesh,
            // Unreachable while the template's shoes are the ones [`Self::ready`] checked.
            Err(why) => {
                error_once!("track belt: rung {rung} cannot be repeated ({why}) — not drawn");
                return None;
            }
        };
        info!(
            "track belt: rung {rung} {side:?} of {count} links built — {} vertices of {} B, {} \
             indices, {:.1} MB DERIVED",
            mesh.count_vertices(),
            mesh.get_vertex_size(),
            mesh.indices().map_or(0, Indices::len),
            (mesh.count_vertices() as u64 * mesh.get_vertex_size()
                + 4 * mesh.indices().map_or(0, Indices::len) as u64) as f64
                / 1e6,
        );
        let handle = meshes.add(mesh);
        *slot = Some(handle.clone());
        Some(handle)
    }

    /// Whether belts of `count` links can be drawn: every rung's shoe loaded and repeatable. The
    /// first answer for a count is kept ([`BeltMeshes`]); nothing is built here.
    pub(crate) fn ready(
        &mut self,
        count: usize,
        template: &LinkTemplate,
        meshes: &Assets<Mesh>,
    ) -> bool {
        if !self.by_count.contains_key(&count) {
            let Some(verdict) = verdict(count, template, meshes) else {
                return false;
            };
            self.by_count.insert(count, verdict);
        }
        matches!(self.by_count.get(&count), Some(CountMeshes::Ready { .. }))
    }

    /// Each side's reach for belts of `count` links, once [`Self::ready`].
    fn reach(&self, count: usize, side: Side) -> Option<f32> {
        let Some(CountMeshes::Ready { reach, .. }) = self.by_count.get(&count) else {
            return None;
        };
        Some(*reach.get(side))
    }

    /// Drop every built count `live` does not name, and its meshes with it. Refusals stay.
    fn evict(&mut self, live: impl Fn(usize) -> bool) {
        self.by_count
            .retain(|&count, meshes| matches!(meshes, CountMeshes::Refused) || live(count));
    }

    /// A fixture's meshes for belts of `count` links, one per rung, without building any.
    #[cfg(test)]
    pub(crate) fn insert_fixture(&mut self, count: usize, rungs: Vec<PerSide<Handle<Mesh>>>) {
        self.by_count.insert(
            count,
            CountMeshes::Ready {
                reach: PerSide::default(),
                rungs: rungs.into_iter().map(|rung| rung.map(Some)).collect(),
            },
        );
    }
}

/// Every rung's shoe on both sides checked for belts of `count` links, before anything is built.
/// `None` while a shoe has not loaded; a refusal is logged here, once.
fn verdict(count: usize, template: &LinkTemplate, meshes: &Assets<Mesh>) -> Option<CountMeshes> {
    let mut reach = PerSide::new(0.0_f32, 0.0_f32);
    for rung in 0..template.levels() {
        for side in Side::ALL {
            let shoe = meshes.get(&template.rung_mesh(rung, side))?;
            let (_, origin) = template.frame(side).correction_and_origin();
            match repeatable(shoe, count).and_then(|()| canonical_reach(shoe, origin)) {
                Ok(shoe_reach) => {
                    let side_reach = reach.get_mut(side);
                    *side_reach = side_reach.max(shoe_reach);
                }
                Err(why) => {
                    error!(
                        "track belt: rung {rung} {side:?} cannot be repeated {count} times ({why}) \
                         — no belt of {count} links is drawn"
                    );
                    return Some(CountMeshes::Refused);
                }
            }
        }
    }
    Some(CountMeshes::Ready {
        reach,
        rungs: vec![PerSide::new(None, None); template.levels()],
    })
}

/// Whether [`repeated_shoe`] can repeat `shoe` `count` times: a triangle list whose data the main
/// world holds, every attribute a float format, and every repeated vertex addressable by a `u32`
/// index.
fn repeatable(shoe: &Mesh, count: usize) -> Result<(), String> {
    if shoe.primitive_topology() != PrimitiveTopology::TriangleList {
        return Err("not a triangle list".into());
    }
    let Ok(attributes) = shoe.try_attributes() else {
        return Err("its data lives in the render world only".into());
    };
    for (attribute, values) in attributes {
        if !matches!(
            values,
            VertexAttributeValues::Float32(_)
                | VertexAttributeValues::Float32x2(_)
                | VertexAttributeValues::Float32x3(_)
                | VertexAttributeValues::Float32x4(_)
        ) {
            return Err(format!(
                "attribute `{}` is {:?}",
                attribute.name,
                VertexFormat::from(values)
            ));
        }
    }
    if count
        .checked_mul(shoe.count_vertices())
        .is_none_or(|vertices| vertices > u32::MAX as usize)
    {
        return Err("more vertices than a u32 index reaches".into());
    }
    Ok(())
}

/// The farthest any vertex of `shoe` stands from the canonical origin `origin` (mesh space), m:
/// the correction is a rotation, so mesh-space distance is canonical distance.
fn canonical_reach(shoe: &Mesh, origin: Vec3) -> Result<f32, String> {
    let Ok(VertexAttributeValues::Float32x3(positions)) =
        shoe.try_attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return Err("no f32x3 positions".into());
    };
    Ok(positions
        .iter()
        .map(|&p| Vec3::from(p).distance(origin))
        .fold(0.0, f32::max))
}

/// `shoe` repeated `count` times, copy `k` tagged as link `k` in [`ATTRIBUTE_BELT_LINK`]: every
/// attribute and index of the source, copy after copy, nothing transformed. Render-world only: no
/// CPU reader needs the stack, and it is the shoe's size times the link count.
fn repeated_shoe(shoe: &Mesh, count: usize) -> Result<Mesh, String> {
    repeatable(shoe, count)?;
    let vertices = shoe.count_vertices();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    for (attribute, values) in shoe.attributes() {
        let repeated = match values {
            VertexAttributeValues::Float32(v) => VertexAttributeValues::Float32(v.repeat(count)),
            VertexAttributeValues::Float32x2(v) => {
                VertexAttributeValues::Float32x2(v.repeat(count))
            }
            VertexAttributeValues::Float32x3(v) => {
                VertexAttributeValues::Float32x3(v.repeat(count))
            }
            VertexAttributeValues::Float32x4(v) => {
                VertexAttributeValues::Float32x4(v.repeat(count))
            }
            other => {
                return Err(format!(
                    "attribute `{}` is {:?}",
                    attribute.name,
                    VertexFormat::from(other)
                ));
            }
        };
        mesh.insert_attribute(*attribute, repeated);
    }
    let links: Vec<u32> = (0..count as u32)
        .flat_map(|link| std::iter::repeat_n(link, vertices))
        .collect();
    mesh.insert_attribute(ATTRIBUTE_BELT_LINK, links);
    let corners: Vec<u32> = match shoe.indices() {
        Some(Indices::U16(indices)) => indices.iter().map(|&i| u32::from(i)).collect(),
        Some(Indices::U32(indices)) => indices.clone(),
        None => (0..vertices as u32).collect(),
    };
    let base = vertices as u32;
    mesh.insert_indices(Indices::U32(
        (0..count as u32)
            .flat_map(|link| corners.iter().map(move |&i| i + link * base))
            .collect(),
    ));
    Ok(mesh)
}

/// The assets a belt's draw is built from.
#[derive(SystemParam)]
pub(crate) struct BeltAssets<'w> {
    pub(crate) meshes: ResMut<'w, Assets<Mesh>>,
    standard: Res<'w, Assets<StandardMaterial>>,
    materials: ResMut<'w, Assets<BeltMaterial>>,
    buffers: ResMut<'w, Assets<ShaderBuffer>>,
    cache: ResMut<'w, BeltMeshes>,
}

impl BeltAssets<'_> {
    /// Whether a belt of `count` links can be drawn: every rung's shoe checked repeatable and the
    /// shoe's material loaded. A consumer checks this before spawning anything, so a bind never
    /// half-lands. Builds nothing.
    pub(crate) fn ready(&mut self, template: &LinkTemplate, count: usize) -> bool {
        self.cache.ready(count, template, &self.meshes)
            && self.standard.contains(template.material())
    }

    /// Everything a belt drawing `frames` on `side` carries, at rung 0: its mesh (built here if no
    /// belt of its count has drawn rung 0 yet), its own material and buffer, and its bounds. `None`
    /// until [`Self::ready`].
    pub(crate) fn draw(
        &mut self,
        template: &LinkTemplate,
        side: Side,
        frames: &BeltFrames,
    ) -> Option<impl Bundle + use<>> {
        if !self.ready(template, frames.count()) {
            return None;
        }
        let mesh = self
            .cache
            .rung_or_build(frames.count(), 0, side, template, &mut self.meshes)?;
        let reach = self.cache.reach(frames.count(), side)?;
        let base = self.standard.get(template.material())?.clone();
        let buffer = self.buffers.add(ShaderBuffer::from(records(frames)));
        let material = self.materials.add(BeltMaterial {
            base,
            extension: BeltShoes {
                params: BeltParams::of(frames),
                frames: buffer.clone(),
            },
        });
        let (low, high) = frames.bounds(reach);
        Some((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            BeltDraw {
                frames: buffer,
                reach,
            },
            NoAutoAabb,
            Aabb::from_min_max(low, high),
        ))
    }
}

/// The table as the GPU reads it: one `(a.z, a.y, b.z, b.y)` per material link.
fn records(frames: &BeltFrames) -> Vec<Vec4> {
    (0..frames.count())
        .map(|link| {
            let [a, b] = frames.span(link).joints();
            Vec4::new(a.x, a.y, b.x, b.y)
        })
        .collect()
}

/// Where a belt's changed table reaches its buffer and its bounds: after every table writer,
/// before visibility reads the bounds.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BeltUpload;

/// Copy every changed table to its GPU buffer and rewrite its bounds. A settled belt's table is
/// unchanged ([`BeltFrames::fit`] marks nothing), so it uploads nothing.
fn upload_belt_frames(
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut belts: Query<(&BeltFrames, &BeltDraw, &mut Aabb), Changed<BeltFrames>>,
) {
    for (frames, draw, mut aabb) in &mut belts {
        if let Some(mut buffer) = buffers.get_mut(&draw.frames) {
            buffer.set_data(records(frames));
        }
        let (low, high) = frames.bounds(draw.reach);
        aabb.set_if_neq(Aabb::from_min_max(low, high));
    }
}

/// Drop the meshes of every link count no live belt table has: a despawned tank's, or the count
/// the sandbox's knob left. In `Last`, after every system that spawns or resizes a table this
/// frame, so a count drawn this frame is never dropped under its belt.
fn evict_unused_counts(mut cache: ResMut<BeltMeshes>, tables: Query<&BeltFrames>) {
    let live = |count: usize| tables.iter().any(|table| table.count() == count);
    let stale = cache
        .by_count
        .iter()
        .any(|(&count, meshes)| matches!(meshes, CountMeshes::Ready { .. }) && !live(count));
    if stale {
        cache.evict(live);
    }
}

/// The belt draw's material, mesh cache and upload. Mounted by [`super::link_view::template_plugin`].
pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(MaterialPlugin::<BeltMaterial>::default())
        .init_resource::<BeltMeshes>()
        .configure_sets(
            PostUpdate,
            BeltUpload.before(VisibilitySystems::CheckVisibility),
        )
        .add_systems(PostUpdate, upload_belt_frames.in_set(BeltUpload))
        .add_systems(Last, evict_unused_counts);
}

#[cfg(test)]
mod tests {
    use super::super::link_view::LinkFrame;
    use super::*;

    /// A two-triangle fixture shoe with every attribute kind the shipped shoe carries.
    fn fixture_shoe() -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
            ],
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; 4])
        .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, vec![[1.0, 0.0, 0.0, 1.0]; 4])
        .with_inserted_indices(Indices::U16(vec![0, 1, 2, 2, 1, 3]))
    }

    /// A template of `levels` shoes per side, each its own asset of the fixture shoe.
    fn fixture_template(assets: &mut Assets<Mesh>, levels: usize) -> LinkTemplate {
        let shoes = (0..levels)
            .map(|_| PerSide::new(assets.add(fixture_shoe()), assets.add(fixture_shoe())))
            .collect();
        LinkTemplate::fixture(shoes, Handle::default())
    }

    /// GPU MEMORY GOES TO WHAT IS DRAWN. A repeated mesh is the shoe times the link count (DERIVED
    /// ~13 MB per side at rung 0 of a 97-link belt), so a count's rungs are built one at a
    /// time, the first time a belt of that count draws each, and a count no live table has lets
    /// go of every one of them. Readiness builds nothing; a rung is built once however many belts
    /// draw it.
    #[test]
    fn a_count_builds_only_the_rungs_drawn_and_lets_go_when_no_table_is_left() {
        const COUNT: usize = 5;
        let mut assets = Assets::<Mesh>::default();
        let template = fixture_template(&mut assets, 3);
        let shoes = assets.len();
        let mut cache = BeltMeshes::default();
        assert!(cache.ready(COUNT, &template, &assets));
        assert_eq!(assets.len(), shoes, "readiness builds nothing");

        let far = cache
            .rung_or_build(COUNT, 2, Side::Left, &template, &mut assets)
            .expect("a ready count builds its rungs");
        assert_eq!(assets.len(), shoes + 1, "one rung on one side is one mesh");
        assert!(cache.rung(COUNT, 0, Side::Left).is_none());
        assert!(cache.rung(COUNT, 2, Side::Right).is_none());
        assert_eq!(
            cache.rung_or_build(COUNT, 2, Side::Left, &template, &mut assets),
            Some(far),
            "a built rung is shared, never rebuilt",
        );
        assert_eq!(assets.len(), shoes + 1);

        let mut app = App::new();
        app.insert_resource(cache)
            .add_systems(Last, evict_unused_counts);
        let table = app
            .world_mut()
            .spawn(BeltFrames::new(LinkFrame::default(), 0.0, COUNT))
            .id();
        app.update();
        assert!(
            app.world()
                .resource::<BeltMeshes>()
                .rung(COUNT, 2, Side::Left)
                .is_some(),
            "a count a live table has keeps its meshes",
        );
        app.world_mut().despawn(table);
        app.update();
        assert!(
            app.world()
                .resource::<BeltMeshes>()
                .rung(COUNT, 2, Side::Left)
                .is_none(),
            "a count no table has lets go of its meshes",
        );
    }

    /// A REFUSAL IS FINAL AND BUILDS NOTHING. Every rung is checked before any is built, so a shoe
    /// that cannot be repeated (here the floor rung, carrying an integer attribute) leaves no
    /// half-built count behind; and the count is never examined again, so a consumer asking every
    /// frame does not redo the work every frame. A shoe still loading is not a refusal: the count
    /// is examined again once it lands.
    #[test]
    fn a_refused_count_builds_nothing_and_is_never_examined_again() {
        const COUNT: usize = 4;
        const REFUSED: usize = COUNT + 1;
        let mut assets = Assets::<Mesh>::default();
        let template = fixture_template(&mut assets, 3);
        let floor = template.rung_mesh(2, Side::Right);
        let mut cache = BeltMeshes::default();

        let shoe = assets.remove(&floor).expect("the fixture shoe");
        assert!(!cache.ready(COUNT, &template, &assets), "a shoe is loading");
        assets.insert(&floor, shoe).expect("the fixture id");
        assert!(
            cache.ready(COUNT, &template, &assets),
            "a count is examined again once its shoe lands",
        );

        let tagged = fixture_shoe().with_inserted_attribute(
            MeshVertexAttribute::new("FixtureTag", 0x0B31_7173, VertexFormat::Uint32),
            vec![0_u32; 4],
        );
        assets.insert(&floor, tagged).expect("the fixture id");
        let before = assets.len();
        assert!(!cache.ready(REFUSED, &template, &assets));
        assert!(
            cache
                .rung_or_build(REFUSED, 0, Side::Left, &template, &mut assets)
                .is_none()
        );
        assert_eq!(assets.len(), before, "a refusal builds nothing");

        assets
            .insert(&floor, fixture_shoe())
            .expect("the fixture id");
        assert!(
            !cache.ready(REFUSED, &template, &assets),
            "a refusal is never examined again"
        );
        cache.evict(|_| false);
        assert!(
            !cache.ready(REFUSED, &template, &assets),
            "eviction keeps a refusal"
        );
    }

    /// The repeated mesh is the draw's geometry: copy `k` must be the source shoe exactly, tagged
    /// link `k`, its triangles indexing its own vertices. The vertex shader relies on the tag to
    /// read a vertex's record; a copy indexing a neighbour's vertices would draw torn shoes.
    #[test]
    fn every_copy_of_the_repeated_shoe_is_the_shoe_tagged_with_its_link() {
        const COUNT: usize = 5;
        let shoe = fixture_shoe();
        let belt = repeated_shoe(&shoe, COUNT).expect("a triangle list repeats");
        let vertices = shoe.count_vertices();
        assert_eq!(belt.count_vertices(), vertices * COUNT);
        let Some(VertexAttributeValues::Uint32(links)) = belt.attribute(ATTRIBUTE_BELT_LINK) else {
            panic!("the link tag is a u32 per vertex");
        };
        let (
            Some(VertexAttributeValues::Float32x3(source)),
            Some(VertexAttributeValues::Float32x3(copies)),
        ) = (
            shoe.attribute(Mesh::ATTRIBUTE_POSITION),
            belt.attribute(Mesh::ATTRIBUTE_POSITION),
        )
        else {
            panic!("positions survive the repeat");
        };
        let Some(Indices::U32(indices)) = belt.indices() else {
            panic!("the repeat indexes in u32");
        };
        let corners = shoe.indices().map_or(0, Indices::len);
        for link in 0..COUNT {
            let block = link * vertices..(link + 1) * vertices;
            assert!(links[block.clone()].iter().all(|&tag| tag as usize == link));
            assert_eq!(&copies[block.clone()], source.as_slice());
            for &index in &indices[link * corners..(link + 1) * corners] {
                assert!(
                    block.contains(&(index as usize)),
                    "copy {link} indexes outside itself"
                );
            }
        }
        for attribute in [
            Mesh::ATTRIBUTE_NORMAL,
            Mesh::ATTRIBUTE_UV_0,
            Mesh::ATTRIBUTE_TANGENT,
        ] {
            assert!(
                belt.contains_attribute(attribute),
                "{} is carried",
                attribute.name
            );
        }
    }
}
