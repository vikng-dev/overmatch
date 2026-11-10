# Rest-energy lane report

Subject: rigid-body rest in the video game's server physics and destructible scenery; game state only.

Decision: keep one rest law, **the engine's original velocity test OR low kinetic energy per mass**. The original angular setting, default, serialization/reflection and readers are restored. The original test is the engine's own and is more permissive for large bodies; the energy clause accepts slender-body roll that angular speed alone misreads. The energy-only law from `cd73b7cd` is no longer the sole criterion. There is no runtime experiment selector.

DERIVED from the repeated captures below: Either reduced scheduled awake piece-ticks against Original by **55.1%** for the twelve-house recipe, **35.1% without house_0011**, and **0.8% for rest-3**. These are local count observations, not hosted CPU savings.

## DERIVED pooled totals from MEASURED captures

Both runs added; cells retain the same **first-sleep ticks / awake piece-ticks** units.

| Scheduled set, both runs | Original | Energy only | Either |
|---|---:|---:|---:|
| match-collapse | >12,095 / 397,305 | 6,151 / 217,837 | 5,199 / 178,479 |
| match-collapse without house_0011 | >8,664 / 220,437 | 5,338 / 179,254 | 4,415 / 143,154 |
| rest-3 | 1,095 / 34,230 | 1,202 / 35,513 | 1,197 / 33,948 |

## Where Either does not win

Either does **not** dominate both alternatives on every total. For rest-3, its pooled sum of first all-sleeping ages is **1,197 ticks**, against Original's **1,095** (DERIVED **+9.3%**); Energy-only is **1,202**. Its pooled awake piece-ticks are only slightly lower than Original's. In twelve-house R1, Either also loses to Energy-only: **102,652 versus 81,837** awake piece-ticks; without house_0011, **77,861 versus 72,708**. Rest-3 R2 likewise has a higher first-sleep sum and awake-piece total than Original.

Why this is possible: OR is a pointwise guarantee on identical body states, not a monotonicity law for complete collapses. Earlier sleep removes a solver body (`vendor/avian3d-0.7.0-child-pose/src/dynamics/solver/solver_body/plugin.rs:93`), changing subsequent integration and contacts. The recipe also dispatches from wall time (`src/net/loadtest.rs:346`), so event ticks and captured tick coverage vary. The captures demonstrate trajectory differences; they do not isolate which interaction caused each loss. The deterministic witnesses below establish the acceptance guarantee independently.

A player can see a slow roll stop sooner and can see different later settling or pile poses. No visual-equivalence claim is made. The owner's server-performance goal still requires hosted pricing; outcomes have not been pinned to these captures.

## Review claims checked

MEASURED in the earlier raw captures, an independent reconstruction confirms the review's eleven-pile first-sleep sums, **2,096 -> 2,530**, excluding house_0011. Their awake piece-ticks were **71,454 -> 87,908** (DERIVED **+23.0%**). The all-scenery linear-over-limit counter was **23,345 -> 27,322**. The DERIVED earlier **57.2%** all-scenery awake-piece saving was one censored capture, not a typical-pile estimate.

DERIVED mechanism: for a uniform box, transverse inertia per mass is `(length² + thickness²)/12`; the fixture's 6.6 m bar has an energy-only angular limit about **0.079 rad/s** at the engine's **0.15 m/s** rest speed, below its original **0.15 rad/s** angular limit. Combining translation and rotation into one energy budget can also reject a state passing both separate limits. The two original-limit regression witnesses fail under Energy-only and pass under Either.

The source conventions agree with the review: `prepare_solver_bodies` copies world angular velocity; computed inertia is local about the centre of mass, stored inverted; `rotated(q)` applies `R I^-1 R^T`; writeback rotates about the centre of mass. Physical inertia, rather than the lock-masked force response, remains the energy input. The inverse-frame mutant now fails the independent local-frame test.

## Count method

MEASURED setup: two fresh captures per law and recipe, all with **30 tanks and one client**, unattended, headless and muted. `scripts/perf/match-collapse.json` used a **100-second** capture; `scripts/perf/rest-3.json` used **60 seconds**. All runs were local `--rehearsal` counts through the machine queue; none prices hosted performance.

