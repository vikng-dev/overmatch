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

Guards live in the game's headless real-UDP hosting tests: a late join receives
its typed reason, and a challenge issued while open cannot complete after closure.
