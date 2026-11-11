//! Replicon round trips of the construction ledger (the game's environment destruction record),
//! the one list replicated by diff. It is state each client derives its constructions from
//! (ADR-0037), so a client
//! holds no entry the authority did not put at that position: it ends with the authority's list,
//! entry for entry, however it joined or what it lost, or refuses the match where replicon offers
//! an entry for another position. A client that is caught up must be sent only what it lacks, or
//! every change resends the whole list.
//!
//! What places every entry (bevy_replicon 0.44.3):
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
//! - That index is a wrapping `u16` (`diff/diff_index.rs:60`), so it does not place a diff by
//!   itself. A delayed message whose diffs the list has run more than 32767 past buffers them as
//!   future diffs (`diff.rs:490`), the first payload buffered for an index keeps it (`diff.rs:492`),
//!   and a full index cycle later it stands in for the diff the authority put there. A client
//!   65536 diffs behind its cursor is sent the whole value, the wrapped distance being zero; one
//!   65537 behind is sent only the last diff (`diff.rs:286`). Netcode's replay window
//!   (`REPLAY_PROTECTION_BUFFER_SIZE`, 256 packets) bounds how late a packet can arrive, not how
//!   many diffs the list runs past it meanwhile.
//! - So each ledger diff names the position it lands at, and a replica refuses one for any other
//!   position and every diff after it (`Ledger`'s `apply_diff`). This game copies only the new
//!   suffix into its record and closes the match on a misplaced, shrunk or rewritten ledger
//!   (`net::construction::receive_ledger`).
//!
//! Both endpoints here are bare replicon apps exchanging messages directly (replicon's
//! `test_app`); the ledger is registered with replicon's `replicate_diff`, the rule the shipped
//! protocol's `replicate_diff` registers. Losing a packet removes the authority's unreliable
//! mutation messages; reliable updates still arrive, as lightyear's reliable channel delivers them.

use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::prelude::*;
use bevy_replicon::shared::backend::channels::ServerChannel;
use bevy_replicon::test_app::ServerTestAppExt;

use super::construction::{Appended, Ledger};
use crate::construction::breakage::{Entry, Event};

/// An authority and one client, each a bare replicon app ticking every update.
fn link() -> (App, App) {
    let mut server = App::new();
    let mut client = App::new();
    for app in [&mut server, &mut client] {
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            RepliconPlugins.set(ServerPlugin::new(PostUpdate)),
        ))
        .replicate_diff::<Ledger>();
        app.finish();
    }
    (server, client)
}

/// The fixture entry for ledger position `n`, told apart by its tick.
fn entry(n: u32) -> Appended {
    Appended {
        at: n,
        entry: Entry {
            construction: 1,
            event: Event::Wear((n % 1024) as u16),
            tick: n,
        },
    }
}

