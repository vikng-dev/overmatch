# Overmatch netcode admission patch

The published `lightyear_netcode` crate, version 0.30.1, with changes marked
`OVERMATCH PATCH`. Registry bookkeeping and the published lockfile are omitted.

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
a pending connection. This crate's own unit tests need its dev-dependencies,
which cargo resolves only for workspace members, so nothing here runs them.
