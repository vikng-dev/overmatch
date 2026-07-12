# Fire replication architecture: state is truth, events are cosmetics, delivery is never load-bearing

Opponent fire — the tracer you see leave another tank's barrel, the round that bounces off its glacis, the recoil that rocks its gun — is entirely cosmetic. Damage is server-authoritative ([[0016-replicate-causes-derive-consequences]]); a shell an observer flies deposits no HP and applies no impulse ([[0014-sim-view-split]], the `ClientReplica` gate). So the whole opponent-fire seam is built on one commitment: **a client's picture of another tank's fire is reconstructed from a few loss-tolerant events, and its *correctness* — where the round is, whether it hit — is carried by replicated state, never by whether an event arrived.** This ADR states the three invariants that follow, the `ShotId` spine that ties the messages together, and how the fail-closed guard and the bounce keyframe compose rather than fight.

The shipped-game canon this rests on is annotated at the end; each invariant names its receipt inline.

## The three invariants

**1. No discrete action on extrapolated input.** Firing consumes ammunition and deals damage — a discrete, irreversible consequence. Extrapolation (holding a remote's last input when the next is missing) is for hiding *movement* latency, not for committing discrete state. When lightyear's native input holds a released trigger under packet loss, the automatic-fire *level* is failed **closed** on the extrapolated tick, and the fire *edge* is dropped, so a lost release cannot cycle one extra server round (`bridge_action_state_to_tank_command`). This is Overwatch's rule (predict-everything for feel, never commit ammo/damage on unconfirmed input) and GGPO's (a predicted input is a guess to be undone, not a fact to act on). The repair for the loss itself is redundancy, not reliability — invariant 3.

**2. No observer-improvised collisions.** A ricochet's outcome depends on the exact pose of the surface it hits *on the server*. An observer re-simulating that bounce against an interpolated, ~100 ms-stale, non-authoritative pose diverges — the round wanders off where the authoritative shell never went. So an observer's deterministic local flight is honest **only up to the first surface**. Past it, the observer either renders the server's *actual* bounce (a replicated significant point) or renders nothing improvised. This is Unreal's "other players see only the server-replicated projectile" (the predicted copy does no collision) and Halo's grenades-as-replicated-object-state; it is [[0017-mutual-contact-resolves-on-the-authority]] applied to the shell-vs-plate interaction — the surface pose is not ours to resolve.

**3. Correctness from state, cosmetics from events, delivery never load-bearing.** A dropped fire event costs a missing tracer; a dropped keyframe costs a truncated trail. Neither can cost a wrong hit, because the hit is absorbing and replicated (`NetCrew`). This is Halo's model exactly — "please fire my weapon" / "this weapon was fired" ride the *unreliable Events* protocol and the game "degrades gracefully rather than stalling" under bandwidth pressure. Making the cosmetic channel reliable to stop tracer loss is the canon anti-pattern (head-of-line blocking on stale effects); the fix is redundancy on an unreliable channel plus receiver tolerance.

## The `ShotId` spine

One shot is `(shooter, weapon slot, fire tick)` — a `ShotId`. It is the correlation key every shot-scoped message shares, and it is deliberately **net-neutral** (a crate-root type with a plain `u32` tick, not lightyear's `Tick`) so the always-runnable sim layer can key a cosmetic shell on it without naming the netcode — the `tests/net_boundary` contract that keeps single-player a runtime mode rather than a compile variant. `net::protocol` converts to/from `Tick` at the wire boundary.

Three properties earn their place:

- **`fire_tick` is load-bearing, not decoration.** An automatic weapon fires the same `(shooter, weapon)` every few ticks; without the tick, every round of a burst shares one id and the redundancy dedup (invariant 3) would collapse the whole burst to a single tracer. It is strictly increasing per weapon, which the dedup relies on.
- **Derived, not carried.** `FireEvent` exposes its `ShotId` by accessor over the fields already on the wire (`shooter`, `weapon`, `fire_tick`) — zero extra bytes, and the id can never disagree with the geometry it names. `RicochetKeyframe`, which has no geometry of its own to derive from, carries the `ShotId` as a field.
- **Both ends stamp it on the local shell** (`ballistics::Shot`). The observer builds it from the wire; the authority — which cannot read the tick in the sim layer — completes it after spawn from the shell's `ShotSource` plus the timeline (`stamp_shot_ids`), yielding the *identical* id the `FireEvent` carried, so a later keyframe correlates to the right shell on every machine. A shell with **no** `Shot` (the owner's own predicted shell, every SP/sandbox shell) is not keyframe-eligible and fail-closes immediately, exactly as before this slice — no keyframe ever addresses it.

This is the same match-id discipline Unreal uses (a client-local `uint32` pairing predicted to replicated), generalised to a first-class type so future shot-scoped messages — an impact confirm, say — reuse it rather than inventing a parallel key.

## Fail-closed and keyframe-upgrade compose; they do not fight

Before this slice the observer march already did the honest thing at armor contact: stop dead, neutral spark, truncate the trail (the `!deposit` guard — invariant 2, before we had a way to show the real bounce). This slice **upgrades that guard without replacing it**, and the composition is the point:

- **Pre-armed (the common case).** The server emits a `RicochetKeyframe` at server-bounce time; the observer's shell flies a delayed timeline and usually reaches contact *after* the keyframe has arrived. It re-seeds from server truth — the exact bounce origin and post-bounce velocity, a point pushed onto `ShellPath` and `PenetrationMarks::ricochets` so the trail ribbon and the tracer-clamp anchor continue *through* the bounce with no gap, the ember riding the same entity untouched — and keeps marching. Allocation-free: a buffer lookup and a push onto vectors that already grow.
- **Hold (the exception).** No keyframe yet: freeze the shell at contact, no impact VFX, for a bounded grace window (~250 ms of ticks — sized to cover the 100 ms interpolation delay plus keyframe send jitter and a redundancy resend interval, yet short enough that a genuinely dropped keyframe truncates within a few frames). The keyframe arriving inside the window re-seeds; **re-aged forward by exactly the ticks it held.** That re-aging is subtle and load-bearing: the observer shell reaches contact when its predicted present ≈ the server bounce tick (both integrate the identical pre-bounce arc from `fire_tick`), and the present advances one tick per held tick, so the hold count *is* `present − bounce_tick` — the exact catch-up the shared integrator (`fast_forward_shell`, the same one the initial `fire_tick` catch-up folds) applies to put the resumed shell back on the present timeline. The sim needs no clock reading to compute it.
- **Truncate (the fallback).** Past the window, the keyframe is treated as lost and the shell degrades to the pre-slice fail-closed stop and neutral spark. **Correctness never depended on the keyframe** — a dropped one is honest truncation, invariant 3.

The authority march (server, SP, sandbox) is completely unchanged: it resolves the bounce for real and, on the authority, raises the sim-layer `ShellRicochet` that `net::server` maps onto the wire (`ADR-0016`: replicate the *cause*, the observer derives the re-seed). This keeps ballistics free of any netcode name — the keyframe buffer, `Shot`, and `ShotId` are all net-neutral crate-root/sim types the net layer fills and the march consumes.

## Redundancy, not reliability

WAN loss of a *cosmetic* stream is repaired the input-redundancy way (Overwatch's sliding window: every packet re-sends recent frames so one delivered packet repairs a burst of prior losses). A `FireBurst` envelope carries the last N=4 fire events **and** the last N keyframes; the channel is now sequenced-unreliable, so a newer burst supersedes an older one and each burst carries the whole current window — dropping a stale or reordered burst loses nothing the next re-carries, at no acks/retries/head-of-line cost. N=4 is sized against the worst case: a 750 rpm MG cycles at 12.5 rounds/s, one burst per shot ≈ every 80 ms, so a 4-deep window keeps each event alive across ~3 subsequent bursts (~240 ms) — past a typical multi-packet WAN loss.

Two decisions fall out:

- **Sent to `All`, deduped at the receiver** — by `ShotId` for fires (spawn each shell exactly once), by `(ShotId, sequence)` for keyframes (the buffer insert is idempotent). This is what lets *one shared burst carry multiple shooters' events correctly*: an `AllExceptSingle(owner)` target could not, because a burst re-carrying shooter B's fires must still reach owner A, yet must not double A's own. The `locally_fired` guard drops a client's own echo; the one-frame self-echo it discards is negligible against a correct redundancy window.
- **Reliability is reconstructed from state, never bought with a reliable channel.** The reliable path already exists for the facts that need it — `NetCrew` (health/death) and `NetBelts` (ammo). The cosmetic streams stay unreliable and lean.

## Costs, named not buried

- The owner's own predicted shells still fail-close at armor contact immediately (no `Shot`, so no keyframe). A player watching their *own* round strike an opponent's plate sees the truncation, not a bounce — the opponent's hull is interpolated and its true pose is the server's, so an owner-side bounce would be improvised. Accepted: the shooter's damage is server-authoritative and the tracer is cosmetic.
- A held shell freezes visibly at the plate for up to the grace window before it re-seeds or truncates. This is the exception path (the keyframe usually pre-arms); the window is bounded so a dropped keyframe never freezes a round for long.
- Redundancy costs bandwidth: ~4× the old single-send on the fire stream (small structs, a 1v1 duel). The window depth is a knob, sized here for droplet-range links.
- The re-aging equates the hold count with `present − bounce_tick`. The equality is exact to within the sub-tick jitter of "contact tick ≈ bounce tick"; the residual is within the integration tolerance the carry-through test asserts, and `bounce_tick` rides the wire (unused on the hot path) for audit and a future RTT-adaptive re-aging path.

## What this ADR does not say

It does not say observers should predict opponent fire. They interpolate other tanks ([[0017-mutual-contact-resolves-on-the-authority]]); the shell is a cosmetic reconstruction co-indexed with the client's *own* predicted present, not a prediction of the opponent. It does not make any cosmetic stream reliable. And it does not move the wire skew guard — `FireBurst`, `RicochetKeyframe`, and `ShotId` are pinned into `WIRE_SURFACE`/`WIRE_TYPES_HASH` and refused on mismatch exactly like every other wire type ([[0018-wire-surface-fingerprinted-and-refused]]); this slice bumped `PROTOCOL_REV` 3 → 4 in the same diff as the wire change.

## Canon receipts

- **Halo: Reach — David Aldridge, "I Shot You First," GDC 2011.** Weapon fire rides the *unreliable Events* protocol ("unreliable notifications of transient occurrences"); under bandwidth pressure the prioritiser drops them and the game degrades gracefully. Grenades — bouncing projectiles — are replicated as *object state with continuous updates*, not fired-and-forgotten. Invariants 2 and 3.
- **Unreal network prediction for projectiles — Steve Streeting, 2024.** Owner fires a predicted actor + a Server RPC; a client-local `uint32` match id pairs predicted to replicated; the server actor owns collision and impact, the predicted copy does none, and **"other players see only the server-replicated projectile."** The blueprint for invariant 2 and the `ShotId` spine.
- **Overwatch — Tim Ford, GDC 2017.** The sliding input window ("bundles every input since the last server-acknowledged state") and predict-everything-then-reconcile; ammo/cooldown are server-authoritative and correct the client. Invariants 1 and 3.
- **Gaffer On Games — Glenn Fiedler, *State Synchronization*.** On a correction "snap the physics state hard"; bounces are the significant points where local re-sim diverges, so replicate them rather than re-derive. The re-seed-from-truth shape of the keyframe upgrade.
- **Rocket League — Jared Cone, GDC 2018.** Extrapolated remote input is *decayed by age* to zero over a few frames — a held value is never held forever. The fail-closed rule of invariant 1.

## Related decisions

[[0014-sim-view-split]] (cosmetic shells are view over sim state) · [[0015-divergence-doctrine]] (netcode as removable scaffolding) · [[0016-replicate-causes-derive-consequences]] (the keyframe replicates the cause; recoil derives) · [[0017-mutual-contact-resolves-on-the-authority]] (the surface pose is not the observer's to resolve) · [[0018-wire-surface-fingerprinted-and-refused]] (the new messages are pinned and refused on skew).
