# Dedicated-server tick attribution (#305)

Subject: the video game's server cost instruments; references to its damage model mean game
state only, with crew represented by HP pools at stations.

## Inventory at the starting point

The pinned sources are Bevy 0.19.1 in the cargo registry and Avian 0.7.0 in
`vendor/avian3d-0.7.0-child-pose`. The following order comes from registrations and those sources,
not from the prose describing them.

| Order | Work | Existing bracket |
|---|---|---|
| Frame, before fixed loop | `First`, `PreUpdate`: time, receive/socket handling, input admission; `StateTransition`; `RunFixedMainLoop` before-loop input | None; outside fixed tick |
| `FixedFirst` | Lightyear local-tick advance, ram speed sampling | Clock opens in an unordered system; earlier work can escape |
| `FixedPreUpdate` | Lightyear input-buffer consumption and fixed network preparation | Tick only |
| `FixedUpdate` | Shot-clock publication, Battle phase/hold, command parsing/bridge, respawn, driving snapshot/forces, weapon fire, recoil, servos, command-edge consumption | Tick only; triggered construction intake has a leaf bracket |
| `FixedPostUpdate`, before physics | Construction hull sweep and reconciliation; finite-state probe | `sweep_us`, `refit_us` system bodies |
| `PhysicsSystems::Prepare` | Collider-tree/transform propagation, collider scale and mass-property updates, broad-phase preparation; collider-motion births before step | None |
| `PhysicsSystems::StepSimulation` | Exclusive runner enters `PhysicsSchedule` | Runner itself outside physics bracket |
| Nested `PhysicsSchedule` | First -> BroadPhase -> NarrowPhase -> Solver -> Sleeping -> Finalize -> Last; solver preparation, joints/contact constraints, substeps, restitution, finalize, stored impulses | `physics_us`; nested `contacts_us` readout/cache and `sleep_us` |
| Nested `SubstepSchedule` | Velocity integration -> warm start -> constraints -> position integration -> relaxation -> damping | Included in physics |
| After physics | Writeback -> collider poses/AABBs/snapshot -> projectile march -> damage consequences -> recorded consequences | None except projectile march in the separate cost trace and triggered intake |
| Construction `LawSet` | Read contacts -> deposits/owner laws -> `LandingSet` crumble -> refit; split groups follows | `intake_us`, `structure_us`, `crumble_us`, `refit_us` system bodies |
| Publication | `SimPhase::Publish`: construction ledger, tank/servo/status publication | Only ledger preparation has `publish_us` |
| `FixedLast` | Lightyear fixed cleanup and other final systems; tick close; stats/ram census and trace readers | Unordered close can precede cleanup; collector separately timed |
| Frame, after fixed loop | `Update`; `PostUpdate`: replicon change scan/serialization -> connection send -> link/socket flush; `Last` stats emission | `send_us` outside fixed tick, not additive with tick |

Bevy inserts `ApplyDeferred` on command-producing dependency edges and applies remaining buffers
at each schedule's end. Command triggers run observers synchronously during application. Avian's
collider add/insert/remove hooks, collider-owner observers, and the game's rehome/ledger observers
therefore run at these flushes, outside system-body brackets. Immediate triggers run inside their
caller. The same applies within the nested physics/substep schedules. Hooks and observers are not
independent scheduled stages.

MEASURED baseline artifacts: `integ-ram-mainbase-a/result.json` and
`integ-ram-mainbase-b/result.json` under bay-2's `target/`. Worst ticks: 23.099125 and 27.388041 ms.
DERIVED after including sweep/crumble and counting physics once: unattributed worst-tick shares
46.35% and 52.57%. These local runs do not price the hosted allocation.

## Mechanism decision

Bevy's `schedule::Schedule::run` and executors provide tracing spans behind `trace`, not a readable
per-tick timing resource. Its `SystemExecutor` extension delegates execution but has no public
per-system timing callbacks. Avian's `PhysicsTotalDiagnosticsPlugin` times First-to-Last inside
`PhysicsSchedule`, just like the existing bracket: it omits outer preparation, command hooks and
the rest of the fixed tick. Its per-plugin diagnostics do not close those gaps either.

Use Bevy's `FixedMainScheduleOrder` for complete-schedule boundaries, and the existing ordered
physics/gameplay phase barriers for the long `FixedPostUpdate` schedule. Keep existing leaf timers
as nested detail and debit their time from the enclosing intervals. Command application and hooks
stay in their owning interval. Include a separately named unattributed remainder and keep send
and collector outside the tick. Do not replace engine executors or enable profiler tracing.

