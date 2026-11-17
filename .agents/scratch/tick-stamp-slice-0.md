# Tick-stamp lane: slice 0 observation

Subject: Overmatch's netcode timing only. This slice measures; no input handling or presentation reads the new records.

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
