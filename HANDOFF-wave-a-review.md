# Handoff — wave-A review: Codex's upstream patches, verified against our game

2026-07-10. For a DEDICATED review session, Yan participating. Delete when consumed.
Predecessor docs: `HANDOFF-upstream-determinism-wave-a.md` (the mission Codex executed — retired,
see git history at ca08668/beb79f4 for its full text) and
`.agents/docs/design/sim-divergence-and-determinism.md` §8 (the instrument + baseline).

## Mission

Codex (external agent) patched three upstream defects on private forks, crate-level tested, per
our reports in `.agents/scratch/upstream-reports/`. This session: (1) adversarially review the
three patches, (2) run the game-level A/B with the divergence instrument, (3) decide per item
whether our workaround retires, (4) produce the corrected filing packet for Yan (he files issues
+ PRs personally; NOTHING is published by this session), (5) write the adoption decision memo.

## Codex's handback (verbatim facts; treat claims as UNVERIFIED until re-run)

- **avian** `fix/solver-constraint-order`, HEAD `cd6bdaf`, RED checkpoint `bc7165d`. Mechanism
  held (entity-keyed coloring order). Fix: canonical solve order (world contact position, then
  normal) rebuilt only on topology change; parallel kept; entity-keyed bookkeeping retained.
  Test `multi_manifold_solver_is_deterministic_across_worlds` (`cargo test -p avian3d ...`):
  bit-identical 180 ticks (≥100 multi-manifold). Suite 68/68 (one pre-existing ARM sub-ULP
  rasterizer failure excluded). **2D cross-platform determinism expected hash CHANGED to
  0x3126af7d** — semantically loaded: the patch changes canonical results, not just our worlds;
  the eventual PR must call this out. Perf: ~6.8 ns/step (wedge); stress 11.5k constraints →
  1.47 ms canonicalization on topology change, whole-step +0.37%.
- **parry** `fix/cast-absolute-tolerance`, HEAD `db36bd0`, RED `e7f61fc`. **MECHANISM CORRECTION
  to our report:** the relative-gap convergence branch NEVER FIRES; the real culprit is the
  "last chance" upper-bound-stagnation exit returning the current lower TOI bound prematurely.
  Fix refines that exit using the simplex witness separation projected along the cast direction;
  witness/normal generation untouched. Errors 5/50/500 m: 0.246/3.637/139.448 mm → 0.113 mm/
  0.0023 mm/0.00024 mm. Test `shape_cast_toi_accuracy_does_not_scale_with_shape_extent`. Suites
  83/83 + 89/89 + f64/fmt clean. Added work only on the rare stagnation exit (≤4 weighted
  witness accumulations).
- **lightyear** `fix/deferred-rollback-check`, HEAD `d3ee2be`, RED `6b6ec12`. Mechanism held
  exactly. Fix: per-component deferred marker recorded only when the receive-time check skips;
  compared ONCE when local prediction passes the tick and replicon completed it; marker removed.
  `balanced()` untouched. Pre-fix: stored mismatch + zero rollbacks; post-fix: exactly one
  rollback, no double-fire. Test `test_future_confirmed_mismatch_is_checked_once_when_prediction_passes_it`
  (`-p lightyear_tests`). Rollback 28/28, prediction 19/19, sync 9/9, clippy all-features clean.
  Full suite 155 pass / 3 ignored / **1 separate baseline failure: `test_replicate_transform_child_collider`
  expects x=4.0, gets ~2.0, identical with and without the patch** — POSSIBLY independent
  upstream evidence for our child-collider reports (#2 blanket ApplyPosToTransform / #5 BVH
  restore); investigate and, if related, add to those report files. Also pre-existing formatting
  drift in `crates/inputs/input_bei/src/plugin.rs` (noise; note for the PR).

All three built from our exact pinned release tags (avian3d 0.7.0 / parry3d 0.27.0 / lightyear
0.28.0), pushed to private forks, RED→GREEN test discipline per the mission.

## Locations (CONFIRM WITH YAN AT SESSION START — do not guess)

- Fork remotes/URLs and any local clones: Yan has them (Codex managed its own checkout; a Codex
  worktree exists at `~/.codex/worktrees/5e16/overmatch`, detached @ 62da9bd — inspect its
  cargo config / git remotes for pointers, but Yan is the authority).
