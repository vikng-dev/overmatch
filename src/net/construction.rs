//! The construction ledger, about the game's environment destruction (game state only): one globally
//! visible entity per Battle whose component lists every change the authority made to every
//! construction in that Battle's world (`net::world_reset` replaces it at the next Battle)
//! ([`Entry`]: wear, dust, a hole or a crack, with its construction's ordinal and tick),
//! replicated as diffs — a client whose acknowledged ledger is within the last
//! `Diffable::HISTORY_LEN` (64) appends receives only the entries past it, one further behind and
//! a late joiner the whole list as a snapshot. A client copies it into its [`Record`], from
//! which `construction::breakage` derives the colliders and `construction::view` the drawn constructions: the
//! fates, cracks, swaps and holes the clients derive are the authority's, entry for entry. A construction's
//! index is its `MapInstance.ordinal` (bound into the map digest the handshake compares); a chunk's
//! and a joint's are the bake's, read from the same construction file on both ends.
//!
//! A fallen group rides its own carrier (`debris_stress::Carrier`), its chunks on it once. A
//! fragment a crumble shed goes to every player once, reliably, tagged with its world
//! ([`WorldShed`]) and held until that world is selected ([`HeldSheds`]); a late joiner takes
//! none, since decoration is presentation only.

use crate::debris::PresentationClock;
use crate::debris::crumble::Shed;
use bevy::ecs::error::Result;
use bevy::prelude::*;
use bevy_replicon::prelude::{Diffable, EntityCommandsDiffExt};
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::server::Server;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use super::world_reset::AuthorityWorld;
use crate::construction::breakage::{
    AdmittedPiece, Breakage, Entry, EntryError, LawSet, Layout, Piece, PieceError, Record,
    ReplicaSet,
};
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

/// A shed fragment on the wire, with the world it was shed in.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct WorldShed {
    pub(crate) world: WorldGeneration,
    pub(crate) shed: Shed,
}

pub(super) fn install_server(app: &mut App) {
    super::world_reset::register_constructor(app, spawn_ledger);
    app.add_message::<Shed>().add_systems(
        FixedPostUpdate,
        (publish_record, publish_sheds)
            .after(LawSet)
            .in_set(crate::state::SimPhase::Publish),
    );
}

pub(super) fn install_client(app: &mut App) {
    app.init_resource::<CurrentWorld>()
        .insert_resource(PresentationClock::Waiting)
        .init_resource::<HeldSheds>()
        .add_message::<ConstructionRefusal>()
        .add_message::<super::matchmaking::ServerGone>()
        .add_message::<Shed>()
        .add_systems(
            Update,
            (
                publish_presentation_clock,
                receive_ledger,
                receive_pieces,
                receive_sheds,
                refuse_duplicate_owners,
                refuse_peer_record,
            )
                .chain()
                .before(ReplicaSet),
        );
}

/// Authority: every shed the crumble wrote this tick goes to every player once.
fn publish_sheds(
    mut sheds: MessageReader<Shed>,
    world: Option<Res<AuthorityWorld>>,
    servers: Query<&Server>,
    mut sender: ServerMultiMessageSender,
) {
    if sheds.is_empty() {
        return;
    }
    let world = world
        .expect("a crumble sheds only in a Battle's world, which installs its generation")
        .0;
    let server = servers
        .single()
        .expect("shed transport is installed with exactly one authority server");
    for shed in sheds.read() {
        let sent = WorldShed {
            world,
            shed: shed.clone(),
        };
        sender
            .send::<WorldShed, super::protocol::DebrisChannel>(&sent, server, &NetworkTarget::All)
            .expect("a registered shed serializes into each live link's unbounded transport queue");
    }
}

/// Sheds received for a world this player has not selected yet: a next Battle's shed can outrun
/// its Battle identity (the `DebrisChannel` and replication are separate streams), so each waits
/// until world selection reaches its world or passes it. Battle numbers rise within one
/// authority's run (`world_reset::start_next`).
#[derive(Resource, Default)]
struct HeldSheds(Vec<WorldShed>);

