# Input attestation: a consumable commits only on a tick the command can PROVE it was authored for

A value read out of an input buffer cannot be trusted to belong to the tick that read it. lightyear's `InputBuffer` hands back an ordinary-looking `TankCommand` for ticks the player authored nothing for — hold-last extrapolation past the buffered range, a `SameAsPrecedent` gap-fill written into the buffer *as data*, a stale entry the server was forbidden to overwrite, an `Absent`-anchored freeze that pins the server's `ActionState` at its last value. On a server-authoritative build that means the server spends ammo and deals damage for input nobody gave (the observed symptom: 1–2 unrequested MG rounds after the player releases fire). The decision, shipped in `PROTOCOL_REV 5` (`da1857d`, merged `8fea8fc`; clippy follow-up `64fa0d0`): **`TankCommand` carries the tick it was authored for, and the sim refuses to commit any *discrete* action on a command that cannot attest that tick is the tick being simulated.** Levels stay on hold-last, deliberately. The invariant is ours, not canon — that is the interesting part, and it is argued below.

## The mechanism: one stamp, one comparison

`TankCommand::for_tick` (`src/command.rs`) is stamped **once**, on the client, by `net::client::stamp_input_tick` — with `local_tick + input_delay()`, computed exactly as lightyear's `buffer_action_state` computes the slot it is about to file the command into, so the stamp names the tick the command will be *read back on*, on this client and on the server alike. It runs in `InputSystems::WriteClientInputs`, after whichever writer filled the `ActionState` and therefore before lightyear buffers it. From there the stamp is **inert**: it rides the input buffer, the wire, the server's buffer and rollback replay without ever being rewritten. It is explicitly *not* re-stamped during rollback — lightyear restores the historical `ActionState` per replayed tick, stamp included, and re-stamping with the replayed tick would forge attestation for input never authored for it, turning the guard into a rubber stamp.

The bridge (`net::protocol::bridge_action_state_to_tank_command`) is then one line:

```rust
if next.for_tick != tick.0 { next.fail_consumables_closed(); }
```

`TankCommand::fail_consumables_closed` is THE definition of the **consumable** set: the edge set (`fire_primary`, `crew_swap`, `respawn` — via `clear_edges`) **plus the automatic-fire level `fire_secondary`**, which is a level in shape but a consumable in consequence (an `Automatic` weapon cycles rounds off it for as long as it is held — [[0020-fire-mode-mechanism-enum]]). Two sets, two meanings, one caller each: `consume_edges` clears edges every tick; the bridge fails consumables closed only on an unattested tick.

**This REPLACED a detector, and that is the whole lesson.** The previous guard (`held_last`, `2ea6cf5`) asked lightyear whether it *had* an entry for the tick: `get(tick).is_none() && get_last().is_some()`. It was structurally blind. It saw case 1 (hold-last) and nothing else — a gap-fill and a genuinely held trigger are the byte-identical `Compressed::SameAsPrecedent`, so no rule over the buffer's *shape* can separate "the player repeated this" from "we filled this in". And case 4 defeats even case 1: behind an `Absent` anchor, `get_last()` dead-ends too, so the detector's second conjunct goes FALSE **precisely when the freeze bites**. Full mechanism, with the vendored line numbers and a runnable reproduction, in `.agents/scratch/upstream-reports/lightyear-absent-anchor-input-freeze.md`.

**Positive attestation over detection** is the general principle, and it is why the fix is not "one more special case": we do not enumerate the ways a value can be wrong — we require proof that it is right. The bridge's doc comment lists the four known routes for the reader's benefit, but the *code* reads none of them. It asks the command to prove itself, so it covers the fifth route we have not met yet, and every future regression in a framework input buffer we do not control.

## The invariant is OURS. Say so.

No shipped netcode does this, and a reader who assumes it is standard practice will delete it.

Repeat-last-input under starvation is **universal** — Source, Valorant and Overwatch all fabricate the missing input, and **none of them exempt discrete actions**. They are not wrong; they are in different families:

