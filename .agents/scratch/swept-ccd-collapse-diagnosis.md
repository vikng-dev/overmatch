# Swept CCD: collapse rest investigation

Subject: Overmatch's environment destruction model, game state only.

Follow-up: the [controlled six-house experiment](swept-ccd-controlled-collapse.md) repeats each
runtime arm exactly but finds identical physical outcomes across arms. It does not reproduce the
extra truncation hypothesis or close this live-run causal question.

## Decision and limit

The expensive tail is an awake debris contact graph: narrow phase, constraint preparation and
solving continue after CCD has become cheap. The original pricing records associate their longer
after tails with later piece sleep. The new runs confirm substantial collider skipping and a
higher median time to sleep, but do **not** establish that deeper initial overlap caused the
longer tail. They also do not reproduce a clean separation between the variants' tail costs.
Part A's causal explanation remains a landing blocker.

Keep the gate and the current ordinary-contact policy unchanged while that cause is unresolved.
No body-specific rule, sibling coupling, contact-margin change or tuned constant is justified by
these records. Avian's general `SpeculativeCcd` lever remains an untested choice here, not a fix
whose need or effect this investigation has demonstrated.

What would settle it: replay one captured six-house state just before first contact, holding
poses, velocities, chunk partition, other actors and event tick fixed, changing only the
per-collider gate. Record each collider's first contact separation and normal impulse, its gate
selection, subsequent split work and its island's final sleep. This also controls the tank-course
rival. The requested revisions include other merged changes; their wall-clock schedule is not
an identical-state replay (`src/net/loadtest.rs:322` reads `started.elapsed()` before dispatch).
The present overlap census samples retained narrow-phase points, not first contacts keyed by
collider, so its averages cannot establish that causal chain.

## Reproduction and measurement scope

MEASURED: three release rehearsals on `602c0a570fc1b7462a62e25dcb999f725d222b34`, then three on
`68952ae9a8a48c231480614e22b2aa1b7bbc9cb7`. The baseline was built detached in this bay; temporary
probes were restored before switching back to `lane/swept-ccd`. No reset, stash, push, web access,
external comment, sub-agent or target deletion was used.

The existing tool ran the full recipe, not the six-house-only fallback:

```sh
scripts/serial-cargo build --release --locked --features dev_ui,bitprobe \
  --bin overmatch-server --bin overmatch
scripts/build-lock python3 /tmp/swept-ccd-run-three.py before
# The same driver ran `after` on the lane head.
```

The driver imports the same `scripts/perf/collapse.py` implementation with only its root path
and window mode adapted. Its arguments are `--schedule scripts/perf/match-collapse.json --tanks
30 --clients 1 --duration 60 --rehearsal --bin target/release --holding-lock`. Every process used
`OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=hidden OVERMATCH_MUTE=1`. The build lock covered each
three-run batch and its load waits. No compile overlapped a rehearsal. All complete population,
link, combat, schedule and tick-continuity checks passed. All six were rejected for
capacity: the hidden client missed its frame gate and `server_over_budget` was true. These are
local physics observations, not capacity or hosted prices.
MEASURED host admission: available parallelism 10; compute/async/IO pools 10/1/1 in all six runs.

The piece cohort is the admitted instance ordinals for `house_0007` through `house_0012` (resolved
from the ID-sorted map, not hard-coded ordinal numbers). Piece creation and rest statistics cover
those six constructions through the completed capture. Timers and active contact counts cover
the entire server during the first 600 ticks from the big event. DERIVED window length:
600 * the recorded 0.015625 s timestep = 9.375 simulation seconds. Heavy ticks count physics
above the owner's 5 ms threshold from the event through elapsed wall time 60 s.

Quantiles use nearest rank. Rest means the final recorded awake episode's completed sleep tick
minus that piece's birth,
in simulation seconds, so later wakes are included; first-sleep and visible-motion statistics
also remain in the raw summaries. A piece retired during its final awake episode has no final
sleep time and is excluded from the quantiles, even if an earlier episode slept.
The awake counts at +5/+10 s reconstruct awake episodes at event tick +320/+640. Frozen counts
below are the next complete stats period at or after the named wall offset; their exact offsets
are printed, rather than treating a period sample as an exact tick.

