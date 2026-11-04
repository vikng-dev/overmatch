//! The construction ledger, about the game's environment destruction (game state only): one globally
//! visible entity per Battle whose component lists every change the authority made to every
//! construction in that Battle's world (`net::world_reset` replaces it at the next Battle)
//! ([`Entry`]: a swap, dust, a hole, a crack or a fall, with its construction's ordinal and tick),
//! replicated as diffs — a client whose acknowledged ledger is within the last
//! `Diffable::HISTORY_LEN` (64) appends receives only the entries past it, one further behind and
//! a late joiner the whole list as a snapshot. A client copies it into its [`Record`], from
//! which `construction::breakage` derives the colliders and `construction::view` the drawn constructions: the
//! fates, cracks, swaps and holes the clients derive are the authority's, entry for entry. A construction's
//! index is its `MapInstance.ordinal` (bound into the map digest the handshake compares); a chunk's
//! and a joint's are the bake's, read from the same construction file on both ends.
//!
//! A fallen group rides its own carrier (`debris_stress::Carrier`), its chunks on it once.

use bevy::ecs::error::Result;
use bevy::prelude::*;
use bevy_replicon::prelude::{Diffable, EntityCommandsDiffExt};
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use super::world_reset::AuthorityWorld;
use crate::construction::breakage::{Breakage, Entry, LawSet, Layout, Piece, Record, ReplicaSet};
use crate::world::{CurrentWorld, WorldGeneration};
use std::collections::HashMap;

/// Every change in this world, in authority order. Appended only through replicon's diff path,
/// `Diffable::HISTORY_LEN` left at its default (64).
#[derive(Component, Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Ledger(pub(crate) Vec<Entry>);

impl Diffable for Ledger {
    type Diff = Entry;

    fn apply_diff(&mut self, diff: &Entry) -> Result<()> {
        self.0.push(*diff);
        Ok(())
    }
}

pub(super) fn install_server(app: &mut App) {
    super::world_reset::register_constructor(app, spawn_ledger);
    app.add_systems(
        FixedPostUpdate,
        publish_record
            .after(LawSet)
            .in_set(crate::state::SimPhase::Publish),
    );
}

pub(super) fn install_client(app: &mut App) {
    app.init_resource::<CurrentWorld>()
        .add_systems(Update, receive_ledger.before(ReplicaSet));
}

fn spawn_ledger(world: &mut World) {
    let generation = world.resource::<AuthorityWorld>().0;
    world.spawn((
        Name::new("Ledger"),
        Ledger::default(),
        generation,
        Replicate::to_clients(NetworkTarget::All),
    ));
}

/// Authority: every record entry not yet on the ledger becomes a diff.
fn publish_record(
    record: Res<Record>,
    ledgers: Query<Entity, With<Ledger>>,
    mut published: Local<(Option<Entity>, usize)>,
    mut commands: Commands,
) {
    let ledger = ledgers.single().expect(
        "authority startup spawns exactly one construction ledger before publishing construction changes",
    );
    if published.0 != Some(ledger) {
        *published = (Some(ledger), 0);
    }
    let fresh = record.entries.get(published.1..).expect(
        "the authority's construction record only grows: the laws extend it and nothing on the authority \
         removes an entry",
    );
    for entry in fresh {
        commands.entity(ledger).apply_diff::<Ledger>(*entry);
    }
    published.1 = record.entries.len();
}