Only the sleep condition was swapped. Original uses the upstream separate velocity comparisons; Energy-only uses the predicate from `cd73b7cd`; Either uses their OR. Timer ownership, island commit, app composition, assets and recipe inputs are the same. The surrounding branch was not rebased.

First all-sleeping age is elapsed ticks from a construction's first piece birth. Reconstruct live membership from births/retirements and awake intervals from sleep episodes and their next awake starts. Apply all events at a tick before testing that at least one live piece remains and none is awake. Awake piece-ticks sum each piece's final `awake_ticks` once; sleep episodes are not summed again. `>N` marks a first-sleep event not observed through age N. In Original twelve-house R1, house_0008 and house_0011 remain censored; both are retained in totals as lower bounds.

Tables cover scheduled IDs. Coverage below exposes unscheduled scenery and unequal trace horizons. All raw records, including background pieces, remain available.

## MEASURED match-collapse

Cells are **first all-sleeping elapsed ticks / awake piece-ticks**. R1 and R2 are separate runs.

| Pile | Original R1 | Original R2 | Energy R1 | Energy R2 | Either R1 | Either R2 |
|---|---:|---:|---:|---:|---:|---:|
| house_0001 | 123 / 4,223 | 105 / 3,539 | 92 / 3,191 | 102 / 3,507 | 99 / 3,310 | 99 / 3,310 |
| house_0002 | 397 / 8,093 | 717 / 17,846 | 230 / 6,250 | 234 / 6,361 | 322 / 7,941 | 325 / 7,930 |
| house_0003 | 252 / 6,350 | 243 / 6,684 | 228 / 6,374 | 225 / 6,120 | 209 / 5,874 | 216 / 5,905 |
| house_0004 | 85 / 2,872 | 110 / 3,563 | 104 / 3,530 | 87 / 2,950 | 86 / 2,632 | 90 / 2,772 |
| house_0005 | 99 / 3,201 | 190 / 7,064 | 94 / 3,516 | 104 / 3,475 | 108 / 3,962 | 111 / 3,919 |
| house_0006 | 69 / 2,569 | 189 / 7,322 | 63 / 2,592 | 64 / 2,349 | 57 / 1,911 | 56 / 1,870 |
| house_0007 | 114 / 4,407 | 106 / 4,355 | 102 / 3,541 | 89 / 3,668 | 87 / 2,747 | 91 / 3,023 |
| house_0008 | >2848 / 18,455 | 583 / 22,659 | 203 / 7,266 | 634 / 23,423 | 189 / 7,798 | 144 / 5,558 |
| house_0009 | 393 / 15,337 | 100 / 3,669 | 445 / 16,381 | 600 / 22,004 | 410 / 13,999 | 250 / 8,730 |
| house_0010 | 348 / 17,793 | 377 / 14,123 | 357 / 11,093 | 306 / 10,333 | 283 / 9,883 | 301 / 8,603 |
| house_0011 | >2848 / 150,761 | 583 / 26,107 | 193 / 9,129 | 620 / 29,454 | 520 / 24,791 | 264 / 10,534 |
| house_0012 | 433 / 20,446 | 783 / 25,867 | 379 / 8,974 | 596 / 22,356 | 494 / 17,804 | 388 / 13,673 |
| **Total** | >8,009 / 254,507 | 4,086 / 142,798 | 2,490 / 81,837 | 3,661 / 136,000 | 2,864 / 102,652 | 2,335 / 75,827 |
| **Without house_0011** | >5,161 / 103,746 | 3,503 / 116,691 | 2,297 / 72,708 | 3,041 / 106,546 | 2,344 / 77,861 | 2,071 / 65,293 |

## MEASURED rest-3

Cells are **first all-sleeping elapsed ticks / awake piece-ticks**. R1 and R2 are separate runs.

