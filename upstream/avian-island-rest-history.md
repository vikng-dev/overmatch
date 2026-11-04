# Avian island waking loses per-body rest history

Subject: the video game's server physics and environment destruction; CPU and game state only.

**Target:** vendored avian3d 0.7.0 with Bevy 0.19.1. **Status:** LOCAL DRAFT, not filed; upstream head was not checked.

## Suggested title

Preserve awake bodies' sleep timers across island merges and keep dormant new contacts sleeping

## Reproduction and invariant

`src/tests/avian_island_sleep.rs` mounts the real physics plugins. Geometry and observation counts are inline Fixture data.

- Naturally settle a dynamic cube on a static slab, then add a coplanar static slab. The new touching contact wakes the sleeper although neither participant supplies an awake non-static body. A sleeper without input must remain asleep, and its new contact must enter the sleeping pair set without active manifold handles.
- Naturally settle one body with zero gravity, spawn another separated body and let it accumulate rest time, then call `PhysicsIslands::merge_islands` followed by `WakeIslands`. MEASURED on the unpatched vendor: the already-awake body's timer changes from 0.0625 s to zero. `SleepTimer` is the individual body's time below its thresholds; a topology change cannot erase that history. The solver still resets the timer when the body actually exceeds a threshold.
- Create an early speculative pair between an awake body and a sleeper; later connect that sleeper to a larger sleeping island in the same narrow phase. Carrying the first merged island's ID loses the wake when the larger island absorbs it. The regression checks between narrow phase and solver, not after the later sleeping pass. MEASURED: restoring the stale-ID queue makes this witness fail with a partly sleeping island.
- Remove `SleepTimer` from a linked sleeping body, then apply `WakeIslands`. Every linked body must retain `BodyIslandNode` and `SleepTimer` until unlinking; the traversal must panic on this broken invariant instead of silently ending after marking the island awake.

## Mechanism and patch

`merge_islands` deliberately keeps a mixed island's aggregate sleeping flag set so the caller can wake its sleeping members and restore their contacts. `WakeIslands` currently also resets every already-awake member's timer. The patch queries `Has<Sleeping>` and resets/removes the marker only for newly woken members. Explicit removal of `Sleeping` resets that body's timer in the removal hook, before the queued wake can no longer see its marker.

The narrow phase shares one contact-link boundary between touching starts and constraint activation. It requests a wake only when an awake non-static participant exists. Otherwise it transfers the sleeping participants' touching contacts through `ContactGraph::sleep_entity_with`, removing their manifold handles. Requested wakes carry a representative body until all contact changes and merges finish, then resolve and deduplicate current island IDs. Contact losses retain their wake path.

The sleep thresholds and time to sleep are untouched. No body-kind rule, debris timer or additional settling system is involved.

The linked-body query in `WakeIslands` uses an invariant `expect`: required state cannot be missing while a body remains linked. This also refuses a stale linked entity instead of leaving a partly woken island.

## Evidence and limits

The local before/after recipes, counters, adverse ram observation and remaining moving pile are recorded in `.agents/scratch/settle-islands/report.md`. This is a correctness patch, not proof that every pile settles or that server capacity improves. Pricing is reserved for the orchestrator by the binding lane rules.

The existing production support-removal, support-move, support-disable and finite-mass impact witnesses in `debris::tests` remain required. On an engine upgrade, remove this vendor patch only when the positive island lifecycle witnesses and those production wake witnesses pass against the replacement implementation.
