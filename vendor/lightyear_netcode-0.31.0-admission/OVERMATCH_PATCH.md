# Overmatch netcode admission patch

This directory is the published `lightyear_netcode` 0.31.0 crate with the admission changes below,
marked `OVERMATCH PATCH` at their sites.

Produced from `https://static.crates.io/crates/lightyear_netcode/lightyear_netcode-0.31.0.crate`,
whose sha256 `2cf16a8bb3ed7746dc8808a2a6c4d026b7bb174692bb70beaf3c42bbd85cae28` matches the
crates.io index's `cksum`, by `tar -xzf lightyear_netcode-0.31.0.crate -C
vendor/lightyear_netcode-0.31.0-admission --strip-components=1 --exclude='*/Cargo.lock'
--exclude='*/.cargo_vcs_info.json'`. The published lockfile and VCS record are omitted; neither is
read for a path dependency.

## Authenticated admission and compatibility refusals

`Client::denied_reason` retains the authenticated `DeniedReason` through the
terminal state reset and clears it on a fresh connect. Upstream only logs the
reason, leaving the application unable to tell a closed Battle from a full server.
The packet format is unchanged.

`Server::process_connection_response` rechecks `ConnectionRequestHandler` before
confirming a pending connection. A mode can close admission while the challenge
is in flight; checking only the initial request admits it after closure.

`ForeignProtocolHandler` answers a connection request under another protocol id.
Upstream drops it unanswered, so a peer of another build cannot tell a refusal
from a down server. With a handler set, the server decrypts the private token
under the requester's own protocol id with its private key; when that succeeds it
sends the handler's `DeniedReason`, encrypted under the requester's protocol id
and the token's server-to-client key, and creates no connection state. A
requester without the key still gets nothing. The packet format is unchanged:
the request and denial layouts are netcode 1.02's, which is what lets builds of
different protocol ids read each other here.

That path runs the upstream request checks that do not compare protocol ids: the
packet size cap, the version, the expiry, the private key and, when enabled, the
server-address check (`token_names_this_server`, shared with the upstream path).
It skips the reused-token check, which guards the connection state a request
creates; this path creates none. A replayed request is therefore answered again,
and that is bounded: each answer is one denial of at most 282 bytes (DERIVED:
prefix 1, sequence at most 8, reason code and length 2, reason at most 255, MAC
16) to a request of 1078 (DERIVED: prefix 1, version 13, protocol id 8, expiry 8,
nonce 24, private token 1024), sent to the request's source, so a replay cannot
amplify traffic, hold a slot or a client id, or read the denial without the
token's server-to-client key.

Guards live in the game's headless real-UDP hosting tests: a late join receives
its typed reason, a challenge issued while open cannot complete after closure,
and a peer of another build receives the server's compatibility record and is
never admitted, while one without the key is told nothing and no refusal leaves
a pending connection.
Standalone validation: `scripts/serial-cargo check --manifest-path
vendor/lightyear_netcode-0.31.0-admission/Cargo.toml` (default client/server features).
