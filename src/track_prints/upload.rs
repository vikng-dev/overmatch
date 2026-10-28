//! The render world's half of the prints: each frame's store tiles ([`PrintWrites`], built in the
//! main world) written into the two GPU images the ground's material reads, by `RenderQueue`
//! texture writes in `PrepareResources`, before the frame's own passes. A write whose image has
//! no GPU texture yet waits for it, in order; writes are whole tiles, so a later write of a tile
//! replaces an earlier one. Nothing without a renderer.

use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    Extent3d, Origin3d, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
};
use bevy::render::renderer::RenderQueue;
use bevy::render::texture::GpuImage;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};

/// One tile of one mip level of a print image, its texels row by row.
#[derive(Clone)]
pub(crate) struct TileWrite {
    pub(crate) image: AssetId<Image>,
    pub(crate) level: u32,
    pub(crate) origin: UVec2,
    pub(crate) size: UVec2,
    pub(crate) bytes_per_texel: u32,
    pub(crate) data: Arc<[u8]>,
}

/// The tiles this frame wrote, replaced whole every frame: the extract takes each frame's once.
#[derive(Resource, Default)]
pub(crate) struct PrintWrites(pub(crate) Vec<TileWrite>);

/// The render world's queue: extracted tiles not yet written.
#[derive(Resource, Default)]
struct PendingWrites(Vec<TileWrite>);

fn extract_writes(mut pending: ResMut<PendingWrites>, writes: Extract<Option<Res<PrintWrites>>>) {
    if let Some(writes) = writes.as_ref() {
        pending.0.extend(writes.0.iter().cloned());
    }
}

fn write_tiles(
    mut pending: ResMut<PendingWrites>,
    images: Res<RenderAssets<GpuImage>>,
    queue: Res<RenderQueue>,
) {
    pending.0.retain(|tile| {
        let Some(gpu) = images.get(tile.image) else {
            return true;
        };
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &gpu.texture,
                mip_level: tile.level,
                origin: Origin3d {
                    x: tile.origin.x,
                    y: tile.origin.y,
                    z: 0,
                },
                aspect: TextureAspect::All,
            },
            &tile.data,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(tile.size.x * tile.bytes_per_texel),
                rows_per_image: Some(tile.size.y),
            },
            Extent3d {
                width: tile.size.x,
                height: tile.size.y,
                depth_or_array_layers: 1,
            },
        );
        false
    });
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<PrintWrites>();
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<PendingWrites>()
        .add_systems(ExtractSchedule, extract_writes)
        .add_systems(Render, write_tiles.in_set(RenderSystems::PrepareResources));
}
