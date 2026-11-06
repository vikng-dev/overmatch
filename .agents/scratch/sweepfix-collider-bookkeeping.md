# Cheaper collider bookkeeping

Subject: the video game's environment destruction model; game state only.

The invariant is the collider representation for a given departure: source chunk address, convex
shape, collision layers, occupied mass and inertia, friction, sensor status, ballistic material
and owning body. Entity allocation order is free to change server physics observations.

## Reuse

Standing chunk colliders transfer to the departing body as contact hulls. Existing contact and
occupied sensor entities transfer again when a body splits; chunks with no prior collider still
create one. The shared construction installer mounts the existing stale-contact cleanup rule.
Owner-local collider lookup is prepared on workers; structural changes remain ordered.

Verified against installed Bevy 0.19.1 and Avian 0.7.0 sources:
- Bevy `ChildOf` relationship insertion/discard maintains `Children`; discard unlinks before a
  retiring parent's linked despawn can reach a transferred child.
- Avian `ColliderHierarchyPlugin::on_collider_body_changed` observes `ChildOf` insertion and
  updates `ColliderOf`; its relationship hook computes the new `ColliderTransform`.
- `ColliderTreeUpdatePlugin` observes collider and `ColliderOf` insertion, removes the old proxy
  and inserts the new body's proxy. `update_moved_collider_aabbs` reads changed shape/pose.
- Avian broad-phase pairs retain their prior body IDs. The game's `collider_rehome::plugin`
  discards stale pairs by disabling/enabling the moved collider, before the next physics step.
- `ColliderBackendPlugin` recomputes cached shape mass on insertion even when supplied manually.
  Supplying a precomputed `ColliderMassProperties` therefore does not avoid that work.

Reuse saves entity allocation/deletion and initial component setup. It still pays for shape
replacement, mass calculation, hierarchy changes, broad-phase proxy transfer and contact cleanup.

The retained class witness compares fresh and reused departures before and after physics ticks, then splits
the group and compares again. It includes cached mass properties and checks that reused entities
survive retirement of their old body. A second witness reaches reuse through the shipped law/refit
installer. Existing headless checks cover the full spawned-body contract. The rejected candidate additionally
preserves fresh collider placeholder initialization and tests a rotated/translated parent. That
hardening is not in the retained reuse commit and remains for a follow-up.

## Batching and the cold ancestor cache

A single deferred batch is rejected. MEASURED application rises from 5.406584 to 168.637792 ms.
Bevy's batch insertion/spawn flushes deferred hooks at the end. Avian's `AncestorMarkerPlugin`
therefore sees a missing parent marker for each child and queues it repeatedly;
`ColliderHierarchyPlugin::on_collider_body_changed` refreshes the growing subtree on every marker
insertion. This explanation is DERIVED from those pinned sources.

The measured warm-cache candidate applies one attachment normally before batching the rest. Its nested command flush
establishes Avian's marker; later children see it. Each construction's refit has its own batch, so
every cold root gets that seed. Reused component updates use `insert_batch`, remaining creates use
`spawn_batch`, and refit removals share one structural command. Geometry still prepares on workers. This entire batching candidate is rejected: the final medians
5.344209 and 5.483375 ms straddle reuse alone's 5.406584 ms, so no repeatable gain was established.
`element_bundle` exposes Bevy's required `NoBundleEffect` guarantee; its contents are unchanged.

MEASURED operation-count witness: 513 `ColliderOf` insertions for a cold body with 256 colliders.
The bound follows the two collider/parent insertion observers and the initial marker inserts,
which now see only the seed; no duration is asserted. Supplying cached mass properties was not
implemented: Avian overwrites them in its insertion hook. No batching implementation or lever remains in code; its patch is preserved in the handoff.

## Measurements

MEASURED in-process command-application microbenchmark: one batch of 600 departing chunks,
1,200 resulting debris colliders, and 600 existing standing colliders. Each variant uses 17 fresh
app fixtures, with geometry preparation and command queuing outside the timed interval. Median
application only, under the machine lock; no frame/tick timer, ram timing harness, capture or UDP.

| Step | Collider creates | Collider deletes | Median, ms | Decision |
|---|---:|---:|---:|---|
| Before | 1,200 | 600 | 6.076167 | Reference |
| Reuse | 600 | 0 | 5.406584 | Keep; 11.0% lower median |
| Cold batch | 600 | 0 | 168.637792 | Reject |
| Warm batch, before pose guard | 600 | 0 | 5.170500 | Intermediate observation |
| Warm batch with pose guard, run 1 | 600 | 0 | 5.344209 | Reject: incremental gain not repeatable |
| Warm batch with pose guard, repeat | 600 | 0 | 5.483375 | Reject; reuse alone is 5.406584 ms |

