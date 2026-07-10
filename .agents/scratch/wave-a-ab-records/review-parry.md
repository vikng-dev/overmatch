# Parry patch review verdict — SOUND-WITH-CORRECTIONS (reviewed 2026-07-10)

Reviewer: adversarial subagent, full protocol (RED→HEAD diff, instrumentation, suites, RED proof).
Repo restored clean at fix/cast-absolute-tolerance @ db36bd0.

## Mechanism determination (VERIFIED — Codex's correction to our report is CORRECT)

Instrumented copy of RED e7f61fc, eprintln at every exit of minkowski_ray_cast, 2400 casts
(the exact test workload):
- EXIT:last_chance 2278 (ALL nonzero error) — the sole culprit
- EXIT:eps_rel 0 — NEVER fires
- EXIT:dist_small 110, EXIT:dim_full_some 12 (zero error)

Structural proof: at base 0.27.0 the eps_rel branch (gjk.rs:770-781, NOT :676 — :676 only
computes _eps_rel) returns None (a miss) under default float features — it cannot produce a
short TOI at all. Our report is doubly wrong: wrong branch, and that branch returns no TOI.
Actual mechanism: gjk.rs:712-715 sets last_chance when max_bound >= old_max_bound (float
cancellation on large support coords translated into the advanced-origin frame); gjk.rs:720-723
returns lower bound ltoi, short by the stagnated gap.

## Traps

- Witness/normal generation UNTOUCHED — confirmed. Base→HEAD src diff = 15 added / 2 removed
  lines, all inside the last_chance block (HEAD gjk.rs:713-736). result() (gjk.rs:814), the
  directional_distance witness call (:649), normal (:748), simplex/support generation unchanged.
  Returned witnesses bit-identical to base; refined TOI now CONSISTENT with them (what our
  workaround reconstructs externally). Our workaround's assumptions unaffected.
- Fix also flows into cast_local_ray (gjk.rs:521-536); the `- ray.origin` term handles non-zero
  origins correctly.

## Minimality — pass, one behavioral note for the PR

New re-check of max_time_of_impact: a stagnation hit whose refined TOI exceeds the cap now
returns None where base returned a wrongly-short hit. Correct per contract; one sentence in PR.
"≤4 weighted witness accumulations" claim accurate (dimension()+1 ≤ 4, only on last_chance path).

## Tests (exact)

- HEAD named test: ok. Errors 0.113248825 / 0.0022649765 / 0.00023841858 mm @ 5/50/500 —
  matches Codex exactly.
- "83/83" = parry3d --tests (61 unit + 22 integration); "89/89" = parry2d --tests (59+30);
  parry3d-f64 60/60, parry2d-f64 59/59; fmt clean; +519 doc-tests pass /39 ignored per 3D crate.
- CAVEAT: bare `cargo test` fails to COMPILE parry3d examples (glam 0.30 vs 0.32 dup via kiss3d
  dev-dep) — pre-existing on the 0.27.0 tag; counts reproduce only with --tests. PR text should
  not claim bare-suite clean.
- RED proof at e7f61fc: FAILED as required, verbatim: "shape-cast TOI error 0.00024616718
  exceeded 0.0002 for half-extent 5" (time_of_impact3.rs:125:9), printed errors
  0.24616718 / 3.6373138 / 139.44792 mm — matches before-numbers.

## NEW substantive caveat: one-sidedness is abandoned (disclose upstream + fix our report)

Post-fix, the stagnation exit's TOI is NO LONGER a certified lower bound. Measured signed error
(fix applied): he=5: 651/770 casts OVERSHOOT, max +0.113249 mm; he=50: 631/708, max +0.00226 mm;
he=500: 209/800, max +0.000238 mm. Base was strictly short. Cause: simplex witness can sit
laterally off true contact (~1.2° off ball pole at he=5); curvature converts lateral deviation
to forward error ≈ r·θ²/2. No proven global sub-mm bound — only empirical on this
perpendicular flat-face configuration. Overshoot = potential tunneling for upstream users; PR
MUST state the property change. The test only asserts a two-sided |err| ≤ 2e-4 bound
(time_of_impact3.rs:100/130) and does not guard one-sidedness; no oblique/curved cases.

## Required corrections to .agents/scratch/upstream-reports/parry-gjk-cast-relative-tolerance.md

1. Lines 14-17 (mechanism): replace relative-bound story with stagnation-exit mechanism (above);
   cite gjk.rs:770-781 for the never-firing eps_rel branch, gjk.rs:712-723 for the culprit.
2. Lines 9-10 title + 32-35 suggested fix: retitle around "upper-bound stagnation exit returns
   unrefined lower bound"; actual fix = refine stagnation exit from simplex witnesses.
3. Line 20 "SHORT (one-sided)": true at base; add that the fix makes error two-sided (up to
   +0.113 mm measured) — drop any one-sidedness reliance. Report line 40 "(TOI is provably never
   long)" is FALSE against post-fix parry — moot in practice: the tripwire (>10 mm at 500 m;
   post-fix 0.00024 mm) still fires correctly and retires the workaround.
