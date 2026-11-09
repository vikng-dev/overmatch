# Pairless colliders keep query-tree proxies without initiating broad-phase traversals

Subject: video game server physics and the game's damage model raycasts; game state only.

Decision: skip a moved proxy's traversals whenever either collision-layer mask is empty.
`CollisionLayers::interacts_with` requires both cross-mask intersections to be nonempty, so
these proxies cannot produce a pair. Keep tree membership, bounds updates and spatial queries
unchanged. Read eligibility from current proxy data rather than caching it.

The broad-phase collector now borrows trees immutably. Its missing-proxy and missing-bounds
cases report broken invariants rather than silently skipping a missing bound.

The new `avian.broad_phase_candidates` tick field counts non-self BVH leaf visits before
pair filtering; visits from both endpoints count separately. Each worker accumulates locally,
then the existing pair reduction sums the counters. Accepted pairs and touching pairs remain
separate observations.

## Evidence

Baseline: `e91b7174` (0.7.0-alpha.13), with the candidate counter added and without the traversal
guard. Baseline tick records: `target/armour-broadphase-before/tick-attribution.json` in bay-1.

MEASURED constructed before/after, in each of twelve cases. The before column came from an
adapted copy of the test with the candidate assertion changed to 2:

| Observation | Before | After |
|---|---:|---:|
| Candidate visits when the pairless collider moves | 2 | 0 |
| Pairs involving the pairless collider | 0 | 0 |
| Peer pairs / touching peer pairs | 1 / 1 | 1 / 1 |
| Empty-filter collider hit by a membership-filtered ray | yes | yes |
| Contacts admitted after restoring layers | 2 | 2 |

The cases cover every tree (static sensor, kinematic, dynamic and standalone) and an empty
filter, membership or both. Empty memberships remain excluded by spatial-query masks, as before.
Fixture sizes and bit assignments are test data. Logs: `target/armour-broadphase-before/constructed.log`
and `target/armour-broadphase-after/constructed.log`.

MEASURED world counts; means per tick, using the provided late-event recipe, 30 tanks,
1 real client, duration 30 s, local `--rehearsal`. Each row gives before / after:

| Window, simulation seconds | Ticks per run | Candidate visits | Graph pairs | Awake touching pairs |
|---|---:|---:|---:|---:|
| [2, 9.5) | 480 | 36279.0 / 15193.9 | 150 / 150 | 0 / 0 |
| [2, 24) | 1408 | 42197.7 / 17696.2 | 150 / 151.0 | 0 / 0 |
| [26, 30) | 256 | 39463.4 / 16499.0 | 2996.9 / 2938.1 | 221.9 / 265.2 |

DERIVED reduction in the clean [2, 9.5) window: 58.12% of candidate visits, from the measured
means. Its graph and touching counts are unchanged. Full tick records:
`target/armour-broadphase-{before,after}/tick-attribution.json` in bay-1.

The post-event touching counts differ. These are independent online runs: the runner starts
fresh server and client processes (`scripts/perf/collapse.py:398`, `:407`, `:409`), rather than
replaying captured per-tick inputs and state. This lane has not attributed the post-event
variation or established post-event run equivalence. Those counts are observations, not a
regression oracle. The exact unchanged-touching claim is witnessed by the constructed case;
the layer law excludes the same impossible pairs for any fixed input state.

No player-visible change is expected from excluding those impossible pairs. Timings remain
unpriced: the orchestrator owns pricing against 0.7.0-alpha.13 on the hosted two-core allocation.

## Gates and disposition

Implementation commit: `22d83da6`. `cargo fmt --all --check`, CI clippy with
`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`, and the targeted
run passed: 63 passed, 6 existing ignored, none failed. That run includes `headless_test`,
`avian_broad_phase_layers`, `net::ram::avian_tests`, `cost::tests`, and the three whole-tree guards
`doc_citations`, `ui_ascii`, `net_boundary`. Both count rehearsals completed successfully.
MEASURED: the after run was contended by machine load (77 over-budget ticks against 10 before),
so its timings are unusable; its counts stand because both runs used `dt_s = 0.015625` every tick.
No lens-backlog row or open sweep issue named an edited file; the diff was read against all six
lenses. No timing claim, push, merge or release is made by this lane. Hosted pricing remains.

## Reproduction

Constructed guard:

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe avian_broad_phase_layers -- --nocapture
```

World counts (replace `before` with `after` for the guarded build):

```sh
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 \
  scripts/perf/collapse.sh --target local --rehearsal \
  --schedule <(printf '%s\n' '{"events": [{"at_s": 25, "instances": ["house_0001"]}]}') \
  --tanks 30 --clients 1 --duration 30 --out target/armour-broadphase-before
```

Pricing command for the orchestrator, after starting the deployment with the matching recipe
from `--print-server-env` (`SERVER_ADDRESS` and `DEPLOYMENT_ID` identify that deployment):

```sh
scripts/perf/collapse.sh --target "$SERVER_ADDRESS" --deployment "$DEPLOYMENT_ID" \
  --schedule <(printf '%s\n' '{"events": [{"at_s": 25, "instances": ["house_0001"]}]}') \
  --tanks 30 --clients 1 --duration 60 --out target/armour-broadphase-priced
```
