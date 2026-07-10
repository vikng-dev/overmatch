# Handoff — upstream determinism, wave A

For the external agent (Codex, cloud environment). Written 2026-07-10. Owner: Yan (yan@vikng.dev).
Delete when wave A is consumed.

## Mission

Fork, patch, and test three upstream defects that Overmatch (a Bevy/avian/lightyear tank PvP
game) has diagnosed and worked around. You produce **patched forks with proof**; you do **not**
publish. Explicitly out of scope for you:

- **No upstream issues, PRs, or comments.** Yan files those himself later, with dedicated
  attention, using your results. Anything you push must be to **private** forks.
- **No changes to the Overmatch repo.** Your deliverable is entirely in the three forks.
- **No architecture work.** Overmatch stays server-authoritative with state replication;
  determinism here is pursued as the property that makes client-side corrections rare
  (the "rollback-killer"), NOT as a step toward lockstep or an inputs-only wire. If a patch
  seems to want an architecture opinion, stop and report instead.

## The three targets

The authoritative briefs are the three report files in this repo — read them first, they are
short and carry mechanism, vendored file:line citations, measurements, the suggested fix shape,
and the trap warnings:

1. `.agents/scratch/upstream-reports/avian-solver-constraint-order.md` — **avian3d 0.7.0**
   (github.com/Jondolf/avian). Solver constraint accumulation order derives from entity index →
   cross-World non-determinism for multi-manifold bodies. THE strategic item; do it first.
2. `.agents/scratch/upstream-reports/parry-gjk-cast-relative-tolerance.md` — **parry3d 0.27.0**
   (github.com/dimforge/parry). Shape-cast TOI relative tolerance → ~200 mm one-sided error vs
   large colliders.
3. `.agents/scratch/upstream-reports/lightyear-check-starvation.md` — **lightyear 0.28.0**,
   crates `lightyear_prediction` + `lightyear_sync` (github.com/cBournhonesque/lightyear).
   Rollback mismatch checks silently skipped forever at zero prediction margin.

## Fork mechanics

- Fork each upstream repo **privately**; create one branch per item (e.g.
  `fix/solver-constraint-order`, `fix/cast-absolute-tolerance`, `fix/deferred-rollback-check`).
- **Patch against the exact versions Overmatch pins** (from our Cargo.lock): avian3d **0.7.0**,
  parry3d **0.27.0**, lightyear/lightyear_prediction/lightyear_sync **0.28.0**, bevy **0.19.0**.
  Check out the matching release tag/commit in each repo and branch from there — NOT from main.
  (Overmatch will consume the branches via `[patch.crates-io]`; a patch against main is
  unusable to us. Rebasing onto main is a later, separate step for the eventual PRs.)
- Keep each patch minimal and surgical. These will become upstream PRs under close review;
  every changed line should trace to the report's mechanism.

## The non-negotiable test discipline

Every patch ships with a **failing-before / passing-after** test committed in the fork, plus a
clean run of the crate's existing test suite. A patch without its repro test is not done.

**Item 1 — avian constraint order.** Repro test: two independent ECS Worlds ("server" and
"client"), identical geometry and scripted forces, but **different entity spawn histories** (spawn
and despawn some dummy entities in one World first so entity indices differ — our measured case
was index 4294966669 vs 4294966650 for the same logical body). Drive a single dynamic body into a
**persistent ≥2-manifold contact state** (the report's case: a hull wedged on a slab edge) for
~100+ ticks. Before the patch: angular velocity diverges from the first settled contact tick
(measured ordering: |Δav| ≫ |Δlv| ≫ |Δp|). After: **bit-identical** across Worlds every tick.
Fix shape per the report: make manifold→color assignment / per-color accumulation order derive
from a stable **spatial key** (contact world position, then normal) — the same remedy avian
PR #480 applied to the broad phase. **Traps:** (a) do NOT touch threading — the parallel step is
order-invariant by construction and rebuilding without `parallel` provably changes nothing; the
defect is the serial, entity-index-keyed coloring; (b) single-World determinism is already
bit-exact (140/140 measured) — your patch must not regress that, and avian CI enforces
cross-platform determinism with parallel ON (2D test) — keep it green; (c) measure or bound the
sort's perf cost per solve and report it.

**Item 2 — parry cast tolerance.** The report contains the exact standalone repro: avian's
`cast_shapes` arrangement, sphere r=0.5166 cast at cuboids of half-extent 5 m / 50 m / 500 m.
Before: one-sided short error 0.25 mm / 3.6 mm / 139–172 mm. After: an absolute (or hybrid
absolute+relative) convergence bound holding error to a documented sub-millimeter figure at all
three scales, with **no precision or perf regression at small scales** (run parry's full query
test suite). **Traps:** (a) the early-return "upper bounds inconsistencies" path
(gjk.rs:713-724) returns the current lower bound — decide deliberately whether it also needs the
absolute bound, and say so; (b) the witness data (`point1`/`normal1`) is exact even when TOI is
wrong — do not perturb witness computation; (c) Overmatch carries an automatic tripwire
(`tests/spherecast_scale.rs`) that FAILS when this is fixed — expected, that's our retirement
signal, not your concern.

**Item 3 — lightyear check starvation.** The defect: `write_history::<C>` skips the rollback
comparison when `confirmed_tick >= current_tick` (registry.rs:426-428) and **no deferred re-check
exists**; the completion-time scan excludes always-confirmed entities (rollback.rs:583). At zero
prediction margin (`InputDelayConfig::balanced()` at LAN RTT) every update skips → rollback
permanently, silently dead (measured: 35–50 m divergence, 3,296 skip events, zero rollbacks).
Fix shape (report offers two; pick and justify): (a) deferred re-check — the skipped sample is
already stored in `ConfirmedHistory`; re-run the comparison once the local tick passes it; or
(b) include receive-skipped entities in the completion-time scan. Repro test: lightyear has
stepper-style client/server test infrastructure — build a case with input delay absorbing all
RTT (zero margin), inject a divergence between confirmed and predicted state, assert pre-patch
that no rollback fires and post-patch that it does, exactly once, at the right tick. **Traps:**
(a) the bug is in the check machinery — do not "fix" `balanced()` itself; documenting the
zero-margin regime is a bonus, changing sync behavior is out of scope; (b) beware double-firing:
a sample checked at receive time must not be re-checked later; (c) our repo-side workaround
(`net/watchdog.rs`) fires after 3 consecutive breaching samples with per-component thresholds —
your fix supersedes it only if a single genuine mismatch triggers exactly one rollback.

## Handback format

One report per item: branch + commit SHAs; whether the report's mechanism held exactly under
your repro (if reality diverged from the report ANYWHERE, say so precisely — these reports
become public filings and corrections matter more than confirmations); test names and the
command to run them; existing-suite results; perf notes (item 1 especially); and where your fix
deviates from the report's suggested shape, why. Flag anything you found that looks like a
SEPARATE defect — do not fix it, report it.

## What happens on our side (context, not your work)

Overmatch consumes your branches via a `[patch.crates-io]` integration branch and validates
game-level with a divergence instrument (per-tick state hash, client vs server): acceptance for
"workaround retired" is solo-play hash-mismatch at or below current baseline with the
corresponding workaround disabled. Cross-platform (x86 Linux ↔ ARM macOS) claims are validated
on our side too — your cloud box only proves cross-World, which is the main prize and
architecture-independent. Yan files the issues and PRs himself afterward, rebasing your branches
onto upstream main as needed.
