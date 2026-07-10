# Handoff — upstream determinism, wave A (local, end-to-end)

For the Codex agent working LOCALLY on Yan's machine. Written 2026-07-10, supersedes the
2026-07-10 cloud version of this file in full. Owner: Yan (yan@vikng.dev). Delete when consumed.

## Mission

Fork, patch, and test three upstream defects that Overmatch (Bevy 0.19 / avian / lightyear tank
PvP) has diagnosed — **end to end**: crate-level proof inside each fork, then game-level proof
through Overmatch's own harness with the corresponding workaround disabled. You produce patched
forks plus evidence; you do **not** publish.

Hard constraints:
- **No upstream issues, PRs, or comments; no public pushes.** Yan files everything himself later
  with dedicated attention. Forks stay private (or purely local clones — nothing needs to leave
  this machine).
- **Never touch the main checkout** at `~/Desktop/github/vikng-dev/personal/overmatch` — it is
  the merge point and other agents work there. All your Overmatch work happens in YOUR worktree
  (below). You never push or merge to `main` — the team lead does that.
- **No architecture opinions.** Overmatch stays server-authoritative with state replication;
  determinism is pursued as the property that makes corrections rare (the "rollback-killer"),
  NOT a step toward lockstep or an inputs-only wire.

## Environment setup (do this first)

1. **Your Overmatch worktree** (persistent — reuse it across sessions, its `target/` warms once):
   ```bash
   cd ~/Desktop/github/vikng-dev/personal/overmatch
   git worktree add ../overmatch-codex -b codex/integration
   ```
   All your Overmatch-side branches live under the `codex/` prefix. Commit early and often.
2. **Fork clones** (separate repos — NOT worktrees of Overmatch):
   ```bash
   mkdir -p ~/Desktop/github/vikng-dev/personal/vendor-forks && cd $_
   git clone <avian>    avian     # github.com/Jondolf/avian
   git clone <parry>    parry     # github.com/dimforge/parry
   git clone <lightyear> lightyear # github.com/cBournhonesque/lightyear
   ```
   **Check out the exact commit matching the crates.io release Overmatch pins** (from our
   Cargo.lock): avian3d **0.7.0**, parry3d **0.27.0**, lightyear + lightyear_prediction +
   lightyear_sync + lightyear_avian3d **0.28.0**, bevy 0.19.0. Find the release tag; if a repo
   has no matching tag, locate the version-bump commit and verify the crate's Cargo.toml version
   matches — and if the repo source at that commit differs from the published .crate, say so in
   your report. Branch per mission: `fix/solver-constraint-order`, `fix/cast-absolute-tolerance`,
   `fix/deferred-rollback-check`.
3. **Path overrides** — in your worktree's `Cargo.toml`, on `codex/integration`:
   ```toml
   [patch.crates-io]
   avian3d = { path = "../vendor-forks/avian/crates/avian3d" }
   # parry3d likewise when testing mission 2;
   # the lightyear family must be patched CONSISTENTLY — every lightyear_* crate Overmatch pulls
   # (lightyear, lightyear_prediction, lightyear_sync, lightyear_avian3d, ...) from the same
   # fork checkout, or cargo will mix registry and path versions and fail or, worse, half-apply.
   ```
   Keep each mission's override commit separate so A/B toggling is one revert.

## Machine etiquette (this box is shared and has 16 GB)

- **One cold build at a time, machine-wide.** Before any first build in a fresh dir:
  `pgrep -l rustc` — if another build is running, wait. A memory watchdog is armed and the team
  lead WILL kill runaway builds.