/// Replica: only the selected world's ledger enters the record. A new Battle or a new link owns
/// a new ledger entity and diff history. Replacing that identity replaces the copied record and
/// moves its generation, so derivations start from intact construction before applying it.
///
/// A ledger shorter than what was copied from the same entity is a malformed peer record: the
/// authority only appends, and a replicon snapshot carries its whole list. It is logged, and the
/// record is replaced by the list as it now stands, as for another link's.
///
/// Each new entry is checked against this map's constructions before the record holds it
/// ([`Entry::admitted`]): one naming a construction the map does not place, or an element, chunk
/// or joint past its layout, is a malformed peer entry, refused and logged with its
/// [`crate::construction::breakage::EntryError`], so every record the derivations read holds only what they can apply.
fn receive_ledger(
    ledgers: Query<(Entity, &Ledger, &WorldGeneration)>,
    generation: Res<CurrentWorld>,
    constructions: Query<&Breakage, Without<Piece>>,
    mut copied: Local<(Option<Entity>, usize)>,
    mut record: ResMut<Record>,
) {
    let mut current = ledgers
        .iter()
        .filter(|(_, _, belongs)| Some(**belongs) == generation.0);
    let (ledger, entries, _) = match current.next() {
        Some(ledger) => ledger,
        // Before the first snapshot, or between links, no authority ledger has arrived yet.
        None => return,
    };
    assert!(
        current.next().is_none(),
        "a replica has one active construction ledger; disconnect cleanup removes the old link's \
             replicated entities before a replacement arrives"
    );
    if copied.0 != Some(ledger) || entries.0.len() < copied.1 {
        if copied.0 == Some(ledger) {
            error!(
                "construction ledger: malformed peer record: {} entries after {} copied from the same \
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
        let layouts: HashMap<u16, &Layout> = constructions
            .iter()
            .map(|breakage| (breakage.construction, &breakage.kind.layout))
            .collect();
        for entry in &entries.0[copied.1..] {
            match entry.admitted(|construction| layouts.get(&construction).copied()) {
                Ok(entry) => record.entries.push(entry),
                Err(error) => error!("construction ledger: peer entry {entry:?} refused: {error}"),
            }
        }
        copied.1 = entries.0.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::construction::breakage::{EntryError, Event, Hole};

    fn entry(event: Event, tick: u32) -> Entry {
        Entry {
            construction: 1,
            event,
            tick,
        }
    }

    /// A replica's receive edge with the shipped house placed as construction 1 (fixture ordinal).
    fn replica() -> App {
        let mut app = App::new();
        app.init_resource::<CurrentWorld>()
            .init_resource::<Record>()
            .add_systems(Update, receive_ledger);
        app.world_mut().spawn(Breakage::new(
            crate::construction::tests::shipped_house(),
            1,
            1.0,
        ));
        app
    }

    /// A PEER ENTRY THE MAP CANNOT HOLD NEVER REACHES THE RECORD: one naming a construction the map
    /// does not place, or an element, chunk or joint past its layout, is refused where it
    /// arrives, and the entries around it still land in order. The derivations rely on it: they
    /// apply every record entry by `expect`, so an admitted bad entry would panic a replica, and
    /// one silently consumed would leave its construction half-applied.
    #[test]
    fn a_peer_entry_the_map_cannot_hold_is_refused_at_receipt() {
        let mut app = replica();
        let unknown = Entry {
            construction: 7,
            ..entry(Event::Swap(0), 4)
        };
        let ledger = vec![
            entry(Event::Swap(0), 3),
            unknown,
            entry(Event::Dust(u16::MAX), 5),
            entry(Event::Crack(u16::MAX), 6),
            entry(Event::Crack(2), 7),
        ];
        app.world_mut()
            .spawn((Ledger(ledger), WorldGeneration::default()));
        app.update();
        assert_eq!(
            app.world().resource::<Record>().entries,
            vec![entry(Event::Swap(0), 3), entry(Event::Crack(2), 7)]
        );
        let layout = &crate::construction::tests::shipped_house().layout;
        assert_eq!(
            unknown.admitted(|_| None),
            Err(EntryError::UnknownConstruction(7))
        );
        assert_eq!(
            entry(Event::Fall(u16::MAX), 0).admitted(|_| Some(layout)),
            Err(EntryError::OutsideLayout(1, Event::Fall(u16::MAX)))
        );
    }

    /// THE RECORD IS THE LEDGER, ENTRY FOR ENTRY, AND ANOTHER LEDGER REPLACES IT: a link's ledger
    /// growing by diff reaches the record in order; the next link's ledger (a new entity, shorter)
    /// replaces the record and moves its generation, so every construction derived from the old one
    /// stands whole again before the new one applies. A replica that appended the new ledger to
    /// the old record would keep a previous server's holes.
    #[test]
    fn the_record_follows_the_ledger_and_another_ledger_replaces_it() {
        let mut app = replica();
        let first = app
            .world_mut()
            .spawn((
                Ledger(vec![entry(Event::Swap(0), 3)]),
                WorldGeneration::default(),
            ))
            .id();
        app.update();
        app.world_mut()
            .get_mut::<Ledger>(first)
            .unwrap()
            .apply_diff(&entry(Event::Dust(4), 5))
            .unwrap();
        app.update();
        let record = app.world().resource::<Record>().clone();
        assert_eq!(
            record.entries,
            vec![entry(Event::Swap(0), 3), entry(Event::Dust(4), 5)]
        );
        let generation = record.generation;
        app.world_mut().entity_mut(first).despawn();
        app.world_mut().spawn((
            Ledger(vec![entry(Event::Crack(2), 9)]),
            WorldGeneration::default(),
        ));
        app.update();
        let record = app.world().resource::<Record>();
        assert_eq!(record.entries, vec![entry(Event::Crack(2), 9)]);
        assert_ne!(record.generation, generation);
    }

    /// Authority publication cannot silently stop without its startup singleton. A mutant that
    /// returns on a missing ledger would leave every client's constructions standing unseen.
    #[test]
    #[should_panic(expected = "authority startup spawns exactly one construction ledger")]
    fn publishing_without_the_startup_ledger_is_a_bug() {
        let mut app = App::new();
        app.init_resource::<CurrentWorld>()
            .init_resource::<Record>()
            .add_systems(Update, publish_record);
        app.update();
    }

    /// Two active ledgers would mix authorities' records. A single-query skip mutant would hide
    /// the composition bug and freeze every replica construction on its previous record.
    #[test]
    #[should_panic(expected = "a replica has one active construction ledger")]
    fn a_replica_with_two_construction_ledgers_is_a_bug() {
        let mut app = App::new();
        app.init_resource::<CurrentWorld>()
            .init_resource::<Record>()
            .add_systems(Update, receive_ledger);
        for _ in 0..2 {
            app.world_mut()
                .spawn((Ledger::default(), WorldGeneration::default()));
        }
        app.update();
    }

    /// A HOLE CROSSES THE WIRE WHOLE, IN A LATE JOINER'S SNAPSHOT TOO: a ledger holding hole
    /// entries, encoded as replicon encodes it (postcard) and decoded, is the same ledger; so is an
    /// entry sent alone as a diff. One hole entry costs 20 bytes for the fixture cut (a chunk index
    /// past 127, millimetres past ±63, a tick past four minutes at 64 Hz) and 33 at most. The
    /// replicas' holes are these entries, and [`crate::construction::breakage::HOLES_PER_CHUNK`]
    /// bounds the record by them.
    #[test]
    fn a_hole_crosses_the_wire_whole() {
        let typical = Entry {
            construction: 3,
            event: Event::Hole(
                200,
                Hole {
                    from: [-120, 80, 100],
                    to: [-118, 81, -100],
                    radius: 51,
                },
            ),
            tick: 20_000,
        };
        let widest = Entry {
            construction: u16::MAX,
            event: Event::Hole(
                u16::MAX,
                Hole {
                    from: [i16::MIN; 3],
                    to: [i16::MAX; 3],
                    radius: u16::MAX,
                },
            ),
            tick: u32::MAX,
        };
        let ledger = Ledger(vec![typical, widest]);
        let bytes = postcard::to_allocvec(&ledger).unwrap();
        assert_eq!(postcard::from_bytes::<Ledger>(&bytes).unwrap(), ledger);
        let sent = postcard::to_allocvec(&typical).unwrap();
        assert_eq!(postcard::from_bytes::<Entry>(&sent).unwrap(), typical);
        assert_eq!(sent.len(), 20);
        assert_eq!(postcard::to_allocvec(&widest).unwrap().len(), 33);
    }
}
