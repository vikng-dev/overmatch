# The wire surface is fingerprinted and refused, not trusted

bevy_replicon addresses replicated components by their **registration index**, not by name, so two builds that registered the wire surface differently do not fail loudly — they silently misapply each other's messages. The deployed alpha.4 server replicated `NetHealth` at the index a `main`-built client had since re-registered as `NetCrew`; the client spammed `unable to apply mutate message … missing history component` every tick, forever, with no hint of the cause (2026-07-11 — the same incident [[0016-replicate-causes-derive-consequences]] revises). The decision: make a skewed peer **refuse to connect** rather than connect and corrupt — both ends bake a build fingerprint into the netcode handshake, and a mismatch is dropped before replication ever starts (`2e18045`).

## The mechanism

**One fingerprint, folded into the connect-token AEAD.** `PROTOCOL_FINGERPRINT` is a compile-time `u64`: a labeled `const` FNV-1a fold of the ordered wire-surface hash, own wire-type-definition hash, pinned avian3d, lightyear and bevy_replicon versions, `PROTOCOL_REV`, and the crate version (`net/protocol.rs`). No build script or proc macro participates. Both ends set `protocol_id()` as netcode.io's `protocol_id`: the hash of the build's **compatibility record** (`net/compatibility.rs`), which states `PROTOCOL_REV`, the crate version, that constant, and a digest per content source each peer derives play from (the map, the materials, the tank construction, the armaments, the aim input, the ground table). The map earns a term because it is a runtime choice over content that never crosses the wire: both peers *derive* their terrain and scatter from the shipped files, so a same-build pair on different maps would connect and then integrate different ground. netcode folds `protocol_id` into the connect token's authenticated encryption and checks it before admission. The refusal lands at the handshake, before a single component replicates.

**A mismatch answers with its cause.** Upstream netcode drops a request under another `protocol_id` unanswered, which would leave it transport-indistinguishable from a down server. The vendored `lightyear_netcode` (`OVERMATCH_PATCH.md`) adds `ForeignProtocolHandler`: the server decrypts such a request's token under the requester's own tag with its private key, and answers a key holder with a denial sealed under that tag, so either build can read it. The record's text is the stable layer both builds share: the client states it in the token's user data, the server's denial carries its own, and each side names the first difference, in the order that decides who acts: the revision (which side is older), the release, the wire build, then each differing content term. The server logs the cause and admits nothing; the player reads it on the joining screen or the Join menu. Only silence is left for the down server: past three unanswered attempts (`unanswered_after_attempts` in `assets/net/client.ron`) the joining screen says the server is not answering. A server whose key the client lacks, or one that predates the record, is silent too.

## The tripwire: a wire-breaking change cannot be silent

The fingerprint only refuses a skewed peer if one of its inputs moves. It therefore folds the pinned manifests themselves, rather than relying on a developer to remember `PROTOCOL_REV`. `WIRE_SURFACE` is the hand-maintained ordered registration list and `WIRE_SURFACE_HASH` pins it. `WIRE_TYPES_HASH` pins normalized definitions of this crate's wire types. `WIRE_DEP_AVIAN3D`, `WIRE_DEP_LIGHTYEAR` and `WIRE_DEP_REPLICON` pin external serialization, framing and replication dependencies. Their tests fail on an unpinned change and print the value to re-pin.

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
