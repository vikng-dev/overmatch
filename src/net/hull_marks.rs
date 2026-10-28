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
