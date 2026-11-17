# Tick-stamp lane: slice 0 observation

Subject: Overmatch's netcode timing only. References to the game's damage-model tests concern game state only. This slice measures; no input handling or presentation reads the new records.

The sections through the first Validation record round 1 (`6d3b567e`). The round-2 section below, with the closing corrections reflected in its implementation claims, supersedes their implementation and completion-check descriptions; proof numbers remain historical observations.

## Implementation and wire

`net::tick_stamp` publishes frame-start `Time<Real>` in `FixedFirst`. The dedicated runner still simulates exactly one tick per frame. The globally visible scoreboard carries the component; no new entity, channel, message, per-tank field or duplicate tick id is needed. Lightyear 0.31.0's `LightyearRepliconServerBackend` enables mutation tracking for interpolation, so an idle connection already receives a checkpoint every tick. Its `write_authoritative_tick_userdata` carries the simulation tick; Replicon's checkpoint index is a different domain.

MEASURED postcard payload: **8 bytes** at every tested epoch (the codec test asserts the length). DERIVED sparse scoreboard mutation cost: payload + one-byte component-function id + one-byte row length + compact entity encoding. For a first-generation entity, that is **11–15 bytes/client/tick**, before the already-present shared checkpoint/transport envelope and any packet-boundary effects. If the scoreboard already has a mutation row, the stamp adds **9 bytes**, plus any length-prefix growth. A standalone message adds its own message type and transport framing; the component shares the per-tick replication path and its tick label.

**Wire change:** `PROTOCOL_REV` **98 → 99**; `WIRE_SURFACE_HASH = 0xfba98390117f50c7`; `WIRE_TYPES_HASH = 0x909cff9054b52400`. The manifest pins the stamp definition and its complete codec. The protocol tests first produced the new hashes, then passed after the re-pin.

`net::view_clock` receives after replication. The decoder preserves every stamp drained in a frame, including overwritten intermediate component values. The record samples each tick once while retained, computes `x` from the preceding frame's real time, and takes the nearest-rank median of neighbouring received stamps in authoritative tick order. Intervals over missing stamps remain observations. Windows expire by client real time. The existing `client_jitter` line carries the new fields beside `min_delay_ms.applied`; nothing writes a timeline.

For atomic application, every Replicon `EntityReplicated` receipt participates, including plain components and interpolated entities. A checkpoint retains each entity's first frame and the stamp's first frame across summary lines. A late tick is counted once; later entities can increase its worst lag. Unmatched or evicted stamp mappings are visible counters. Atomic bookkeeping uses the view record's retention window, so this is an observation within retained history, not proof about arbitrarily late packets.

`net::input_gap` owns one independent record per connected server link and evaluates those records in parallel. It reads queue readiness after message receipt and before input validation, without draining or rewriting anything. The existing `server_stats.inputs[].arrival` carries its summary. Collection is unconditional; the existing server-stats emission lever/cadence remains the owner of that line, with no new switch. Raw gaps remain in the histogram; applying the clamp after `max gap + T` gives the same S as capping each gap at the ceiling first.

The required Taste data has one owner, `assets/net_timing.ron`: view and uplink windows each start at the owner-chosen **10 s**, and the stale floor/ceiling at **100/250 ms**. Parsing refuses missing, unknown, nonpositive, nonfinite or reversed values before play.

## Local proof

The existing GPU-less `Idle` harness completed **MEASURED 1,024 script ticks** with a successful client exit. Both processes used `OVERMATCH_UNATTENDED=1` and the existing runner's `OVERMATCH_WINDOW=headless` mode. No frame-cost trace was enabled or analysed.

The client used `SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 SPIKE_JITTER_SEED=1`; the server used `OVERMATCH_SERVER_STATS_S=1`. DERIVED from the conditioner: the added round-trip path delay is **80 ms**, placed entirely on client receives, with integer jitter in **[-10, 10) ms** there. This lever does **not** simulate two independent symmetric one-way delays. MEASURED final ping RTT readout: **88.8 ms**, including scheduling and the existing ping smoothing.

The final complete client summary, after the windows filled:

