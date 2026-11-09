# Stationary pairs retain a non-touching geometry result

Subject: video game server physics and the game's environment destruction model; game state only.

Decision: cache a geometry miss against its complete query and pruning inputs. Keep touching
pairs and hook-rejected contacts on the ordinary path. No flag or alternate engine path remains.
Base: `a6880cfc`, including the armour broad-phase change. The pricing prototype `63a0e00c`
provided the idea; its velocity-only early return was not copied.

## Validity

A `ContactPair` owns an optional boxed snapshot after the built-in collider backend produces
no manifolds **before** contact hooks. A hit allocates nothing; moving pairs carry only the
optional pointer. The cache is omitted from serialization, so restored pairs query first.

| Input or lifecycle change | Why the previous miss cannot hide a new contact |
|---|---|
| Either collider translates or rotates | Compare current world poses without a movement tolerance. Child motion reaches these poses before narrow phase. |
| Shape replacement or scaling | Compare each collider component's change tick. Even an equal-value replacement re-tests. |
| Body attachment or rehoming | Compare current owner entities, including their generations. |
| Wake, including wake then sleep between queries | Compare sleep-entry ticks, with absence representing awake. A new sleep episode cannot reuse the preceding episode's stamp. |
| Margin, global tolerance or length-unit change | Compare current query/pruning margins. Global margin caches refresh on configuration **or** length-unit changes. |
| Velocity, COM, offset or step-length change | Compare relative linear velocity, both angular velocities, separate collider-offset and rotated-COM operands, and step length. These are the point-pruning inputs; sleeping bodies retain velocities. |
| Bounds or collision-layer change | Test current bounds and layer compatibility before cache reuse; remove invalid pairs normally. Constraint eligibility is refreshed there too. |
| Spawn, newly discovered pair, removal or despawn | New pairs start without a snapshot; removing a pair drops it. Broad-phase proxy traversal and new-pair admission remain unchanged. |
| Touching pair | Never eligible for the miss cache; geometry, hooks, warm starting and contact transitions continue normally. |
| Hook rejection or custom backend context | A hook-rejected real contact is never cached. Custom backends remain uncached because their arbitrary `Context` is outside this key. |

The built-in backend has unit context. An empty pre-hook geometry result depends only on the
recorded inputs; another body's arrival creates its own uncached pair, and any resulting motion
or wake invalidates the existing pair at the next narrow phase. No body-specific physics rule
or change to the broad phase, solver ordering or wake scheduling is introduced.

The sweep-contact fix `27562bd8` retains cast targets when contact refinement fails, including
an unresolved initial overlap. Its per-step hook verdict must be reconsidered even at a held
pose. Recording the miss before that hook preserves this path. The captured contact-solve,
initial-overlap and mounted-intake regression tests all pass.

## Same-input comparison

MEASURED local count rehearsals, headless and muted, with one tank and one real client.
Rest used `scripts/perf/rest-1.json` for a 60-second capture. Ram used `scripts/perf/ram.json`;
the table selects its impact-to-deadline window. Counts below are **pair-ticks**, summed over
recorded fixed steps, rather than distinct pairs.

A temporary validator replaced the cache early return with the existing full geometry/pruning
path. Every would-be hit asserted that its recomputed pre-hook manifolds were empty. It also
asserted that no stationary candidate was rejected by bounds/layers, so the candidate/query
difference in these captures is exactly the cache-hit count. All assertions ran in the ordinary
dev build and passed. Thus the reference touching counts below are MEASURED; the identical
cached-path totals and avoided-query counts are DERIVED from checked empty results on the
**same inputs**, rather than from comparing two separate falls.

| Recipe/window | Steps | Stationary pair-ticks | Reference geometry queries | Cached-path queries | Queries avoided | Touching pair-ticks, reference / cached |
|---|---:|---:|---:|---:|---:|---:|
| Rest, complete recorded capture | 3,900 | 7,206,507 | 7,206,507 | 2,947 | 7,203,560 | 71,568 / 71,568 |
| Ram, impact window | 321 | 10,481 | 10,481 | 2,577 | 7,904 | 193,246 / 193,246 |

DERIVED query reductions: 99.96% for rest and 75.41% for the ram window, from the table.
MEASURED rest validation's final 640 steps: 1,979 stationary pairs per step, zero required
geometry queries and zero awake touching pairs. Every proposed skip was a geometry miss.

Validator artifacts: `/private/tmp/still-pairs-validate-rest/` and
`/private/tmp/still-pairs-validate-ram/`. The exact temporary diff is
`/private/tmp/still-pairs-validation.patch`; it is absent from the shipping source.

