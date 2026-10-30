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

/// The repeated meshes, per link count and per rung, each side's — and each side's reach.
#[derive(Resource, Default)]
pub(crate) struct BeltMeshes {
    by_count: HashMap<usize, Vec<PerSide<Handle<Mesh>>>>,
    reach: Option<PerSide<f32>>,
}

impl BeltMeshes {
    /// The mesh a belt of `count` links draws at `rung` on `side`; a rung past the floor draws the
    /// floor ([`LinkTemplate::rung_mesh`]).
    pub(crate) fn rung(&self, count: usize, rung: usize, side: Side) -> Option<&Handle<Mesh>> {
        let rungs = self.by_count.get(&count)?;
        Some(rungs[rung.min(rungs.len().checked_sub(1)?)].get(side))
    }

    /// A fixture's meshes for belts of `count` links, one per rung, without building any.
    #[cfg(test)]
    pub(crate) fn insert_fixture(&mut self, count: usize, rungs: Vec<PerSide<Handle<Mesh>>>) {
        self.by_count.insert(count, rungs);
    }

    /// Build every rung's repeated mesh for `count` links, once per count. `None` while a rung's
    /// shoe has not loaded, or for a shoe that cannot be repeated (refused loudly, once).
    fn ensure(
        &mut self,
        count: usize,
        template: &LinkTemplate,
        meshes: &mut Assets<Mesh>,
    ) -> Option<()> {
        if self.by_count.contains_key(&count) {
            return Some(());
        }
        let mut rungs = Vec::with_capacity(template.levels());
        let mut reach = PerSide::new(0.0_f32, 0.0_f32);
        for rung in 0..template.levels() {
            let mut built = Vec::with_capacity(2);
            for side in Side::ALL {
                let shoe = meshes.get(&template.rung_mesh(rung, side))?;
                let (_, origin) = template.frame(side).correction_and_origin();
                let side_reach = reach.get_mut(side);
                *side_reach = side_reach.max(canonical_reach(shoe, origin)?);
                match repeated_shoe(shoe, count) {
                    Ok(mesh) => built.push(mesh),
                    Err(why) => {
                        error_once!("track belt: rung {rung} cannot be repeated ({why}) — no belt");
                        return None;
                    }
                }
            }
            let right = meshes.add(built.pop()?);
            let left = meshes.add(built.pop()?);
            rungs.push(PerSide::new(left, right));
        }
        info!(
            "track belt: {} rung meshes of {count} links per side built; shoe reach L {:.3} m R \
             {:.3} m MEASURED",
            rungs.len(),
            reach.get(Side::Left),
            reach.get(Side::Right),
        );
        self.reach = Some(reach);
        self.by_count.insert(count, rungs);
        Some(())
    }
}

/// The farthest any vertex of `shoe` stands from the canonical origin `origin` (mesh space), m:
/// the correction is a rotation, so mesh-space distance is canonical distance.
fn canonical_reach(shoe: &Mesh, origin: Vec3) -> Option<f32> {
    let Ok(VertexAttributeValues::Float32x3(positions)) =
        shoe.try_attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return None;
    };
    Some(
        positions
            .iter()
            .map(|&p| Vec3::from(p).distance(origin))
            .fold(0.0, f32::max),
    )
}

/// `shoe` repeated `count` times, copy `k` tagged as link `k` in [`ATTRIBUTE_BELT_LINK`]: every
/// attribute and index of the source, copy after copy, nothing transformed. Render-world only: no
/// CPU reader needs the stack, and it is the shoe's size times the link count.
fn repeated_shoe(shoe: &Mesh, count: usize) -> Result<Mesh, String> {
    if shoe.primitive_topology() != PrimitiveTopology::TriangleList {
        return Err("not a triangle list".into());
    }
    if shoe.try_attributes().is_err() {
        return Err("its data lives in the render world only".into());
    }
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
    /// Whether a belt of `count` links can be drawn: every rung's mesh built and the shoe's
    /// material loaded. A consumer checks this before spawning anything, so a bind never half-lands.
    pub(crate) fn ready(&mut self, template: &LinkTemplate, count: usize) -> bool {
        self.cache
            .ensure(count, template, &mut self.meshes)
            .is_some()
            && self.standard.contains(template.material())
    }

    /// Everything a belt drawing `frames` on `side` carries, at rung 0: its mesh, its own material
    /// and buffer, and its bounds. `None` until [`Self::ready`].
    pub(crate) fn draw(
        &mut self,
        template: &LinkTemplate,
        side: Side,
        frames: &BeltFrames,
    ) -> Option<impl Bundle + use<>> {
        if !self.ready(template, frames.count()) {
            return None;
        }
        let mesh = self.cache.rung(frames.count(), 0, side)?.clone();
        let reach = *self.cache.reach.as_ref()?.get(side);
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

/// The belt draw's material, mesh cache and upload. Mounted by [`super::link_view::template_plugin`].
pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(MaterialPlugin::<BeltMaterial>::default())
        .init_resource::<BeltMeshes>()
        .configure_sets(
            PostUpdate,
            BeltUpload.before(VisibilitySystems::CheckVisibility),
        )
        .add_systems(PostUpdate, upload_belt_frames.in_set(BeltUpload));
}

#[cfg(test)]
mod tests {
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