| Classification | Reading | Value |
|---|---|---|
| DERIVED from received records | Raw signed Δ* | -2853.441125 ms |
| MEASURED existing config readout | Today's applied minimum interpolation delay | 81.1 ms |
| DERIVED on the same stamp map | Today's cursor lag | -2837.067147 ms |
| DERIVED, epoch offset cancels | Δ* minus today's cursor lag | -16.373978 ms |
| DERIVED, epoch offset cancels | Δ* above the newest tick's x | 36.008958 ms |
| MEASURED received-stamp intervals | count; min / p50 / p95 / max | 639; 12.509917 / 15.606250 / 18.174417 / 19.112625 ms |
| MEASURED histogram counts | [0,T), [T,2T), higher bins | 327, 312, all remaining zero |
| DERIVED U4 threshold | Final would-be S | 100.0 ms |
| MEASURED U3 gaps | Final count; min / p50 / p95 / max | 641; all zero ms |

The negative signed lags contain the unknown difference between process time origins. They are **not negative physical network delays**. Comparing raw Δ* directly to the positive `min_delay` would compare different quantities. Their stamp-map difference says the would-be delay in this summary is **DERIVED 16.374 ms smaller** than today's cursor lag (so its unconstrained target is newer stream state), without estimating the server clock or calibrating an offset.

Startup is included, not discarded: the largest neighbouring-received-stamp gap in the logged windows was **MEASURED 204.217167 ms**, which can include missing ticks. The target-minus-today comparison peaked at **DERIVED 194.368534 ms** before settling; these logs alone do not identify which startup processing or delivery stage caused that sample.

Across **MEASURED 15 client summaries**, the atomic counters total **MEASURED 1001 stamped ticks** and **MEASURED 1953 first entity receipts**. Late ticks: **MEASURED 0**; worst lag: **MEASURED 0 frames / 0 ms**. Unmatched ticks, expired-without-stamp ticks and unmapped stamps were all **MEASURED zero** in these summaries. This idle workload observed same-frame application relative to the stamp for every entity receipt. It does not establish whole-tick atomicity for larger sends: Replicon 0.45.5 splits at entity boundaries and applies split messages independently. If `late_ticks` rises on a hosted workload, D2's stamp-only bound under-reads by the reported application lag.

Across **MEASURED 19 server summaries**, the would-be stale count was **MEASURED 1**, in the first connected summary. Its first-arrival U3 gap was **MEASURED 502.243206 ms** while today's client was establishing its input stream. That made S **DERIVED 250.0 ms** until the startup gap expired; the final threshold was **DERIVED 100.0 ms**. Every later connected summary reported **MEASURED zero** new stale declarations. A healthy one-message-per-receive stream legitimately has U3 gap zero: the law counts only empty receives.

Raw proof artifacts: `/tmp/overmatch-tick-stamp-proof/client.log`, `server.log`, `rows.json`, `idle.ron`; orchestration: `/tmp/tick-stamp-proof.py`. This is a correctness observation on one client and an idle world, not a hosted or performance result.

## Notes checked against source; limits for the next slices

The timing map's relevant receive ordering and one-tick runner claims matched source. Its unverified packet-header question is resolved: transport `buffer_send` reads the same `LocalTimeline` tick after the fixed loop; replication separately carries that tick in checkpoint userdata. The per-tick idle checkpoint mechanism was absent from the notes' carrier discussion.

Two design claims need narrower wording:

- “A server stall never enters x” holds for a pause **before the next frame stamp**, with the same subsequent delivery wait. A stall **inside** a frame occurs after its stamp and delays that frame's send; D2 can count that delay. The frame-start stamp cannot distinguish server work from delivery. No frame-cost timing was added to infer a cause.
- U3's cap alone does not prove a server stall can **never** coincide with a stale crossing: an already-near-threshold silent link can cross after the allowed one-tick charge on an empty receive. A queued arrival resets silence, and a fresh live record cannot acquire a whole stall's duration. The tests pin those actual U3 properties.

For slice 1, retain the per-link record/reset and pre-validation receive seam, but change its observed message type together with the full arrival-based input law. Recovery must compare the arrival gap against S **before** inserting that gap. Today's native-input handshake produced the startup stale declaration; it is not evidence of an uplink outage. Remove the old input-buffer and lookahead census when the native input path goes, keeping this arrival record.

For slice 2, preserve signed epochs and the retained tick/stamp map; do not turn raw Δ* into a nonnegative Duration. Drive through public `NetworkTimeline::set_now`, disable the old controllers, retain cursor brackets through holds, and resolve checkpoint completeness before trusting a stamp-only frontier for a larger workload. Lightyear's checkpoint map retains only a bounded recent set; the `unmapped_stamps` counter exposes a large drain that outlives it. The old controller still runs in this slice, so the would-be comparisons are observations, not a promise about the eventual controller's feel.

