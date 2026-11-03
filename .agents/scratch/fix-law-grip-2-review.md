# Ground-contact delivery review (#198)

Scope: the driving model. The contact law and its parameters are unchanged.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | Stop forecast omits delivered rolling; its test repeats the omission | Verified: a saturated adapter stop forecast 98,396.8 N against delivered 159,326.58 N, including 60,929.89 N rolling (MEASURED red regression). The forecast now reads the contact law's delivered rolling vector, projected opposite lane travel; its regression compares saturated delivery, and the caller witness varies rolling independently of grip. | driving: forecast stops from delivered rolling drag (#198) |
| Claude | Recipient reactions and contact travel speed have no distinguishing rolling witness | Verified: recipient fixtures had zero rolling or rolling parallel to grip. Added perpendicular grip/rolling with an independently specified total vector for both reaction routes, and moving-support cases with zero and full support-relative drag. Grip-only whole-force and clamped-direction mutations, and hull-only travel speed, each fail the targeted assertion. | driving: witness complete recipient reactions (#198) |
| Claude | Loose-piece full-app test survives a no-op `apply_pushes` | Verified: the old test checked contact telemetry and arrival only. Cut the recipient-delivery claim. Removed motion counters that supplied no asserted recipient-delivery invariant. The test now claims and asserts positive hull lift, settled upward piece support and the footprint; lift magnitude stays an observation. | driving: bound the loose-piece support claim (#198) |
| Claude | Fade omits locked-belt slope creep; `.1715` lacks ground and pressure | Verified: DERIVED balance is `v = band (tan(theta) - mu) / r` for uniform support between `atan(mu)` and `atan(mu+r)` with brakes holding the belts. The constant comment and section 5a state it; the slope table identifies creep. `.1715` is mud at nominal pressure 107 kPa. | driving: state rolling-fade slope creep (#198) |
| Claude | Rubble module reaction and direction prose still says grip alone | Verified: both recipient paths use grip plus rolling. The module now names the total tangent reaction and its complete force. | driving: witness complete recipient reactions (#198) |
| Claude, Codex | Piece-impulse test promises a grip-only magnitude bound | Verified: the bound is `abs(grip + rolling) dt`, alongside the separate Coulomb bound. Corrected the test contract. | driving: witness complete recipient reactions (#198) |
| Claude | Force module order and coast test claim rolling enters belt balance | Verified: support precedes total tangent force, while only longitudinal grip enters belt balance. Corrected both comments, preserving the coupled coast derivation. | driving: witness complete recipient reactions (#198) |
| Claude | Total grip-plus-rolling telemetry is called traction; debug and sandbox disagree | Verified: renamed local effect force/torque and sandbox contact force to `tangential_*`; debug and sandbox now draw the same total tangent force. Bitprobe's self-describing field labels follow the rename; payload ordering is unchanged. | driving: name total tangential force consistently (#198) |
| Claude | `SideReport` rolling sum is dead | Verified: production never reads it; removed its field, accumulation and return slot. Tests read the existing contact vectors. | driving: remove redundant grip bookkeeping (#198) |
| Claude | Strain step has a one-valued `shear_m` parameter | Verified: every caller passes the common solver floor. Removed the argument; the function reads that owner directly and its contract names it. | driving: remove redundant grip bookkeeping (#198) |
| Claude | Hard-support datum charges hull travel regardless of belt motion | Verified; report only. A locked belt pays it during hull travel, while spinning belts on a stationary hull create no such load on the engine. Its running-gear provenance remains a question for the owner. | Unchanged, as instructed |

Recovery validation (MEASURED): the restored driving/caller/bitprobe/headless/citation batch
passed 347 tests, with four intentionally ignored. Each completed group passed
`cargo fmt --all --check`, CI-feature workspace/all-target clippy, targeted tests and
`doc_citations`; the group batches passed 32, 4, 3 and 38 tests. The final batch passed
83 tests, including `track::forces`, the pressure-heavy and service-stop witnesses, and
all required whole-tree guards: `doc_citations`, `ui_ascii`, `net_boundary`, `bake::`.
All temporary mutation probes were restored before green validation.

MEASURED full-app support observation: hull rise 0.076588 m over FIXTURE 0.1 m pieces;
the level-hull coverage derivation gives 0.072800 m. Only positive lift and settled
support are obligations; neither magnitude is pinned.

MEASURED before/after comparison for the telemetry rename and dead-code removal:
firm/soft coasts remain 12.632/4.187 m; the adapter stop retains 98,429.234 N capacity,
2.927493 m peak travel, 0.075154 m rollback and -0.149379 m/s reverse speed, identical
at printed precision. These are observations, with no new output-pin test.

No open backlog row names an edited file. Reserved turret-chain rows remain untouched.
`TrackGripEffect`, contacts and reports are local telemetry; network wire types and
`PROTOCOL_REV` remain unchanged at DECLARED 83. The bitprobe header names change while
its payload order remains unchanged.

MEASURED diagnostic changes: the pressure-heavy fixtures' full-drag force readouts move
from -25,915.0645 to -25,915.0098 N (grip-limited) and from 2.0114 to 2.0112 N (launch)
when their test sums contact vectors directly instead of the retired per-side cache.
Their printed speed ranges and delivered late forces are unchanged; these rounding
changes are in the fixture's diagnostic sum.

Integration validation (MEASURED): after rebasing onto main's newer keyboard-input changes,
formatting, CI-feature workspace/all-target clippy and 104 affected driving/caller,
loose-piece and whole-tree-guard tests pass. The printed service-stop and loose-piece
support observations remain identical.