/// The sheds of world `selected` among `held`, in arrival order: one of a later world stays held,
/// and one of an earlier world goes, a stale cosmetic of a world this player has left. With no
/// world selected (before the first Battle) every one waits.
fn select_sheds(held: &mut Vec<WorldShed>, selected: Option<WorldGeneration>) -> Vec<Shed> {
    let Some(selected) = selected else {
        return Vec::new();
    };
    let mut due = Vec::new();
    for WorldShed { world, shed } in std::mem::take(held) {
        match world.0.cmp(&selected.0) {
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => due.push(shed),
            std::cmp::Ordering::Greater => held.push(WorldShed { world, shed }),
        }
    }
    due
}

/// Replica: a shed of the selected world, admitted against its map, becomes the decoration's; one
/// of a world not yet selected waits for it ([`HeldSheds`]). A malformed one refuses the match
/// before any view reads it.
fn receive_sheds(
    mut receivers: Query<&mut MessageReceiver<WorldShed>>,
    generation: Res<CurrentWorld>,
    constructions: Query<(Entity, &Breakage), Without<Piece>>,
    mut held: ResMut<HeldSheds>,
    mut admitted: MessageWriter<Shed>,
    mut refused: MessageWriter<ConstructionRefusal>,
) {
    for mut receiver in &mut receivers {
        held.0.extend(receiver.receive());
    }
    for shed in select_sheds(&mut held.0, generation.0) {
        match admit_shed(shed, &constructions) {
            Ok(shed) => {
                admitted.write(shed);
            }
            Err(refusal) => {
                refused.write(refusal);
            }
        }
    }
}

/// A peer's shed checked against the selected map: its chunks a valid group of a placed
/// construction, its motion finite and its rotation a unit quaternion.
fn admit_shed(
    shed: Shed,
    constructions: &Query<(Entity, &Breakage), Without<Piece>>,
) -> Result<Shed, ConstructionRefusal> {
    let selected = constructions
        .iter()
        .find(|(_, breakage)| breakage.construction == shed.construction);
    let piece = Piece {
        construction: shed.construction,
        chunks: shed.chunks.clone(),
        born: shed.tick,
        retired: None,
    };
    piece
        .admitted(selected)
        .map_err(ConstructionRefusal::Piece)?;
    let finite = [shed.at, shed.velocity, shed.spin]
        .iter()
        .flatten()
        .chain(&shed.rotation)
        .all(|x| x.is_finite());
    if !finite
        || !Quat::from_array(shed.rotation).is_normalized()
        || !crate::debris::decor::representable(&shed)
    {
        return Err(ConstructionRefusal::Shed(shed.construction));
    }
    Ok(shed)
}

