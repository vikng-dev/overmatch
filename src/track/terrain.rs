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

/// This collider is already represented in the belt's terrain field (heightfield or authored
/// block). Solver contacts still use it; the belt must not count the collider a second time.
#[derive(Component)]
pub(crate) struct FieldSurface;

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
    ground: Option<Res<crate::ground_field::GroundField>>,
    mut track: ResMut<TrackField>,
) {
    let Some(map) = map else {
        // Bare test worlds may carry no terrain map and therefore publish no field.
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
    // PreUpdate build): an Arc-cheap clone of the same immutable samples every peer decoded. The
    // ground field is required whenever a TerrainMap is published.
    track.field = Some(
        BlockField::new(blocks)
            .with_height(grid.map(|g| g.clone()))
            .with_ground(Some(ground.expect("a published TerrainMap requires its decoded GroundField; insert both at world construction").clone())),
    );
    track.revision = Some(map.revision);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Simulation must refuse a partial world composition before using a fixture's rigid
    /// pair as ground truth. Server and sandbox construction rely on this publication gate.
    #[test]
    #[should_panic(expected = "a published TerrainMap requires its decoded GroundField")]
    fn a_published_map_requires_its_ground_facts() {
        let mut world = World::new();
        world.insert_resource(TerrainMap {
            revision: 0,
            blocks: Vec::new(),
        });
        world.init_resource::<TrackField>();
        world
            .run_system_once(build_track_field)
            .expect("the publication system's parameters are present");
    }
}
