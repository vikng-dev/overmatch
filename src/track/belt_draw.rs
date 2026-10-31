//! The belt's GPU DRAW: every shoe of one side drawn by ONE mesh entity from that belt's frame
//! table ([`BeltFrames`]).
//!
//! # The adapter: the shoe repeated, placed in the vertex shader
//!
//! A belt's mesh is its rung's shoe repeated once per material link ([`repeated_shoe`]): the
//! authored geometry untouched, every vertex tagged with the link it belongs to
//! ([`ATTRIBUTE_BELT_LINK`]). The vertex shader (`shaders/track_belt_mesh.wgsl`, placing through
//! `shaders/track_belt.wgsl`) reads that link's span from the belt's block of the shared table and
//! places the vertex rigidly, as [`BeltFrames::pose`] places the shoe on the CPU: the template
//! correction, the swing about the belt's lateral axis onto the chord, the anchor. A shoe is never
//! stretched or bent.
//!
//! The material is the shoe's own `StandardMaterial` extended by [`BeltShoes`], so the belt rides
//! every phase that material rides: opaque, depth prepass and shadow. The extension supplies a
//! prepass vertex shader, which is also what makes bevy bind the material in its depth-only
//! prepass and shadow paths (`prepass_reads_material`); without it those paths would draw the
//! undeformed stack.
//!
//! # One material per look, one block per belt
//!
//! Every belt is drawn from a shared table ([`BeltTable`]): pages of 16-byte records, each belt
//! owning one block of [`HEADER_RECORDS`] placement constants followed by one span per material
//! link. The belt's `MeshTag` is its block's first record, which the vertex stage reads through
//! `mesh_functions::get_tag`. Every belt of one look on one page wears ONE material, so belts of
//! the same count, rung and side batch across tanks: bevy keys a batch set on the material's bind
//! group (`Opaque3dBatchSetKey::material_bind_group_index`) and a batch on the mesh, never on the
//! tag. DERIVED: one page holds 81 blocks of a 97-link belt.
//!
//! A page never changes size. A same-size `ShaderBuffer` write lands in the same GPU buffer
//! (`GpuShaderBuffer::prepare_asset`), so a material's bind group stays valid for the session;
//! a resized buffer would leave every material bound to the old one, because nothing re-prepares
//! a material when a buffer it binds changes. A belt that fits no page opens a new page, with its
//! own material per look. A block is released when its [`BeltDraw`] leaves the belt (despawn, or a
//! new draw replacing it) and handed to the next belt that fits; pages and their materials live
//! for the session.
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
//! # Upload
//!
//! A changed belt table is written into its block in place; each written page is marked modified
//! once per frame, and reaches the GPU whole (DERIVED 128 KiB a page), in the frame the table
//! changed ([`BeltUpload`] orders before the asset-event flush). A settled belt's table is
//! unchanged and writes nothing.
//!
//! # Bounds
//!
//! The mesh's own bounds are the undeformed stack of shoes about the shoe origin, so the belt
//! carries [`NoAutoAabb`] and an [`Aabb`] written from its table ([`BeltFrames::bounds`]): every
//! anchor grown by the shoe's canonical reach.

use std::ops::Range;

use bevy::asset::{AssetEventSystems, RenderAssetUsages};
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::{NoAutoAabb, VisibilitySystems};
use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::system::SystemParam;
use bevy::ecs::world::DeferredWorld;
use bevy::mesh::{
    Indices, MeshTag, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology,
    VertexAttributeValues, VertexFormat,
};
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
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

/// Bytes in one table record: one `vec4<f32>`.
const RECORD_BYTES: usize = 16;

/// Records a belt's block starts with, before its spans: the mesh → canonical rotation's three
/// columns (`xyz`), then the canonical origin in mesh space (`xyz`) and the shoe's lateral centre
/// in the belt's frame (`w`). `shaders/track_belt.wgsl`'s `belt_at` reads them in this order.
const HEADER_RECORDS: u32 = 4;

