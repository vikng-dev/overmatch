# Swept CCD investigation: stopped by the hull-first wall probe

Subject: the multiplayer tank game's rigid-body driving physics; game state only.

## Disposition

Started from `12196bbf`, on `lane/swept-ccd`. The requested per-collider candidate passed its
small collision tests but worsened the shipped tank's hull-first thin-wall contact. The owner's
instruction was to stop and report if the probe worsened, without tuning. The candidate was
therefore removed; the sensor/disabled-collider extension was not attempted. No engine or tank
construction change is committed. No settings, defaults, speculative contacts or contact tolerance
were changed. The retained ignored probe and this report make the finding reviewable.

The probe's broad plausibility assertions passed both versions. That is not equivalence: the
comparison showed worse intrusion, triggering the separate stop condition. No earlier output
was turned into a test obligation, and no bound was adjusted after observing the result.

## Verified source claims

Verified in the pinned fork, against the cached published avian3d 0.8.0 crate, offline:

- `solve_continuous` defaults a missing `SweptCcd`; its default threshold is 0.5. Dynamic bodies
  over their threshold enter the supported-collider loop, which had no per-collider risk test.
- `update_body_size_metrics` takes the minimum CCD thickness of the body's colliders, including
  sensors and `ColliderDisabled` colliders. It also computes a body-wide sweep radius about the
  centre of mass. The body motion test combines translation and rotation using that radius;
  replacing it with centre translation or a child's own radius would miss offset rotation.
  The candidate kept that existing body-wide bound; the offset-rotation test passed.
- The #1101 patch already excludes `ColliderDisabled` in `solve_continuous`'s shared collider
  query. A disabled moving collider therefore **is already not swept in this fork**. Sensors
  remain in that query. Targets reject sensor tree proxies; moving sensors have no such filter
  and can reach TOI/clamping. This sensor claim was verified by source, not run-tested.
- MEASURED in the shipped harness: one tank body has 11 colliders. Four have CCD thickness
  0.047770023 m; the others have 0.34869206, 0.38982457, 0.37047285 m and four at 0.36235714 m.
  DERIVED at 64 Hz and threshold 0.5: the body's purely translational fast threshold is about
  1.529 m/s; the hull-only minimum gives about 11.158 m/s. The side slices' full depth is about
  0.09554 m, consistent with the investigation's approximate 9.6 cm.
- The claim of five TOI queries against terrain is not universal: the measured open-ground
  sample below makes zero. `solve_continuous` skips pairs already touching in the contact graph.
  No claim about five heightfield queries in another scenario was established.

The source's sweep-radius contract is retained, not independently strengthened by this lane.
The offset-rotation fixture checks the relevant stationary-COM case; it is not a proof of every
possible shape's existing size-metric implementation.

## Before and candidate observations

Run environment: `OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=hidden OVERMATCH_MUTE=1`.
All commands ran through `scripts/serial-cargo` with `--features dev_ui,bitprobe`, offline.
No timing conclusion was drawn.

Temporary counters used relaxed atomics scoped to `solve_continuous`: after the body early-out,
after sweep support selection, and immediately before each `compute_ccd_toi` call. Thus a TOI
query here means a collider-pair dispatch, not every internal Parry retry. The baseline sample
used the shipped driving app, rested on flat terrain, drove at full throttle for four seconds,
and observed the next eight ticks. MEASURED starting speed: 4.420108 m/s.

| Per tick, each of eight baseline ticks | Main |
|---|---:|
| Fast bodies | 1 |
| Colliders swept | 11 |
| TOI pair queries | 0 |

The candidate run's collider counter was before its new early-out, so that counter no longer
measured actual sweeps. Its apparent 11 is invalid as an after count and is not reported as one.
The stop condition arrived before correcting/repeating instrumentation. All instrumentation was
removed. Valid after sweep counts and the requested complete before/after counter comparison
remain open.

The retained `headless_test::track_scorecard::swept_ccd_probe::hull_first_thin_wall` mounts the
shipped driving plugin and tank construction. Fixture: a 0.5 m wide central static wall, 4 m tall,
0.02 m thick, initially 0.1 m ahead of the hull. It misses the belts. First contact was reported as
`hull proxy Hull_Collider` at every speed in both versions. The hull is given one initial velocity,
then receives neutral drive input for one second; the probe observes geometric hull AABBs from
body pose and collider-local transforms, rather than broad-phase boxes from an earlier phase.
The believable-range judgement rejects peak intrusion of a quarter metre, final intrusion of a
decimetre, or no reported contact. These are loose plausibility limits, not pinned results.

