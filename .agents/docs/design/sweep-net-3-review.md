# Net sweep 3: review and disposition

Subject: the game's netcode and damage model, game state only.

Base: `e17642e7`; lane `lane/sweep-net-3`, bay-2. The MEASURED start was
2026-11-01 07:52:23 +0700. Functional implementation is committed through `9430dced`.
This report records the implementation and closing checks. No subagents were used.

## Sweep-net-2 standards review

| ID | Severity | Disposition |
|---|---|---|
| R2-window | P2 | Fixed (`01919dc3`). Bevy assumes `PrimaryWindow` is unique; it does not enforce it. The device feed distinguishes a closed window from duplicate windows and panics on duplicates. |
| R2-revision | P3 | Fixed (`5e131818`). The construction digest already changed compatibility, but the documented revision signal had not advanced. The lane advances it deliberately for each subsequent wire-contract change; final revision is **54**. |
| R2-bot | P3 | Fixed (`d8afac61`). Empty/on aliases had no tracked runner; the lever now accepts an explicit u32 count or absence. Numeric zero remains a valid count. |
| R2-delay | P3 | Disagreed with the link-only/maximum claims; corrected (`eac0d4fb`). The mixed stamps include sender pacing and receiver scheduling. DERIVED: the full-ring inverse-CDF rank is N-1, excluding one sample. Retention is nominal, not a wall-clock eviction deadline. The percentile law is unchanged. |

No P1 was found. These standards findings are resolved; no reviewed P3 remains open.

## Sweep-net-2 specification review

| ID | Severity | Disposition |
|---|---|---|
| R2-armament-data | P2 | Fixed (`aaae01b0`). Tank construction names registry keys; it does not define their facts or fuze modes. Round types and every embedded round/gun key and source now join map, materials and construction in the handshake. A registry-only edit changes the connection tag. |
| R2-fire-origin | P2 | Fixed (`f04c6a54`). `ReceivedFire` accepted a non-finite origin. A typed `Origin` refusal now logs and drops it before any presentation queue or shell exists. |
| R2-disconnect | No finding | Pinned `lightyear_replication` 0.30.1 `on_replication_disconnect` observes removal of either `Connected` or `ReplicationReceiver`, and queues non-persistent replicas for despawn in that flush. Production links/replicas do not opt into persistence. Real-UDP leave/outage tests and buffer guards pass. Multiple client links remain a construction bug. |
| R2-boxed-round | No finding | Boxing the bound round changes storage, not its value, mode, binding or wire form. The clippy-driven enum shape preserves the selected round. |
| R2-measurements | Qualified | The motion table retains the prior lane's reported MEASURED headless and hidden runs separately, including hidden stalls/resyncs. This lane did not remeasure them. They do not certify a link-only bound or predict rendered feel. |

A mismatched tag produces an undecryptable token, indistinguishable at the client from an
unreachable server. The joining hint honestly names server availability and build/asset skew;
it does not invent an authenticated registry-specific denial or diagnose startup from a retry count.

## Assigned inventory