## Independent shipping-binary captures

MEASURED observations below come from the actual baseline and shipping binaries. These are
fresh server/client processes (`scripts/perf/collapse.py:398`, `:407`, `:409`), not replayed tick
inputs. Their touching totals do **not** match, and these runs do not establish trajectory
identity. No source of that variation is attributed here. The identical-input check above
establishes the unchanged contact decision; these separate observations establish actual work
removal. In particular, the ram's first impact is at fixed tick 1312 before and
1301 after, so its inputs were already different at the start of the compared window.

| Recipe/build | Recorded steps | Stationary pair-ticks | Geometry queries | Stationary pair-ticks without a query | Awake touching pair-ticks |
|---|---:|---:|---:|---:|---:|
| Rest, before | 3,917 | 7,130,409 | 7,130,409 | 0 | 71,858 |
| Rest, shipping | 3,859 | 7,267,887 | 3,000 | 7,264,887 | 94,995 |
| Ram impact, before | 321 | 10,481 | 10,481 | 0 | 193,246 |
| Ram impact, shipping | 321 | 138,767 | 2,486 | 136,281 | 163,746 |

The last column counts active touching pairs, including sensors, from the existing `avian`
account. The unqueried column is DERIVED as stationary candidates minus queries; unlike the
validated table, it also permits ordinary bounds/layer refusal. MEASURED shipping rest's last
640 steps have a stable population: 2,031 stationary pairs per step, zero geometry queries and
zero awake touching pairs. Baseline's corresponding tail has 1,949 queries per step and zero
awake touching pairs. Persistent tail pairs therefore really skip geometry every step.

All six captures passed client/link validity checks. Artifacts are under
`/private/tmp/still-pairs-{before,validate,after}-{rest,ram}/`. Scheduled captures use
`tick-attribution.json`; ram captures use `result.json`. Their timers are unpriced and were not
used for a performance claim. Expected player effect: less server work while scenery rests;
contact decisions and the laws governing a fall are preserved. Hosted pricing remains with
the orchestrator.

## Gates

`cargo fmt --all --check`, per-file Rust formatting and CI clippy passed:
`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`.
MEASURED targeted result: 97 passed, zero failed, six existing ignored. Selection:
`net::ram::avian_tests`, `headless_test`, `construction::sweep`, the mounted unresolved-target
intake witness, and `doc_citations`, `ui_ascii`, `net_boundary`.

The constructed cases prove: repeated misses skip; a zero-velocity teleport re-tests that
step; touching geometry refreshes every step; rotation, shape revision, collider/body margin,
tolerance, scale and wake/resleep invalidate; a new neighbor still discovers contact; child
motion and rehoming invalidate; and a hook rejection is reconsidered on the next step.
The initial reuse test was observed failing on the baseline (one query instead of zero).
All six independent lens reviews are clean; no backlog row names an edited file.
Logs: `/private/tmp/still-pairs-{red,final-tests,clippy}.log`.

## Reproduction and pricing

Constructed and integration checks:

```sh
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 \
  scripts/serial-cargo test --lib --features dev_ui,bitprobe -- \
  net::ram::avian_tests headless_test doc_citations ui_ascii net_boundary \
  construction::sweep \
  construction::breakage::tests::detected_targets_keep_lookahead_contacts_through_intake
```

Shipping count runs (local rehearsal, no timing claim):

```sh
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 \
  scripts/perf/collapse.sh --target local --rehearsal \
  --schedule scripts/perf/rest-1.json --tanks 1 --clients 1 --duration 60 \
  --out target/still-pairs-rest
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 \
  scripts/perf/collapse.sh --target local --rehearsal \
  --ram scripts/perf/ram.json --out target/still-pairs-ram
```

Exact pricing commands for the orchestrator, after starting the matching deployment with the
recipe from `--print-server-env`. `SERVER_ADDRESS` and `DEPLOYMENT_ID` identify that deployment;
these commands read it and do not deploy or stop it:

```sh
scripts/perf/collapse.sh --target "$SERVER_ADDRESS" --deployment "$DEPLOYMENT_ID" \
  --schedule scripts/perf/rest-1.json --tanks 1 --clients 1 --duration 60 \
  --out target/still-pairs-priced-rest
scripts/perf/collapse.sh --target "$SERVER_ADDRESS" --deployment "$DEPLOYMENT_ID" \
  --ram scripts/perf/ram.json --out target/still-pairs-priced-ram
```
