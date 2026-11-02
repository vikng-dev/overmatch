//! The hull marks' wire half (the visual layer of the game's damage model; the record is
//! `hull_marks`): each tank root's `HullMarks` replicates by diff — `net::protocol` registers it
//! public, under ADR-0039 — and the authority appends each strike's mark through replicon's diff
//! path, so a caught-up client receives the one new mark and a late joiner the held set as a
//! snapshot.
//!
//! A diff is a whole mark, applied by the same [`HullMarks::record`] the authority's own copy took,
//! so the replica's set, its drops past the cap included, is the authority's.

use bevy::ecs::error::Result;
use bevy::prelude::*;
use bevy_replicon::prelude::{Diffable, EntityCommandsDiffExt};

use crate::ballistics::ShellStrike;
use crate::hull_marks::{HullMark, HullMarks, MarkBuilder};

impl Diffable for HullMarks {
    type Diff = HullMark;

    fn apply_diff(&mut self, mark: &HullMark) -> Result<()> {
        self.record(*mark);
        Ok(())
    }
}

/// The authority's recorder: each strike's mark rides the diff path to every client.
pub(super) fn install_server(app: &mut App) {
    app.add_observer(record);
}

fn record(strike: On<ShellStrike>, builder: MarkBuilder, mut commands: Commands) {
    if let Some((root, mark)) = builder.mark(&strike) {
        commands.entity(root).apply_diff::<HullMarks>(mark);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_replicon::prelude::EntityDiffExt;
    use bevy_replicon::shared::replication::{
        diff::{ComponentDeltaRef, DiffHistory, diff_index::DiffIndex},
        storage::ReplicationStorage,
    };

    /// The game's damage-model view sends one strike per public diff. Measure the actual
    /// replicon delta encoding, including its tag, cursor and sequence length; packet/entity
    /// framing is shared by a batch and excluded. Fixture positions span the packed coordinate
    /// range, so postcard's variable-length integers are visible in the measurement.
    #[test]
    fn public_strike_diff_bytes() {
        let mut world = World::new();
        world.init_resource::<ReplicationStorage>();
        let root = world.spawn(HullMarks::default()).id();
        for n in [0, 1, 127, 32767] {
            let mark = crate::hull_marks::tests::numbered(n);
            let payload = postcard::to_allocvec(&mark).unwrap().len();
            let mut entity = world.entity_mut(root);
            entity.apply_diff::<HullMarks>(mark).unwrap();
            entity.apply_diff::<HullMarks>(mark).unwrap();
            let changed = entity.get_ref::<HullMarks>().unwrap().last_changed();
            let mut storage = world.resource_mut::<ReplicationStorage>();
            let history = storage.get_or_default::<DiffHistory<HullMarks>>(root);
            let cursor = DiffIndex::new(history.current_index().get().wrapping_sub(1));
            let (index, diffs) = history.diffs_after(Some(cursor), changed);
            assert_eq!(diffs.len(), 1, "measure a single strike, not a snapshot");
            let bytes =
                postcard::to_allocvec(&ComponentDeltaRef::<HullMarks>::Diffs { index, diffs })
                    .unwrap();
            eprintln!(
                "MEASURED public strike: fixture {n}, payload {payload} B, diff {} B",
                bytes.len()
            );
        }
    }
}
