# Lightyear patch review verdict — SOUND-WITH-CORRECTIONS (reviewed 2026-07-10)

Repo restored clean at fix/deferred-rollback-check @ d3ee2be. Provenance: tag 0.28.0 = 28e823d9
= merge-base, reachable from upstream/main. RED→HEAD touches exactly 4 files, all in
crates/replication/prediction/: manager.rs +14, plugin.rs +7/−1, registry.rs +100,
rollback.rs +45/−19.

## Traps — all clean

- Marker lifecycle: set gate (registry.rs:488-497) is the exact complement of the receive-time
  check gate (registry.rs:454-456); mutually exclusive — no double-check possible. Consumed
  exactly once via retain (registry.rs:524-583); bitmask idempotent on duplicate delivery;
  clear_mismatch_history() on consumption (rollback.rs:565) prevents double-fire. Markers for
  still-future ticks survive rollbacks correctly; unconsumed bits persist to next frame, not lost.
- balanced()/lightyear_sync: UNTOUCHED (diff has nothing under crates/core/sync).
- Unchanged-entity scan: exclusive if/else-if branches (rollback.rs:519-575), no double-fire;
  base guard tests still pass.
- Cleanup: component self-removes when Vec empties; despawn OK. Latent retain-forever only if
  upstream later prunes predicted-entity ConfirmedHistory (currently dead path) — comment-worthy.

## Two non-blocking edges for the PR discussion

1. Backward SyncEvent can drop local tick below a recorded mismatch_tick → new branch
   (rollback.rs:534-539) would do_rollback to a locally-FUTURE tick; base couldn't. Ultra-edge.
2. Stale-mismatch consumption marginally more likely after a forced rollback (at-or-before vs
   exact lookup) — wasted, not incorrect, rollback.

## Tests (exact)

- Named test HEAD: ok 1 passed. RED 6b6ec123 proof verbatim: rollback.rs:693 assertion failed —
  "the deferred mismatch should trigger exactly one rollback from its confirmed tick /
  left: [] / right: [Tick(77)]" — exactly the starvation mechanism.
- rollback 28/28 (clean run), prediction 19 passed + 1 ignored doctest, sync 9. Full suite HEAD
  SINGLE-THREADED: 155 passed / 1 failed / 3 ignored (sole failure =
  test_replicate_transform_child_collider) — matches Codex.
- CORRECTION: suite is load-flaky in parallel (pre-existing at base; HEAD 152/4/3, base
  151-154 varying; each flaky test passes 10/10 isolated). Codex's counts reproduce only
  single-threaded. PR text must not imply parallel-stable.
- Clippy: -p lightyear_prediction -p lightyear_tests default features clean. --all-features NOT
  reverified (memory-pressure directive) — Codex's claim accepted-not-reverified.
- Test not vacuous: constructs genuine zero-margin condition (balanced()=3 delay @ 20 ms RTT,
  margin assert >= 0, server not stepped afterward so deferred check is the only detector),
  exactly-once enforced by assert_eq!(observed, vec![mismatch_tick]).

## Child-collider investigation — NEITHER #2 NOR #5; a THIRD upstream defect

test_replicate_transform_child_collider fails identically at base (left = 1.9999998, right =
4.0). Instrumented: client-side child has ChildOf = None — the parent-child relationship was
NEVER replicated/applied in AvianReplicationMode::Transform. The test's own observer
(transform_replication.rs:160-172) then takes the no-ChildOf branch, makes the child an
independent kinematic root whose local Transform (2.0) is read as world position → 2.0 ≠
parent(2)+local(2)=4.0. Hierarchy-replication defect in Transform mode, upstream's own. Not #2
(no client hierarchy to poison), not #5 (no rollback in the test).

## Corrections to OUR reports

- Report #2 (blanket ApplyPosToTransform): the blanket requirement registration
  (crates/integration/avian/src/plugin.rs:613-624, requirement at 620-623) is mounted by EVERY
  mode that calls sync_position_to_transform — Position mode, Transform mode (plugin.rs:302),
  PositionButInterpolateTransform (plugin.rs:245/250) — not "Position mode" only. Mechanism
  otherwise unaffected.
- New standalone report candidate: ChildOf replication loss in Transform mode (above), with
  upstream's own failing test as the repro.
