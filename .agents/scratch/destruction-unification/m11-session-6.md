Subject: **the video game's environment destruction and damage model; game state and presentation only.**

## Joined production witness

`admitted_map_kinds_detach_with_common_centroid_local_colliders` now joins the common
production detachment plugin to `RubbleQuery::near`, `RubbleBodies::gather` and the belt's
footprint reader. Every admitted map kind releases an isolated above-ground fixture chunk.
The witness checks its source address, dynamic owner, occupied centroid frame, zero child
translation, moving bounds, direct convex shape and positive footprint area. It reads the
actual released collider and Avian force state; it constructs no substitute support body.

MEASURED: both admitted kinds pass. Restoring a translated single-child compound in
`loose_collider` makes this witness panic inside the production gather's `Surface::triangles`,
with the saved crash's unsupported-shape diagnosis. The compound is removed afterward.

The analytical common-law witnesses also pass: linear/angular momentum conservation,
launch without created energy, simultaneous exchanges sharing one kinetic budget, and split
children inheriting their parent's rigid momentum. No recorded driving outcome becomes a test.

## Original drive and observing clients

Ran the original, unchanged
`.agents/scratch/diag-tree-crash/evidence/reproduce.py` with its unchanged `nearest.ron`.
The binaries were built through `scripts/serial-cargo`, with `dev_ui,bitprobe`. Every game
process was unattended and muted; the server was headless and every client logged `window: hidden`.
The complete presentation stack ran, without captures or cost sampling.

Temporary read-only probes observed production contact transfers, the authority record and
ledger, admitted carrier ownership, presented source membership and visibility. They changed
no simulation state. Their source and registrations are removed. The retained regression is
the joined witness above.

MEASURED results; these are observations, not acceptance pins:

| Run | Contact target | Closing speed | Ordered causes | Current carriers / owned chunks | Driver completion | Server alive at completion |
|---|---|---:|---:|---:|---:|---|
| Original runner | `fir_a_0020`, ordinal 19 | 8.28646 m/s | 6 cracks | 1 / 37 | 2,200 ticks | Yes |
| Original runner with observer/rejoin | Same fixture target | 8.28821 m/s | 6 cracks | 1 / 37 | 2,200 ticks | Yes |

Both authority traces contain positive applied throttle, both contacts came from the production
solver, and both clients completed successfully. The published ledger equals the ordered
record; the driver's admitted, visible carrier ownership equals the authority's. Each server
remained alive until the runner stopped it. Both server/client pairs exited with status zero.

The second run kept an idle observing client through the drive. It matched every ordered cause,
the current world generation, the active carrier and all its chunk addresses, with the carrier
visible and its chunks present in the source's shown departed membership. The observer was then
disconnected and its process stopped; a fresh client rejoined the same Battle and satisfied those
same checks. MEASURED: each observed **6 causes, 1 carrier, 37 chunks, generation 1**. Both observer
processes exited cleanly. This restart proof complements the same-link reconnect test below.

The production real-UDP world-cycle fixture passed after all mutants were restored. It covers
an observing late join, disconnect/reconnect of the same client link within the same Battle,
retirement of the old link's carriers and map roots, and startup-equivalent reset on authority,
connected clients and the next-Battle joiner. MEASURED: **3 map identities and 57 colliders**
rebuilt on authority and **3 clients**. This transport fixture injects its release state; the
joined witness and real drive supply the production physical-release evidence.

The live server logs also contain **2 startup input mismatch diagnostics in the first run and
1 in the second, MEASURED**. The installed `lightyear_inputs 0.30.1` emits these when a received
input targets an already simulated tick (`receive_input_message`, upstream `src/server.rs`,
lines 538–541 and 580–583). They were not diagnosed or changed here. Neither run logged a
construction-stream refusal or a panic.

Local evidence: `target/diag-tree-crash/m11-s6-original.*`, `m11-s6-observed.*`,
`m11-s6-observer.client.log`, `m11-s6-rejoined.client.log`, and
`target/m11-s6-live-evidence.json`. The observer runner and probes remain only as local evidence
under ignored `target/`; no runtime lever or alternate implementation is retained.

## Mutants

Each mutation was applied alone, its source diff saved, its named test built and run, and the
source restored. MEASURED: all **6** failed at behavioral assertions or the intended panic;
none failed to compile.