## Results

Every number in the following tables is **MEASURED, three runs per revision**. Slash-separated
values within a cell are labelled in that table's introduction; they are not pooled quantiles.

| Run | Admitted load (1 min) | Pieces created | Final sleep / retired without final sleep | Rest p50 / p90 / worst, s | Awake +5 / +10 s | Heavy ticks | Physics, first 600 ticks, s |
|---|---:|---:|---:|---|---|---:|---:|
| before-1 | 3.769 | 345 | 254 / 91 | 1.359 / 10.594 / 10.844 | 54 / 87 | 665 | 4.463 |
| before-2 | 3.583 | 344 | 256 / 88 | 1.219 / 5.219 / 5.375 | 35 / 0 | 220 | 2.857 |
| before-3 | 3.934 | 352 | 260 / 92 | 1.516 / 5.750 / 6.016 | 89 / 0 | 386 | 4.050 |
| after-1 | 3.804 | 339 | 245 / 94 | 3.750 / 5.969 / 6.188 | 136 / 0 | 396 | 4.005 |
| after-2 | 3.816 | 323 | 229 / 94 | 2.734 / 4.516 / 4.828 | 0 / 0 | 272 | 3.283 |
| after-3 | 3.637 | 360 | 259 / 101 | 4.734 / 11.562 / 11.781 | 135 / 102 | 755 | 5.463 |

MEASURED in every run: 48 initial pieces; the six targets' reported `falls` / `dust` / `chunks`
were 5772 / 960 / 6732 after the event. The created-piece counts include intermediate parents
that split. The affected chunk counts did not increase in this census, but the rigid
piece partition differs. These are behaviour observations for the owner, not pass conditions.

Per-tick active contact counts, **mean / p90 / worst**. These are the census's awake,
constraint-generating touching pairs, retained manifolds and their points after reduction and
before sleeping. Raw records keep each kind and tick as well as Avian's prepared constraint count.

| Run | Touching pairs | Manifolds | Points |
|---|---|---|---|
| before-1 | 2125.9 / 3118.0 / 5327.0 | 1365.4 / 1814.0 / 2226.0 | 3896.7 / 5381.0 / 6396.0 |
| before-2 | 1173.1 / 4433.0 / 5577.0 | 676.4 / 1858.0 / 2475.0 | 1968.6 / 5263.0 / 7133.0 |
| before-3 | 1899.1 / 4252.0 / 5416.0 | 1157.0 / 2173.0 / 2455.0 | 3426.5 / 6819.0 / 8067.0 |
| after-1 | 1988.0 / 4558.0 / 4966.0 | 1375.2 / 2533.0 / 2631.0 | 4090.8 / 7985.0 / 8376.0 |
| after-2 | 1678.1 / 4312.0 / 4562.0 | 1040.0 / 2572.0 / 2991.0 | 3053.5 / 8275.0 / 9115.0 |
| after-3 | 2642.9 / 3178.0 / 5052.0 | 2072.7 / 2956.0 / 3051.0 | 6461.3 / 9843.0 / 10179.0 |

MEASURED across all three runs of each revision: **zero** recorded piece/belt, piece/hull and
piece/standing contact-pair visits in this window. The observed piece contacts are with terrain
and other pieces, frozen pieces included. Direct tank/piece contact has no recorded support as
the cause; tank trajectories and all tank-generated work were not controlled away.

Stage totals over the same 600 ticks, seconds. Solver is the sum of prepare constraints,
warm start, biased solve, relaxation and impulse storage; other engine timers remain separate
in the raw record. `Structure` and `crumble` are the game's own named stages.

