# Wave-A adoption decision memo — for Yan (2026-07-11)

The wave-A review session's final deliverable. Evidence: `.agents/scratch/wave-a-ab-records/`
(review verdicts + A/B records; raw traces in the session scratchpad, ask if needed). Updated
report files: `.agents/scratch/upstream-reports/` (all corrections landed). Nothing published —
you file. Bay worktree branches (kept for you): `wave-a/integration` (3 stacked overrides,
rebased on d7d103e), `wave-a/parry-alone` (+SPIKE_RAW_TOI lever), `wave-a/ly-gate`
(+SPIKE_NO_WATCHDOG lever), `wave-a/ly-alone`.

## Per-item verdict and adoption decision

### avian `fix/solver-constraint-order` — patch SOUND; adopt WHEN WANTED, file AFTER hash fix

- Crate review: sound, minimal, RED proof genuine (order-1 Gauss-Seidel divergence at tick 1 —
  stronger mechanism framing than our report had; report updated). **BLOCKING for filing: the
  2D determinism hash constant is calibrated against the wrong feature set** — under avian CI's
  exact features the patch produces `0x4fa858dc`, not the committed `0x3126af7d`; as submitted
  their CI fails. Re-derive + confirm on the PR's own CI. Three disclosure items for PR text
  (tie-break residue, global dirty-flag rebuild + unverified perf numbers, sort-key panic edge,
  semver private-fields note) — all in the updated report file.
- Game level: flat-cruise long course bit-exact physics with the patch (94.69% vs 94.45%
  baseline, hsim-only residue) — no regression. The wedge signature is NOT re-measurable in a
  live jittered run (documented why in ab-avian.md); the crate test is the proof.
- **Class-3 attribution result (hsim handback): the avian patch does NOT fix class-3** —
  identical 0.230 mm fire-edge seed on patched and unpatched builds, incidence 2/12 vs 1/12
  (n.s.). Class-3 goes back to the divergence track pointing at contact-restore/BVH (#5 class)
  or the shell-spawn path. The determinism-enabler case for the patch is unchanged; the hope it
  would kill class-3 is dead.
- **Adopt on main?** No urgency from class-3 anymore. Value is strategic (predict-both enabler)
  — adopt when the determinism plan calls for it, or simply when it ships upstream.

### parry `fix/cast-absolute-tolerance` — patch SOUND; RETIRE workaround on upgrade; file freely

- Crate review: sound; our report's mechanism was WRONG and is now corrected (stagnation exit,
  not eps_rel — instrumented 0/2400 vs 2278/2278). One property change to disclose: TOI error
  becomes two-sided (overshoot ≤ +0.113 mm measured); suites clean with `--tests` (bare
  `cargo test` hits a pre-existing example compile error at the tag — don't claim bare-clean).
- Game level: tripwire test FAILS against the fork exactly as designed (retirement condition
  met); at-rest idle with raw TOI + patch ≡ workaround-on (per-tick |dy| p99 0.027 vs
  0.029 mm, no limit cycle); long course at baseline (94.85%, physics bit-exact).
- **Adopt on main?** Not as a `[patch]` — wait for a parry release containing the fix, then
  retire `sphere_cast_ground_contact`'s reconstruction (the tripwire will flag it). The
  workaround is cheap and correct meanwhile.

### lightyear `fix/deferred-rollback-check` — mechanism VALIDATED, but DO NOT adopt / file with a required guard

- Crate review: marker lifecycle exactly-once verified; balanced() untouched; suite counts
  reproduce single-threaded only (parallel-flaky at the tag, pre-existing — disclose).
- Game level, where the edge doesn't fire, the fix WORKS: wedge at lat10 watchdog-off held
  |Δp| p50 3.4 / p99 21.9 mm with mid-run rollbacks vs unpatched 17.6 / 47.6 mm and zero
  post-connect rollbacks (starvation demonstrated raw: 55.3 mm above the bar, 2 connect-only
  rollbacks).
- **DISQUALIFIER, measured: rollback-to-future-tick.** At zero-margin connect a backward
  SyncEvent leaves deferred markers above the local tick; consumption rolls FORWARD ~280 ticks;
  3/4 flat-course runs ballooned to 7.5 GB and died, the 4th ran permanently ~6,400 ticks
  ahead. The adversarial review predicted exactly this edge. Fix is small (clamp consumption to
  `min(mismatch_tick, current_tick − 1)` or drop markers on backward sync) — require it in the
  PR (or push a follow-up commit to the fork first).
- **Watchdog: KEEP.** Retirement condition = guarded fix ships upstream AND re-passes the
  ab-lightyear A/B (branches + levers are ready to re-run).

## Operational: how adoption would land when we do it

Local path overrides don't work in CI/releases. Options, in my order:
1. **Public forks under vikng-dev + git-URL `[patch]`** — zero auth complexity, works in CI and
   release workflows, and the forks must go public anyway to back the upstream PRs. Pin by rev.
2. Private forks + auth token in CI — secrets churn in the release pipeline (the tag→full
   release path builds Windows too), more moving parts, no benefit once 1 is needed anyway.
3. Vendoring — heaviest diff, breaks `cargo update` hygiene; only if forks must stay private.
Ordering with task #26 (feature-gate collapse, PENDING, rebuilds everything): land #26 first,
then any `[patch]` adoption rides one rebuild; doing it in the other order rebuilds twice.

## Filing sequence for you (nothing published by this session)

1. parry: file issue + PR now (report file is filing-ready; fork branch needs pushing public).
2. avian: fix the hash constant on the fork (re-derive under CI features), then file; call out
   the canonical-results change prominently.
3. lightyear: add the future-tick guard to the fork (small), re-run ab-lightyear A/B (branches
   ready), then file with both the crate test and the guard test.
4. lightyear bonus: file the ChildOf/Transform-mode report (#9, upstream's own failing test —
   cheapest issue of the batch).
5. Suggested per-issue shape: mechanism with their file:line + two-command RED/GREEN repro on
   the public fork + before/after numbers table + our game-level instrument numbers as the
   real-world tier + video only for lightyear (and optionally parry limit-cycle) — per the
   "hands-on, reproducible, no slop" plan.