| Mutant | Witness | Failure signal |
|---|---|---|
| Translated single-child compound from `loose_collider` | Joined admitted-kind detachment/belt witness | Production gather rejects the non-convex support shape |
| Omit ledger publication | Real-UDP world cycle | Connected/observing clients cannot observe the common state |
| Omit the production carrier observer | Real-UDP world cycle | Required replicated carriers never arrive |
| Include carrier entity identity in the duplicate-ownership key | Duplicate-owner admission witness | The required visible refusal is absent |
| Keep the authority's old generation across `start_next` | Real-UDP world cycle | Generation 1 remains when Battle 2 requires generation 2 |
| Make handover due at arrival | Split/cursor witness | The parent is hidden before its presentation tick |

The restored-source run passes the corresponding witnesses, including arrival ordering, card
ownership, tick-wrap/backward-cursor handover, real reconnect and reset. Raw mutant diffs/logs
and their result summary are under `target/m11-s6-mutant-*` and `target/m11-s6-mutants.json`.

## Deletion census

The final census covers `src/`, `scripts/`, `tools/`, `assets/`, `.github/`, deployment/README
contracts and local upstream drafts. Searches for the retired runtime types, schedules, HP
fields, tree ledger, metadata and segment-grid constants have no active-source/data/script hits.
`src/trees.rs`, `src/trees/`, `src/net/destruction.rs` and `assets/trees/wood.ron` are absent.
An empty, untracked `src/trees/` directory left by the prior deletion was removed.

The only tracked `assets/trees/` files are species data, generator source, inspection artifacts
and their certificate. MEASURED:
`TreeSource` has four authored fields (species, seed, height and DBH); its GLB has **2 meshes**
and **3 nodes**, carrying generator inputs and `construction_part` metadata. It has no retired
segment/cap ownership metadata. Its certificate names `source_ron_sha`, not a runtime-sim hash.
The prop suite re-exports and verifies those artifacts. No old segment, slice or band structure
remains in the generator or construction-tool inputs. Generic geometry segments, masonry bands,
filesystem/ECS/BVH trees and species appearance data retain their actual meanings.

The tracked server allowlist dry run contains **12 files, MEASURED**, including all **4** common
construction geometry/dressing files and no generator-only tree file. The live map selects
`Construction` for every former tree placement. MEASURED fresh intact construction census:
**635 fir instances / 635 colliders**, **74 house instances / 4,070 colliders**, **4,705 total**.
Terrain and vehicle colliders are outside that census. One intact fir has one collider.

The census corrected stale current claims in source comments, the line-dose data header,
construction/Battle design contracts and a local upstream draft: segment HP, separate trunk
loads/carriers, the removed tree-fall cue, obsolete prototype variants, bands, and a house-only
map bound. The bounds remain generic functions of all admitted breakable chunks. No authored
value or physical law changes. Historical reports, design dispositions and the saved original
crash evidence remain historical records; they are not live contracts.

## Verification and handoff

MEASURED test results overlap; they are not a sum of distinct tests:

- Broad affected run: **331 passed, 7 existing ignored**. It covers common construction and
  analytical conservation, all headless tests, ledger/carrier/reset boundaries, collider/material
  guards, and `doc_citations`, `ui_ascii`, `net_boundary`, `bake::`.
- Centroid/guard recheck: **50 passed, none ignored**.
- Restored-mutant, live-map census and lifecycle recheck: **55 passed, none ignored**.
- Prop pipeline: **33 passed**, including pinned Blender generation, re-export, refusal and
  certificate/dressing checks.
- Final clean-source recheck: **55 passed, none ignored**. Formatting, diff whitespace and
  CI-feature workspace/all-target clippy pass after probe removal.

Formatting, CI-feature workspace/all-target clippy and diff whitespace passed before the
witness commit. Every compile and game/asset run used the machine lock. No subagents, external
issue comments, push, pricing or captures were used. The supplied `lanes/sweep.md` remains absent;
review used the binding rules and all six repository lenses. No remaining backlog row names a
permanently edited item, and no row is closed in this session.

Session 6 completes M11's implementation and production proof. No further M11 session or new
owner decision is required. The orchestrator retains the planned eye review and merge;
M12 representation work, M13 forest comparison, M14 building-drive proof and M15 pricing
retain their assignments. Grip dwell, `GRIP_SHEAR_FLOOR_M`, `piece_impulses`, `net-22`, `servo.rs`, `load-27`
and `load-31` are untouched. The pre-cutover parent/content record for M13 remains
[`m11-pre-cutover-manifest.json`](m11-pre-cutover-manifest.json).