| Pile | Original R1 | Original R2 | Energy R1 | Energy R2 | Either R1 | Either R2 |
|---|---:|---:|---:|---:|---:|---:|
| house_0001 | 138 / 4,425 | 128 / 4,528 | 95 / 3,297 | 117 / 3,789 | 100 / 3,518 | 120 / 3,897 |
| house_0002 | 231 / 7,653 | 255 / 7,234 | 236 / 6,356 | 256 / 10,993 | 214 / 6,631 | 305 / 9,152 |
| house_0003 | 249 / 7,472 | 94 / 2,918 | 246 / 5,959 | 252 / 5,119 | 215 / 5,381 | 243 / 5,369 |
| **Total** | 618 / 19,550 | 477 / 14,680 | 577 / 15,612 | 625 / 19,901 | 529 / 15,530 | 668 / 18,418 |

house_0011 is absent; totals with and without it are identical.

## MEASURED capture coverage

Ticks are complete trace rows. Other awake piece-ticks belong to unscheduled constructions and are excluded from the scheduled tables.

| Law / recipe | Captured ticks R1 / R2 | Other awake piece-ticks R1 / R2 |
|---|---:|---:|
| original / match-collapse | 4,750 / 5,822 | 0 / 0 |
| energy / match-collapse | 5,654 / 5,676 | 704 / 1,042 |
| either / match-collapse | 6,015 / 6,170 | 1,156 / 1,361 |
| original / rest-3 | 3,877 / 3,847 | 624 / 949 |
| energy / rest-3 | 3,713 / 3,892 | 0 / 32 |
| either / rest-3 | 3,856 / 3,852 | 913 / 0 |

## Regression evidence and gates

- The shipped-house deadline test is deleted. Its claimed motion bound did not constrain oscillation and was not a law invariant.
- The supported-bar witness uses Fixture one-tick long-axis roll at **0.4 rad/s every ten ticks**, matching the requested measured blocker scale. It asserts sleep within resource-owned `TimeToSleep` plus **two fixed ticks**: one exposes the initial fixed step and one permits the deferred sleeping commit. It passes under Either and fails under Original alone.
- A generic rotation and off-axis world angular velocity are checked against an independent local-principal-frame energy calculation. The inverse-rotation mutant fails; the vacuous locked-flags assertion is removed.
- Both the combined near-limit large body and the transverse see-sawing long bar rest under Either. Energy-only fails exactly these two witnesses.
- `MotionCounts`, its reference constant, island scratch/query and `Thresholds.angular_reference_rad_s` are deleted. The live recorder uses actual `Thresholds.angular` and records `last_fast` only when neither rest test passes. The existing `test_collapse.py` angular fixture is valid again because the field is restored.
- The debris, landing-threshold, sleep-field and solver-parameter prose is corrected; the vendor patch record describes the OR law.
- Final `cargo fmt --all --check`: passed. CI clippy: locked workspace, all targets, `dev_ui,bitprobe`, `-D warnings`, passed. Targeted rest/census/column tests, whole `headless_test`, and required `doc_citations`, `ui_ascii`, `net_boundary`, `net::protocol` guards: **100 passed, 6 ignored observations**. Final rest/census/whole-tree guard recheck after prose clarification: **58 passed**. Collapse-script tests: **42 passed**.
- The final diff was checked against all six review lenses; no backlog row names an edited file. Neither `src/tank/spawn.rs` nor `src/ballistics/motion.rs` is edited.

## Artifacts and remaining work

The fresh matrix is under `target/rest-energy-round2/<original|energy|either>-<match-collapse|rest-3>-<1|2>/`: `tick-attribution.json`, `rows.json`, `server.log` and the runner's recipe snapshot. `summary.json`, `tables.md`, `pooled.md` and `coverage.md` contain the reconstructed counts. `summarize.py`, `write_tables.py` and the captured `instance-ordinals.json` rebuild the tables from those records. Mutation failures and final gate logs are alongside them. The prior verification uses `target/rest-energy-before-scene/` and `target/rest-energy-after-scene/`. No temporary binary/symlink folder was used.

Remaining: orchestrator merge and hosted timing. Set these two variables to the existing deployment and run the same twelve-house recipe without `--rehearsal`:

```sh
scripts/perf/collapse.sh --target "$OVERMATCH_TIMING_TARGET" --deployment "$OVERMATCH_TIMING_DEPLOYMENT" --schedule scripts/perf/match-collapse.json --tanks 30 --clients 1 --duration 100
```
