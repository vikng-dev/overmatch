//! The wall ledger, about the game's environment destruction (game state only): one globally
//! visible entity whose component lists every change the authority made to every building
//! ([`WallEntry`]: a swap, dust, a hole, a crack or a fall, with its building's ordinal and tick),
//! replicated as diffs — a client whose acknowledged ledger is within the last
//! `Diffable::HISTORY_LEN` (64) appends receives only the entries past it, one further behind and
//! a late joiner the whole list as a snapshot. A client copies it into its [`WallRecord`], from
//! which `buildings::breakage` derives the colliders and `buildings::view` the drawn walls: the
//! fates, cracks, swaps and holes the clients derive are the authority's, entry for entry. A building's
//! index is its `MapInstance.ordinal` (bound into the map digest the handshake compares); a chunk's
//! and a joint's are the bake's, read from the same building file on both ends.
//!
//! A fallen group rides its own carrier (`debris_stress::Carrier`), its chunks on it once.

use bevy::ecs::error::Result;
use bevy::ecs::query::QuerySingleError;
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
    app.add_systems(Startup, spawn_ledger).add_systems(
        FixedPostUpdate,
        publish_record
            .after(WallLawSet)
            .in_set(crate::state::SimPhase::Publish),
    );
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
    ledgers: Query<Entity, With<WallLedger>>,
    mut published: Local<usize>,
    mut commands: Commands,
) {
    let ledger = ledgers.single().expect(
        "authority startup spawns exactly one wall ledger before publishing building changes",
    );
    let fresh = record.entries.get(*published..).expect(
        "the authority's wall record only grows: the laws extend it and nothing on the authority \
         removes an entry",
    );
    for entry in fresh {
        commands.entity(ledger).apply_diff::<WallLedger>(*entry);
    }
    *published = record.entries.len();
}

/// Replica: the ledger's new entries into the record. A ledger is one replicated entity per link —
/// lightyear despawns every replicated entity when a link ends and the next server's arrives as a
/// new one — so another entity is another authority's record: the record is replaced and its
/// generation moves, and every building stands whole again before the new record is applied.
///
/// A ledger shorter than what was copied from the same entity is a malformed peer record: the
/// authority only appends, and a replicon snapshot carries its whole list. It is logged, and the
/// record is replaced by the list as it now stands, as for another link's.
fn receive_ledger(
    ledgers: Query<(Entity, &WallLedger)>,
    mut copied: Local<(Option<Entity>, usize)>,
    mut record: ResMut<WallRecord>,
) {
    let (ledger, entries) = match ledgers.single() {
        Ok(ledger) => ledger,
        // Before the first snapshot, or between links, no authority ledger has arrived yet.
        Err(QuerySingleError::NoEntities(_)) => return,
        Err(QuerySingleError::MultipleEntities(_)) => panic!(
            "a replica has one active wall ledger; disconnect cleanup removes the old link's \
             replicated entities before a replacement arrives"
        ),
    };
    if copied.0 != Some(ledger) || entries.0.len() < copied.1 {
        if copied.0 == Some(ledger) {
            error!(
                "wall ledger: malformed peer record: {} entries after {} copied from the same \
                 ledger; replacing the record with it",
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
    use crate::buildings::breakage::{Hole, WallEvent};

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

    /// Authority publication cannot silently stop without its startup singleton. A mutant that
    /// returns on a missing ledger would leave every client's buildings standing unseen.
    #[test]
    #[should_panic(expected = "authority startup spawns exactly one wall ledger")]
    fn publishing_without_the_startup_ledger_is_a_bug() {
        let mut app = App::new();
        app.init_resource::<WallRecord>()
            .add_systems(Update, publish_record);
        app.update();
    }

    /// Two active ledgers would mix authorities' records. A single-query skip mutant would hide
    /// the composition bug and freeze every replica building on its previous record.
    #[test]
    #[should_panic(expected = "a replica has one active wall ledger")]
    fn a_replica_with_two_wall_ledgers_is_a_bug() {
        let mut app = App::new();
        app.init_resource::<WallRecord>()
            .add_systems(Update, receive_ledger);
        for _ in 0..2 {
            app.world_mut().spawn(WallLedger::default());
        }
        app.update();
    }

    /// A HOLE CROSSES THE WIRE WHOLE, IN A LATE JOINER'S SNAPSHOT TOO: a ledger holding hole
    /// entries, encoded as replicon encodes it (postcard) and decoded, is the same ledger; so is an
    /// entry sent alone as a diff. One hole entry costs 20 bytes for the fixture cut (a chunk index
    /// past 127, millimetres past ±63, a tick past four minutes at 64 Hz) and 33 at most. The
    /// replicas' holes are these entries, and [`crate::buildings::breakage::HOLES_PER_CHUNK`]
    /// bounds the record by them.
    #[test]
    fn a_hole_crosses_the_wire_whole() {
        let typical = WallEntry {
            building: 3,
            event: WallEvent::Hole(
                200,
                Hole {
                    from: [-120, 80, 100],
                    to: [-118, 81, -100],
                    radius: 51,
                },
            ),
            tick: 20_000,
        };
        let widest = WallEntry {
            building: u16::MAX,
            event: WallEvent::Hole(
                u16::MAX,
                Hole {
                    from: [i16::MIN; 3],
                    to: [i16::MAX; 3],
                    radius: u16::MAX,
                },
            ),
            tick: u32::MAX,
        };
        let ledger = WallLedger(vec![typical, widest]);
        let bytes = postcard::to_allocvec(&ledger).unwrap();
        assert_eq!(postcard::from_bytes::<WallLedger>(&bytes).unwrap(), ledger);
        let sent = postcard::to_allocvec(&typical).unwrap();
        assert_eq!(postcard::from_bytes::<WallEntry>(&sent).unwrap(), typical);
        assert_eq!(sent.len(), 20);
        assert_eq!(postcard::to_allocvec(&widest).unwrap().len(), 33);
    }
}
