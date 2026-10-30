# Shadow pricing by caster class and filter, 2026-10-30 (#169)

Apple M4, 2560x1440, offscreen (`OFFSCREEN=1`), binary `bin/shadow-bcc104c` (profiling build of `bcc104c`), shipped settings (3 cascades, 4096 maps, 350 m, Gaussian filter). Each lever is one A/B/B/A block of 60 s runs, taken by day between lane compiles under the build lock. Runs: `runs/shadow-classes/`, `runs/shadow-filter/` (gitignored). Per-condition percentiles are MEASURED; each Δ = mean(lever-a, lever-b) − mean(base-a, base-b), DERIVED.

## Δp50, ms

| Lever | 2 tanks | 30 tanks |
|---|---|---|
| terrain casters off | **−0.52** | **−0.29** |
| tank casters off (hull, turret, parts, track ribbons) | −0.10 | **−0.47** |
| tree casters off | −0.14 | +0.05 (noise) |
| house casters off | +0.13 (noise) | +0.07 (noise) |
| filter Gaussian → Hardware2x2 | **−0.61** | **−0.36** |
| filter Gaussian → Temporal | +0.06 (noise) | 0.00 |
| all shadows off (night battery, for reference) | −2.14 | −3.05 |

## Reading

- Casters account for ~0.6 ms at 2 tanks and ~0.75 ms at 30 (DERIVED sum; additivity ASSUMED). Most of the shadow cost is not in what casts.
- The filter is a look trade, not an optimization: Hardware2x2 gives blockier penumbrae. It belongs to the presets (#159) as a LOW candidate, with a side-by-side.
- Residual after casters and the filter (DERIVED): ~0.9 ms at 2 tanks, ~1.9 ms at 30. Candidates: the fixed per-cascade pass cost (cascades 3→2 saved 0.46 / 0.72 at night) and the main pass's cascade sampling. Cascade splits and overlap are still unpriced (`scripts/perf/run-shadow-pricing.sh`, tonight).
- Ceilings for the planned work: #171 tank proxies ≤ 0.47 ms at 30 tanks; #170 terrain bake ≤ 0.52 ms at 2 tanks / 0.29 at 30; trees and houses in the bake ≈ 0.
