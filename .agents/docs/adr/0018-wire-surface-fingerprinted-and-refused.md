# The wire surface is fingerprinted and refused, not trusted

bevy_replicon addresses replicated components by their **registration index**, not by name, so two builds that registered the wire surface differently do not fail loudly — they silently misapply each other's messages. The deployed alpha.4 server replicated `NetHealth` at the index a `main`-built client had since re-registered as `NetCrew`; the client spammed `unable to apply mutate message … missing history component` every tick, forever, with no hint of the cause (2026-07-11 — the same incident [[0016-replicate-causes-derive-consequences]] revises). The decision: make a skewed peer **refuse to connect** rather than connect and corrupt — both ends bake a build fingerprint into the netcode handshake, and a mismatch is dropped before replication ever starts (`2e18045`).

## The mechanism

**One fingerprint, folded into the connect-token AEAD.** `PROTOCOL_FINGERPRINT` is a compile-time `u64`: a labeled `const` FNV-1a fold of the ordered wire-surface hash, own wire-type-definition hash, pinned avian3d and lightyear versions, `PROTOCOL_REV`, and the crate version (`net/protocol.rs`). No build script or proc macro participates. Both ends set `protocol_id()` — that constant with the **map's content digest** folded onto it — as netcode.io's `protocol_id`: the client in `Authentication::Manual` (`net::client`), the server in `NetcodeConfig` (`net::server`). The map earns a term because it is a runtime choice over content that never crosses the wire: both peers *derive* their terrain and scatter from the shipped files, so a same-build pair on different maps would connect and then integrate different ground. netcode folds `protocol_id` into the connect token's authenticated encryption, so a client whose tag differs produces a token the server **cannot decrypt** — it drops the request. The refusal lands at the handshake, before a single component replicates.

**A mismatch is transport-indistinguishable from a down server.** The dropped token surfaces to the client as `ConnectionRequestTimedOut` — byte-for-byte the terminal state of an unreachable server (verified against vendored `lightyear_netcode`). We do not try to tell them apart, because netcode gives us nothing to tell them apart *with*: the connect overlay waits out three attempts (`MISMATCH_HINT_AFTER_ATTEMPTS` — long enough to rule out a server still starting up), then names **both** causes — "server down or client/server build mismatch (update the client or redeploy the server)". An honest ambiguous message beats a confident wrong one.

## The tripwire: a wire-breaking change cannot be silent

The fingerprint only refuses a skewed peer if one of its inputs moves. It therefore folds the pinned manifests themselves, rather than relying on a developer to remember `PROTOCOL_REV`. `WIRE_SURFACE` is the hand-maintained ordered registration list and `WIRE_SURFACE_HASH` pins it. `WIRE_TYPES_HASH` pins normalized definitions of this crate's wire types. `WIRE_DEP_AVIAN3D` and `WIRE_DEP_LIGHTYEAR` pin external serialization and framing dependencies. Their tests fail on an unpinned change and print the value to re-pin.

The house process still bumps `PROTOCOL_REV` with a wire change because the revision is useful release vocabulary. Compatibility safety does not depend on that convention: re-pinning either hash or dependency version changes `PROTOCOL_FINGERPRINT` directly even if the revision bump is forgotten. `fingerprint_couples_every_pinned_wire_manifest_value` exercises the production fold and proves sensitivity to each input. These tests do not prove semantic interoperability; they make an unnoticed manifest change and an unchanged handshake tag unable to coexist.

Enumerating lightyear's `ComponentRegistry` at runtime was considered and rejected as disproportionate — it keys on `TypeId`, mixes in lightyear-internal registrations, and its `finish()` poisons the registry. A list next to the code it shadows, bound by a hash, is the proportionate guard.

## The operational half

The dedicated server is deployed by the release pipeline, not by merges: a `vX.Y.Z` tag builds the Linux server, ships that artifact to the droplet, and only then publishes the GitHub Release (`release.yml`, `DEPLOY.md`). So the droplet and the latest downloadable client are the same build *by construction* — the pairing this ADR's refusal exists to protect is guaranteed for every player who has a release in hand, rather than converging after a merge.

What that leaves open is the developer on a `main` ahead of the last tag: their locally built client is skewed against the droplet until the next release, and refuses cleanly instead of corrupting. That skew is now bounded by the release cadence rather than by minutes, which is why the guard's job is to *refuse*, not to *reconcile* — playtest `main` against a local server, and the droplet against a release.

## What this ADR does not say

It does not make skewed builds *interoperate* — it makes them decline to try. There is no wire-format negotiation and no versioned migration; a mismatch is a refusal, full stop. That is correct for an alpha with one authoritative server we redeploy in minutes, and would not be for a world of long-lived heterogeneous clients.

It does **not** solve the stale *client*. A release build in a friend's hands is not auto-updated; when the wire moves under it, that client gets the honest refusal and the "update the client" hint — not a new binary. Closing that needs a client updater or a store-side version gate, out of scope here ([[0009-release-artifacts-and-repo-layout]] owns the release channel).

Nor does the fingerprint authenticate anything — `protocol_id` is a compatibility tag, not a secret (the dev private key is a separate `[0; 32]`), and FNV-1a is chosen for being `const`-evaluable, not cryptographic. A hostile peer is a different threat than a skewed one; this ADR is only about the skew.

## Related

- [[0016-replicate-causes-derive-consequences]] — the `NetHealth`→`NetCrew` re-registration that motivated the fingerprint is the same incident that motivated 0016's atomic-snapshot rule; the wire surface this pins is the set of causes 0016 decides to replicate.
- [[0014-sim-view-split]] *Deferred — phase 2* — the "baked artifact + connect-handshake hash" is this same handshake seam one layer down: the tank *bake* proven identical by a hash folded into the same connect token. Same mechanism (refuse at handshake on a build hash), different surface (geometry vs the component registry).
- [[0009-release-artifacts-and-repo-layout]] — the release/deploy channel whose stale-client gap this ADR names but does not close.