- **Rollback netcode** (GGPO and descendants) repeats the last input freely because a predicted input *commits nothing* — the frame is re-simulated when the real input lands. There is always something to un-simulate.
- **Unreal-style reliable move queues** never face the question: the client's moves arrive as an ordered reliable stream, so there is no gap to fill in the first place.
- **Server-authoritative + client prediction — our family — is the one place it is unsafe.** The server has already spent the ammo, already dealt the damage, already replicated it to the victim. When the truth arrives there is nothing left to take back, and the client predicted none of it.

So the *stamp* is well-precedented and we claim no novelty for it: Source's `usercmd` carries `tick_count` + `command_number` for exactly this reason. What is **ours** is the per-tick **gate on consumables** — the refusal to act on a command whose stamp does not match. State it as ours in the code and here, because "everyone repeats the last input" is true and is not a reason to drop this.

## The consumable/level asymmetry is not arbitrary

Same frozen tick. Categorically different consequence:

- A **level** (`throttle`/`steer`) or an **absolute** (`aim`/`range`) → 15.6 ms of stale throttle on a 57-tonne vehicle (`mass: 57_000.0`): roughly a centimetre, erased by the next tick, beneath the suspension noise floor. Hold-last here is not a bug being tolerated — it is **correct extrapolation**. A starved stream that keeps the last drive and lay is making the right guess, and nothing it guessed cannot be taken back. Ungated, on purpose.
- A **consumable** → a round leaves the barrel. Ammo spent, damage dealt, replicated to the victim. Irreversible.

The asymmetry is therefore *between the consequences*, not between the fields, which is why `fire_secondary` — a level by shape — sits with the edges. And the exposure is small and bounded: measured across 32 seed positions, the freeze's worst case is **exactly 1 transition tick** (the gap tick between the last *pressed* tick and the first *released* one; a release is a change, and a change re-anchors the buffer). One tick of stale throttle is nothing. One tick of stale trigger is a round. That is the entire argument, and it is why "the server drives your tank away" is a claim we do **not** make upstream — overstating the freeze invites a maintainer to hold a stick, see nothing, and close the report.

## The companion fix: pin the input delay

`for_tick` refuses to *commit* on a corrupt buffer. It does not *un-corrupt* it — and the corruption had a root cause worth removing separately. `InputDelayConfig::balanced()` recomputes the input delay from live RTT + jitter on every `SyncEvent`; on a WAN link whose RTT sits near a tick boundary it oscillates continuously (measured on our link: flipping 2↔3 at 64 Hz). Every one of those flips breaks lightyear's own `Δend_tick == 1` invariant — the buffer's write tick **stalls** (two local ticks author the same slot; the client's correction is refused by `update_buffer`'s `last_remote_tick` gate and the server keeps the superseded value) or **jumps** (`set_raw` gap-fills the skipped tick with a fabricated `SameAsPrecedent` that *both ends* then fire). Same root, three defects.

So `net::client` pins it: `SHIPPING_INPUT_DELAY_TICKS = 3`, `fixed_input_delay(3)` replacing `balanced()` (≈47 ms), with the `input_delay_is_constant` tripwire asserting `minimum == maximum` so nobody reintroduces an adaptive delay without confronting this. The pin is not purely a cost: 3 ticks buys the deepest input buffer (the most jitter tolerance before an input goes missing at all) and the shallowest rollback window.

**The two are complementary, not redundant.** The pin removes the seeds it can reach — it makes gap-fills and stalls impossible to construct. Attestation refuses to commit on any seed that survives, and seeds *do* survive: the connect-time `Absent` head is guaranteed every session, regardless of the delay config. Attestation is what makes us robust *whatever* the framework does next; the pin is what stops us provoking it.

**One honest non-coverage.** The delay-SHRINK case strands a value that IS correctly stamped for its own tick — the player authored it for that tick, then *revised* it, and the server never received the revision. No stamp can see that; the revision simply never arrived. It is closed by the pin, upstream of the bridge, and by nothing else.

## Costs, named not buried

