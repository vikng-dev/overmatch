# Announced fire owns its trajectory

Subject: Overmatch's fire presentation and damage model; game state only. Issue #388.

## Delivery facts

DERIVED from the shipped code and pinned dependency source:

| Fact | Carrier | Guarantee |
|---|---|---|
| Own `WeaponGate.belt_remaining` and timing | `component::<WeaponGate>().replicate()` in `net::protocol` | Reliable ordered insertion; subsequent mutations use unordered unreliable replication. Replicon tracks acknowledgements and sends newer state, so a snapshot can account for several consumptions. |
| Automatic `FireEvent` | `FireVisualBatch` on `FireChannel` | Unordered unreliable. `shot_transport::VisualQueue` offers bounded copies, not acknowledgement or eventual delivery. |
| Single-shot `FireEvent` | Individual message on `OutcomeChannel` | Unordered reliable for the live connection. Reliability supplies no finite arrival deadline. |
| Every `FlightOutcome` | Individual message on `OutcomeChannel` | Unordered reliable, including automatic weapons. Paths join to a presented round by `ShotId`, independently of arrival order. |

The transport's existing automatic policy gives DERIVED three send opportunities within sixteen
authority ticks (`VISUAL_COPIES`, `VISUAL_TTL_TICKS`). This bounds the server queue; it does not
bound when an emitted packet can reach the client. No cross-channel order relates a component
mutation to its fire announcement. A later announcement is ordinary delivery, not evidence of loss.

Pinned sources checked locally, without web search:

- Lightyear 0.31.0 `lightyear_replication/src/channels.rs`:
  `RepliconChannelRegistrationPlugin`, `RepliconUpdatesChannel`, `RepliconMutationsChannel`.
- Lightyear 0.31.0 `lightyear_transport/src/channel/receive.rs`:
  `RecvState::UnreliableUnordered`, `prepare_receive`, `read_message`.
- Replicon 0.45.5 `shared/backend/channels.rs::ServerChannel` and
  `shared/replication/client_ticks.rs::ClientTicks` (latest-state acknowledgement).
- Replicon 0.45.5 `shared/replication/registry/receive_fns.rs::default_write` (the unconditional
  mutable write used by the own-gate arrival bracket).

## Failure and decision

At baseline commit `7c0156e3`, `fire_presentation::fold_arrivals` records a consumption before the
render-frame message drain. Its reveal stamp is `ArrivalDelay::newest_remote`, a remote packet
high-water mark, not the local tick at which that gate was observed. `recover_unannounced_rounds`
then treats an estimator-sized wait as proof of a lost announcement. It constructs a round with
`shot: None` and records an owed swallow. `client::spawn_reconstructed_fire` consumes that entry
and returns before constructing the announced keyed round. `ballistics::path::present` needs
`Shot(ShotId)` to attach a buffered path, so the replacement cannot play the server path.

Historical source anchors at `7c0156e3`: `announce_wait_ticks` at
`src/net/fire_presentation.rs:579`, recovery at `:604`, `shot: None` at `:703`, owed-swallow insertion
at `:725`; the early return in `src/net/client.rs:2111-2120`; and the keyed path join in
`src/ballistics/path.rs:464-466`.

The belt delta proves a consumption count. It does not carry each fire's identity, bore pose,
physical birth, fired load or tracer phase. Coalesced gate/load snapshots cannot reconstruct those
per-fire facts from silence. The old queue also
counts **presentations**, so an announcement already received but held for the cursor can still
race recovery before its release.

Decision: only a validated announcement authors a cosmetic round. `ReleasedShot` requires a
`ShotId`; own and remote rounds use the same reconstruction path. Delete belt-derived round
synthesis, the reveal/load-history and swallow queues, the announcement wait, presentation-count
bookkeeping and their exclusive tests. Retain authored input, local reload-view cadence,
consumption confirmation, and the separate unconfirmed-reload view refresh. The latter cannot
suppress later state or fire facts.

The chosen outcomes are:

- A fresh valid announcement reconstructs once, keyed to its server path, regardless of belt or
  path arrival order. Redundant copies are deduplicated by `ShotId`.
- An announcement whose shooter is not yet assembled follows the existing bounded pending queue;
  malformed, expired and capacity-refused facts retain their named diagnostic outcomes.
- An over-horizon release is explicitly traced as `stale_release`.
- If all automatic announcement copies are lost, its cosmetic round is omitted. Durable gameplay
  state and reliable outcomes still arrive. This is ADR-0021's existing best-effort visual policy,
  now applied to owners and observers alike, consistent with ADR-0037's single authority timeline.
  A belt count never substitutes an invented trajectory.

Rejected alternatives:

- **Wait longer.** An unordered unreliable channel has no delivered-prefix watermark or finite
  maximum lateness. An observed quantile, even the largest observation, cannot establish one.