The pre-existing `net-22` remote-angle timeline finding in the lens backlog remains unchanged: altering that presentation would violate this slice's measurement-only scope. Its disposition belongs with the later presentation slice.

## Validation

- `cargo fmt --all --check`: passed.
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`: passed.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe -- net:: tests::doc_citations`: **MEASURED 454 passed, 7 existing ignored, zero failed**. This includes protocol re-pin tripwires, real-loopback tests and the stamp/clock/gap invariants.
- `scripts/serial-cargo build --locked --features dev_ui,bitprobe --bin overmatch --bin overmatch-server`: passed. The linker reports the existing large `__eh_frame` compact-unwind warning; clippy is clean.
- Local proof: passed as described above. No push or merge was performed.


## Round 2: current implementation and finding dispositions

Both independent review files were recovered in full from the pre-restart session transcript;
the original temporary files had not survived. Their findings match the checked source. No
review finding was rejected.

**WIRE CHANGE: `PROTOCOL_REV` 99 → 100.** `TickStamp` is now the checked pair
`{ frame_start_s: f64, departure_offset_s: f32 }`, serialized in that order. Both fields and
their sum must be finite and nonnegative; the decoder refuses invalid peer values before timing
arithmetic. MEASURED postcard payload: **12 fixed bytes**, **4 more** than round 1. The ordered
registration surface is unchanged, so `WIRE_SURFACE_HASH` remains `0xfba98390117f50c7`;
the round-2 definitions/codec pin was `WIRE_TYPES_HASH = 0xbe9e48fdef318ef5`. The closing
serialization/refusal correction changes that full-source codec manifest to
`WIRE_TYPES_HASH = 0xdc746cf1dbc42659` and `PROTOCOL_REV` **100 → 101**, retaining the same
twelve-byte field layout. Invalid decoding preserves its typed cause in replication storage;
after receive, it enters the existing `ServerGone` match-close path, which removes the link,
stops reconnecting and shows the player the cause.

DERIVED sparse scoreboard mutation cost under the same compact-entity assumptions as round 1:
**15–19 bytes/client/tick** before shared framing and packet-boundary effects; when the scoreboard
already has a row, **13 added bytes**, plus any row-length-prefix growth. The added four payload
bytes cost **256 bytes/client/second at 64 Hz**, before packet effects. The `f32` offset avoids a
fixed integer-duration ceiling while limiting rounding to half an offset ULP. A fixed `u16` or
`u32` microsecond count would impose a maximum frame duration. Departure is read inside the stamp serialization callback, not at a socket timestamp. Remaining
collection/serialization (including reuse of cached stamp bytes for other clients), message
framing/queueing, transport/delivery and whole-tick completion wait to the previous client frame
remain in x, alongside the unknown epoch offset and offset rounding. No send is delayed or newly
ordered against other frame work.

