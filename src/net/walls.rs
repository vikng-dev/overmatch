//! The wall ledger, about the game's environment destruction (game state only): one globally
//! visible entity whose component lists every change the authority made to every building
//! ([`WallEntry`]: a swap, dust, a crack or a fall, with its building's ordinal and tick),
//! replicated as diffs — a client whose acknowledged ledger is within the last
//! `Diffable::HISTORY_LEN` (64) appends receives only the entries past it, one further behind and
//! a late joiner the whole list as a snapshot. A client copies it into its [`WallRecord`], from
//! which `buildings::breakage` derives the colliders and `buildings::view` the drawn walls: the
//! fates, cracks and swaps the clients derive are the authority's, entry for entry. A building's
//! index is its `MapInstance.ordinal` (bound into the map digest the handshake compares); a chunk's
//! and a joint's are the bake's, read from the same building file on both ends.
//!
//! A fallen group rides its own carrier (`debris_stress::Carrier`), its chunks on it once.

use bevy::ecs::error::Result;
use bevy::prelude::*;
use bevy_replicon::prelude::{Diffable, EntityCommandsDiffExt};
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::buildings::breakage::{WallEntry, WallLawSet, WallRecord, WallReplicaSet};

/// Every change so far, in authority order. Appended only through replicon's diff path,
/// `Diffable::HISTORY_LEN` left at its default (64).
#[derive(Component, Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct WallLedger(pub(crate) Vec<WallEntry>);

impl Diffable for WallLedger {
    type Diff = WallEntry;

    fn apply_diff(&mut self, diff: &WallEntry) -> Result<()> {
        self.0.push(*diff);
        Ok(())
    }
}

pub(super) fn install_server(app: &mut App) {
    app.add_systems(Startup, spawn_ledger)
        .add_systems(FixedUpdate, publish_record.after(WallLawSet));
}

pub(super) fn install_client(app: &mut App) {
    app.add_systems(Update, receive_ledger.before(WallReplicaSet));
}

fn spawn_ledger(mut commands: Commands) {
    commands.spawn((
        Name::new("WallLedger"),
        WallLedger::default(),
        Replicate::to_clients(NetworkTarget::All),
    ));
}

/// Authority: every record entry not yet on the ledger becomes a diff.
fn publish_record(
    record: Res<WallRecord>,
    ledgers: Query<(Entity, &WallLedger)>,
    mut published: Local<usize>,
    mut commands: Commands,
) {
    let Ok((ledger, _)) = ledgers.single() else {
        return;
    };
    for entry in record.entries.get(*published..).unwrap_or_default() {
        commands.entity(ledger).apply_diff::<WallLedger>(*entry);
    }
    *published = record.entries.len();
}

/// Replica: the ledger's new entries into the record. A ledger is one replicated entity per link —
/// lightyear despawns every replicated entity when a link ends and the next server's arrives as a
/// new one — so another entity, or a ledger shorter than what was copied, is another authority's
/// record: the record is replaced and its generation moves, and every building stands whole again
/// before the new record is applied.
fn receive_ledger(
    ledgers: Query<(Entity, &WallLedger)>,
    mut copied: Local<(Option<Entity>, usize)>,
    mut record: ResMut<WallRecord>,
) {
    let Ok((ledger, entries)) = ledgers.single() else {
        return;
    };
    if copied.0 != Some(ledger) || entries.0.len() < copied.1 {
        if copied.0 == Some(ledger) {
            error!(
                "wall ledger: {} entries after {} copied; the record shrank",
                entries.0.len(),
                copied.1
            );
        }
        record.entries.clear();
        record.generation = record.generation.wrapping_add(1);
        *copied = (Some(ledger), 0);
    }
    if entries.0.len() > copied.1 {
        record.entries.extend_from_slice(&entries.0[copied.1..]);
        copied.1 = entries.0.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildings::breakage::WallEvent;

    fn entry(event: WallEvent, tick: u32) -> WallEntry {
        WallEntry {
            building: 1,
            event,
            tick,
        }
    }

    /// THE RECORD IS THE LEDGER, ENTRY FOR ENTRY, AND ANOTHER LEDGER REPLACES IT: a link's ledger
    /// growing by diff reaches the record in order; the next link's ledger (a new entity, shorter)
    /// replaces the record and moves its generation, so every building derived from the old one
    /// stands whole again before the new one applies. A replica that appended the new ledger to
    /// the old record would keep a previous server's holes.
    #[test]
    fn the_record_follows_the_ledger_and_another_ledger_replaces_it() {
        let mut app = App::new();
        app.init_resource::<WallRecord>()
            .add_systems(Update, receive_ledger);
        let first = app
            .world_mut()
            .spawn(WallLedger(vec![entry(WallEvent::Swap(0), 3)]))
            .id();
        app.update();
        app.world_mut()
            .get_mut::<WallLedger>(first)
            .unwrap()
            .apply_diff(&entry(WallEvent::Dust(4), 5))
            .unwrap();
        app.update();
        let record = app.world().resource::<WallRecord>().clone();
        assert_eq!(
            record.entries,
            vec![entry(WallEvent::Swap(0), 3), entry(WallEvent::Dust(4), 5)]
        );
        let generation = record.generation;
        app.world_mut().entity_mut(first).despawn();
        app.world_mut()
            .spawn(WallLedger(vec![entry(WallEvent::Crack(2), 9)]));
        app.update();
        let record = app.world().resource::<WallRecord>();
        assert_eq!(record.entries, vec![entry(WallEvent::Crack(2), 9)]);
        assert_ne!(record.generation, generation);
    }
}