fn publish_presentation_clock(
    cursor: Option<Res<InterpolationTimeline>>,
    mut clock: ResMut<PresentationClock>,
) {
    *clock = match cursor.filter(|timeline| timeline.is_synced()) {
        Some(timeline) => PresentationClock::At {
            tick: timeline.tick().0,
            overstep: f64::from(timeline.overstep().to_f32()),
        },
        None => PresentationClock::Waiting,
    };
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
    stages: Option<Res<crate::cost::stages::Stages>>,
    record: Res<Record>,
    ledgers: Query<Entity, With<Ledger>>,
    mut published: Local<(Option<Entity>, usize)>,
    mut commands: Commands,
) {
    let _publish =
        crate::cost::stages::Stages::start(stages.as_deref(), crate::cost::stages::Stage::Publish);
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

/// Replica: admit the selected world's new entries as one batch before mutating its record.
/// Another ledger identity replaces the record and advances its generation. Within one identity
/// the authority only appends; duplicates, shrinking and malformed entries refuse the match.
fn receive_ledger(
    ledgers: Query<(Entity, Ref<Ledger>, Option<&WorldGeneration>)>,
    generation: Res<CurrentWorld>,
    constructions: Query<&Breakage, Without<Piece>>,
    mut copied: Local<(Option<Entity>, usize)>,
    mut record: ResMut<Record>,
    mut refused: MessageWriter<ConstructionRefusal>,
) {
    if ledgers.iter().any(|(_, _, belongs)| belongs.is_none()) {
        refused.write(ConstructionRefusal::UntaggedLedger);
        return;
    }
    let mut current = ledgers
        .iter()
        .filter(|(_, _, belongs)| belongs.copied() == generation.0);
    let (ledger, entries, _) = match current.next() {
        Some(ledger) => ledger,
        // Before the first snapshot, or between links, no authority ledger has arrived yet.
        None => return,
    };
    if current.next().is_some() {
        refused.write(ConstructionRefusal::DuplicateLedgers);
        return;
    }
    let replaced = copied.0 != Some(ledger);
    let from = if replaced { 0 } else { copied.1 };
    let Some(fresh) = entries.0.get(from..) else {
        refused.write(ConstructionRefusal::ShrunkLedger {
            copied: from,
            received: entries.0.len(),
        });
        return;
    };
    if !replaced && entries.is_changed() && entries.0[..from] != record.entries {
        refused.write(ConstructionRefusal::RewrittenLedger);
        return;
    }
    if !replaced && fresh.is_empty() {
        return;
    }
    let layouts: HashMap<u16, &Layout> = constructions
        .iter()
        .map(|breakage| (breakage.construction, &breakage.kind.layout))
        .collect();
    let admitted = fresh
        .iter()
        .map(|entry| entry.admitted(|ordinal| layouts.get(&ordinal).copied()))
        .collect::<std::result::Result<Vec<_>, _>>();
    let admitted = match admitted {
        Ok(entries) => entries,
        Err(error) => {
            refused.write(ConstructionRefusal::Entry(error));
            return;
        }
    };
    if replaced {
        record.entries.clear();
        record.generation = record.generation.wrapping_add(1);
    }
    record.entries.extend(admitted);
    *copied = (Some(ledger), entries.0.len());
}

#[derive(Message, Debug, Clone, PartialEq)]
enum ConstructionRefusal {
    DuplicateLedgers,
    UntaggedLedger,
    UntaggedPiece,
    RewrittenLedger,
    ShrunkLedger {
        copied: usize,
        received: usize,
    },
    Entry(EntryError),
    Piece(PieceError),
    /// A shed fragment of this construction moves by a non-finite or unnormalized pose or motion.
    Shed(u16),
}

impl std::fmt::Display for ConstructionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Server construction stream refused: ")?;
        match self {
            Self::RewrittenLedger => f.write_str("ledger rewrote previously admitted entries"),
            Self::UntaggedLedger => f.write_str("ledger has no world generation"),
            Self::UntaggedPiece => f.write_str("piece has no world generation"),
            Self::DuplicateLedgers => f.write_str("multiple ledgers for the selected world"),
            Self::ShrunkLedger { copied, received } => {
                write!(f, "ledger shrank from {copied} to {received} entries")
            }
            Self::Entry(error) => error.fmt(f),
            Self::Piece(error) => error.fmt(f),
            Self::Shed(c) => write!(
                f,
                "a shed fragment of construction {c} has a non-finite or unnormalized motion"
            ),
        }
    }
}

/// A selected CurrentWorld has synchronously reconstructed all map owners. Only a piece of a
/// different generation waits; a missing ordinal in the selected map is malformed peer input.
fn receive_pieces(
    pieces: Query<
        (
            Entity,
            &Piece,
            Option<&WorldGeneration>,
            Option<&AdmittedPiece>,
        ),
        Or<(
            Changed<Piece>,
            Changed<WorldGeneration>,
            Without<AdmittedPiece>,
        )>,
    >,
    generation: Res<CurrentWorld>,
    constructions: Query<(Entity, &Breakage), Without<Piece>>,
    mut refused: MessageWriter<ConstructionRefusal>,
    mut commands: Commands,
) {
    for (entity, piece, belongs, admitted) in &pieces {
        let Some(belongs) = belongs else {
            refused.write(ConstructionRefusal::UntaggedPiece);
            commands.entity(entity).despawn();
            continue;
        };
        if Some(*belongs) != generation.0 {
            continue;
        }
        let selected = constructions
            .iter()
            .find(|(_, b)| b.construction == piece.construction);
        let result = match admitted {
            Some(admitted) => match selected {
                Some((root, _)) if root == admitted.construction => {
                    admitted.admits_update(piece).map(|()| None)
                }
                _ => Err(PieceError::Changed(piece.construction)),
            },
            None => piece.admitted(selected).map(Some),
        };
        match result {
            Ok(Some(admitted)) => {
                commands.entity(entity).insert(admitted);
            }
            Ok(None) => {}
            Err(error) => {
                refused.write(ConstructionRefusal::Piece(error));
                // Flush before ReplicaSet and the view: a malformed carrier must never be indexed.
                commands.entity(entity).despawn();
            }
        }
    }
}