- **Wire.** `for_tick` changes every tick and therefore **destroys `SameAsPrecedent` run-compression by design** — that is the point of it, not a side effect. Measured through the real `NativeStateSequence` and real bincode (`input_message_wire_cost`, `net/protocol.rs`): **aiming — the realistic regime — 156 → 176 B/message, +20 B, ≈ +1.2 KB/s upstream per client at 64 Hz.** The delta is small there precisely because our hull-local `aim` point ([[0001-aim-stored-hull-local]]) already changed every tick, so compression was *already* dead and the stamp only adds its own bytes. **Parked and perfectly still — the worst case — 36 → 176 B, +140 B, ≈ +8.8 KB/s** (an absolute ~11 KB/s), idle being the worst case for exactly the reason it sounds wrong: idle is the only regime where run-compression was still paying off. We pay most where the player is doing least, which is the cheap place to pay.
- **A 256 B ceiling, asserted.** `input_message_wire_cost` fails if the attested payload passes 256 B/message, and its failure message names the mitigations rather than leaving them to be reinvented: shrink the *attested* payload (a `u8` tick-LSB; or a fire-fields-only sub-struct carrying the stamp) — **do not drop attestation**, which is what keeps the server from firing rounds nobody asked for. The ceiling exists because the cost scales with `TankCommand`, not with the stamp.
- **~47 ms of pinned input delay** (the companion fix), traded for jitter tolerance and a shallow rollback window.
- **A late fire edge is dropped, not fired late** — deliberate, and not a defect of this ADR. Firing an edge on a tick it was not issued for *is* the bug (the shot leaves at the wrong muzzle pose and diverges from the client's prediction), so every netcode drops past ticks. lightyear's per-message redundancy (`num_ticks *= packet_redundancy`) means an isolated loss does not lose the edge; only loss deep enough to outlast that window drops a shot the client predicted, leaving `reload_remaining` disagreeing until the next shot reconciles it. Inherent to predicting fire on a lossy input stream.

## What this ADR does not say

It does not say hold-last is wrong. It is *right* for levels and absolutes, and it stays.

It does not claim the stamp is novel — only the gate.

It does not survive being "cleaned up" alongside the pin. When upstream fixes the `Absent` anchor (fix directions 1–3 in the report), the **pin** and its tripwire may retire — provided `lightyear-check-starvation.md` is also resolved, since `balanced()` triggers that one too. **`for_tick` and `fail_consumables_closed` stay regardless.** What changes is their *status*: they stop being load-bearing and become belt-and-braces, at which point their wire cost becomes a choice (shrink the attested payload, or keep paying 20 B and stop thinking about it). Today neither option is open — thinning attestation reopens unauthored rounds. The removal conditions are written down in the upstream report, not here, because they belong to the defect's lifecycle.

## Canon receipts

- **Source engine — `CUserCmd`.** The client's usercmd carries `tick_count` and `command_number`; the server keys the command to the tick it was issued for. Precedent for the STAMP. The per-tick refusal on consumables is not in it.
- **GGPO / rollback netcode.** A predicted input is a guess to be undone, not a fact to act on — which is exactly why repeat-last is *safe* there and unsafe here.
- **Overwatch — Tim Ford, GDC 2017.** Predict-everything-then-reconcile, with ammo and cooldowns server-authoritative and correcting the client. Repeats the last input under loss; does not exempt discrete actions.
- **Rocket League — Jared Cone, GDC 2018.** Extrapolated remote input is decayed by age to zero over a few frames — a held value is never held *forever*. The nearest thing to this rule in shipped canon, and still a decay, not a refusal.
- **lightyear issue #1559** (open, maintainer unconvinced) and **PR #1471** (`pop_keeping_last`, merged — whose stated purpose the `Absent` case defeats). Our report supplements both.

## Related decisions

[[0021-fire-replication-architecture]] — its **invariant 1** ("no discrete action on extrapolated input") is this rule, stated at the fire seam before it had a mechanism; this ADR is the mechanism, and the two compose: 0021's `FireEvent` survives the future predict-everyone change *precisely because* a starved remote fails its consumables closed here and fires nothing locally, so the event is the fallback tracer. · [[0018-wire-surface-fingerprinted-and-refused]] — `for_tick` is a wire-surface change: `PROTOCOL_REV` 4 → 5, `WIRE_TYPES_HASH` re-pinned in the same diff, skewed builds refused at the handshake. · [[0016-replicate-causes-derive-consequences]] — the input IS the cause; this decides when the authority is allowed to believe one. · [[0020-fire-mode-mechanism-enum]] — `Automatic` is why a *level* had to join the consumable set. · [[0001-aim-stored-hull-local]] — the hull-local aim point is why the realistic wire cost is 20 B and not 140.
