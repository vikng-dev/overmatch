# lightyear_avian 0.28: ChildOf never replicated/applied in AvianReplicationMode::Transform — upstream's own test fails

**Target:** github.com/cBournhonesque/lightyear · lightyear_avian / replication 0.28 · **Severity
for us:** NONE (we run Position replication; found while reviewing the deferred-rollback patch) ·
**Status:** unfiled
**Repro: upstream's own suite.** `test_replicate_transform_child_collider` (`-p lightyear_tests`,
crates/tests/src/client_server/avian/transform_replication.rs) FAILS at the 0.28.0 release tag
(28e823d9): asserts child world x = 4.0, gets 1.9999998. Patch-independent (identical with and
without fix/deferred-rollback-check, verified 2026-07-10).

## Mechanism (instrumented at 0.28.0, edit reverted)

At assert time the client-side child entity has **`ChildOf = None`** — the parent-child
relationship was never replicated (or never applied) on the client under
`AvianReplicationMode::Transform`. The test's own setup observer
(transform_replication.rs:160-172) then takes its no-`ChildOf` branch and inserts
`RigidBody::Kinematic` on the child, making it an independent root kinematic body whose
replicated LOCAL `Transform` (2.0) is interpreted as WORLD position: `child_pos = 1.9999998`,
`ColliderOf = itself`, `child_gt = 1.9999998`. GlobalTransform ≈ 2.0 instead of
parent(2.0) + local(2.0) = 4.0.

This is a hierarchy-replication defect (ChildOf loss), NOT a transform-math defect. It is
distinct from our reports on the blanket `ApplyPosToTransform` requirement
([lightyear-avian-blanket-apply-pos-to-transform.md] — no client hierarchy exists here to
poison) and the enlarged-AABB restore assumption
([lightyear-avian-restore-assumes-enlarged-aabb.md] — no rollback occurs in this test).

## Suggested upstream framing

The failing test is already in their tree — the filing is "your own
`test_replicate_transform_child_collider` fails at the 0.28.0 tag; here is why," with the
ChildOf = None instrumentation. Cheapest possible issue for a maintainer to act on. Note the
suite is load-flaky in parallel (several unrelated tests are wall-clock sensitive at the tag);
this failure is NOT flake — it fails identically every run, single-threaded included.

## Our workaround + removal condition

None needed — we do not use `AvianReplicationMode::Transform`. Filed for upstream's benefit and
as context if we ever switch replication modes.

## What fixing this unlocks for us

**Nothing for us — filed for the ecosystem.** We run `AvianReplicationMode::Position` and have no
plans to switch; there is no workaround to delete, no cost to recover, and no experiment this gates.
The value of filing is entirely upstream's: it is a failing test *already in their tree* at the 0.28.0
tag, which makes it the cheapest issue on this list for a maintainer to act on, and clearing it is
good for the ecosystem we depend on.

The only self-interested reason to keep it on the list: a fixed `Transform` mode would make that mode
a real option for us if we ever had cause to move (we do not — Position mode is what the rig and the
rollback path are built around). Filed as courtesy, tracked as nothing.
