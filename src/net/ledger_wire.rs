//! Replicon round trips of the diff-replicated lists: the construction ledger (the game's
//! environment destruction record) and the hull marks (the visual layer of the game's damage
//! model). The ledger is state each client derives its constructions from (ADR-0037), so every
//! client must end with the authority's list, entry for entry, however it joined or what it lost;
//! and a client that is caught up must be sent only what it lacks, or every change resends the
//! whole list.
//!
//! What guarantees order and completeness (bevy_replicon 0.44.3):
//! - The authority numbers each diff as it records it, one index after the last
//!   (`shared/replication/diff.rs:220`, `DiffHistory::record`), and sends a client the contiguous
//!   run of diffs past its acknowledged cursor, or the whole value when that run is no longer
//!   retained (`diff.rs:262`, `diffs_after`).
//! - A cursor moves only on what the client confirmed: the reliable update that inserted the value
//!   (`server.rs:875`, `update_ticks`) or an acknowledged mutation (`client_ticks.rs:67`,
//!   `ack_mutate_message`). Until then every send repeats the run from the old cursor, because
//!   the value counts as changed since the client's last acknowledged tick (`server.rs:763`).
//! - A client keys each received diff by its index and applies one only after its predecessor
//!   (`diff.rs:482` and `diff.rs:503`, `push_with_tick` and `drain_ready`), ignores duplicates,
//!   and takes a whole value only when it is newer than what it applied (`diff.rs:439`,
//!   `apply_snapshot`).
//! - This game then copies only the new suffix of the replicated ledger into its record, refusing
//!   a shrunk or rewritten one (`net::construction::receive_ledger`).
//!
//! These hold while a client's acknowledged cursor is fewer than 65536 diffs behind the
//! authority's: the index is a wrapping `u16` (`diff/diff_index.rs:60`, `distance_after`), so a
//! cursor that never moves for that long reads as near again, at any history length.
//!
//! Both endpoints here are bare replicon apps exchanging messages directly (replicon's
//! `test_app`); each list is registered with replicon's `replicate_diff`, the rule the shipped
//! protocol's `replicate_diff` registers. Losing a packet removes the authority's unreliable
//! mutation messages; reliable updates still arrive, as lightyear's reliable channel delivers them.

use std::fmt::Debug;

use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::prelude::*;
use bevy_replicon::shared::backend::channels::ServerChannel;
use bevy_replicon::test_app::ServerTestAppExt;

use super::construction::Ledger;
use crate::construction::breakage::{Entry, Event};
use crate::hull_marks::HullMarks;

/// An authority and one client, each a bare replicon app ticking every update.
fn link<C: Diffable>() -> (App, App) {
    let mut server = App::new();
    let mut client = App::new();
    for app in [&mut server, &mut client] {
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            RepliconPlugins.set(ServerPlugin::new(PostUpdate)),
        ))
        .replicate_diff::<C>();
        app.finish();
    }
    (server, client)
}

/// Fixture entries, told apart by `n`.
fn entry(n: u32) -> Entry {
    Entry {
        construction: 1,
        event: Event::Wear((n % 1024) as u16),
        tick: n,
    }
}

fn append<C: Diffable>(server: &mut App, list: Entity, diffs: impl IntoIterator<Item = C::Diff>) {
    let mut entity = server.world_mut().entity_mut(list);
    for diff in diffs {
        entity
            .apply_diff::<C>(diff)
            .expect("the fixture list carries the diffed component");
    }
}

/// One authority tick whose packets all arrive.
fn deliver(server: &mut App, client: &mut App) {
    server.update();
    server.exchange_with_client(client);
    client.update();
}

/// One authority tick whose unreliable mutation messages are lost.
fn lose(server: &mut App, client: &mut App) {
    server.update();
    server
        .world_mut()
        .resource_mut::<ServerMessages>()
        .retain_sent(|(_, channel, _)| *channel != usize::from(ServerChannel::Mutations));
    server.exchange_with_client(client);
    client.update();
}