MEASURED (metres; negative final intrusion means separated):

| Initial speed, m/s | Main peak | Candidate peak | Main final | Candidate final |
|---|---:|---:|---:|---:|
| 6 | 0 | 0 | -0.0048439503 | -0.0048439503 |
| 8 | 0 | 0.024153233 | -0.004497528 | 0.0025353432 |
| 10 | 0 | 0.055403233 | -0.0048971176 | 0.0039668083 |

The candidate did not tunnel in this observation, but increased peak intrusion and left residual
embedding at two speeds. That is the worse outcome that stopped the lane. The code's explanation
is straightforward: these hull colliders fail the candidate's per-collider fast test, so their
first contact is left to the next discrete step instead of clamping this step. No fix or tuning
for that tradeoff is proposed here.

## Tests and gates

Temporary collision harness command:

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe avian_swept_ccd -- --nocapture
```

MEASURED on main: two passed, one failed. The failed test was
`a_thin_child_does_not_force_a_slow_thick_child_to_sweep`: the thick child moved 0.049999956 m,
clamped at its wall. On the candidate all three passed. The compatibility witnesses were
`a_fast_thin_child_and_a_single_slab_still_stop_at_a_thin_wall` and
`an_offset_rotating_child_still_sweeps`. A temporary thin/thick pair used 0.04 m and 2 m boxes,
8 m/s at 64 Hz, and separate thin static walls meeting only one child. No public counter API
was added. These temporary tests were removed with the rejected candidate.

An explicit apply-reverse/run/restore mutant cycle was **not completed** before the stop condition.
The initial main-versus-candidate run demonstrated the regression test's red/green distinction,
not the separately requested mutant procedure. Sensor mutants were not attempted.

MEASURED: the complete baseline track scorecard passed all law checks (slope, ride, rubble).
No after scorecard was run after the stop condition, so moved scorecard numbers are unknown.
The independent draft was prepared and the cached published crate was extracted into the new
empty directory `per-collider-r3uj4i0q` under the requested `scratchpad/draft-check-ccd/` path.
It was not applied or compile-checked before stopping. No upstream draft or index row is retained
as a proposed fix; both requested proven drafts remain open with the implementation task.

Final retained-change gates, MEASURED, all passed after restoring the engine:

- `cargo fmt --all --check` (module declaration ordering corrected before the final run).
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`.
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe swept_ccd_probe -- --ignored --nocapture`:
  one test passed, covering all three approach speeds. Every restored-engine probe output exactly
  reproduced the main column above.
- `git diff --check`; the engine and `src/tests.rs` have no diff from `12196bbf`.

The six lenses were read and applied locally to the retained diff: no unresolved finding;
no backlog row names an edited file.
Independent six-lens reviewers remain the orchestrator's landing gate; no agents were spawned.

## Fork inventory

MEASURED by `diff -ruN` of the cached published crate's extracted `src/` and the final vendored
`src/`: 843 added + 228 removed = 1,071 fork lines, unchanged from main. This is 993 lines in
upstream files plus the 78-line fork-only `stationary_miss.rs`. The CCD file retains only the
existing #1101 filter: 5 added + 1 removed = 6 lines. Body size metrics retains 60 added + 20
removed = 80 lines. This lane adds zero engine source lines, so `OVERMATCH_PATCH.md` is unchanged.

## Rejected candidate patch (prose only)

This is the precise independent patch considered against published 0.8.0. It is retained here
for reproduction, not as an accepted implementation or a tested upstream submission.

```diff
--- a/src/dynamics/ccd/mod.rs
+++ b/src/dynamics/ccd/mod.rs
@@ -741,6 +741,13 @@
                 continue;
             }

+            // A thin child must not force thicker children to sweep.
+            // The body-wide motion bound includes rotation of offset children about its COM.
+            let min_extent = shape1.ccd_thickness();
+            if max_motion <= ccd.threshold * min_extent.f32() {
+                continue;
+            }
+
             let layers1 = layers1.copied().unwrap_or_default();

             let motion1 =
@@ -767,7 +774,7 @@
                     swept_aabb.max.real(),
                 ),
                 local_centroid: shape1.mass_properties(1.0).local_com,
-                min_extent: shape1.ccd_thickness(),
+                min_extent,
             };

             // Sweep the collider against the relevant trees.
```
