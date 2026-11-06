# Collider lifetimes on the heavy collapse

Subject: the video game's environment destruction model; game state only.

Decision: retain the colliders. The reproduced burst contains no standing chunk collider that
exists for zero physics steps. This lane delivers the diagnosis and operation counts; production
Rust, strengths, assets and drive settings remain at the dispatch baseline. There is no
after-optimization row or claimed cost reduction.

## Mechanism

`construction/breakage.rs::run_laws` swaps the elements adjoining changed chunks and failed
joints. `prepare_reconcile` indexes existing children and emits only the difference:
`Shape::chunk_present` requires a swapped element and `Fate::Standing`, and a new collider also
requires that chunk's entity to be absent. Existing standing colliders are retained. Released
groups have already become `Fate::Fallen` before reconciliation and cannot enter its create arm.

`fracture/parts.rs::Assessment::advance` records changed topology for the next tick's rebuild
and solve. The authority reconciles after the current law pass. At C the new chunks still stand;
the next physics step runs their geometry before A's law pass releases them. Omitting them would
remove geometry from that physics step. Advancing their release would change the law's output
tick. Neither suspected defect was found: a whole-body rebuild of existing colliders, or creation
for an already-decided departure.

## Operation census

MEASURED with lifecycle observers on `On<Add, Collider>` and `On<Remove, Collider>`, recording each
entity's birth and the number of completed physics steps at removal. The counter advances after
`PhysicsSystems::Writeback`, before `LawSet`; `FixedLast` reports counts. It reads simulation time
only. No pricing, stage or tick timers were armed.

The network replay reproduces the reported heavy burst. C/A/B below are respectively contact
+2/+3/+5 in this replay. Counts include child colliders removed recursively with retiring bodies.

| Tick | Standing chunk creates/deletes | Element deletes | Debris collider creates/deletes | Zero-step standing deletions |
|---|---:|---:|---:|---:|
| C | 804 / 26 | 49 | 68 / 0 | 0 |
| A | 0 / 600 | 0 | 1,238 / 52 | 0 |
| B | 0 / 191 | 0 | 484 / 614 | 0 |

MEASURED origins of A's 600 standing deletions: 495 born at C survived one physics step;
66 born before C survived three; 39 born before those survived four. Across the network replay,
931 standing chunk colliders were deleted; none had a zero-step lifetime.

Main has the same mechanism. The reconciliation implementation and its scheduling are identical
between the dispatch baseline (*construction: the gravity preload solves its parts in parallel*)
and the requested main snapshot (*destruction: landings and holes solve in parallel, the factor
assembles in place*). A separate build and network replay of that main snapshot, in this bay,
also observes the churn:

| Main tick | Standing chunk creates/deletes | Element deletes | Debris collider creates/deletes |
|---|---:|---:|---:|
| Creation burst | 874 / 37 | 50 | 98 / 0 |
| Following tick | 0 / 686 | 0 | 1,374 / 24 |

MEASURED: 602 of those 686 deletions were born in the creation burst and survived one physics
step; 84 were older. Across that replay, 924 standing colliders were deleted, with zero zero-step
lifetimes. The bay was then restored to the integrated dispatch baseline.

## Functional witnesses

MEASURED historical headless ram witness, speeds in m/s; cumulative chunks and mass through +2:

| Tree | +1 | +2 | +5 | +10 | Chunks | Mass, kg |
|---|---:|---:|---:|---:|---:|---:|
| Session 6 | 10.813708 | 10.441001 | 10.005212 | 9.994593 | 106 | 2,495.940 |
| Session 7 | 10.813708 | 10.441001 | 10.005212 | 9.994593 | 106 | 2,495.940 |

MEASURED 400 N side press: zero chunks removed; detached cut 108,133.96 N, unchanged.

The network census uses the recipe-derived pose through the harness's `spawn_pose` field, with
live UDP commands, rather than the headless witness's direct placement. DERIVED from the recipe:
128 idle input ticks and 1,152 throttle ticks. `OVERMATCH_RAM`, `OVERMATCH_SERVER_STATS_S` and
`SPIKE_*` are unset. Its speeds are separate observations:

| Network snapshot | +1 | +2 | +5 | +10 | Chunks by +2 | Mass by +2, kg |
|---|---:|---:|---:|---:|---:|---:|
| Integrated | 10.693793 | 4.5231104 | 4.1529593 | 3.7338762 | 42 | 994.22437 |
| Main | 10.728291 | 10.665252 | 8.885736 | 8.485814 | 52 | 1,232.5459 |

## Validation and handoff

Formatting and CI clippy with warnings denied pass. The required construction, contact-transfer,
physical-law, headless, shape-reachability, citation, font, net-boundary and bake groups, plus the
tracked ram composition witness, pass: **285 passed, 6 ignored, zero failed**. Every compilation
and test used `scripts/serial-cargo` with `dev_ui,bitprobe`. Each network scenario ran headless,
unattended and silent under one machine lock; both processes were stopped afterwards.

The lane handoff folder contains the `sweepfix-s7-` report, structured counts for every observed
tick, lifecycle probes, injection script, network driver, headless observers and raw logs.
Temporary Rust includes, counters and their lever are removed. No gameplay source changes,
new geometry choices or historical-output assertions are delivered. The remaining collider cost
still needs an optimization that preserves the required physics step, or the owner's geometry
decision; the excluded debris representation changes were not attempted.