/// Same-birth owners cannot claim the same chunk. Parent/child generations may overlap in
/// arrival time; their distinct birth ticks are resolved by the presentation handover.
fn refuse_duplicate_owners(
    pieces: Query<(Entity, &Piece, &AdmittedPiece, &WorldGeneration)>,
    generation: Res<CurrentWorld>,
    mut refused: MessageWriter<ConstructionRefusal>,
    mut commands: Commands,
) {
    let mut owners = std::collections::BTreeMap::new();
    for (entity, piece, admitted, belongs) in &pieces {
        if Some(*belongs) != generation.0 {
            continue;
        }
        for &chunk in &admitted.chunks {
            if owners
                .insert((admitted.construction, piece.born, chunk), entity)
                .is_some()
            {
                refused.write(ConstructionRefusal::Piece(PieceError::DuplicateOwnership {
                    construction: piece.construction,
                    chunk,
                    born: piece.born,
                }));
                commands.entity(entity).despawn();
                return;
            }
        }
    }
}

/// The ordinary match-close path removes the link, stops reconnecting and shows this cause.
fn refuse_peer_record(
    mut refused: MessageReader<ConstructionRefusal>,
    mut gone: MessageWriter<super::matchmaking::ServerGone>,
) {
    if let Some(refusal) = refused.read().next() {
        warn!("client: {refusal}");
        gone.write(super::matchmaking::ServerGone(refusal.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::construction::breakage::{Event, Hole};
    use lightyear::prelude::client::NetcodeClient;

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
        app.init_resource::<Record>();
        install_client(&mut app);
        app.world_mut().spawn(Breakage::new(
            crate::construction::tests::shipped_house(),
            1,
        ));
        app
    }

    /// A NEXT WORLD'S SHED WAITS FOR ITS BATTLE: a shed that arrives before the Battle identity
    /// selects its world is held, and shown once when that world is selected; one of a world left
    /// behind goes. A mutant treating every other world's shed as stale loses it, and every player
    /// it reached first draws a different world. Fixture: the house's first two chunks shed at
    /// tick 40 in world 2.
    #[test]
    fn a_next_worlds_shed_is_shown_once_in_its_world() {
        let mut app = replica();
        app.world_mut().resource_mut::<CurrentWorld>().0 = Some(WorldGeneration(1));
        let shed = Shed {
            construction: 1,
            chunks: vec![0, 1],
            at: [0.0; 3],
            rotation: Quat::IDENTITY.to_array(),
            velocity: [0.0; 3],
            spin: [0.0; 3],
            seed: 0,
            tick: 40,
        };
        let held = |world: u32| WorldShed {
            world: WorldGeneration(world),
            shed: shed.clone(),
        };
        app.world_mut().resource_mut::<HeldSheds>().0.push(held(2));
        let mut cursor = app.world().resource::<Messages<Shed>>().get_cursor();
        let mut shown = |app: &mut App| {
            app.update();
            cursor
                .read(app.world().resource::<Messages<Shed>>())
                .count()
        };
        assert_eq!(shown(&mut app), 0, "world 2's shed waits for its Battle");
        app.world_mut().resource_mut::<CurrentWorld>().0 = Some(WorldGeneration(2));
        assert_eq!(shown(&mut app), 1, "and shows in it");
        assert_eq!(shown(&mut app), 0, "once");
        app.world_mut().resource_mut::<HeldSheds>().0.push(held(1));
        assert_eq!(shown(&mut app), 0);
        assert!(
            app.world().resource::<HeldSheds>().0.is_empty(),
            "a stale shed goes"
        );
    }

    fn admitted_ledger() -> (App, Entity, Record) {
        let mut app = replica();
        let ledger = app
            .world_mut()
            .spawn((
                Ledger(vec![entry(Event::Wear(0), 3)]),
                WorldGeneration::default(),
            ))
            .id();
        app.update();
        let before = app.world().resource::<Record>().clone();
        (app, ledger, before)
    }

    /// A peer ledger without world identity must close the match and preserve admitted state,
    /// even when another, correctly tagged ledger is present.
    #[test]
    fn an_untagged_peer_ledger_is_visibly_refused() {
        let (mut app, _, before) = admitted_ledger();
        app.world_mut().spawn(Ledger::default());
        app.update();
        assert_eq!(*app.world().resource::<Record>(), before);
        assert_refused(&mut app, ConstructionRefusal::UntaggedLedger);
    }

    /// A malformed entry refuses the entire batch visibly, preserving the previous admitted
    /// record. Accepting surrounding entries would leave the player's destruction record incomplete.
    #[test]
    fn a_peer_entry_the_map_cannot_hold_is_refused_at_receipt() {
        for (bad, error) in [
            (
                Entry {
                    construction: 7,
                    ..entry(Event::Wear(0), 4)
                },
                EntryError::UnknownConstruction(7),
            ),
            (
                entry(Event::Dust(u16::MAX), 5),
                EntryError::OutsideLayout(1, Event::Dust(u16::MAX)),
            ),
            (
                entry(Event::Crack(u16::MAX), 6),
                EntryError::OutsideLayout(1, Event::Crack(u16::MAX)),
            ),
            (
                entry(Event::Separate(u16::MAX), 6),
                EntryError::OutsideLayout(1, Event::Separate(u16::MAX)),
            ),
            (
                entry(Event::Wear(u16::MAX), 7),
                EntryError::OutsideLayout(1, Event::Wear(u16::MAX)),
            ),
        ] {
            let (mut app, ledger, before) = admitted_ledger();
            app.world_mut()
                .get_mut::<Ledger>(ledger)
                .unwrap()
                .0
                .extend([entry(Event::Crack(2), 4), bad, entry(Event::Dust(0), 9)]);
            app.update();
            assert_eq!(
                *app.world().resource::<Record>(),
                before,
                "the whole malformed batch is refused"
            );
            assert_refused(&mut app, ConstructionRefusal::Entry(error));
        }
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
                Ledger(vec![entry(Event::Wear(0), 3)]),
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
            vec![entry(Event::Wear(0), 3), entry(Event::Dust(4), 5)]
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

    /// Peer-controlled ledger cardinality must close the match, never panic or skip silently.
    #[test]
    fn duplicate_active_ledgers_are_visibly_refused() {
        let mut app = replica();
        for _ in 0..2 {
            app.world_mut()
                .spawn((Ledger::default(), WorldGeneration::default()));
        }
        app.update();
        assert_refused(&mut app, ConstructionRefusal::DuplicateLedgers);
        assert!(app.world().resource::<Record>().entries.is_empty());
    }

    /// A peer cannot replace its append-only ledger with a shorter same-identity snapshot.
    #[test]
    fn a_shrinking_peer_ledger_is_visibly_refused() {
        let (mut app, ledger, before) = admitted_ledger();
        app.world_mut().get_mut::<Ledger>(ledger).unwrap().0.clear();
        app.update();
        assert_eq!(*app.world().resource::<Record>(), before);
        assert_refused(
            &mut app,
            ConstructionRefusal::ShrunkLedger {
                copied: 1,
                received: 0,
            },
        );
    }

    /// Append-only admission also refuses a snapshot rewriting its admitted prefix, even when
    /// the replacement event is individually valid. Otherwise the record silently diverges.
    #[test]
    fn a_peer_cannot_rewrite_previously_admitted_entries() {
        for event in [Event::Dust(0), Event::Dust(u16::MAX)] {
            let (mut app, ledger, before) = admitted_ledger();
            app.world_mut().get_mut::<Ledger>(ledger).unwrap().0[0] = entry(event, 3);
            app.update();
            assert_eq!(*app.world().resource::<Record>(), before);
            assert_refused(&mut app, ConstructionRefusal::RewrittenLedger);
        }
    }

    /// Peer descriptors are checked before a pose or view exists; neither bad indices nor unknown
    /// ordinals may reach the view. A future generation waits only until its map is selected.
    #[test]
    fn invalid_piece_descriptors_are_refused_at_the_selected_map_edge() {
        for (ordinal, chunks, error) in [
            (1, vec![u16::MAX], PieceError::OutsideLayout(1, u16::MAX)),
            (7, vec![0], PieceError::UnknownConstruction(7)),
            (1, vec![], PieceError::Empty(1)),
            (1, vec![0, 0], PieceError::Unordered(1)),
            (1, vec![1, 0], PieceError::Unordered(1)),
        ] {
            let mut app = replica();
            let piece = app
                .world_mut()
                .spawn((
                    Piece {
                        construction: ordinal,
                        chunks,
                        born: 0,
                        retired: None,
                    },
                    WorldGeneration(2),
                ))
                .id();
            app.update();
            assert!(app.world().get::<AdmittedPiece>(piece).is_none());
            assert!(
                app.world()
                    .resource::<Messages<super::super::matchmaking::ServerGone>>()
                    .is_empty()
            );
            app.world_mut().resource_mut::<CurrentWorld>().0 = Some(WorldGeneration(2));
            app.update();
            assert!(
                app.world().get_entity(piece).is_err(),
                "the malformed carrier cannot reach presentation"
            );
            assert_refused(&mut app, ConstructionRefusal::Piece(error));
        }
    }

    /// Only retirement changes on an admitted carrier. Mutating its identity would invalidate the
    /// checked mesh while show_pieces still reads the descriptor's group.
    #[test]
    fn an_admitted_piece_accepts_retirement_and_refuses_group_mutation() {
        let mut app = replica();
        let piece = app
            .world_mut()
            .spawn((
                Piece {
                    construction: 1,
                    chunks: vec![0],
                    born: 3,
                    retired: None,
                },
                WorldGeneration::default(),
            ))
            .id();
        app.update();
        assert!(app.world().get::<AdmittedPiece>(piece).is_some());
        app.world_mut().get_mut::<Piece>(piece).unwrap().retired = Some(5);
        app.update();
        assert!(app.world().get::<AdmittedPiece>(piece).is_some());
        app.world_mut().get_mut::<Piece>(piece).unwrap().chunks = vec![u16::MAX];
        app.update();
        assert!(app.world().get_entity(piece).is_err());
        assert_refused(&mut app, ConstructionRefusal::Piece(PieceError::Changed(1)));
    }

    /// The game's destruction stream must close through the shipped lifecycle and show its cause
    /// in the lobby. Merely logging a typed refusal, or leaving reconnect alive, fails this witness.
    #[test]
    fn a_construction_refusal_closes_the_match_and_reaches_the_players_lobby() {
        use super::super::matchmaking::MatchClosed;
        use bevy::ecs::message::MessageCursor;
        let _udp = super::super::test_harness::lock_real_udp_test();
        let (mut server, mut client) = super::super::client::match_exit::in_match();
        client.init_resource::<Record>();
        install_client(&mut client);
        let expected = ConstructionRefusal::DuplicateLedgers.to_string();
        // Fixture: two active peer ledger entities delivered through the installed receive edge.
        for _ in 0..2 {
            client
                .world_mut()
                .spawn((Ledger::default(), WorldGeneration::default()));
        }
        let mut closed = MessageCursor::<MatchClosed>::default();
        let mut observed = false;
        for _ in 0..8 {
            server.update();
            client.update();
            let causes: Vec<_> = closed
                .read(client.world().resource::<Messages<MatchClosed>>())
                .cloned()
                .collect();
            if !causes.is_empty() {
                assert_eq!(causes, [MatchClosed::Ended(expected.clone())]);
                observed = true;
                break;
            }
        }
        assert!(observed, "the receive refusal reaches the match-close path");
        // The wire entities normally belong to the link; this injected fixture owns their cleanup.
        let ledgers: Vec<_> = client
            .world_mut()
            .query_filtered::<Entity, With<Ledger>>()
            .iter(client.world())
            .collect();
        for entity in ledgers {
            client.world_mut().despawn(entity);
        }
        super::super::client::match_exit::advance_past_reconnect_window(&mut client);
        for _ in 0..8 {
            client.update();
        }
        assert_eq!(
            client
                .world_mut()
                .query_filtered::<(), With<NetcodeClient>>()
                .iter(client.world())
                .count(),
            0
        );
        assert_eq!(
            *client.world().resource::<State<crate::flow::Flow>>().get(),
            crate::flow::Flow::Lobby
        );
        assert!(
            client
                .world_mut()
                .query::<&Text>()
                .iter(client.world())
                .any(|text| text.0.contains(&expected))
        );
    }

    /// Admission must stop duplicate ownership before the same chunk can be drawn twice.
    /// Different birth ticks remain legal while a parent's retirement is in flight.
    #[test]
    fn duplicate_chunk_owners_are_refused_before_presentation() {
        let mut app = replica();
        for born in [3, 5] {
            app.world_mut().spawn((
                Piece {
                    construction: 1,
                    chunks: vec![0],
                    born,
                    retired: None,
                },
                WorldGeneration::default(),
            ));
        }
        app.update();
        assert!(
            app.world()
                .resource::<Messages<super::super::matchmaking::ServerGone>>()
                .is_empty()
        );
        app.world_mut().spawn((
            Piece {
                construction: 1,
                chunks: vec![0],
                born: 5,
                retired: None,
            },
            WorldGeneration::default(),
        ));
        app.update();
        assert_refused(
            &mut app,
            ConstructionRefusal::Piece(PieceError::DuplicateOwnership {
                construction: 1,
                chunk: 0,
                born: 5,
            }),
        );
    }

    /// A replica has no local substitute for the shared interpolation cursor before sync.
    #[test]
    fn an_unsynchronized_replica_has_no_presentation_cursor() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = replica();
        *app.world_mut().resource_mut::<PresentationClock>() = PresentationClock::At {
            tick: 100,
            overstep: 0.5,
        };
        app.world_mut()
            .run_system_once(publish_presentation_clock)
            .unwrap();
        let clock = *app.world().resource::<PresentationClock>();
        for tick in [0, 1, 100, u32::MAX] {
            assert_eq!(clock.since(tick), None);
        }
    }

    /// A PEER'S SHED REACHES THE DECORATION ONLY AS A DRAWABLE GROUP WITH A FINITE MOTION THE
    /// LANDING SEARCH RESOLVES: the decoration indexes the chunks' layout and poses the mesh from
    /// the motion unchecked, so a mutant that skipped a check would panic, draw NaN, or search a
    /// touch finer than f32 holds on a peer's word. Fixture: 2 km/s and 100 km lie past the
    /// decoration's contract.
    #[test]
    fn a_peer_shed_the_map_cannot_draw_is_refused() {
        use bevy::ecs::system::RunSystemOnce;
        let good = Shed {
            construction: 1,
            chunks: vec![0, 1],
            at: [0.0; 3],
            rotation: Quat::IDENTITY.to_array(),
            velocity: [1.0, 2.0, 0.0],
            spin: [0.0; 3],
            seed: 9,
            tick: 4,
        };
        let cases = [
            (good.clone(), None),
            (
                Shed {
                    chunks: vec![1, 0],
                    ..good.clone()
                },
                Some(ConstructionRefusal::Piece(PieceError::Unordered(1))),
            ),
            (
                Shed {
                    construction: 7,
                    ..good.clone()
                },
                Some(ConstructionRefusal::Piece(PieceError::UnknownConstruction(
                    7,
                ))),
            ),
            (
                Shed {
                    velocity: [f32::NAN, 0.0, 0.0],
                    ..good.clone()
                },
                Some(ConstructionRefusal::Shed(1)),
            ),
            (
                Shed {
                    rotation: [0.0, 0.0, 0.0, 2.0],
                    ..good.clone()
                },
                Some(ConstructionRefusal::Shed(1)),
            ),
            (
                Shed {
                    velocity: [2.0e3, 0.0, 0.0],
                    ..good.clone()
                },
                Some(ConstructionRefusal::Shed(1)),
            ),
            (
                Shed {
                    at: [0.0, 0.0, 1.0e5],
                    ..good
                },
                Some(ConstructionRefusal::Shed(1)),
            ),
        ];
        let mut app = replica();
        for (shed, refusal) in cases {
            let sent = shed.clone();
            let admitted = app
                .world_mut()
                .run_system_once(
                    move |constructions: Query<(Entity, &Breakage), Without<Piece>>| {
                        admit_shed(sent.clone(), &constructions)
                    },
                )
                .expect("the admission runs");
            match refusal {
                None => assert_eq!(admitted, Ok(shed)),
                Some(refusal) => assert_eq!(admitted, Err(refusal)),
            }
        }
    }

    fn assert_refused(app: &mut App, expected: ConstructionRefusal) {
        let causes: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<super::super::matchmaking::ServerGone>>()
            .drain()
            .map(|cause| cause.0)
            .collect();
        assert_eq!(causes, [expected.to_string()]);
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