| Run | Narrow phase | Solver | CCD | Structure | Crumble | Contact census |
|---|---:|---:|---:|---:|---:|---:|
| before-1 | 0.949 | 1.146 | 0.690 | 0.264 | 0.317 | 0.148 |
| before-2 | 0.549 | 0.653 | 0.628 | 0.160 | 0.210 | 0.086 |
| before-3 | 0.789 | 1.079 | 0.768 | 0.212 | 0.246 | 0.132 |
| after-1 | 0.862 | 1.365 | 0.178 | 0.226 | 0.275 | 0.150 |
| after-2 | 0.700 | 1.044 | 0.162 | 0.193 | 0.246 | 0.118 |
| after-3 | 1.139 | 2.067 | 0.210 | 0.296 | 0.340 | 0.201 |

Times include the probes. CCD adds two local integer counts and commits its per-body results
beside the existing result reduction; the contact census adds penetration tallies in its
existing pair visit. Probe overhead is not independently priced. `contact_census_us` brackets
the census, including its added work. No temporary instrumentation remains in shipping source.

CCD counts below belong to the six-house cohort over its captured lifetimes. “Fast” means entry
through CCD's body gate, not the rest record's pre-existing `last_fast` (which means failing the
sleep test). Counts are collider-step visits, not distinct persistent collider identities; the
raw per-piece records also retain each piece's peak number skipped in one fast step.

| Run | Pieces ever CCD-fast | Pieces ever gated | Swept collider visits | Gated collider visits | Most colliders skipped on one piece/step |
|---|---:|---:|---:|---:|---:|
| before-1 | 176 | 0 | 222175 | 0 | 0 |
| before-2 | 165 | 0 | 170981 | 0 | 0 |
| before-3 | 167 | 0 | 190240 | 0 | 0 |
| after-1 | 180 | 156 | 106223 | 102546 | 884 |
| after-2 | 161 | 136 | 104641 | 107483 | 870 |
| after-3 | 226 | 199 | 143936 | 139402 | 884 |

Overlap during the first 64 ticks after the event, millimetres. “Mean positive depth” is the sum
of positive point penetrations divided by their observation count; it weights repeated contacts.
The p90 column is the nearest-rank p90 of each tick's deepest terrain point. Both are retained
narrow-phase geometry measurements, not final post-solver poses or first-impact measurements.

| Run | Terrain mean positive depth | Terrain deepest-point p90 | Piece/piece mean positive depth |
|---|---:|---:|---:|
| before-1 | 15.961 | 44.254 | 1.634 |
| before-2 | 15.120 | 44.593 | 1.400 |
| before-3 | 15.748 | 44.404 | 1.474 |
| after-1 | 15.582 | 44.410 | 1.793 |
| after-2 | 16.696 | 44.320 | 1.705 |
| after-3 | 15.501 | 44.395 | 1.370 |

MEASURED in all six runs: the maximum first-second terrain penetration was 180.262 mm. The
head runs do not show a consistent increase in terrain or piece/piece overlap depth. Standing
contact was absent. These records do not establish the proposed deeper-overlap mechanism.

Frozen six-house bodies: **count @ actual wall seconds after the event**.

| Run | Near +2 s | Near +5 s | Near +10 s | Near +20 s |
|---|---|---|---|---|
| before-1 | 123 @ 2.24 | 180 @ 5.27 | 163 @ 10.32 | 254 @ 20.42 |
| before-2 | 77 @ 2.21 | 220 @ 5.23 | 256 @ 10.29 | 256 @ 20.37 |
| before-3 | 84 @ 2.23 | 131 @ 5.26 | 260 @ 10.30 | 260 @ 20.39 |
| after-1 | 104 @ 2.32 | 107 @ 5.35 | 245 @ 10.41 | 245 @ 20.50 |
| after-2 | 37 @ 2.30 | 135 @ 5.33 | 229 @ 10.38 | 229 @ 20.49 |
| after-3 | 107 @ 2.04 | 107 @ 5.06 | 155 @ 10.08 | 259 @ 20.16 |

Freeze is not monotone: pieces can wake. MEASURED counts of pieces sleeping more than once were
33 / 2 / 0 before and 5 / 3 / 14 after. The raw episodes preserve those wakes and sleeps.

## Reconciliation with the original price