| Finding ID | Disposition and invariant |
|---|---|
| EH-N3-fire | Fixed (`ab81e059`). Missing/conflicting spawn identities, combatant identity and duplicate routes panic. The tank load gate refuses weapon counts beyond the wire's u8 slots, naming `weapons` and the fix. `shooting::fire` relies on that gate instead of losing identity. |
| EH-N3-explanation | Fixed (`ea318a76`). Required vocabulary and one authority server; records must encode against their own certified blueprint. Missing identity/life, vocabulary names or legal quantized record data are internal recording-contract bugs, not dropped explanations. |
| EH-N3-wire | Fixed (`09984e19`, `8520218d`). Non-finite authority poses, speeds and phases panic instead of becoming identity/zero. Finite saturation and full-precision large positions remain. Truncation, non-finite peer positions and impossible rotations return typed decoding causes. Pinned Replicon 0.44.2 `try_apply_update` visibly logs decoding failures. |
| EH-N3-diagnostics | Fixed (`a77b355a`). Corrupt state stops in every build with entity/pose/hierarchy context. The finite Avian placeholder is checked by its actual symbol, replacing an unexplained magnitude threshold. |
| EH-N3-command | Fixed (`69fc4ffa`, `65e5d6d3`). Parse raw input once per boundary into private checked state. Malformed levels/range/aim file idle input and log the offending field/link; repeated identical refusals are rate-limited. Tick attestation remains. Local writers use the same parser. Servo consumers take checked aim and assert rig/pose/range-table invariants. Scripted out-of-envelope aim refuses startup. Parsed input is required on both raw commands and the replicated identity, before rig attachment. |
| EH-N3-batch/server/send | Fixed (`ab81e059`). Batch bounds and exactly one authority server are unconditional invariants. Pinned Lightyear sends serialize into live transports' unbounded in-process queues; registration/serialization/queue failure is a bug, so the log-and-continue arms and obsolete error counters are gone. |
| EH-N3-route-absence | Partly disagreed; fixed (`ab81e059`). Authority ricochet/terminal/dose events are emitted before projectile removal, so a missing route is a bug. Ownerless bots, a spent private receipt, disconnected recipients and an empty observer set are legitimate absences and are named. |
| EH-N3-client-vocabulary | Fixed (`ea318a76`). Receipt requires the vocabulary built before play; bare fixtures supply it explicitly. |
| AngularVelocity question | Removed (`09984e19`). No presentation reader uses its replicated value. The codec, registration, manifest row and test arms are gone. A live-registry regression verifies Avian's disabled automatic registration cannot silently put it back. `LinearVelocity` retains its drive-HUD reader. |
| N4 / TankSimSource::get | Deferred by dispatch, unchanged. Ledger/UI work and that source API belong to the next lane. |

## Additional findings closed

- Parsed input initially followed raw-command attachment on a received root. Requiring it on
  `NetTank` itself fixes that spawn boundary; the rig-free replica regression pins it.
- The old rotation decoder clamped an impossible stored-square sum to zero and normalized a
  plausible pose. It now refuses the malformed payload. DERIVED: the omitted largest unit
  component leaves the stored sum near at most three quarters, safely below one.
- Unattributed emissions skip transport only when unkeyed. A keyed emission without its source
  now fails at `queue_fire`, before any later path event can discover the missing route (`9430dced`).
- The route fixture now emits twice from the same world. The weapon-width test requires the exact
  refusal cause, so digits in a fixture name cannot mask a later validation error (`b470608e`).
- The optimized servo pass's oracle comments now state same-input equivalence, without claiming
  a cross-peer determinism requirement or that the edited oracle is verbatim historical code.

## Verification and handoff

MEASURED green targeted batches, each with formatting, locked workspace/all-target clippy and
`dev_ui,bitprobe`, plus `doc_citations`:

- Command/input: command **6**, protocol **18**, aim **9**, harness **11**, native fire attestation
  **7**, death-flow **3**, headless **15**. The **3** ignored headless cases are explicitly measurement beds.
- Transport: **15**, real-UDP loss **2**, shooting **16**, weapon-width refusal **1**.
- Explanation routing/lifecycle/refusal: **3**. Diagnostic corruption refusals: **3**.
- Contract/lifecycle: sync margins **12**, delay **5**, protocol **19**, real-UDP match exits **3**,
  input guard **1**, native-buffer dependency tripwires **3**.
- Final registry/codec state: protocol **21**, codecs **9**, explanation **3**, real-UDP loss **2**.
  Font coverage **4** and citation checks **3** also pass.

All runs were unattended and headless under the machine queue. No full suite, capture, pricing,
deployment, Edgegap/Cloudflare call, external repository write, merge, push, or bay release occurred.
The full suite remains CI's job after the orchestrator integrates and pushes. Main advanced while
this fixed-base lane ran; integration belongs to the orchestrator. The existing batched feel/pricing
review of the empirical delay remains with the owner; no new design decision blocks this scope.

MEASURED closing verification clock: 2026-11-01 09:05:36 +0700. DERIVED elapsed: 73.2 minutes,
inside the dispatched limit. The closing documentation batch also passes formatting, CI clippy,
aim (**9**), codecs (**9**), protocol (**21**) and citations (**3**). The final handoff check repeats
formatting, CI clippy and citations against the same source snapshot.
