# Draft: lightyear_netcode drops the denial reason, and admits a challenge after its handler closes

**LOCAL DRAFT — not filed.** Two API gaps in `lightyear_netcode 0.30.1`, read in the published
source. Upstream `main` was not re-read for this draft: check it before filing. Subject: the
connection handshake only.

## Suggested title

Netcode client cannot read why it was denied; `ConnectionRequestHandler` is consulted only at the
connection request, not when the challenge response completes it

## 1. The client logs the denial reason and then discards it

`Client::process_packet` (`src/client.rs`), on a `Packet::Denied` while
`SendingConnectionRequest` or `SendingChallengeResponse`, logs `pkt.reason` at `error!`, sets
`should_disconnect` and `should_disconnect_state = ClientState::ConnectionDenied`, and keeps
nothing else. The client plugin (`src/client_plugin.rs`) then inserts
`Disconnected { reason: DisconnectedReason::TransportError(format!("{state:?}")) }`, so the
application sees the string `"ConnectionDenied"` and no `DeniedReason`. A server that refuses with
`DeniedReason::Custom(..)` (the reason `ConnectionRequestHandler` exists to send) cannot tell its
client why. The reason is authenticated (it arrives in a packet sealed with the token's
server-to-client key), so it is safe to act on; an application wants to tell "server full" from
"this match no longer takes joins", show the cause, and stop retrying a server that will refuse
again.

**Proposed change.** Keep the last authenticated reason on `Client` and expose it:

```rust
denied_reason: Option<DeniedReason>,          // field, `None` in `new`
self.denied_reason = Some(pkt.reason);        // in the `Packet::Denied` arm
pub fn denied_reason(&self) -> Option<&DeniedReason> { self.denied_reason.as_ref() }
self.denied_reason = None;                    // first line of `connect`
```

It must survive `reset_connection` and `reset`, which run on the terminal transition, and clear in
`connect`, which a fresh attempt and `connect_to_next_server` both call, so a token listing another
address moves on with no stale reason. The packet format is unchanged. A richer alternative is a
`DisconnectedReason::Denied(DeniedReason)` variant filled by the client plugin; it changes a public
enum, so the getter is the smaller first step.

## 2. A closed handler still admits a challenge issued while it was open

`Server::process_connection_request` (`src/server.rs`) asks
`cfg.connection_request_handler.handle_request(..)` before it issues a challenge. Its sibling
`Server::process_connection_response`, which turns that challenge into a connection
(`client.connect()`, `on_connect`), rechecks `max_clients` but not the handler. A handler whose
answer changes over time (a match that stops taking joins, a maintenance drain) therefore admits
every client whose request it accepted before it closed, for as long as challenges take to return
(one round trip, and the client resends responses until it is answered).

**Proposed change.** In `process_connection_response`, after the `ServerFull` recheck and before
`client.connect()`, ask the handler again and refuse the same way the request path does:

```rust
if let Some(reason) = self.cfg.connection_request_handler.handle_request(id::PeerId::Netcode(id)) {
    self.send_netcode_packet(DeniedPacket::create(reason), client.send_key, entity)?;
    return Err(Error::Denied(id::PeerId::Netcode(id)));
}
```

The client is in `SendingChallengeResponse`, which already accepts `Packet::Denied`, so it gets an
authenticated refusal rather than a timeout. A handler that is a pure predicate (the default
accepts everyone) behaves the same at both stages; a stateful handler that counts its calls would
see two per admitted client, which the trait's documentation should then say.

One upstream property carries over unchanged: a client refused at the response stage, like one
refused there for `ServerFull` today, keeps its pending `conn_cache` entry, since
`check_for_timeouts` visits only connected clients. Each refused client id holds one small entry
for the server's lifetime.

## Evidence

Overmatch closes late joins partway through a match: its handler is one shared `AtomicBool` the
match clock writes each tick (`src/net/admission.rs`, `JoinAdmission`). Two headless real-UDP tests
in `src/net/hosting.rs` pin both changes:
`a_challenge_issued_while_open_cannot_connect_after_admission_closes` closes the gate between a
client's request and its response, and asserts the client receives the custom reason and never
connects; `a_short_mode_cycles_replicates_and_refuses_a_late_join_with_its_reason` asserts a late
join reads the typed reason and renders it. Before the response-stage recheck, the first test's
client connected after closure (CLAIMED by the lane that wrote the patch; not re-run without it).

## Workaround

`vendor/lightyear_netcode-0.30.1-admission` carries both changes, each marked `OVERMATCH PATCH`
(`OVERMATCH_PATCH.md`), behind `[patch.crates-io]`. Removal condition: upstream exposes the
client's denial reason and rechecks the handler at challenge completion.

## What fixing this unlocks for us

Drop the vendored crate and its `[patch.crates-io]` line. Nothing else changes: the game's own
refusal type and its tests stay.
