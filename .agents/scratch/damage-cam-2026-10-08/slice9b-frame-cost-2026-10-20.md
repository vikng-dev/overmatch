# Slice 9b — the pen cam's frame cost (MEASURED 2026-10-19/20)

Release, 2560x1440 physical, present mode Immediate, shadows M350, Apple M4 (10 cores, 16 GiB),
main `3f15af8` plus the marks (`log_presence`, `log_run`). Runner `scripts/perf/run-pencam-cost.sh`,
analyzer `scripts/perf/pencam_cost.py`; the conditions and gates are in the runner's runbook.
Raw streams and `*.cost.json` stayed in the session's scratchpad.

Per surface, p50 frame ms within ONE run: frame rows with the surface on screen (`on`) against
rows with no surface on screen (`quiet`); a row carries the frame that ended at its stamp, so a
window is `(start, end]`. "Same seconds" is the paired `-off` run: its rows in those windows
against its own quiet rows, the workload there without the surface. Net is the two subtracted
(DERIVED, a difference in differences). The MP rows stop at the kill cam run's start (the death
screen is another workload).

| surface | on | quiet | delta | same seconds, off run | net p50 | on-screen |
|---|---|---|---|---|---|---|
| shooter corner, offline (main gun hits) | 17.68 | 12.92 | +4.77 | +0.50 | ≈ +4.3 | 16.3 s, 873 frames |
| kill cam run, offline (hit beat + lesson + killer beat) | 13.29 | 11.56 | +1.73 | +0.31 | ≈ +1.4 | 7.1 s, 472 frames |
| the lesson alone, offline (the World showing) | 18.01 | 11.56 | +6.44 | +0.43 | ≈ +6.0 | 3.6 s, 190 frames |
| shooter corner, MP | 17.50 | 11.33 | +6.17 | −0.56 | ≈ +6.2–6.8 | 5.7 s, 294 frames |
| victim glass (corner inset), MP | 17.59 | 11.33 | +6.26 | −0.75 | ≈ +6.3–7.0 | 9.0 s, 474 frames |

The lesson's tails: mean +7.3 (same seconds +0.9), p95 +16.7 (+1.3), worst 72 ms at the run's
staging (stage spawn, mask images). The whole run's +1.4 is the lesson averaged with its two
cheap beats (the hit beat shows the world, the killer beat the killer's tank).

## Reads

- The corner re-measures at ≈ +4.3 ms p50; slice 5's +4.0 (17.64 vs 13.62) stands.
- The victim corner glass costs what the shooter corner costs: the inset's passes, not the content.
- The lesson — a full-viewport World-surface base look drawn by the player's camera — costs
  ≈ +6 ms p50 at 2560x1440. It is the structural proxy for the X world glass (same passes:
  two full-viewport masks, the look geometry in the main pass): DERIVED ≈ +6 ms p50, above the
  corner's +4.3 and above the earlier DERIVED +2.7; corner + glass at once DERIVED ≈ +10 ms
  (13 → 23 ms frames while both show). The queued optimizations (one `LookMaterial` per tier,
  half-resolution world masks) and the corner-wins suspension policy go to the owner with this
  number.

## Caveats

- MP windows are short: the opponent's scripted rounds knock the measured tank out at 22–31 s
  (MEASURED twice), and the scripted client never respawns. The MP schedule needs a non-lethal
  aim before it is worth more runs.
- Run-to-run baseline drift reaches ≈ 1 ms p50 (the two kill cam runs' quiet frames: 11.56 vs
  12.67), which bounds every across-run number; the within-run deltas are the firmer ones.
- The MG phase of the corner schedule opens no corner (automatic fire records nothing, by design).
- Pre-existing, seen again: slab allocator "use-after-free" errors at the lesson's staging and at
  cook-off; one `witness pass failed … a collider reached the world at a scale other than 1`
  ballistics error on a main-gun round (the `damage.rs` ~635 class).