- Overmatch main @ `545fc65` (includes the aim point-commit redesign AND the divergence
  instrument). Warm agent worktree: `../overmatch-bay-1`.

## Review discipline (per item)

Diff RED checkpoint → HEAD, not just HEAD. Verify mechanically: (a) the trap list from each
report file (avian: no threading changes, order geometry-derived, avian's own determinism CI
green; parry: witness path untouched — diff it; lightyear: no double-check of receive-checked
samples, `balanced()` untouched); (b) minimality — every changed line traces to the mechanism
(these become PRs under Yan's name); (c) re-run the named tests + suites yourself, foreground;
(d) the failing-before proof: check out RED, confirm the test fails for the STATED reason.
Corrections outrank confirmations — anything that contradicts Codex's report or ours gets
written down precisely.

## Game-level A/B (the instrument is the acceptance harness)

Read design doc §8 first (fields, analyzer usage, baseline table, and its stated limits).

0. **REBASELINE on current main (545fc65)** before any fork integration — the recorded baseline
   predates the aim redesign; A/B must compare same-commit runs. Standard 80/10 runs, both
   courses, `scripts/divergence/analyze.py`.
1. **Integration branch** in the bay worktree: `[patch.crates-io]` overrides — avian alone, then
   parry alone, then lightyear (the whole lightyear_* FAMILY from one fork checkout — mixing
   registry and fork versions half-applies), then all three. Keep each override its own commit
   for one-revert A/B toggling.
2. **avian**: the baseline courses are NON-DISCRIMINATING (§8 — they never enter the wedge
   state). Reproduce the wedge scenario the constraint-order report measured (multi-manifold
   hull contact — see that report's methodology; build a SPIKE_ course/scenario if none exists).
   Unpatched: cross-World |Δav| ≈ 0.15 signature in the wedge. Patched: collapsed — report the
   number. Flat cruise must STAY bit-exact.
3. **parry**: (a) `tests/spherecast_scale.rs` must FAIL against the patched fork — that failure
   is the designed retirement tripwire, the success signal; (b) gate off the witness
   reconstruction (`sphere_cast_ground_contact`, src/driving.rs) on the integration branch and
   verify the at-rest idle metric (hull p.y spread ≲ 0.02 mm) and no limit-cycle return, plus a
   full-course divergence run at or below baseline.
4. **lightyear**: gate off `net/watchdog.rs` on the integration branch. Unpatched + watchdog
   off at LOW latency (SPIKE_LATENCY_MS=10, NEVER 0 — unresolved connect hang): the starvation
   reproduces (runaway divergence, zero rollbacks, skip events counting). Patched + watchdog
   off: bounded divergence, mismatches roll back (reference falsifier bound: 0.015–0.57 m).
   Report skip/recheck counts. Then the retirement per report #1: watchdog removed or demoted to
   debug-assert.
5. Every run: NAN-tripwire/panic greps zero; one client pair at a time; port 5888 clean;
   foreground builds only; check `pgrep -l rustc` before cold builds (Yan's aim session may be
   building too).

## Deliverables

1. **Per-item verdict**: patch sound (with the RED→HEAD review), crate tests re-verified, A/B
   numbers vs rebaseline, workaround retire/keep/demote decision with the evidence.
2. **The corrected filing packet** for Yan: updated report files in
   `.agents/scratch/upstream-reports/` — the parry mechanism correction is MANDATORY before
   filing; the avian 2D-hash-change callout; the lightyear child-collider baseline failure
   cross-referenced into reports #2/#5 if the investigation supports it. Each report keeps its
   file:line evidence style.
3. **Adoption decision memo** for Yan: whether to land `[patch]` overrides on main now. The
   agreed bar: solo hash-mismatch ≤ rebaseline with the corresponding workaround off. Plus the
   operational fork: local path overrides DO NOT work in CI/releases — adoption on main needs
   git-URL patches to reachable forks (private repo + auth for CI, public forks, or vendoring)
   — lay out the options, Yan decides. Note interaction: task #26 (feature-gate collapse) is
   PENDING and rebuilds everything — coordinate ordering.
4. Board updates + a dated §9 in the design doc if the A/B changes the divergence picture.

## Constraints

No publishing (no PRs/issues/comments — Yan files with dedicated attention). No pushes to main
except the report-file updates and doc sections (gates green, story commits). The instrument and
its baseline are the arbiter — numbers, not narratives, retire workarounds.
