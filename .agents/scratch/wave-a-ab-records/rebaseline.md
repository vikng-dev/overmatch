# Wave-A rebaseline record — main @ 2a482c6, dev profile, 2026-07-10 evening

Standard condition: SPIKE_PERTURB=0 server, scripted client SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10.
All NaN/panic greps zero in every log (NAN-TRIPWIRE|FIXED-NAN|panicked|B0004).
Traces: this directory (long.*, short.*, short2.*, short3.*).

## Long course (SPIKE_SIM_LONG=1) — 1 run

- shared ticks 1262; hash match overall 94.45% (1192/1262); flat 93.87%, contact 95.89%
- mismatched ticks 70 — ALL hsim-only; first divergence tick 274 (sim)
- |Δp|/rot/|Δlv|/|Δav| all exactly 0 on every shared tick (physics bit-exact whole course)
- Matches §8 baseline character (91.71% then). LONG COURSE = the stable A/B regression gate.

## Short course (default) — 3 runs, BIMODAL (NEW vs §8 baseline)

| run | match overall | mismatch tally | first hsim | first hpos |
|-----|---------------|----------------|-----------|-----------|
| 1 | 39.63% (233/588) | pos=rot=lv=av=288, sim=355 | 193 | 493 |
| 2 | 82.50% (481/583) | sim=102 only | 205 | never |
| 3 | 38.72% (230/594) | pos=rot=lv=av=293, sim=364 | 196 | 497 |

- Bad mode (runs 1,3): persistent physics divergence from ~tick 493-497 ≈ the scripted FIRE
  edge, sub-threshold (|Δp| max ~1.95e-3 m, |Δav| max ~2.8e-2), never reconverges, never rolls
  back. Good mode (run 2): hsim-only, physics bit-exact — the old baseline character.
- §8 baseline (pre-aim-redesign, divergence-instrument branch) had physics bit-exact every
  shared tick on BOTH courses over the runs measured, and noted the fire-adjacent hsim term was
  not deterministic run-to-run. The aim point-commit redesign window (545fc65 lineage) is the
  prime suspect for the new fire-edge physics seeding, but attribution is NOT established here.
- Consequence for wave-A A/B: use the LONG course as the regression gate; short-course
  comparisons must be mode-aware (N≥3 runs, classify by tally: hsim-only vs physics). The
  "≤ rebaseline" bar on the short course means: good-mode runs comparable to run 2, and bad-mode
  incidence/magnitude not worse than 2/3 at ~2 mm.
- HANDED to the hsim-divergence session as a finding (see HANDOFF-NOTE-wave-a-session-live.md);
  wave-A does not investigate it.
