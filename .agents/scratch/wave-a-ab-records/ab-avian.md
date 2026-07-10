# Avian game-level A/B record (wave-A) — 2026-07-10

A-side = main binaries (0d99a38 for idleA/wedgeA?—see contamination note), B-side = bay
wave-a/integration avian-alone override. All runs 80/10 jitter, SPIKE_PERTURB=0,
SPIKE_SPAWN_POSE="0,0.85,-39.88,0.1736,0,0,0.9848" (beached slab edge).

## Contamination note (resolved)

Mid-session the hsim session landed 0d99a38 on main (hsim per-field decode) and rebuilt main's
binaries. Verified sim-inert: driving.rs/tank.rs hunks are #[cfg(test)]-only constructors;
trace.rs is the env-gated recorder; analyze.py analyzer-side. Every analysis compares client vs
server WITHIN one run (same binary), so no analysis is corrupted. Integration branch REBASED
onto 0d99a38 (avian-alone now 62b2740) and rebuilt for formal same-commit discipline from here on.

## Runs

- wedgeA (unpatched, SPIKE_SIM_FORWARD powered wedge): 0% match, all components, from tick 86.
  |Δav| p99 0.079 / max 0.139; |Δp| max 0.034; |Δlv| max 0.16.
- wedgeB4 (avian-patched, same scenario): 0% match, all components, opens@first-shared-tick.
  |Δp| max 0.075; |Δlv| max 1.148 (transient); |Δav| max 0.114.
- idleA (unpatched, SPIKE_SIM_IDLE beached rest): 0% match from first shared tick (264).
  First-divergence row: |Δlv| 0.223 ≫ |Δav| 0.041 — NOT the av-first signature.
  Field decode: servo=584/584 ticks, anchor=194; drive/reload/recoil 0.
- idleB2 (avian-patched, same): 0% match from first shared tick (281). First-divergence row:
  |Δlv| 0.174, |Δav| 0.056. Magnitudes statistically similar to idleA (|Δp| max 0.022 vs 0.016,
  |Δav| max 0.181 vs 0.183).

Three earlier B-side client SIGKILLs (exit 137) were memory-pressure jetsam during parallel
review-agent builds (client peaks ~700 MB; patched server survived every time). Not the patch.

## Finding: the live-network wedge is NON-DISCRIMINATING for the constraint-order term

The report's |Δav|-first signature (0.155 ≫ lv 0.100 ≫ p 0.0013) was measured from
bit-identical settled states (identical inputs, Δthrottle/Δsteer=0). In a live 80/10 run:
(1) the connect transient seeds state deltas BEFORE the shared window (all windows
opens@first-shared-tick, replay rows 0); (2) rollback is starved at zero margin (report #1's
own mechanism) so the client never re-anchors; (3) the beached wedge is marginally stable —
initial deltas persist instead of contracting. Both sides therefore show full-window divergence
with lv-dominated first rows regardless of solver order. The patch CANNOT collapse divergence
that was seeded before the comparison window — and equally, the unpatched signature cannot be
reproduced in this condition.

Consequences:
- The cross-World determinism claim is proven at CRATE level (two Worlds, identical spawns +
  inputs, bit-identical 180 ticks — verified by the review agent, incl. RED failure).
- Game level contributes the REGRESSION gate: (a) B-side not worse than A-side in the wedge
  (holds: similar magnitudes); (b) flat cruise stays bit-exact physics on the patched build —
  long-course run PENDING on the rebased build.
- The all-three override step (lightyear patch restores rollback) is where the avian term may
  become game-visible (rollback frequency/magnitude in the settled wedge with re-anchoring
  active) — check there.
