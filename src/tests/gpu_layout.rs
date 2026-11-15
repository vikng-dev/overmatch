//! Static layout gates for GPU data uploaded by copying its Rust representation.
//!
//! These tests need no adapter or GPU. They guard the contract between the Rust byte allocation
//! paths and Encase's shader layout before wgpu ever creates a buffer or bind group.

use std::any::type_name;
use std::mem::{align_of, offset_of, size_of};

use bevy::pbr::{
    GpuClusteredLight, MeshCullingData, MeshInputUniform, MeshUniform,
    decal::clustered::RenderClusteredDecal,
};
use bevy::render::{mesh::MeshMetadata, render_resource::ShaderType};

fn assert_rust_upload_matches_shader<T: ShaderType>() {
    assert_eq!(
        size_of::<T>() as u64,
        T::min_size().get(),
        "{} Rust upload size differs from its shader minimum size",
        type_name::<T>()
    );
}

#[test]
fn rust_repr_gpu_uploads_match_shader_layout_sizes() {
    // The audited Rust-repr uploads must agree with their Bevy 0.20.1 shader layouts.
    assert_rust_upload_matches_shader::<MeshMetadata>();
    assert_rust_upload_matches_shader::<MeshUniform>();
    assert_rust_upload_matches_shader::<MeshInputUniform>();
    assert_rust_upload_matches_shader::<MeshCullingData>();
    assert_rust_upload_matches_shader::<GpuClusteredLight>();
    assert_rust_upload_matches_shader::<RenderClusteredDecal>();
}

#[test]
fn mesh_uniform_scalar_math_layout_matches_bevy_shader_contract() {
    // DERIVED from the Bevy 0.20.1 field order and Encase's WGSL alignment rules.
    // The final metadata index leaves eight bytes of tail padding at shader alignment.
    assert_eq!(align_of::<MeshUniform>(), 16);
    assert_eq!(size_of::<MeshUniform>(), 176);
    assert_eq!(MeshUniform::min_size().get(), 176);
    assert_eq!(
        [
            offset_of!(MeshUniform, world_from_local),
            offset_of!(MeshUniform, previous_world_from_local),
            offset_of!(MeshUniform, local_from_world_transpose_a),
            offset_of!(MeshUniform, local_from_world_transpose_b),
            offset_of!(MeshUniform, flags),
            offset_of!(MeshUniform, lightmap_uv_rect),
            offset_of!(MeshUniform, first_vertex_index),
            offset_of!(MeshUniform, current_skin_index),
            offset_of!(MeshUniform, material_and_lightmap_bind_group_slot),
            offset_of!(MeshUniform, tag),
            offset_of!(MeshUniform, morph_descriptor_index),
            offset_of!(MeshUniform, metadata_index),
        ],
        [0, 48, 96, 128, 132, 136, 144, 148, 152, 156, 160, 164]
    );
}
