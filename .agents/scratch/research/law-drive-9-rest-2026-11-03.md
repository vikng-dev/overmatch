# Driving final-review remainder (#146)

Subject: the game's driving model. Where staffing is mentioned, the game's damage model means HP pools at stations only.

## Reverse construction and crank admission

Commit: `driving: give reverse gears their own steering rows (#146)`.

Verified Claude's reverse-row defect against the previous lane's executed red: forward speeds
[5,12], reverse [10], reverse tight radius 6 m read 3 m. Those numbers are inline Fixture data.
Each ladder now authors its own required radii. The observer, candidate pricing, steering solve
and debug HUD read the active ladder; the startup construction fingerprint includes reverse rows.
The regression checks the observer radius, candidate turn load, steering Jacobian and HUD together.
Missing rows name `transmission.steering.reverse_radii` at admission. The shipped sheet, spec,
inline bake sheet and Blender sheet change together.

The previous lane also executed a red for finite scalar inputs with overflowing crank energy.
Admission now bounds the virtual free tick's energy using the guarded crank plus the admitted
fuel-cut width; it refuses overflow before publishing parameters, naming engine inertia/curve
and crank energy. No engine or over-rev outcome changed. No wire type changed; `PROTOCOL_REV`
is deliberately unchanged.

MEASURED verification: formatting and CI clippy pass. The broad run passed 439 tests, including
both regressions and all headless checks, with four ignored; three test failures were corrected:
a reverse-index fixture needed matching rows to isolate u8 admission, and two overly broad test
edits incorrectly demanded an energy error for other refusals. The corrected affected-module run
passed 416 tests, including track, spec, bake, command, crew UI and whole-tree source guards.
No backlog row names these edited items. Existing unrelated bake rows name `src/bake.rs`, which
this change does not edit.

## Clock and queues

MEASURED start: 2026-11-02 21:48:36 UTC; the lane's sixty-minute boundary is 22:48:36 UTC.
Builds and tests use `serial-cargo` with the unattended/headless/mute environment. Queue waits
are taken from the build queue's queued/started timestamps, not inferred from compilation time.
The first correction waited behind another session's whole-library run; final totals follow below.

## Reachable notch admission

Commit: `driving: admit the notches the client actually reaches (#146)`.

Verified Codex's binary32 counterexample from the preceding lane. Admission now traverses the
actual tap transition in both directions from neutral and both clamped endpoints. The client
calls that same `DriveFeel::tap_notch` operation. MEASURED: step 0.4 reaches thirteen states;
one decrement from one equals tight_on 0.6 exactly, and admission refuses it. Moving that
Fixture threshold to 0.65 is admitted. The regression also checks transition closure.

Decision: a Resolution budget of 4,096 reachable states bounds startup memory and work; a finer
lattice is refused by name with an authoring fix, rather than making startup enumerate millions
of binary32 states. This is an admission cost bound, not a control threshold. Default shipped
keyboard feel is unchanged. Formatting, CI clippy and 417 affected-module/guard tests pass.
