//! Static layout gates for GPU data uploaded by copying its Rust representation.
//!
//! These tests need no adapter or GPU. They guard the contract between the Rust byte allocation
//! paths and Encase's shader layout before wgpu ever creates a buffer or bind group.

use std::any::type_name;
use std::mem::size_of;

use bevy::pbr::{
    GpuClusteredLight, MeshCullingData, MeshInputUniform, decal::clustered::RenderClusteredDecal,
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
    assert_rust_upload_matches_shader::<MeshInputUniform>();
    assert_rust_upload_matches_shader::<MeshCullingData>();
    assert_rust_upload_matches_shader::<GpuClusteredLight>();
    assert_rust_upload_matches_shader::<RenderClusteredDecal>();
}
