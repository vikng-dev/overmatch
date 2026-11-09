# Game scenery sweep: coarse admission

Implementation: `a2109aedaa2c13e5368c2a2d27c21a404f0c4379` on `lane/sweep-coarse`.
Baseline: `e91b7174ccdecba8421b5d64f315ea7b3dc0d102`, with observation-only preparation and
deposit counters (`target/sweep-coarse-baseline-counters.patch`). All captures used
`dev_ui,bitprobe`, unattended, headless, muted local rehearsals. No timing was priced.

Decision: bound the entire source before preparing fine geometry. Immutable free groups cache
their local box at spawn/split; articulated bodies merge their current contact boxes without
cloning hulls. One spatial query supplies candidates. Fine bounds filter those candidates by
the same `ColliderAabb::intersects` test as Avian's original query.

The box encloses each hull's identity-pose AABB transformed into body space, including corners
outside the hull. Rigid rotation about the same COM preserves that inclusion. Coarse and fine
queries share rotation extrema and `RigidMotion::travel_bounds`, including the cast velocity's
acceleration lead. Numerical padding covers nested pose composition and world-coordinate
rounding, including zero spin. A rejected source retains its empty `SweepCast` verdict and census.

## Paired counts on identical state

MEASURED. A temporary probe built the original fine snapshot/query for every source, asserted
identical owner sets, then ran the original owner sequence on a copy of the same owner state.
For every source-owner resolution it compared cast counts, deposit presence, exact released plug
membership and swept chunk membership before real effects committed. Every assertion passed.
The probe was removed; the working source was restored byte-for-byte to the implementation commit.

| Recipe | Sources, both paths | Owner-met sources, both | Fine preparations before → after | Casts before → after | Deposits before → after |
|---|---:|---:|---:|---:|---:|
| late-event, 30 tanks / 1 client | 88,701 | 0 | 88,701 → 0 | 0 → 0 | 0 → 0 |
| rest-1, 30 tanks / 1 client | 43,654 | 0 | 43,654 → 0 | 0 → 0 | 0 → 0 |
| ram, 1 tank / 1 client | 18,450 | 3,590 | 18,450 → 3,824 | 264 → 264 | 4 → 4 |

DERIVED: ram skips 79.27% of fine preparations. Its 234 prepared sources without a fine owner
are conservative coarse false positives. Both quiet recipes skip all fine preparations.
These are work counts, not a time-saving estimate.

All three paired captures passed client/link validity. Counts sum every complete active probe
row in the server log, including any trailing rows beyond the last drained stats period.
Artifacts: `target/sweep-coarse-audit-{late,rest,ram}/`,
`target/sweep-coarse-paired-counts.json`, and the removed probe's
`target/sweep-coarse-audit.patch`.

## Independent captures

MEASURED. These retain the requested before/after observations, without pinning emergent results.
Each source population and per-source cast sum agrees with its tick totals.

| Recipe / build | Captured ticks | Sources | Owner-met sources | Fine preparations | Casts | Deposits |
|---|---:|---:|---:|---:|---:|---:|
| late-event / before | 2,625 | 88,973 | 0 | 88,973 | 0 | 0 |
| late-event / after | 2,483 | 83,899 | 0 | 0 | 0 | 0 |
| rest-1 / before | 1,256 | 41,405 | 0 | 41,405 | 0 | 0 |
| rest-1 / after | 1,350 | 44,532 | 582 | 582 | 10,866 | 0 |
| ram / before | 1,631 | 18,310 | 3,602 | 18,310 | 264 | 4 |
| ram / after | 1,621 | 16,557 | 2,812 | 2,994 | 481 | 8 |

The independent ram captures first contacted at ticks 1,311 and 1,301 respectively (MEASURED).
Their source/owner snapshots differ; the paired comparison above establishes unchanged work on
identical snapshots. No causal claim is drawn from the independent aggregate movements.

All six rows use valid captures. Two rejected attempts remain under
`target/sweep-coarse-after-{late,rest}-invalid-1/`; both failed client coverage and are excluded.
Accepted artifacts are under `target/sweep-coarse-{baseline,after}-{late,rest,ram}/`.
The only source edit after the production capture binary build controlled a `cfg(test)` fixture's
contact tolerance; production code and data were identical.

## Verification and player effect

The constructed intake cases retain sources one millimetre inside reach and reject those one
millimetre outside: slow motion, fast motion, acceleration from rest and a rotating long body.
The fixture sets contact tolerance to zero so its query boundary equals its geometric face.
Outside cases also prove empty verdict and source/cast/deposit accounting. Containment cases
cover disconnected and locally rotated hulls, offset COM, distant coordinates, negligible spin
and multiple revolutions.

Passed:

- `cargo fmt --all --check` and `git diff --check`.
- CI Clippy, locked workspace/all targets, `dev_ui,bitprobe`, warnings denied.
- Targeted `sweep`, `doc_citations`, `ui_ascii`, `net_boundary`, and full `headless_test` selection:
  110 passed, seven operator-only tests ignored.
- After probe removal: all eight whole-tree guards and CI Clippy passed again.
- Six independent lens reviews; the one duplicate invariant check found by brevity was removed.

Expected player effect: the same contact and release decisions, with distant sources doing less
query preparation. The paired probe verifies unchanged casts and deposits for owner-met sources.
Hosted timing remains for the orchestrator; no savings in milliseconds are claimed here.

## Orchestrator timing commands

Use the already-started two-vCPU deployment containing the implementation; set
`OVERMATCH_PRICE_TARGET` to its `IP:PORT` and `OVERMATCH_PRICE_DEPLOYMENT` to its deployment ID.
Run the same commands against the alpha.13 baseline for pricing. These commands were not run here.

```sh
cd /Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-2
scripts/perf/collapse.sh --target "$OVERMATCH_PRICE_TARGET" --deployment "$OVERMATCH_PRICE_DEPLOYMENT" --schedule /private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/tracks/late-event.json --tanks 30 --clients 1 --duration 40 --out target/sweep-coarse-priced-late
scripts/perf/collapse.sh --target "$OVERMATCH_PRICE_TARGET" --deployment "$OVERMATCH_PRICE_DEPLOYMENT" --schedule scripts/perf/rest-1.json --tanks 30 --clients 1 --duration 20 --out target/sweep-coarse-priced-rest
scripts/perf/collapse.sh --target "$OVERMATCH_PRICE_TARGET" --deployment "$OVERMATCH_PRICE_DEPLOYMENT" --ram scripts/perf/ram.json --out target/sweep-coarse-priced-ram
```
