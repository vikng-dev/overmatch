//! The destruction ledger: one globally visible entity whose component lists every tree state
//! transition the authority has made, replicated as diffs — a fall costs one entry on the wire, a
//! late joiner receives the list once, and replicon's per-tick change scan touches one entity
//! instead of one per tree.
//!
//! An entry is `(index, word, tick)`: the tree's `MapInstance.ordinal` (id-sorted, dense, bound
//! into the map digest the handshake compares, so both peers number the same trees the same way
//! or never match), `TreeState::encode`'s word, and the authority tick of the transition, which
//! the fall presentation fuses to the interpolation cursor (ADR-0037). The replica decodes the
//! word against its own copy of the tree and refuses one that names no state of it; everything
//! else about a fallen tree — colliders, pool, pose, view — it derives (ADR-0016).

use std::collections::HashMap;

use bevy::ecs::error::Result;
use bevy::prelude::*;
use bevy_replicon::prelude::{Diffable, EntityCommandsDiffExt};
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::scatter::MapInstance;
use crate::trees::{FallClock, Fell, TreeBody, TreeFallSet, TreeState};

/// One transition: which tree, what state, which authority tick.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LedgerEntry {
    pub(crate) index: u16,
    pub(crate) word: u16,
    pub(crate) tick: u32,
}

/// Every transition so far, in authority order. Appended only through replicon's diff path, so
/// the wire carries the appended entry and a fresh replica the whole list.
#[derive(Component, Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DestructionLedger(pub(crate) Vec<LedgerEntry>);

impl Diffable for DestructionLedger {
    type Diff = LedgerEntry;

    fn apply_diff(&mut self, diff: &LedgerEntry) -> Result<()> {
        self.0.push(*diff);
        Ok(())
    }
}

/// How many entries this replica has applied to its trees.
#[derive(Resource, Default)]
struct Applied(usize);

pub(super) fn install_server(app: &mut App) {
    app.add_systems(Startup, spawn_ledger)
        .add_systems(FixedPostUpdate, publish_falls.after(TreeFallSet));
}

pub(super) fn install_client(app: &mut App) {
    app.init_resource::<Applied>()
        .init_resource::<FallClock>()
        .add_systems(Update, (apply_ledger, publish_fall_clock));
}

fn spawn_ledger(mut commands: Commands) {
    commands.spawn((
        Name::new("DestructionLedger"),
        DestructionLedger::default(),
        Replicate::to_clients(NetworkTarget::All),
    ));
}

/// Authority: every tree whose state left `Standing` this tick becomes an entry.
fn publish_falls(
    fallen: Query<(&MapInstance, &TreeState, &Fell), Changed<TreeState>>,
    ledgers: Query<Entity, With<DestructionLedger>>,
    mut commands: Commands,
) {
    let Ok(ledger) = ledgers.single() else {
        return;
    };
    for (identity, state, fell) in &fallen {
        if *state == TreeState::Standing {
            continue;
        }
        commands
            .entity(ledger)
            .apply_diff::<DestructionLedger>(LedgerEntry {
                index: identity.ordinal,
                word: state.encode(),
                tick: fell.tick,
            });
    }
}

/// Replica: apply every entry not yet applied to the tree its index names. A word no state of
/// that tree encodes to is refused and logged, never clamped into a pose the authority did not
/// set; an index no tree carries is a map mismatch the handshake should have refused.
fn apply_ledger(
    ledgers: Query<&DestructionLedger>,
    mut applied: ResMut<Applied>,
    mut trees: Query<(Entity, &MapInstance, &TreeBody, &mut TreeState)>,
    mut commands: Commands,
) {
    let Ok(ledger) = ledgers.single() else {
        return;
    };
    if ledger.0.len() < applied.0 {
        // A fresh ledger (a reconnect replaced the entity): its list starts over.
        applied.0 = 0;
    }
    if ledger.0.len() == applied.0 {
        return;
    }
    let by_ordinal: HashMap<u16, Entity> = trees
        .iter()
        .map(|(entity, identity, _, _)| (identity.ordinal, entity))
        .collect();
    for entry in &ledger.0[applied.0..] {
        let Some(&tree) = by_ordinal.get(&entry.index) else {
            error!("destruction ledger: entry {entry:?} names no tree of this map");
            continue;
        };
        let Ok((_, _, body, mut state)) = trees.get_mut(tree) else {
            continue;
        };
        match TreeState::decode(entry.word, body.kind.segments.len()) {
            Ok(decoded) => {
                *state = decoded;
                commands.entity(tree).insert(Fell { tick: entry.tick });
            }
            Err(reason) => error!("destruction ledger: entry {entry:?} refused: {reason}"),
        }
    }
    applied.0 = ledger.0.len();
}

/// Replica: the presentation clock is the interpolation cursor once it has synced — the time the
/// hulls draw at — and the local tick before that (announcements present at arrival, as the fire
/// queue's no-cursor fallback does).
fn publish_fall_clock(
    cursor: Option<Res<InterpolationTimeline>>,
    local: Res<LocalTimeline>,
    mut clock: ResMut<FallClock>,
) {
    let (tick, overstep) = match cursor.filter(|timeline| timeline.is_synced()) {
        Some(timeline) => (timeline.tick(), f64::from(timeline.overstep().to_f32())),
        None => (local.tick(), 0.0),
    };
    clock.tick = tick.0;
    clock.overstep = overstep;
}
