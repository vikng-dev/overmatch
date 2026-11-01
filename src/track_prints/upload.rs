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

fn extract_writes(
    mut pending: ResMut<PendingWrites>,
    writes: Extract<Option<Res<PrintWrites>>>,
    prints: Extract<Option<Res<super::TrackPrints>>>,
) {
    let Some(prints) = prints.as_ref() else {
        pending.0.clear(); // No drawn heightmap is bound to a print store.
        return;
    };
    if let Some(writes) = writes.as_ref() {
        pending.0.extend(writes.0.iter().cloned());
    }
    // A replaced image may never have reached the GPU. Its queued writes must retire rather
    // than wait forever for a texture whose last handle left with the previous world.
    pending.0.retain(|tile| {
        tile.image == prints.near_image.id() || tile.image == prints.world_image.id()
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::render::MainWorld;

    /// Pending writes wait only for images still bound by the current construction. A reset
    /// before GPU creation cannot leave an immortal queue, or discard the new world's writes.
    #[test]
    fn a_world_change_retires_pending_writes_to_images_that_never_reached_the_gpu() {
        // ASSUMED fixture tiles: no renderer is needed to exercise extraction and queue ownership.
        let mut world = World::new();
        world.init_resource::<PendingWrites>();
        world.init_resource::<MainWorld>();
        let (old, current) = {
            let mut main = world.resource_mut::<MainWorld>();
            main.init_resource::<Assets<Image>>();
            let old = super::super::images(&mut main.resource_mut::<Assets<Image>>());
            let current = super::super::images(&mut main.resource_mut::<Assets<Image>>());
            let current_id = current.world.id();
            main.insert_resource(super::super::TrackPrints::new(
                current,
                Handle::default(),
                64.0,
            ));
            (old.near.id(), current_id)
        };
        let tile = |image| TileWrite {
            image,
            level: 0,
            origin: UVec2::ZERO,
            size: UVec2::ONE,
            bytes_per_texel: 1,
            data: Arc::from([255]),
        };
        world.resource_mut::<PendingWrites>().0.push(tile(old));
        world
            .resource_mut::<MainWorld>()
            .insert_resource(PrintWrites(vec![tile(old), tile(current)]));
        let mut schedule = Schedule::default();
        schedule.add_systems(extract_writes);
        schedule.run(&mut world);
        let pending = &world.resource::<PendingWrites>().0;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].image, current);
    }
}