MEASURED headless tracked recipe, from first occupied contact. Ram rows and lifecycle counts
were collected before the final fresh-pose initialization guard; the final guard is covered by
the rotated-parent witness. A ram census after that guard remains at the time limit. This is the historical in-process
witness, not session 7's separate UDP replay; it has a smaller collapse burst. Emergent outputs are
observations and no historical-speed assertion is retained.

| Step | +1 m/s | +2 m/s | +5 m/s | +10 m/s | Chunks by +2 | Mass by +2, kg |
|---|---:|---:|---:|---:|---:|---:|
| Before | 10.813708 | 10.441001 | 10.005212 | 9.994593 | 106 | 2,495.940 |
| Reuse | 10.813708 | 10.441001 | 10.005212 | 9.994593 | 106 | 2,495.940 |
| Cold batch, rejected | 10.813708 | 10.441001 | 10.005212 | 9.994593 | 106 | 2,495.940 |
| Warm batch | 10.813708 | 10.441001 | 10.005212 | 9.994593 | 106 | 2,495.940 |

MEASURED 400 N press: every variant removes zero chunks; detached exact cut 108,133.96 N throughout.

MEASURED collider entity lifecycle counts through contact +320, including every collider category:

| Step | Creates | Deletes |
|---|---:|---:|
| Before | 1,005 | 695 |
| Reuse | 806 | 550 |
| Cold batch, rejected | 1,185 | 944 |
| Warm batch | 1,215 | 970 |

Later solver-order changes move the collapse counts; subtracting these totals does not establish
identical law events. [All per-tick event counts](sweepfix-collider-bookkeeping-counts.json) are
committed alongside this report; an absent tick has zero events. The table below shows the union
of each variant's largest event ticks. Tick indices are DERIVED from the fixture's 64 Hz simulation
clock, never wall-clock pricing.

| Tick | Before creates/deletes | Reuse | Cold batch, rejected | Warm batch |
|---|---:|---:|---:|---:|
| 1263 | 131 / 2 | 131 / 2 | 131 / 2 | 131 / 2 |
| 1265 | 123 / 41 | 123 / 41 | 123 / 41 | 123 / 41 |
| 1266 | 90 / 76 | 90 / 76 | 90 / 76 | 90 / 76 |
| 1287 | 91 / 59 | 91 / 59 | 91 / 59 | 91 / 59 |
| 1301 | 105 / 48 | 85 / 32 | 95 / 38 | 85 / 32 |
| 1302 | 78 / 110 | 87 / 114 | 78 / 110 | 87 / 108 |
| 1308 | 0 / 2 | 0 / 2 | 184 / 3 | 118 / 4 |
| 1310 | 10 / 5 | 5 / 2 | 5 / 22 | 167 / 1 |

## Authority and validation

Clients receive `DebrisPose` and `Piece` on separate replicated carriers. Server publication copies
`Position` and `Rotation` at `src/net/debris_stress.rs:485`; client `follow_pose` copies the received
pose into `Transform` at `src/net/debris_stress.rs:584`. `carry_construction_piece` selects the shared
interpolation cursor at `src/net/debris_stress.rs:269`. Those
paths use chunk IDs and replicated poses, not server collider entity order (ADR-0037).

Reuse: formatting and CI clippy pass; required targeted tests: 289 passed, 6 ignored, zero failed. No strengths, assets,
drive settings, debris geometry or network source changed.

The rejected final candidate passed formatting, CI clippy, 289 required targeted tests (7 ignored),
and its rotated-parent/ownership witnesses. The retained reuse commit previously passed formatting,
CI clippy and 289 required targeted tests (6 ignored). Its exact committed Rust files are restored. Every compile/test used `scripts/serial-cargo --features dev_ui,bitprobe` with the required
lock wait, unattended/headless. No ram timing harness ran and nothing bound UDP 5888. No strength,
asset, drive-setting, debris-module or network-module edit is delivered.

Reproduce the candidate manual benchmark after applying the handoff patch with:

```sh
OVERMATCH_CARGO_LOCK_WAIT_S=10800 OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless \
  scripts/serial-cargo test --lib --features dev_ui,bitprobe \
  departure_batch_application_observation -- --ignored --nocapture --test-threads=1
```

## Final disposition

Keep only `construction: reuse departing chunk collider entities (#235)`. Reject cold batching and
the warm-cache batching refinement because its final incremental gain did not repeat. The final
source is restored exactly to the reuse commit. Remaining: independently land/review the rotated-
parent initialization hardening, rerun its ram census, and orchestrator pricing/integration.
