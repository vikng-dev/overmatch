//! The shared analytic terrain field (architecture §5): a [`BlockField`] rebuilt whenever the
//! [`TerrainMap`]'s revision changes, carrying BOTH ground terms — the authored blocks (the
//! colliders and this field derive from ONE block list) and the decoded `HeightGrid` (the same
//! immutable samples the collider, the render mesh, spawn placement and ballistics read). So
//! physics, sim forces, and the track view can never disagree about terrain.
//!
//! Mounted by `SimPlugin` (the plugin fn below), so it exists on the dedicated server, the SP
//! client, and the net client alike: the phase-B force systems read it inside `FixedUpdate`,
//! and the track view reads the SAME resource in `PostUpdate` — one oracle, everywhere.

use bevy::prelude::*;

use crate::world::TerrainMap;

use super::oracle::{BlockField, TerrainBlock};

/// The built field + the [`TerrainMap`] revision it was built from. `field` is `None` only
/// before the first build (or with no `TerrainMap` at all — e.g. bare test worlds); consumers
/// guard on it. A revision change (map load; future streaming/destruction) rebuilds, and
/// consumers that cache terrain-derived state key their reseed on `revision`.
#[derive(Resource, Default)]
pub struct TrackField {
    pub revision: Option<u64>,
    pub field: Option<BlockField>,
}

pub fn terrain_plugin(app: &mut App) {
    app.init_resource::<TrackField>();
    // PreUpdate: after `TerrainMap` exists (Startup), before this frame's FixedUpdate sim and
    // PostUpdate view both read the field. The revision check makes steady-state cost nil.
    app.add_systems(PreUpdate, build_track_field);
}

fn build_track_field(
    map: Option<Res<TerrainMap>>,
    grid: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut track: ResMut<TrackField>,
) {
    let Some(map) = map else {
        return;
    };
    if track.revision == Some(map.revision) {
        return;
    }
    let blocks = map
        .blocks
        .iter()
        .map(|t| TerrainBlock::new(t.translation, t.rotation, t.scale))
        .collect();
    // The heightmap ground term rides along when the grid decoded (Startup, before the first
    // PreUpdate build): an Arc-cheap clone of the same immutable samples every peer decoded.
    track.field = Some(BlockField::new(blocks).with_height(grid.map(|g| g.clone())));
    track.revision = Some(map.revision);
}