The old complete runner artifacts, not just the compact pricing rows, were also read. MEASURED
from three original runs per revision: first-sleep p90s for the six-house cohort were
4.188 / 6.797 / 4.781 s before and 8.094 / 12.078 / 10.719 s after. Awake counts at +5 s were
0 / 75 / 0 before and 132 / 172 / 136 after; at +10 s they were 0 / 0 / 0 and 0 / 97 / 45.
Their piece/hull and piece/belt contact counts were also zero. That locates the original extra
physics work in longer-lived piece contacts rather than CCD or direct tank contact. The
original piece counts were 346 / 366 / 312 versus 349 / 354 / 348, not a consistent increase
in fragmentation count. Historical summaries and their original artifact paths are archived
separately in the new record; none are mixed into the new run tables.

The new medians are higher after the gate, but the tail and cost ranges overlap. In particular,
one new before run itself has a long wake/rest tail, and one new after run settles sooner than
any new before run's worst piece. Together with the overlap census and the source changes
between revisions, this prevents a stronger causal verdict. No parameter was tuned to recover
any earlier number.

## Review fixes and verification

- The rotating-child guard checks its leading corner against the fixture's near wall plane,
  including body translation and a labelled float allowance.
- The thick/thin fixture sets its CCD threshold explicitly; shared geometry constants define
  both its setup and geometric assertions. The repeated invariant comment is gone.
- The non-solid draft resolves owners through `Allow<Disabled>` and keeps pending invalidations
  until recomputation or genuine body despawn. It remains a compile-only, unshipped proposal.
- The per-collider draft explicitly states that its behavioural witnesses ran on the fork.
- The inventory separates retirement of upstream's disabled-collider filter from the sensor draft.
- The fork transforms the body COM into collider coordinates before querying sweep radius. Its
  guard requires the bound to enclose all transformed fixture corners, allowing an engine to
  tighten its conservative bound. MEASURED red witness, one test invocation before the fix:
  reported radius 0.4330127 m, farthest fixture corner 4.2646804 m. The independent published-crate
  draft is `upstream/avian-body-size-metrics-collider-frame.md`.

MEASURED on the final source, one completed invocation of each gate: `cargo fmt --all --check`
passed; CI clippy (`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`)
passed; all 110 selected `tests::avian` tests passed; all 16 ordinary track-scorecard tests passed
(the 8 on-demand observations remained ignored). The radius guard was red before the frame fix
and green in the Avian gate after it. No full suite ran locally.

Both final Markdown patches applied byte-for-byte to fresh cached published-crate extractions.
Both independent offline library checks passed through `scripts/serial-cargo`, with the
published manifests unchanged and targets inside this bay. The sensor proposal's lifecycle is
compile-checked only; no published-crate behavioural test is claimed.

## Source census and artifacts

MEASURED by `diff --minimal -u` per source file against cached published archives: Avian
707 added + 168 removed = **875** fork lines, **797** in upstream files plus **78** fork-only.
Its metrics file is **46 + 19 = 65**; the radius fix adds **3 + 1 = 4** to the existing batch
and diagnostic patch. CCD remains **18 + 4 = 22**. The complete fork total is **1081**:
Avian 875, Bevy PBR 47, Lightyear netcode 159, Lightyear Avian 0. Archive digests and per-file
counts: [swept-ccd-collapse-fork-lines.json](swept-ccd-collapse-fork-lines.json).

[swept-ccd-collapse-record.json.gz](swept-ccd-collapse-record.json.gz) holds all new per-tick
stage times, contact counts, work counters, full piece records and sleep episodes, period target
states, capacity verdicts, admitted load and waits, binary hashes, exact temporary patches,
drivers and analysis source. The six full original runner directories remain at
`/private/tmp/swept-ccd-collapse/{before,after}-{1,2,3}`. Run/build log:
`/private/tmp/swept-ccd-experiment.log`. Gate and red-witness logs:
`/private/tmp/swept-ccd-gates.log` and `/private/tmp/swept-ccd-radius-red.log`.

The record is evidence, never a regression oracle. Part A remains open as described above;
independent six-lens landing review and the owner's judgement of the changed piece partitions
remain with the orchestrator.