- **Adopt a guessed round by belt order.** Coalesced counts and reordered announcements do not
  supply a one-to-one identity mapping. A provisional flight also invents its pose and path before
  the authority supplied them. Keeping that machinery requires a different presentation contract.
- **Make automatic announcements reliable.** That changes ADR-0021's deliberate cosmetic-debt
  policy and the transport surface to solve a client-side discard. The received-announcement
  failure needs neither change. Existing bounded transport copies remain the loss repair.

The module's former narrative pointers were
`.agents/scratch/burst-state-fire-stack-map-2026-08-14.md` and
`.agents/scratch/adaptive-cursor-frontier-2026-08-15.md` (section 3 for recovery). Neither file exists
in this checkout, and no tracked version was found. Introducing commit `feb41576` preserves the
rationale: client-only recovery was chosen over announcement duplication to repair true loss.
This document owns the current decision and evidence; module documentation states only its API
and invariants.

## Regression evidence

The command

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe \
  net::client::tests::belt_arrival_cannot_discard_an_announced_round_or_its_path -- --nocapture
```

was run before the fix. MEASURED: it failed in the asserted path identity, with `None` instead of
the announced `ShotId`. It mounts the shipped own-fire plugin and ballistics simulation plugin,
feeds the production ingress/release seams, and observes the actual projectile and path playback.
Fixture clocks let the belt's recovery deadline pass before release. Both announcement/belt orders
and both path/announcement orders are covered, with a redundant announcement copy; the assertions
require one keyed round, one recoil presentation, and the sanctioned bounce to play once.

## Capture method

The passive `belt_rx` trace records the observing local tick `t`, remote reveal stamp `rt`, slot,
consumption count and remaining belt. It is enabled by the MG runner's existing `SPIKE_SHOT_TRACE`.
For each slot, pair its cumulative consumption count with the authority's ordered fire identities;
refuse the analysis if the total counts do not match. These captures start before firing and contain
no belt refill, so this pairing is unambiguous. Pair each identity with its first `fire_rx` receipt.

Report both `fire_rx.t - belt_rx.t` (the local-observation gap) and
`fire_rx.t - belt_rx.rt` (the subtraction used in the original issue). The latter includes the
client timeline's lead over the remote stamp; it is not a delivery-skew measurement. The remote
stamp is a packet high-water mark, not necessarily this component's own snapshot tick.

Each capture uses `scripts/shot/run-mg-armor.sh`, its unchanged latency/jitter conditioner, hidden
window mode and unattended flag, with the machine build/run lock held. No intentional packet loss
is configured. Recovery counts come from the own-client log; swallow counts come from the keyed
`drop res=owed_swallow` trace rows. A recovery matched by a subsequent swallow is false; a never
received announcement can only be called absent through capture end, not proven lost forever.
The runner's quiet tail and strict delivery/path checks remain enabled. A torn final server gauge
row at requested shutdown is disclosed and excluded, as in the existing analyzer; complete rows
are retained.

## Baseline captures

MEASURED, five complete captures before the behavior change, each with 128 authority fires and
128 first announcement receipts. The baseline differs from `7c0156e3` only by passive belt-arrival
instrumentation in production; the regression fixture added during the set is test-only. No capture
reproduced recovery: zero false recoveries, zero announcements absent through the quiet tail,
and zero swallows. This is a negative live sample,
not evidence that the old branch was safe; the deterministic regression reproduced its exact
unkeyed-round failure.

All counts in the following tables are MEASURED. `gap:count` is a tick histogram. Local gaps use
application observations at the gate fold and message drain, not datagram ingress timestamps.

| Capture | Local arrival-minus-belt histogram | Arrival-minus-remote-reveal histogram | Recovery / false / absent / swallow | Paths completed / received | Strict |
|---|---|---|---|---|---|
| before-1 | 0:88, 1:40 | 12:16, 13:56, 14:22, 15:14, 16:8, 17:8, 18:4 | 0 / 0 / 0 / 0 | 184 / 184 | PASS |
| before-2 | -1:8, 0:88, 1:32 | 13:4, 14:26, 15:84, 16:14 | 0 / 0 / 0 / 0 | 187 / 187 | PASS |
| before-3 | -1:4, 0:94, 1:30 | 7:20, 8:64, 9:34, 10:10 | 0 / 0 / 0 / 0 | 184 / 184 | PASS |
| before-4 | -1:2, 0:78, 1:48 | 15:12, 16:72, 17:38, 18:6 | 0 / 0 / 0 / 0 | 184 / 184 | PASS |
| before-5 | -1:2, 0:78, 1:48 | 13:20, 14:56, 15:16, 16:16, 17:8, 18:8, 19:4 | 0 / 0 / 0 / 0 | 187 / 187 | PASS |

MEASURED aggregate, 640 rounds:

- Local gap: `-1:16, 0:426, +1:198`; median zero ticks.
- Remote-stamp subtraction: `7:20, 8:64, 9:34, 10:10, 12:16, 13:80, 14:104, 15:126,
  16:110, 17:54, 18:18, 19:4`; median fourteen ticks, nearest-rank p95 seventeen ticks.
- Every announcement had three accepted visual send opportunities; no visual expiry, send-call
  error or recipient omission was recorded. All delivered paths completed on both clients.
  Each idle target also received and spawned every public fire once. Target public counters were read without the analyzer's shooter-only HP gate;
  the MG runner's strict gate applies to the shooter.

Raw capture directories: `/tmp/overmatch-fire-recovery/before-1` through `before-5`.
The joined tick pairs and compact per-run results are preserved in
`../../scratch/fire-recovery-388-captures.json`.

## Post-fix captures

MEASURED, five complete captures, run sequentially under one `scripts/build-lock` acquisition after
building both binaries through `scripts/serial-cargo build --locked --features dev_ui,bitprobe`.
Each run invoked the same MG runner with its internal `--holding-lock` entry. Every run delivered
and spawned all 128 rounds once on **each** client. No recovery, false recovery, announcement
absence, swallow, missing keyed spawn or duplicate spawn was observed. All received paths completed.

All counts below are MEASURED. Paths show completed / received on each client; both clients had
the same counts. Every row passed the runner's strict shooter verification.

| Capture | Local arrival-minus-belt histogram | Arrival-minus-remote-reveal histogram | Recovery / false / absent / swallow | Paths, each client | Strict |
|---|---|---|---|---|---|
| after-1 | -1:2, 0:82, 1:44 | 7:18, 8:84, 9:26 | 0 / 0 / 0 / 0 | 189 / 189 | PASS |
| after-2 | -1:8, 0:74, 1:46 | 14:4, 15:52, 16:64, 17:8 | 0 / 0 / 0 / 0 | 187 / 187 | PASS |
| after-3 | -1:2, 0:68, 1:56, 2:2 | 6:2, 7:80, 8:46 | 0 / 0 / 0 / 0 | 184 / 184 | PASS |
| after-4 | -1:6, 0:84, 1:38 | 6:34, 7:92, 8:2 | 0 / 0 / 0 / 0 | 184 / 184 | PASS |
| after-5 | -1:2, 0:86, 1:38, 2:2 | 6:16, 7:100, 8:12 | 0 / 0 / 0 / 0 | 189 / 189 | PASS |

MEASURED aggregate, 640 rounds per client:

- Local gap: `-1:20, 0:394, +1:222, +2:4`; median zero ticks.
- Remote-stamp subtraction: `6:52, 7:290, 8:144, 9:26, 14:4, 15:52, 16:64, 17:8`;
  median seven ticks, nearest-rank p95 sixteen ticks. Timeline lead varies between runs; these
  stamp differences are not a latency benchmark.
- Both clients completed all 933 received paths. Every fire had three accepted visual send
  opportunities, with zero visual expiry, send-call error or public-recipient omission.

Raw directories: `/tmp/overmatch-fire-recovery/after-1` through `after-5`. The joined artifact above
includes every before/after pair, compact public counters for both clients and raw-file SHA-256s.

## Final validation and boundary

PASS on the final source, all compiles through `scripts/serial-cargo`, with `--features dev_ui,bitprobe`:

| Targeted test filter | Passed | Ignored |
|---|---:|---:|
| `net::fire_presentation` | 14 | 0 |
| `net::client` | 38 | 1 |
| `net::sync_margin` | 17 | 0 |
| `net::shot_loss` | 2 | 0 |
| `tests::doc_citations` | 3 | 0 |

The existing ignored client test needs a live hosted API and may start a billed server; it was
not enabled. `cargo fmt --all --check` and CI clippy
(`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`) passed.
The deterministic witness was red before the fix and green after it, including the played bounce.
The live baseline did not reproduce the intermittent failure; the live repeat sets are observations,
while the regression and mandatory announced identity guard the ordering invariant.

No wire type, channel mode, `WIRE_TYPES_HASH` or `PROTOCOL_REV` changed. All type changes are
client-local presentation/diagnostic state; `ReleasedShot` now requires its `ShotId`. Backlog row `net-04` is
closed by deleting the recovery path that substituted a default round for an invalid disclosed index.
No transport tuning, fallback mode or alternative implementation remains from this investigation.

The deliberate limitation is total automatic-announcement loss: it omits a cosmetic round under
the existing delivery contract. A guaranteed presentation of every automatic start would require
a separate delivery-policy decision. Landing review and CI remain the orchestrator's responsibility;
this lane commits without pushing or merging.