/// Records in one table page: 128 KiB. A page never changes size (module doc).
const PAGE_RECORDS: u32 = 8192;

/// The belt's half of its material: the table page its belts' blocks live in.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub(crate) struct BeltShoes {
    /// Every belt's block on this page, at the record its `MeshTag` names: [`HEADER_RECORDS`]
    /// placement constants, then one span `(a.z, a.y, b.z, b.y)` per material link.
    #[storage(100, read_only)]
    pub(crate) table: Handle<ShaderBuffer>,
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

/// A belt's block of the shared table, and its bounds' growth.
///
/// Immutable, so the block it names is the one the table lent: the `on_discard` hook gives the
/// block back whenever this component leaves the belt, by despawn or by a new draw replacing it.
#[derive(Component)]
#[component(immutable, on_discard = BeltDraw::release)]
pub(crate) struct BeltDraw {
    page: usize,
    /// Records of `page`: the header, then one span per material link.
    block: Range<u32>,
    /// The farthest any vertex of any rung of this side's shoe stands from the canonical origin, m.
    reach: f32,
}

impl BeltDraw {
    fn release(mut world: DeferredWorld, HookContext { entity, .. }: HookContext) {
        let Some(draw) = world.entity(entity).get::<Self>() else {
            return;
        };
        let (page, block) = (draw.page, draw.block.clone());
        if let Some(mut table) = world.get_resource_mut::<BeltTable>() {
            table.release(page, block);
        }
    }
}

/// The shared table every belt is drawn from: pages that never change size, each lent out in
/// blocks (module doc).
#[derive(Resource, Default)]
pub(crate) struct BeltTable {
    pages: Vec<TablePage>,
}

/// One page of the [`BeltTable`].
struct TablePage {
    buffer: Handle<ShaderBuffer>,
    /// The records no belt holds: ascending, never adjacent.
    free: Vec<Range<u32>>,
    /// This page's material per look, keyed by the shoe's own `StandardMaterial`.
    looks: HashMap<AssetId<StandardMaterial>, Handle<BeltMaterial>>,
}

impl TablePage {
    /// The first free block of `records`, taken.
    fn take(&mut self, records: u32) -> Option<Range<u32>> {
        let index = self
            .free
            .iter()
            .position(|free| free.end - free.start >= records)?;
        let free = &mut self.free[index];
        let block = free.start..free.start + records;
        free.start = block.end;
        if free.start == free.end {
            self.free.remove(index);
        }
        Some(block)
    }

    /// `block` given back, merged with the free records beside it.
    fn give(&mut self, block: Range<u32>) {
        let at = self.free.partition_point(|free| free.start < block.start);
        self.free.insert(at, block);
        if at + 1 < self.free.len() && self.free[at].end == self.free[at + 1].start {
            let next = self.free.remove(at + 1);
            self.free[at].end = next.end;
        }
        if at > 0 && self.free[at - 1].end == self.free[at].start {
            let this = self.free.remove(at);
            self.free[at - 1].end = this.end;
        }
    }
}

impl BeltTable {
    /// A block of `records`: the first page with room, or a new page of at least [`PAGE_RECORDS`].
    fn lend(&mut self, records: u32, buffers: &mut Assets<ShaderBuffer>) -> (usize, Range<u32>) {
        for (index, page) in self.pages.iter_mut().enumerate() {
            if let Some(block) = page.take(records) {
                return (index, block);
            }
        }
        let capacity = records.max(PAGE_RECORDS);
        let mut page = TablePage {
            buffer: buffers.add(ShaderBuffer::new(
                &vec![0; capacity as usize * RECORD_BYTES],
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            )),
            free: vec![Range {
                start: 0,
                end: capacity,
            }],
            looks: HashMap::default(),
        };
        let block = page
            .take(records)
            .expect("a new page holds its first block");
        self.pages.push(page);
        (self.pages.len() - 1, block)
    }

    fn release(&mut self, page: usize, block: Range<u32>) {
        if let Some(page) = self.pages.get_mut(page) {
            page.give(block);
        }
    }