## Implemented intervals
`cost::phases` inserts timer schedules through `FixedMainScheduleOrder` and checkpoints at the existing sweep, physics, consequence, split and publication barriers. `TickClock` opens before the fixed runner and closes after its final flush; readers run afterwards. The simulation keeps its existing executors.
Phase values debit all leaf-bracket deltas, regardless of which phase a leaf runs in. The reader adds every emitted scalar leaf and named phase, then separates nested sleep from the physics-step residual. Inclusive `physics_us`, collector and send never enter that sum. Unordered systems remain unordered and charge to the wall interval in which they execute; these are wall intervals, not per-system CPU profiles.
`construction_ecs` includes unbracketed law/refit/split work, AABB updates, deferred application and hooks. `post_physics` includes writeback, collider poses/history, the game-state consequence chain and dispatch. `fixed_update` includes command/Battle/respawn work, driving, firing and servos.
## Tracked ram evidence
MEASURED with the exact tracked local ram commands, headless, unattended and muted, one run at a time under the machine lock. Both runs used available parallelism 10 and pools 10/1/1; this is an unpriced local rehearsal, not the hosted allocation. Both passed full-speed admission and ended at rest. No captures or attached profiler.
Artifacts: `target/ticks-ram-a/{server.log,client.log,result.json}` and `target/ticks-ram-b/{server.log,client.log,result.json}`. `result.json` contains each over-budget tick and its additive `parts_us`. The existing `worst_tick` is contact-window scoped; `worst_capture_tick_attribution` explicitly includes the approach.
DERIVED coverage = 100 × (tick time − unattributed remainder) / tick time. All captured over-budget ticks meet the owner’s 95% requirement. Displayed 100.000000% rounds floating conversion noise; the largest remainder across both captures is below one nanosecond.
| Run | Tick | MEASURED wall ms | DERIVED attributed % |
|---|---:|---:|---:|
| A | 1317 | 16.855667 | 100.000000 |
| A | 1319 | 21.043583 | 100.000000 |
| A | 1320 | 22.230166 | 100.000000 |
| A | 1321 | 28.464916 | 100.000000 |
| A | 1322 | 22.391250 | 100.000000 |
| A | 1323 | 17.544917 | 100.000000 |
| B | 992 | 36.402417 | 100.000000 |
| B | 995 | 19.291542 | 100.000000 |
| B | 996 | 17.185834 | 100.000000 |
| B | 1023 | 23.914750 | 100.000000 |
| B | 1024 | 26.440042 | 100.000000 |
| B | 1027 | 25.048375 | 100.000000 |
| B | 1032 | 26.517666 | 100.000000 |
| B | 1033 | 31.911500 | 100.000000 |
| B | 1043 | 17.622042 | 100.000000 |
| B | 1054 | 58.252750 | 100.000000 |
| B | 1320 | 18.102958 | 100.000000 |
| B | 1321 | 19.841375 | 100.000000 |
| B | 1322 | 16.719917 | 100.000000 |
| B | 1323 | 23.662125 | 100.000000 |
| B | 1324 | 20.155667 | 100.000000 |
| B | 1539 | 24.993792 | 100.000000 |
| B | 1556 | 19.341042 | 100.000000 |

### Where the peaks land
| Run/window | Tick | MEASURED wall ms | Largest additive intervals, MEASURED ms |
|---|---:|---:|---|
| A complete capture | 1321 | 28.464916 | construction_ecs: 7.754041; crumble: 6.168959; fixed_update: 5.521042; physics_step: 3.956708 |
| A contact window | 1321 | 28.464916 | construction_ecs: 7.754041; crumble: 6.168959; fixed_update: 5.521042; physics_step: 3.956708 |
| B complete capture | 1054 | 58.252750 | post_physics: 22.380250; physics_step: 17.204667; physics_prepare: 11.745500; construction_ecs: 3.099208 |
| B contact window | 1539 | 24.993792 | post_physics: 7.563750; construction_ecs: 4.260875; physics_prepare: 3.788000; physics_step: 3.597166 |

The B capture’s largest spike precedes contact. The intervals identify where wall time passed; these runs do not distinguish OS scheduling from computation inside an interval. No construction outcome or tick-cost observation becomes a gameplay assertion.
| Run | MEASURED contact-window mean tick ms | MEASURED collector mean/max us | Captured/contact-window over-budget ticks |
|---|---:|---:|---:|
| A | 4.880131 | 0.961682 / 27.500000 | 6 / 6 |
| B | 5.394491 | 1.284357 / 20.791000 | 17 / 7 |

### Instrument cost and validation
MEASURED isolated empty-`FixedMain` A/B, phase instrument off/on: **8.436 / 9.152 us mean**, observed difference **0.717 us/tick**. The test warms schedule construction for 128 iterations, then measures 10,000 per composition in the edit profile, with one test thread and the machine lock. The outside stopwatch includes timer finalization. Artifact: `target/ticks-instrument-cost.log`. This isolates added clock/dispatch cost; the real ram collector costs are in the table above. Periodic summary/JSON emission and whole-Battle process overhead are outside this A/B.
Formatting and CI clippy passed. The Rust instrumentation passed 76 targeted/guard tests; the final whole-tree guard selection passed 48 tests. Reader tests pass 30 cases, including complete-capture versus contact-window peaks, new stage inclusion, overlap/nonfinite/schema refusal, and the explicit remainder. No backlog rows name the edited files.
The hosted 30-tank Battle measurement in #306 remains the capacity/realism decision’s input; these local observations do not settle it.