| Item | Disposition |
|---|---|
| P1: frame start versus departure | Done. `publish` records frame start and exposes the runner's clock seam before `ServerSystems::Send`; the codec reads departure during serialization. The map retains frame start; x subtracts frame start plus departure offset. `Time<Real>` supplies the frame origin, never departure. |
| P2.4: completion | Done. Delete all per-entity atomicity state and consume every `MutateTickReceived` after replication receive. Match it to the first retained stamp arrival; sample x with the preceding completion-frame Real sample. Summaries count later-frame completions and their maximum frame/ms lag. |
| P2.2: provenance | Done. Each of the four numbers has its own `(value, Taste("…"))` label, loaded through `Sourced` and `from_shipped_str`; missing/blank/Fixture provenance is refused by the shipped reader. |
| P2.3: first arrival | Done. `AwaitingFirstArrival` accrues neither silence nor gaps. First arrival returns `First`; active arrivals return `Live` or `Stale` against S before inserting their gap. The per-link census counts `stale_arrivals` per emission period. This remains measurement only. |
| P3.5 and testing lens | Done. The runner mounts the shipped protocol with its physics dependencies, advances the injected clock in `FixedUpdate` and after publication in pre-send `PostUpdate`, and asserts serialized departure and x against independent fixture work. Reading departure in publication fails too; fixture output is not labelled measured. The completion-frame test distinguishes short/long client frames and a later-frame completion across a summary boundary. Input assertions derive declarations and recovery S from the authored floor/ceiling; a separate fixture witnesses classify-before-admit. |
| P3.6: Ledger history | Done. The dependency is owned by the stamp codec module, with a pointer at protocol registration: Ledger's `replicate_diff` enables Replicon's global history decoding. It is retained deliberately. |
| P3.7: compositions | Done. The decoder itself caps `ReceivedStamps`, including protocol-only apps without a view consumer. The shared protocol installs publication for every server composition, gated on Replicon's running-server state; clients only forward decoder refusals. The runner proof exercises that installation. |
| P3.8: per-frame scans | Done. Raw windows expire by front-pop; a monotonic maximum deque replaces full-window max scans. Each deque entry is pushed/popped at most once. Sorting is confined to summary emission. |
| P3.9: one number, one job | Done. `view_window_s` owns the observation window only. Pending completion receipts have an independent Resolution cap derived from the upstream tracker horizon. |
| P3.10: disconnected counts | Done. Last-period input counts survive disconnect and emit with `arrival.state = not_connected_at_emission`. The disconnected link's arrival history/declaration counter is not retained; that absence is explicit rather than a fabricated live row. |
| P3.11: smaller items | Done. Revision/registration comments name the stamp. `Timing` initializes only at the shared composition root. Stamp construction/decoding is checked. Floor-sensitive tests use authored parameters. |
| Parallel-work lens | Done. On emission, independent per-link histogram sorting and declaration resets use Bevy's parallel query and thread-local result buffers. Only collecting keyed rows and emitting the shared line is serial. |
| Error-handling lens | Done. Name lazy creation of typed input receivers and missing cursor brackets during startup/starvation beside their absence paths. |
| Brevity lens | Done. Adjacent stamp intervals use the requested zipped adjacent-pair iterator, preserving the comment about authoritative order and missing ticks. |

Completion correlation is expected O(1) per tick: one keyed pending record and fixed-size counter
updates, with no entity traversal. Ordered sample-map insertion/removal remains O(log n).
The decoder queue and pending receipt order each retain at most **DERIVED 64 entries**, matching
`u64::BITS` in Replicon's tracker; overflow, uncompleted eviction, unmapped stamps and completions
without retained stamps are separate visible counters. Pending receipts survive summary resets.

The pre-existing `net-22` remote-servo presentation finding is unchanged. Altering that behaviour
would violate this lane's timing-observation scope; it remains with the later presentation slice.

## Verified upstream contracts and qualifications

Sources are the locally pinned registry crates: `bevy_time-0.20.1`, `bevy_replicon-0.45.5` and
`lightyear_replication-0.31.0`.

- Bevy `src/real.rs:101–113` updates Real only at the frame clock sample; `last_update` exposes
  that Instant. The dedicated runner sets `ManualInstant` before `app.update`.
- Replicon `src/server.rs:185–201` chains preparation, collection and message send inside
  `ServerSystems::Send`. Lightyear `src/server.rs:52` writes authoritative tick userdata before
  that same set. Game publication orders before that boundary; departure is read by the codec inside
  `collect_changes`. `src/server/replication_messages/serialized_data.rs:34–51,96–111` caches the
  serialized component bytes for reuse across clients; `server.rs:1106–1165` subsequently frames
  and queues messages. The simulation tick rides each replication message's userdata; the stamp
  rides the scoreboard component.
- Replicon `src/client.rs:314–315` holds mutations until their required reliable update has
  applied. `apply_mutate_message` processes mutations and then confirms the message
  (`:498–549`); entity changes flush before that return (`:995–1001`).
- Replicon emits `MutateTickReceived` only when the applied-message count equals that tick's
  advertised total (`src/client.rs:859–868`; `src/client/server_mutate_ticks.rs:179–204`).
  This means all mutate messages for that checkpoint were processed successfully, subject to
  Replicon's normal skips of obsolete/unknown entities. It does not mean the view has rendered
  the tick or that every older tick completed. Lightyear's server backend enables tracking through
  its `interpolation` feature (`lightyear_replication/src/lib.rs:263–271`). The clean-link test
  `shipped_replication_completes_stamped_ticks_for_view_clock_samples` guards that build dependency
  with the shipped client/server plugins and protocol. Incomplete ticks under network loss are
  measured by `completed_ticks` and `evicted_incomplete`, without a runtime assertion.
- If a mutate message is lost, that tick emits no completion event. Later ticks can complete
  independently. Recovery within the tracker horizon may finish the old tick; after it ages out,
  `confirm` returns false (`server_mutate_ticks.rs:112–143`). The separate `contains` API treats
  aged-out ticks as applied (`:56–70`), which is deliberately not used as completion evidence.