    /// The material every belt of `look` on `page` wears, made on the first.
    fn material(
        &mut self,
        page: usize,
        look: &Handle<StandardMaterial>,
        base: &StandardMaterial,
        materials: &mut Assets<BeltMaterial>,
    ) -> Handle<BeltMaterial> {
        let TablePage { buffer, looks, .. } = &mut self.pages[page];
        looks
            .entry(look.id())
            .or_insert_with(|| {
                materials.add(BeltMaterial {
                    base: base.clone(),
                    extension: BeltShoes {
                        table: buffer.clone(),
                    },
                })
            })
            .clone()
    }
}

/// `value` written as table record `record` of a page's bytes.
fn write_record(bytes: &mut [u8], record: u32, value: Vec4) {
    let start = record as usize * RECORD_BYTES;
    for (lane, component) in bytes[start..start + RECORD_BYTES]
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(value.to_array())
    {
        *lane = component.to_le_bytes();
    }
}

/// `frames`' header written at the start of `block`.
fn write_header(bytes: &mut [u8], block: &Range<u32>, frames: &BeltFrames) {
    let (correction, origin) = frames.frame().correction_and_origin();
    let header = [
        correction.x_axis.extend(0.0),
        correction.y_axis.extend(0.0),
        correction.z_axis.extend(0.0),
        origin.extend(frames.lateral_x()),
    ];
    for (record, value) in (block.start..).zip(header) {
        write_record(bytes, record, value);
    }
}

/// `frames`' spans written after `block`'s header: one `(a.z, a.y, b.z, b.y)` per material link,
/// as many as the block holds.
fn write_spans(bytes: &mut [u8], block: &Range<u32>, frames: &BeltFrames) {
    for (record, link) in (block.start + HEADER_RECORDS..block.end).zip(0..frames.count()) {
        let [a, b] = frames.span(link).joints();
        write_record(bytes, record, Vec4::new(a.x, a.y, b.x, b.y));
    }
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
    table: ResMut<'w, BeltTable>,
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
    /// belt of its count has drawn rung 0 yet), its look's shared material, its block of the table
    /// (`MeshTag` and [`BeltDraw`]) written with its header and spans, and its bounds. `None` until
    /// [`Self::ready`].
    ///
    /// The block is lent here and given back when the [`BeltDraw`] leaves an entity, so a caller
    /// spawns or inserts the bundle it is handed.
    pub(crate) fn draw(
        &mut self,
        template: &LinkTemplate,
        side: Side,
        frames: &BeltFrames,
    ) -> Option<impl Bundle + use<>> {
        if !self.ready(template, frames.count()) {
            return None;
        }
        let records = HEADER_RECORDS.checked_add(u32::try_from(frames.count()).ok()?)?;
        let mesh = self
            .cache
            .rung_or_build(frames.count(), 0, side, template, &mut self.meshes)?;
        let reach = self.cache.reach(frames.count(), side)?;
        let base = self.standard.get(template.material())?;
        let (page, block) = self.table.lend(records, &mut self.buffers);
        let material = self
            .table
            .material(page, template.material(), base, &mut self.materials);
        if let Some(bytes) = self
            .buffers
            .get_mut(&self.table.pages[page].buffer)
            .and_then(|buffer| buffer.into_inner().data.as_mut())
        {
            write_header(bytes, &block, frames);
            write_spans(bytes, &block, frames);
        }
        let (low, high) = frames.bounds(reach);
        Some((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            MeshTag(block.start),
            BeltDraw { page, block, reach },
            NoAutoAabb,
            Aabb::from_min_max(low, high),
        ))
    }
}

/// Where a belt's changed table reaches its page and its bounds: after every table writer (each
/// consumer orders its writer before this set), before visibility reads the bounds, and before
/// `AssetEventSystems` flushes the page's `Modified` event, which is the only way a page reaches
/// the render world (`bevy_render`'s `extract_render_asset`). So a table, its bounds and the GPU's
/// copy of it all belong to the frame that wrote the table.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BeltUpload;

