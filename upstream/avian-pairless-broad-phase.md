# Pairless colliders remain queryable without initiating broad-phase traversals

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
guard. Baseline logs: `/tmp/armour-broadphase-before.log` and
`target/armour-broadphase-before/tick-attribution.json` in bay-1.

MEASURED constructed baseline: each of the twelve cases visits 2 candidates, retains 1 peer
pair and 1 touching pair, and produces no pair involving the pairless collider. The cases cover
every tree (static sensor, kinematic, dynamic and standalone) and an empty filter, membership
or both. A ray filtered to the nonempty membership finds the empty-filter collider. Restoring
layers admits both overlapping peers. Fixture sizes and bit assignments are test data.

MEASURED world baseline, means per tick, using the provided late-event recipe, 30 tanks,
1 real client, duration 30 s, local `--rehearsal`:

| Window, simulation seconds | Ticks | Candidate visits | Graph pairs | Awake touching pairs |
|---|---:|---:|---:|---:|
| [2, 9.5) | 480 | 36279.0375 | 150 | 0 |
| [2, 24) | 1408 | 42197.673295454544 | 150 | 0 |
| [26, 30) | 256 | 39463.3828125 | 2996.859375 | 221.9296875 |

The post-event world is an observation, not a regression oracle. Timings are unpriced: the
orchestrator owns pricing against 0.7.0-alpha.13 on the hosted two-core allocation.

## Reproduction

Constructed guard:

```sh
scripts/serial-cargo test --lib --features dev_ui,bitprobe avian_broad_phase_layers -- --nocapture
```

World counts (replace `before` with `after` for the guarded build):

```sh
OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 \
  scripts/perf/collapse.sh --target local --rehearsal \
  --schedule /private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/tracks/late-event.json \
  --tanks 30 --clients 1 --duration 30 --out target/armour-broadphase-before
```

Pricing command for the orchestrator, after starting the deployment with the matching recipe
from `--print-server-env` (`SERVER_ADDRESS` and `DEPLOYMENT_ID` identify that deployment):

```sh
scripts/perf/collapse.sh --target "$SERVER_ADDRESS" --deployment "$DEPLOYMENT_ID" \
  --schedule /private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/tracks/late-event.json \
  --tanks 30 --clients 1 --duration 60 --out target/armour-broadphase-priced
```