/// The bytes of this tick's mutation messages, taken from the authority before delivery.
fn take_mutations(server: &mut App) -> Vec<Bytes> {
    let mut taken = Vec::new();
    server
        .world_mut()
        .resource_mut::<ServerMessages>()
        .retain_sent(|(_, channel, message)| {
            let mutation = *channel == usize::from(ServerChannel::Mutations);
            if mutation {
                taken.push(message.clone());
            }
            !mutation
        });
    taken
}

fn held<C: Component + Clone>(app: &mut App) -> C {
    app.world_mut()
        .query::<&C>()
        .single(app.world())
        .expect("one replicated list on each endpoint")
        .clone()
}

fn assert_same<C: Component + Clone + PartialEq + Debug>(
    server: &mut App,
    client: &mut App,
    when: &str,
) {
    assert_eq!(held::<C>(client), held::<C>(server), "{when}");
}

/// The mutation bytes one authority tick sends after a client already holding `held` diffs is sent
/// one more, against that list's whole serialized size.
fn sent_for_one_more<C: Diffable + Default + Clone + PartialEq + Debug>(
    diff: impl Fn(u32) -> C::Diff,
    held_diffs: u32,
) -> (usize, usize) {
    let (mut server, mut client) = link::<C>();
    server.connect_client(&mut client);
    let list = server.world_mut().spawn((Replicated, C::default())).id();
    append::<C>(&mut server, list, (0..held_diffs).map(&diff));
    deliver(&mut server, &mut client);
    assert_same::<C>(
        &mut server,
        &mut client,
        "the inserting update carries the list",
    );
    append::<C>(&mut server, list, [diff(held_diffs)]);
    server.update();
    let sent = take_mutations(&mut server)
        .iter()
        .map(|message| message.len())
        .sum();
    let whole = postcard::to_allocvec(&held::<C>(&mut server))
        .expect("the list serializes")
        .len();
    (sent, whole)
}

/// A CAUGHT-UP CLIENT IS SENT ONLY WHAT IT LACKS: after a client holds a list, one more entry or
/// mark reaches it as that one diff, not as the whole list again. A diff cursor that never starts
/// (bevy_replicon 0.44.2, MEASURED in the send census: 158 of 158 ledger sends whole) resends the
/// growing list on every change; it reached 103 KB a send in the match recipe. Fixtures: 200
/// ledger entries; 40 hull marks, under the 50-mark cap.
#[test]
fn a_caught_up_client_is_sent_only_what_it_lacks() {
    let (sent, whole) = sent_for_one_more::<Ledger>(entry, 200);
    assert!(
        0 < sent && sent < whole,
        "one more entry sends {sent} B against the {whole} B ledger"
    );
    let (sent, whole) =
        sent_for_one_more::<HullMarks>(|n| crate::hull_marks::tests::numbered(n as u16), 40);
    assert!(
        0 < sent && sent < whole,
        "one more mark sends {sent} B against the {whole} B marks"
    );
}

/// A LATE JOINER ENDS WITH THE AUTHORITY'S LEDGER: a client that connects after entries were
/// appended receives them whole in the inserting update, then each later entry by diff. A cursor
/// started anywhere but at the inserted value would skip or repeat entries. Fixture: 300 entries
/// before the join, five after.
#[test]
fn a_late_joiner_ends_with_the_authoritys_ledger() {
    let (mut server, mut client) = link::<Ledger>();
    let ledger = server
        .world_mut()
        .spawn((Replicated, Ledger::default()))
        .id();
    append::<Ledger>(&mut server, ledger, (0..300).map(entry));
    server.update();
    server.connect_client(&mut client);
    deliver(&mut server, &mut client);
    assert_same::<Ledger>(
        &mut server,
        &mut client,
        "the join carries the whole ledger",
    );
    for n in 300..305 {
        append::<Ledger>(&mut server, ledger, [entry(n)]);
        deliver(&mut server, &mut client);
        assert_same::<Ledger>(
            &mut server,
            &mut client,
            "each later entry arrives in order",
        );
    }
}