/// Write every changed table into its block and rewrite its bounds; mark each written page
/// modified once. A settled belt's table is unchanged ([`BeltFrames::fit`] marks nothing), so it
/// writes nothing.
fn upload_belt_frames(
    table: Res<BeltTable>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut belts: Query<(&BeltFrames, &BeltDraw, &mut Aabb), Changed<BeltFrames>>,
    mut written: Local<Vec<bool>>,
) {
    written.clear();
    written.resize(table.pages.len(), false);
    for (frames, draw, mut aabb) in &mut belts {
        if let Some(bytes) = table
            .pages
            .get(draw.page)
            .and_then(|page| buffers.get_mut_untracked(&page.buffer))
            .and_then(|buffer| buffer.data.as_mut())
        {
            write_spans(bytes, &draw.block, frames);
            written[draw.page] = true;
        }
        let (low, high) = frames.bounds(draw.reach);
        aabb.set_if_neq(Aabb::from_min_max(low, high));
    }
    for (page, _) in table.pages.iter().zip(written.iter()).filter(|(_, w)| **w) {
        if let Some(buffer) = buffers.get_mut(&page.buffer) {
            buffer.into_inner();
        }
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

/// The belt draw's material, mesh cache, table and upload. Mounted by
/// [`super::link_view::template_plugin`].
pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(MaterialPlugin::<BeltMaterial>::default());
    schedule(app);
}

/// Everything [`plugin`] mounts but the material's render plugin.
fn schedule(app: &mut App) {
    app.init_resource::<BeltMeshes>()
        .init_resource::<BeltTable>()
        .configure_sets(
            PostUpdate,
            BeltUpload
                .before(VisibilitySystems::CheckVisibility)
                .before(AssetEventSystems),
        )
        .add_systems(PostUpdate, upload_belt_frames.in_set(BeltUpload))
        .add_systems(Last, evict_unused_counts);
}

#[cfg(test)]
mod tests {
    use super::super::link_view::{LinkFrame, Stragglers};
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

    /// A world holding every asset store and resource a belt's draw takes, and a template of
    /// `levels` fixture shoes in a real `StandardMaterial`.
    fn draw_world(levels: usize) -> World {
        let mut world = World::new();
        let mut meshes = Assets::<Mesh>::default();
        let mut standard = Assets::<StandardMaterial>::default();
        let look = standard.add(StandardMaterial::default());
        let shoes = (0..levels)
            .map(|_| PerSide::new(meshes.add(fixture_shoe()), meshes.add(fixture_shoe())))
            .collect();
        world.insert_resource(LinkTemplate::fixture(shoes, look));
        world.insert_resource(meshes);
        world.insert_resource(standard);
        world.insert_resource(Assets::<BeltMaterial>::default());
        world.insert_resource(Assets::<ShaderBuffer>::default());
        world.init_resource::<BeltMeshes>();
        world.init_resource::<BeltTable>();
        world
    }

    /// Spawn a belt drawing `frames` on `side`, as a consumer does.
    fn spawn_drawn(world: &mut World, side: Side, frames: BeltFrames) -> Entity {
        use bevy::ecs::system::RunSystemOnce;
        world
            .run_system_once_with(
                |In((side, frames)): In<(Side, BeltFrames)>,
                 template: Res<LinkTemplate>,
                 mut assets: BeltAssets,
                 mut commands: Commands| {
                    let draw = assets.draw(&template, side, &frames)?;
                    Some(commands.spawn((frames, draw)).id())
                },
                (side, frames),
            )
            .expect("the draw runs")
            .expect("a ready template draws")
    }

    /// Table record `record` of a page's bytes.
    fn record(bytes: &[u8], record: u32) -> Vec4 {
        let start = record as usize * RECORD_BYTES;
        Vec4::from_array(std::array::from_fn(|lane| {
            let at = start + 4 * lane;
            f32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
        }))
    }

    /// ONE MATERIAL PER LOOK, ONE BLOCK PER BELT. Belts of different tanks must wear the same
    /// material and, at one count, rung and side, the same mesh: bevy batches on exactly those two
    /// (`Opaque3dBatchSetKey` keys the material's bind group, the bin the mesh), so a material per
    /// belt costs a batch set per belt. Each belt's records live in its own block, at the record
    /// its `MeshTag` names, which is all the vertex stage has to find them by: blocks that overlap
    /// would draw one belt's shoes on another's spans. A despawned belt's block goes to the next
    /// belt, so a respawn does not grow the table.
    #[test]
    fn belts_share_one_material_and_each_reads_its_own_block() {
        const COUNT: usize = 6;
        const PITCH: f32 = 0.1;
        let mut world = draw_world(2);
        let table = |lateral_x: f32, lift: f32| {
            let stations: Vec<Vec2> = (0..COUNT)
                .map(|i| Vec2::new(i as f32 * PITCH, lift + (i % 2) as f32 * 0.03))
                .collect();
            let mut frames = BeltFrames::new(LinkFrame::default(), lateral_x, COUNT);
            frames.fit(&stations, 0.0, PITCH, Stragglers::Keep);
            frames
        };
        let belts = [
            spawn_drawn(&mut world, Side::Left, table(-1.5, 0.0)),
            spawn_drawn(&mut world, Side::Right, table(1.5, 0.2)),
            spawn_drawn(&mut world, Side::Left, table(-1.5, 0.4)),
        ];

        let material = |belt: Entity| {
            world
                .get::<MeshMaterial3d<BeltMaterial>>(belt)
                .expect("a drawn belt")
                .id()
        };
        assert!(
            belts
                .iter()
                .all(|&belt| material(belt) == material(belts[0])),
            "every belt of one look wears one material",
        );
        let mesh = |belt: Entity| world.get::<Mesh3d>(belt).expect("a drawn belt").id();
        assert_eq!(
            mesh(belts[0]),
            mesh(belts[2]),
            "one count, rung and side, one mesh"
        );

        let tag = |world: &World, belt: Entity| world.get::<MeshTag>(belt).expect("a tag").0;
        let page = world.resource::<BeltTable>().pages[0].buffer.clone();
        let bytes = world
            .resource::<Assets<ShaderBuffer>>()
            .get(&page)
            .expect("the page")
            .data
            .clone()
            .expect("a main-world page");
        let mut starts: Vec<u32> = belts.iter().map(|&belt| tag(&world, belt)).collect();
        for &belt in &belts {
            let block = tag(&world, belt);
            let frames = world.get::<BeltFrames>(belt).expect("its table");
            let (correction, origin) = frames.frame().correction_and_origin();
            assert_eq!(record(&bytes, block), correction.x_axis.extend(0.0));
            assert_eq!(
                record(&bytes, block + 3),
                origin.extend(frames.lateral_x()),
                "the header carries this belt's lateral centre",
            );
            for link in 0..COUNT {
                let [a, b] = frames.span(link).joints();
                assert_eq!(
                    record(&bytes, block + HEADER_RECORDS + link as u32),
                    Vec4::new(a.x, a.y, b.x, b.y),
                    "link {link} reads its own span",
                );
            }
        }
        starts.sort_unstable();
        assert!(
            starts
                .windows(2)
                .all(|pair| pair[1] - pair[0] >= HEADER_RECORDS + COUNT as u32),
            "blocks never overlap: {starts:?}",
        );

        let freed = tag(&world, belts[1]);
        world.despawn(belts[1]);
        let next = spawn_drawn(&mut world, Side::Right, table(1.5, 0.6));
        assert_eq!(
            tag(&world, next),
            freed,
            "a despawned belt's block is lent again"
        );
        assert_eq!(world.resource::<BeltTable>().pages.len(), 1);
    }

    /// A CHANGED TABLE REACHES THE RENDER WORLD IN THE FRAME IT CHANGED. A page reaches the render
    /// world only through its `AssetEvent`, flushed in `AssetEventSystems`; an upload after the
    /// flush would leave the first frame of every change (motion onset, a teleport, a respawn)
    /// drawing last frame's spans under this frame's hull, wheels, bounds and prints. The schedule
    /// must order the upload before the flush (an unordered pair can run either way, frame to
    /// frame), and a reader in `Last`, after this frame's flush, must see the page modified in the
    /// frame its table changed, with the new spans in it, and see nothing in a frame no table
    /// changed.
    #[test]
    fn a_changed_table_reaches_the_render_world_in_the_frame_it_changed() {
        use bevy::ecs::schedule::{NodeId, Schedules};

        /// Whether `PostUpdate` orders set `first` ahead of set `then` by a path of ordering edges.
        fn ordered(app: &App, first: impl SystemSet, then: impl SystemSet) -> bool {
            let graph = app
                .world()
                .resource::<Schedules>()
                .get(PostUpdate)
                .expect("PostUpdate")
                .graph();
            let node = |set| graph.system_sets.get_key(set).map(NodeId::Set);
            let (Some(from), Some(to)) = (node(first.intern()), node(then.intern())) else {
                return false;
            };
            super::super::schedule_path::orders(graph, from, to)
        }

        #[derive(Resource, Default)]
        struct Flushed(Vec<AssetId<ShaderBuffer>>);
        fn flushed(mut events: MessageReader<AssetEvent<ShaderBuffer>>, mut seen: ResMut<Flushed>) {
            seen.0 = events
                .read()
                .filter_map(|event| match event {
                    AssetEvent::Modified { id } => Some(*id),
                    _ => None,
                })
                .collect();
        }

        const COUNT: usize = 5;
        const PITCH: f32 = 0.1;
        let mut app = App::new();
        app.add_plugins((TaskPoolPlugin::default(), AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<BeltMaterial>()
            .init_asset::<ShaderBuffer>()
            .init_resource::<Flushed>()
            .add_systems(Last, flushed);
        schedule(&mut app);
        let world = app.world_mut();
        let mut meshes = world.resource_mut::<Assets<Mesh>>();
        let shoes = vec![PerSide::new(
            meshes.add(fixture_shoe()),
            meshes.add(fixture_shoe()),
        )];
        let look = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        world.insert_resource(LinkTemplate::fixture(shoes, look));
        let belt = spawn_drawn(
            app.world_mut(),
            Side::Right,
            BeltFrames::new(LinkFrame::default(), 1.5, COUNT),
        );
        let page = app.world().resource::<BeltTable>().pages[0].buffer.id();
        app.update();
        assert!(
            ordered(&app, BeltUpload, AssetEventSystems),
            "the upload is ordered before the asset-event flush",
        );
        app.update();
        assert!(
            app.world().resource::<Flushed>().0.is_empty(),
            "a frame no table changed sends no page",
        );

        let stations: Vec<Vec2> = (0..COUNT)
            .map(|i| Vec2::new(i as f32 * PITCH, 0.3))
            .collect();
        assert!(
            app.world_mut()
                .get_mut::<BeltFrames>(belt)
                .expect("the table")
                .fit(&stations, 0.0, PITCH, Stragglers::Keep)
        );
        app.update();
        assert_eq!(
            app.world().resource::<Flushed>().0,
            vec![page],
            "the page reaches the render world in the frame its table changed, once",
        );
        let block = app.world().get::<MeshTag>(belt).expect("a tag").0;
        let bytes = app
            .world()
            .resource::<Assets<ShaderBuffer>>()
            .get(page)
            .and_then(|page| page.data.clone())
            .expect("a main-world page");
        let [a, b] = app
            .world()
            .get::<BeltFrames>(belt)
            .expect("the table")
            .span(0)
            .joints();
        assert_eq!(
            record(&bytes, block + HEADER_RECORDS),
            Vec4::new(a.x, a.y, b.x, b.y)
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
