# Island settling — T4

Subject: the video game's server physics and environment destruction; CPU and game state only.

**Decision:** keep the generic library correctness fix, with unchanged sleep thresholds and time to sleep. Do not declare the full settling/performance objective complete: the ram observation is adverse and the independent pile still moves. No debris freeze, timer or body-kind exception was added. The [upstream draft](../../../upstream/avian-island-rest-history.md) owns the mechanism and minimal reproductions; [replay recipes](evidence/README.md) own the commands.

## Local observations

**MEASURED on this Mac:** arm64, macOS 26.5.2, 16 GiB RAM, Rust 1.98.0. Before/after use identical recipes and feature sets. Native and independent fixtures use `edit`; production server and client use `dev`, both with the declared optimization profile unchanged. These are one local pair of runs, not server capacity or CPU prices.

The table shows median awake / asleep pieces over the final twenty recipe seconds. Native coordinates are nominal fixture-loop seconds; production coordinates are wall seconds. [summary.json](evidence/summary.json) includes every period, counter totals, end-island membership and collection completeness.

| Recipe | Before awake / asleep | After awake / asleep | End population before / after |
|---|---:|---:|---:|
| Native collapse | 146.5 / 43.5 | 0 / 125 | 191 / 125 |
| Independent pile | 944.5 / 55.5 | 941 / 59 | 1,000 / 1,000 |
| Production shelling | 20 / 33 | 0 / 52 | 53 / 52 |
| Production ram | 96.5 / 50.5 | 121 / 50 | 147 / 171 |

**The ram is the adverse finding:** more awake pieces and more total pieces after the fix. Independent-pile wake requests also increase. These outcomes are not hidden behind the native and shelling successes. Different emergent populations make an equal-population cost claim invalid; pricing is still required. No output count is pinned in a test.

**MEASURED totals**, before → after. Timer resets are assignments, not unique bodies; counters cover all physics bodies.

| Recipe | Contact requests | Wake requests | Timer resets | Already-awake timer resets |
|---|---:|---:|---:|---:|
| Native | 1,371 → 40 | 438 → 73 | 12,010 → 19 | 10,672 → 0 |
| Independent pile | 17,137 → 24,974 | 562 → 1,565 | 442,642 → 1,953 | 441,927 → 0 |
| Shelling | 1,468 → 1,779 | 293 → 211 | 5,357 → 245 | 4,873 → 0 |
| Ram | 1,131 → 1,578 | 337 → 845 | 21,587 → 1,302 | 21,217 → 0 |

This contact counter includes contact losses and constraint activation; it is not the hunt's narrower touching-start-only counter. The paired definitions here match.

## What remains awake

**MEASURED:** the native fixture first reaches its final wholly sleeping population at nominal recipe second 22 and ends asleep. The independent after run ends with 934 awake / 66 asleep. Its largest persistent island has 925 members: 27 above a sleep threshold and 650 awake members with mature timers. Before, the largest island had 950 members, 36 above threshold and no mature timer. Preserving rest history is proved; whole-island rest is not achieved.

A separate fixed-step pose census reproduced every common independent-pile census row. **DERIVED from its measured poses** over the final recipe second: maximum displacement within the largest island is approximately 0.0725 m; maximum rotation is approximately 0.3282 rad. Actual motion remains, so forcing that island asleep would bypass the unchanged eligibility law.

The explanatory ram repeat ends at 119 awake / 29 asleep, with one above-threshold member in its final piece census and no non-piece physics bodies present. It does not support an attached-vehicle explanation at that endpoint. Its population differs from the primary after run, so it does not replace the adverse matched observation. The pose summaries are retained separately in the JSON.

The binding lane rules explicitly prohibit tick/frame pricing even when a brief requests it. Tick and physics milliseconds were therefore not measured; lower counter totals are not claimed as lower CPU cost. The orchestrator must price this patch and retain the ram and large-pile findings in T4's disposition.

## Verification

**MEASURED:** both initial witnesses fail on the unpatched library. The stationary coplanar contact wakes its sleeper; a merge clears an already-awake body's accumulated rest interval. The stale-ID mutant also fails the strengthened witness before the solver; the restored implementation passes it. The fixture derives a non-touching speculative gap from the library's contact tolerance and creates that pair before the later sleeping bridge, making the ordering case explicit.

PASS: workspace formatting; CI-feature workspace/all-target clippy with warnings denied; 53 targeted/headless tests, 4 ignored; all 48 whole-tree guard tests (`doc_citations`, `ui_ascii`, `net_boundary`, `bake::`). The targeted run includes the collider/material one-rule gate, ordinary finite-mass impacts, support removal/move/disable, dormant contact bookkeeping, explicit wake timer reset and the merge-before-solver witness. The full suite was not run.

The initially broad headless filter completed successfully before its attempted narrowing; only the duplicate queued rerun was cancelled. The first shell collector attempt encountered an incomplete JSON line and was excluded; the buffered collector completed all four primary runs without incomplete trailing windows. Both ram clients completed normally. All game/test processes were reaped. Raw build/check logs and excluded attempts remain under `target/settle-islands/`.

Permanent changes were checked against all five lenses; no backlog row names their files. All temporary runtime probes and the stale-ID mutant are removed. The sleep defaults, assets, game laws and wire formats are untouched. The library fix is reviewable independently; T4 remains open for the adverse ram result, residual moving pile and orchestrator pricing.