- **One game client+server pair at a time.** Before a harness run: `lsof -nP -i :5888` must be
  empty; if it isn't and the processes aren't yours, wait — never kill processes you didn't
  start. Clean up your own (`pkill -f overmatch` scoped to your worktree's target path) and
  re-check the port after.
- macOS has no `timeout`: background the client and `kill` it (see the harness recipes in
  `.agents/` docs). **Avoid `SPIKE_LATENCY_MS=0`** — known unresolved client hang at connect.

## The three missions

The authoritative briefs are in your worktree: `.agents/scratch/upstream-reports/` —
`avian-solver-constraint-order.md`, `parry-gjk-cast-relative-tolerance.md`,
`lightyear-check-starvation.md`. Read each fully; they carry mechanism, vendored file:line,
measurements, suggested fix shape, and trap warnings. Priority order as listed. Non-negotiable
discipline per mission: a **failing-before / passing-after test committed in the fork**, plus a
clean run of the crate's existing suite. Summary of the traps (details in the reports):

1. **avian constraint order** — the fix is geometry-derived (spatial-key) ordering of the
   manifold→color assignment, per avian PR #480's broad-phase precedent. Do NOT touch threading
   (the parallel step is order-invariant by construction; measured: disabling `parallel` changes
   nothing). Crate test: two Worlds, different entity spawn histories, identical geometry,
   persistent ≥2-manifold contact, ~100+ ticks → bit-identical angular velocity after the patch.
   Keep avian's own determinism CI test green. Measure/bound the sort cost.
2. **parry cast tolerance** — absolute (or hybrid) convergence bound; the report has the exact
   standalone repro (sphere r=0.5166 vs cuboids at 5/50/500 m half-extent; error must drop from
   ~139–172 mm @500 m to a documented sub-mm bound, no small-scale or perf regression). Decide
   deliberately about the early-return path (gjk.rs:713-724). Don't perturb witness computation.
3. **lightyear check starvation** — deferred re-check of receive-time-skipped samples (or
   inclusion in the completion-time scan); the sample is already stored in `ConfirmedHistory`.
   Stepper-style test: zero prediction margin, injected divergence → pre-patch: no rollback ever;
   post-patch: exactly one, at the right tick. Don't "fix" `balanced()` itself; don't
   double-check samples already checked at receive time.

## End-to-end validation (your worktree, workaround off + fork on)

General loop per mission: build `codex/integration` with the path override, **disable our
workaround for that mission**, run the harness (server + one headless scripted client, e.g.
`SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 SPIKE_SIM_LONG=1`, `SPIKE_TRACE=<path>` on BOTH ends —
role-suffixed files), and compare against the same run WITHOUT the fork patch. Baseline gates in
every run: `NAN-TRIPWIRE|FIXED-NAN|panicked|B0004` all zero. Tick rows carry per-tick
Position/Rotation/velocities on both ends with aligned tick numbers — write your own small
join/diff script (keep it in your worktree or scratch, not main's src) to compare client vs
server per tick. Note: the team is building a first-class divergence instrument (per-tick state
hash) on `main` — rebase your worktree onto main when it lands and prefer it.

Per mission:
1. **avian** — our workaround is NONE (the divergence is absorbed, not patched). The e2e
   signature: drive the multi-manifold wedge state (see the report; `SPIKE_SIM_*` scripts +
   the washboard/wedge course) and join tick rows. Unpatched: cross-World |Δav| ≈ 0.15 in the
   wedged state. Patched: bit-identical (or collapse by orders of magnitude — report the number).
   Flat-ground cruise must STAY bit-exact (it already is — regression guard).
2. **parry** — two signals. (a) The automatic tripwire: `tests/spherecast_scale.rs` in Overmatch
   MUST FAIL against your patched parry (it asserts the raw TOI defect exists) — that failure is
   the success signal; note it, don't "fix" the test on your branch. (b) Disable the workaround
   (the witness-reconstruction path in `src/driving.rs` — gate it off locally on your branch) and
   verify the at-rest idle metric the docs name (hull p.y spread ≲ 0.02 mm at rest) holds with
   raw TOI + your patched parry.
3. **lightyear** — disable `net/watchdog.rs` (gate its registration off on your branch). With
   UNPATCHED lightyear + watchdog off at low latency (e.g. `SPIKE_LATENCY_MS=10`, NOT 0), the
   starvation reproduces (runaway |Δp| divergence with zero rollbacks; skip-trace events count
   it). With your patch + watchdog off: divergence stays bounded (reference: the
   `SPIKE_INPUT_DELAY_TICKS=0` falsifier capped it at 0.015–0.57 m) and mismatches roll back.
   Report the skip/re-check event counts.

## Handback format

One report per mission: fork branch + SHAs; whether the report's mechanism held EXACTLY under
your repro (corrections matter more than confirmations — these become public filings);
crate-test names + run commands; existing-suite results; perf notes; the e2e A/B numbers
(unpatched vs patched, workaround off) with the trace evidence paths; where your fix deviates
from the report's suggested shape and why; anything that looks like a SEPARATE defect (report,
don't fix). Plus: the state of `codex/integration` (override commits, workaround-disable
commits) so the team lead can reproduce your A/B in one checkout.