fn append(server: &mut App, ledger: Entity, diffs: impl IntoIterator<Item = Appended>) {
    let mut entity = server.world_mut().entity_mut(ledger);
    for diff in diffs {
        entity
            .apply_diff::<Ledger>(diff)
            .expect("the fixture ledger takes each next position");
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

/// The bytes of this tick's mutation messages, left queued for delivery.
fn mutation_bytes(server: &App) -> usize {
    server
        .world()
        .resource::<ServerMessages>()
        .iter_sent()
        .filter(|(_, channel, _)| *channel == usize::from(ServerChannel::Mutations))
        .map(|(_, _, message)| message.len())
        .sum()
}

/// This tick's mutation messages, taken from the authority before delivery.
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

fn held(app: &mut App) -> Ledger {
    app.world_mut()
        .query::<&Ledger>()
        .single(app.world())
        .expect("one replicated ledger on each endpoint")
        .clone()
}

fn assert_same(server: &mut App, client: &mut App, when: &str) {
    assert_eq!(held(client), held(server), "{when}");
}

/// A CAUGHT-UP CLIENT IS SENT ONLY WHAT IT LACKS: after a client holds the ledger, one more entry
/// reaches it as that one diff, not as the whole ledger again. A diff cursor that never starts
/// (bevy_replicon 0.44.2, MEASURED in the send census: 158 of 158 ledger sends whole) resends the
/// growing list on every change; it reached 103 KB a send in the match recipe. Fixture: 200
/// entries.
#[test]
fn a_caught_up_client_is_sent_only_what_it_lacks() {
    let (mut server, mut client) = link();
    server.connect_client(&mut client);
    let ledger = server
        .world_mut()
        .spawn((Replicated, Ledger::default()))
        .id();
    append(&mut server, ledger, (0..200).map(entry));
    deliver(&mut server, &mut client);
    assert_same(
        &mut server,
        &mut client,
        "the inserting update carries the ledger",
    );
    append(&mut server, ledger, [entry(200)]);
    server.update();
    let sent = mutation_bytes(&server);
    let whole = postcard::to_allocvec(&held(&mut server))
        .expect("the ledger serializes")
        .len();
    assert!(
        0 < sent && sent < whole,
        "one more entry sends {sent} B against the {whole} B ledger"
    );
}

/// A LATE JOINER ENDS WITH THE AUTHORITY'S LEDGER: a client that connects after entries were
/// appended receives them whole in the inserting update, then each later entry by diff. A cursor
/// started anywhere but at the inserted value would skip or repeat entries. Fixture: 300 entries
/// before the join, five after.
#[test]
fn a_late_joiner_ends_with_the_authoritys_ledger() {
    let (mut server, mut client) = link();
    let ledger = server
        .world_mut()
        .spawn((Replicated, Ledger::default()))
        .id();
    append(&mut server, ledger, (0..300).map(entry));
    server.update();
    server.connect_client(&mut client);
    deliver(&mut server, &mut client);
    assert_same(
        &mut server,
        &mut client,
        "the join carries the whole ledger",
    );
    for n in 300..305 {
        append(&mut server, ledger, [entry(n)]);
        deliver(&mut server, &mut client);
        assert_same(
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
    let (mut server, mut client) = link();
    server.connect_client(&mut client);
    let ledger = server
        .world_mut()
        .spawn((Replicated, Ledger::default()))
        .id();
    append(&mut server, ledger, (0..20).map(entry));
    deliver(&mut server, &mut client);
    for tick in 0..10 {
        append(
            &mut server,
            ledger,
            (0..7).map(|n| entry(20 + tick * 7 + n)),
        );
        lose(&mut server, &mut client);
    }
    assert_eq!(held(&mut client).entries.len(), 20, "nothing lost arrived");
    deliver(&mut server, &mut client);
    assert_same(
        &mut server,
        &mut client,
        "the next delivered tick carries every lost entry, in order",
    );

    append(&mut server, ledger, [entry(90)]);
    server.update();
    let late = take_mutations(&mut server);
    server.exchange_with_client(&mut client);
    client.update();
    append(&mut server, ledger, [entry(91)]);
    deliver(&mut server, &mut client);
    assert_same(
        &mut server,
        &mut client,
        "the newer tick carries both entries",
    );
    let mut messages = client.world_mut().resource_mut::<ClientMessages>();
    for message in late {
        messages.insert_received(ServerChannel::Mutations, message);
    }
    client.update();
    assert_same(&mut server, &mut client, "a late message changes nothing");
}

/// A DELAYED PACKET PLACES NO OLD ENTRY ONCE THE INDEX HAS WRAPPED: the original message of an
/// entry the client already took through a resend arrives after the ledger has run past half the
/// wrapping index range, so replicon buffers it as a future diff and, a full index cycle later,
/// offers it in place of the entry the authority put there. The replica refuses it and holds the
/// authority's entries up to that slot, none after. Taking it left the old entry at that slot
/// with the length and the prefix unchanged, which the record's checks cannot see. Fixture: 4096
/// entries a tick, the delayed message delivered at 40960 entries, the ledger run to 69632.
#[test]
fn a_delayed_packet_after_the_index_wraps_places_no_old_entry() {
    let (mut server, mut client) = link();
    server.connect_client(&mut client);
    let ledger = server
        .world_mut()
        .spawn((Replicated, Ledger::default()))
        .id();
    deliver(&mut server, &mut client);
    append(&mut server, ledger, [entry(0)]);
    server.update();
    let delayed = take_mutations(&mut server);
    server.exchange_with_client(&mut client);
    client.update();
    deliver(&mut server, &mut client);
    assert_same(&mut server, &mut client, "the resend carries the entry");

    let batch = 4096;
    let mut appended = 1;
    let mut run_to = |to: u32, server: &mut App, client: &mut App| {
        while appended < to {
            append(server, ledger, (appended..appended + batch).map(entry));
            appended += batch;
            deliver(server, client);
        }
    };
    run_to(10 * batch, &mut server, &mut client);
    let mut messages = client.world_mut().resource_mut::<ClientMessages>();
    for message in delayed {
        messages.insert_received(ServerChannel::Mutations, message);
    }
    client.update();
    run_to(17 * batch, &mut server, &mut client);
    deliver(&mut server, &mut client);
    let authority = held(&mut server);
    let replica = held(&mut client);
    assert!(
        replica.misplaced().is_some(),
        "the replica refused the stale entry"
    );
    assert_eq!(
        replica.entries[..],
        authority.entries[..1 << 16],
        "the authority's entries up to the stale entry's slot, one index cycle, and none after"
    );
}

/// A CLIENT FURTHER BEHIND THAN THE HISTORY ENDS WITH THE AUTHORITY'S LEDGER, FROM ONE WHOLE COPY:
/// once more entries were appended past its cursor than the authority retains, the next delivered
/// tick sends the whole ledger, and the next entry after that goes by diff again. A sender that
/// sent the retained run alone would leave a gap. Fixture: the retained history plus one entry,
/// over two lost ticks; `HISTORY_LEN` is read from its owner.
#[test]
fn a_client_further_behind_than_the_history_gets_one_whole_copy() {
    let (mut server, mut client) = link();
    server.connect_client(&mut client);
    let ledger = server
        .world_mut()
        .spawn((Replicated, Ledger::default()))
        .id();
    append(&mut server, ledger, (0..10).map(entry));
    deliver(&mut server, &mut client);
    let behind = <Ledger as Diffable>::HISTORY_LEN as u32 + 1;
    let mut from = 10;
    for to in [10 + behind / 2, 10 + behind] {
        append(&mut server, ledger, (from..to).map(entry));
        lose(&mut server, &mut client);
        from = to;
    }
    assert_eq!(held(&mut server).entries.len(), 10 + behind as usize);
    server.update();
    let whole = postcard::to_allocvec(&held(&mut server))
        .expect("the ledger serializes")
        .len();
    let sent = mutation_bytes(&server);
    assert!(
        sent >= whole,
        "the whole ledger goes once: {sent} B against {whole} B"
    );
    server.exchange_with_client(&mut client);
    client.update();
    assert_same(&mut server, &mut client, "the whole copy closes the gap");

    deliver(&mut server, &mut client);
    let next = 10 + behind;
    append(&mut server, ledger, [entry(next)]);
    server.update();
    let sent = mutation_bytes(&server);
    assert!(
        0 < sent && sent < whole,
        "then diffs again: {sent} B against {whole} B"
    );
    server.exchange_with_client(&mut client);
    client.update();
    assert_same(&mut server, &mut client, "and the diff arrives");
}
