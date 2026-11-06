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