- Lightyear mirrors the latest completed checkpoint, not a gap-free prefix and not all events
  in a receive batch (`src/client.rs:80–105`; `src/checkpoint.rs:202–215`). Reading every Replicon
  event preserves intermediate and out-of-order completions.
- Ledger's diff rule sets `needs_history` (`bevy_replicon/src/shared/replication/registry/rule_fns.rs:192`),
  ORed globally in `registry.rs:118`. Without it, `client.rs:942` skips stale entity decoding.
  With it, `component_fns.rs:153–160` routes stale stamps to consume, whose default invokes the
  custom decoder (`rule_fns.rs:255–264`).

The design correction is sound for **x**. Its absolute “delay target unchanged” wording is still
broader than the implemented law: frame-start intervals feed the median, and the target adds that
median (`src/net/view_clock.rs:144–159`). Changed production pace can therefore change the target
through the median without adding server work to x. The long-frame proof asserts the latter.
The U3 correction also matches the code: capped charges never over-count silence, but can delay a
stale declaration and classify an outage overlapping a server stall leniently.

## Round-2 local proof beside round 1

The same existing GPU-less Idle script completed **MEASURED 1,024 ticks**, exit status zero.
Both processes used `OVERMATCH_UNATTENDED=1` and the runner's `OVERMATCH_WINDOW=headless` mode.
The client conditioner was unchanged: `SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 SPIKE_JITTER_SEED=1`.
DERIVED conditioner delay is 80 ms added to client receives, with per-packet integer jitter in
[-10, 10) ms; this is not a symmetric two-way-delay experiment. Server summaries used the same
one-second emission lever. No cost tracing or new lever was added.

Final complete summaries, after the observation windows filled:

| Classification / reading | Round 1 | Round 2 |
|---|---:|---:|
| MEASURED ping RTT readout, ms | 88.8 | 90.3 |
| MEASURED existing applied minimum delay, ms | 81.1 | 82.7 |
| DERIVED raw signed target, ms | -2853.441125 | -1437.574708 |
| DERIVED cursor lag on stamp map, ms | -2837.067147 | -1424.851957 |
| DERIVED target minus cursor lag, offset cancels, ms | -16.373978 | -12.722751 |
| DERIVED target above latest x, offset cancels, ms | 36.008958 | 22.562958 |
| MEASURED interval count | 639 | 639 |
| MEASURED interval min / p50 / p95 / max, ms | 12.509917 / 15.606250 / 18.174417 / 19.112625 | 12.595125 / 15.577083 / 18.277958 / 19.083208 |
| MEASURED first two interval bins; all higher bins zero | 327 / 312 | 347 / 292 |
| DERIVED final S, ms | 100.0 | 100.0 |
| MEASURED final input-gap count | 641 | 638 |
| MEASURED gap min / p50 / p95 / max, ms | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 15.023750 |
| MEASURED stale declarations across logged periods | 1 | 0 |
| MEASURED client / server summary counts | 15 / 19 | 15 / 18 |

Raw signed targets contain different process epoch offsets and cannot be compared as physical
delays. The offset-cancelling differences are observations; neither run pins a desired delay.

Round 2 observed **MEASURED 1,002 completed stamped ticks**, with **zero** later-frame completions
and **zero frames / zero ms** worst completion lag. Completions without stamps, pending evictions,
receipt overflow and unmapped stamps were all **MEASURED zero**. Round 1's zero late ticks was a
per-entity receipt observation over 1,001 stamps/1,953 first entity receipts; it was not a
whole-tick completion measurement and is not equivalent evidence. The idle workload does not
establish behaviour of large split sends. The scheduled completion/loss tests cover those states.

Startup remains included. Peak received-stamp interval was **MEASURED 204.394417 ms**, beside
round 1's **204.217167 ms**; peak target-minus-today was **DERIVED 61.310554 ms**, beside
**194.368534 ms**. The first connected server row was explicitly `awaiting_first_arrival`, with
no gap, no silence, no declaration and S at the floor; no startup gap was admitted.

No server-frame-stall lever exists in the scripted harness (the burst lever delays receives).
The allowed unit-level alternative uses the existing injected runner clock. MEASURED from that
run: **8 ticks**, ordinary work **0.9765625 ms**, one frame with **1,000 ms** injected work, and
**x = 62.5 ms** in every frame. DERIVED ordinary work is T/16; the fixed absence fixture is
independent of the published stamp. Replacing departure with frame start would instead produce
**DERIVED x = 1,062.5 ms** on the long frame and fail. The completion-frame test separately proves
that extending the client completion frame leaves its previous-frame bound unchanged.