/// A CLIENT THAT MISSES OR REORDERS PACKETS ENDS WITH THE AUTHORITY'S LEDGER: entries whose
/// mutation messages are lost arrive with the next delivered tick, since the authority resends
/// everything past the client's acknowledged cursor; a mutation delivered after a newer one
/// changes nothing. A client that applied a diff before its predecessor, or dropped a resent run as
/// already seen, would derive constructions the authority does not hold. Fixture: seven entries a
/// tick, ten ticks lost, then one late, out-of-order message.
#[test]
fn a_client_that_misses_packets_ends_with_the_authoritys_ledger() {
    let (mut server, mut client) = link::<Ledger>();
    server.connect_client(&mut client);
    let ledger = server
        .world_mut()
        .spawn((Replicated, Ledger::default()))
        .id();
    append::<Ledger>(&mut server, ledger, (0..20).map(entry));
    deliver(&mut server, &mut client);
    for tick in 0..10 {
        append::<Ledger>(
            &mut server,
            ledger,
            (0..7).map(|n| entry(100 + tick * 7 + n)),
        );
        lose(&mut server, &mut client);
    }
    assert_eq!(
        held::<Ledger>(&mut client).0.len(),
        20,
        "nothing lost arrived"
    );
    deliver(&mut server, &mut client);
    assert_same::<Ledger>(
        &mut server,
        &mut client,
        "the next delivered tick carries every lost entry, in order",
    );

    append::<Ledger>(&mut server, ledger, [entry(500)]);
    server.update();
    let late = take_mutations(&mut server);
    server.exchange_with_client(&mut client);
    client.update();
    append::<Ledger>(&mut server, ledger, [entry(501)]);
    deliver(&mut server, &mut client);
    assert_same::<Ledger>(
        &mut server,
        &mut client,
        "the newer tick carries both entries",
    );
    let mut messages = client.world_mut().resource_mut::<ClientMessages>();
    for message in late {
        messages.insert_received(ServerChannel::Mutations, message);
    }
    client.update();
    assert_same::<Ledger>(&mut server, &mut client, "a late message changes nothing");
}

/// A CLIENT FURTHER BEHIND THAN THE HISTORY ENDS WITH THE AUTHORITY'S LEDGER, FROM ONE WHOLE COPY:
/// once more entries were appended past its cursor than the authority retains, the next delivered
/// tick sends the whole ledger, and the next entry after that goes by diff again. A sender that
/// sent the retained run alone would leave a gap. Fixture: the retained history plus one entry,
/// over two lost ticks; `HISTORY_LEN` is read from its owner.
#[test]
fn a_client_further_behind_than_the_history_gets_one_whole_copy() {
    let (mut server, mut client) = link::<Ledger>();
    server.connect_client(&mut client);
    let ledger = server
        .world_mut()
        .spawn((Replicated, Ledger::default()))
        .id();
    append::<Ledger>(&mut server, ledger, (0..10).map(entry));
    deliver(&mut server, &mut client);
    let behind = <Ledger as Diffable>::HISTORY_LEN as u32 + 1;
    let mut from = 10;
    for to in [10 + behind / 2, 10 + behind] {
        append::<Ledger>(&mut server, ledger, (from..to).map(entry));
        lose(&mut server, &mut client);
        from = to;
    }
    assert_eq!(held::<Ledger>(&mut server).0.len(), 10 + behind as usize);
    server.update();
    let whole = postcard::to_allocvec(&held::<Ledger>(&mut server))
        .expect("the ledger serializes")
        .len();
    let sent: usize = server
        .world()
        .resource::<ServerMessages>()
        .iter_sent()
        .filter(|(_, channel, _)| *channel == usize::from(ServerChannel::Mutations))
        .map(|(_, _, message)| message.len())
        .sum();
    assert!(
        sent >= whole,
        "the whole ledger goes once: {sent} B against {whole} B"
    );
    server.exchange_with_client(&mut client);
    client.update();
    assert_same::<Ledger>(&mut server, &mut client, "the whole copy closes the gap");

    deliver(&mut server, &mut client);
    let next = 10 + behind;
    append::<Ledger>(&mut server, ledger, [entry(next)]);
    server.update();
    let sent: usize = take_mutations(&mut server)
        .iter()
        .map(|message| message.len())
        .sum();
    assert!(sent < whole, "then diffs again: {sent} B against {whole} B");
}