Raw artifacts: `/tmp/overmatch-tick-stamp-r2-proof/{client.log,server.log,rows.json,idle.ron}`;
restored orchestration: `/tmp/tick-stamp-proof.py`; long-frame output:
`/tmp/tick-stamp-r2-long-frame.log`.

## Round-2 validation and whole-suite failures

- `cargo fmt --all --check`: passed.
- CI clippy, locked workspace/all-targets with `dev_ui,bitprobe` and `-D warnings`: passed after
  removing a redundant clone in the runner test; no lint allowance added.
- Final `net::` plus `tests::doc_citations`: **MEASURED 457 passed, 0 failed, 7 ignored**.
- Explicit long-frame runner test with `--nocapture`: **MEASURED 1 passed**.
- Both proof binaries built under `dev` with `dev_ui,bitprobe`; the linker emitted the existing
  large `__eh_frame` compact-unwind warning. Conditioned proof: exit status zero.
- Whole library suite, once, detached: **MEASURED 2,842 passed, 23 failed, 50 ignored**, elapsed
  **304.93 seconds**. No full-suite rerun. The only subsequent Rust change moves a test clock
  instead of redundantly cloning it; final targeted tests and clippy cover that change.

The brief's failure count was off by one: there are **MEASURED 22 bake failures plus one
rolling-fade failure**, not 23 bake plus one. The exact names match the existing
`.agents/scratch/red-fix-a2cef3d1.md` report: set difference in both directions is empty.
Every failure is the same fixture mass validation refusal documented there. `src/bake.rs` and
`src/track/sim.rs` are unchanged from `6d3b567e` and its base `a2cef3d1`; none is from this lane.

The bake failures, all under `bake::tests`:

```text
a_collider_whose_substance_states_no_friction_is_refused_at_load
a_component_of_several_substances_is_refused
a_coordinate_outside_the_certified_range_is_refused
a_node_that_cannot_play_its_declared_role_is_refused
a_primitive_the_sim_cannot_read_is_refused
a_scaled_node_the_sim_composes_through_is_refused
a_shell_that_passes_through_itself_is_refused
a_station_can_carry_its_declared_solids_on_children
a_volume_wearing_an_unworn_substance_is_refused
a_zero_area_face_is_refused
an_unreadable_sheet_or_an_unresolved_reference_is_refused
armour_is_what_the_sheet_declares_not_what_a_material_implies
assembly_root_does_not_follow_optics
certification_separates_assembly_from_the_gunner_chain
crew_and_witness_substance_come_together_or_not_at_all
every_shipped_asset_passes_the_consumer_contract
open_inverted_and_doubled_shells_are_refused
shipped_file_readers_refuse_fixture_provenance
the_asset_door_refuses_a_file_below_the_crew_floor
the_canon_file_carries_the_reference_list_and_the_registry_keys
the_registry_the_door_supplies_is_the_one_the_contract_reads
the_report_is_deterministic_and_carries_every_field
```

The remaining failure is `track::sim::tests::a_pressure_heavy_construction_stalls_in_the_rolling_fade`.

Full output: `/tmp/tick-stamp-r2-full.log`; final targeted/clippy output:
`/tmp/tick-stamp-r2-final-targeted.log`, `/tmp/tick-stamp-r2-final-clippy.log`.
Read-only self-review of the diff under the six lenses found no further timing finding;
the orchestrator's independent reviews remain its merge gate. No push or merge was performed.

## What the next slices need

Slice 1 replaces native tick-addressed inputs with the decided arrival/edge protocol. It keeps
connection reset, awaiting-first-arrival and the pre-validation receive seam; uses the returned
arrival classification before admitting that gap; answers click fate on the server; and removes
the native input-buffer/lookahead census with that path. It must exercise a continuously sending
client across a server stall and preserve the declared leniency for an overlapping outage.

Slice 2 consumes completed stamped ticks, preserving the two distinct server times and signed
clock offset. It drives the shared cursor through public `NetworkTimeline::set_now`, removes the
old controllers, retains cursor brackets across holds, and names startup/no-cursor state. A tick
without completion evidence is not a usable frontier. The bounded receipt/checkpoint horizons
must be accounted for explicitly in a longer outage. Pacing/cut tastes and their provenance still
belong to that slice; today's target comparisons remain observations.
