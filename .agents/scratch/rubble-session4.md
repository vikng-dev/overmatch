# Rubble sweep investigation and b22 disposition

Subject: the game's environment destruction and damage models; game state only.

The owner's MEASURED sweep regression, 6.1 to 28.5 ms, remains **unresolved**. No tracked ram
pricing harness, UDP transport, capture or external service ran in this session. The verified
b22 work-ownership defects are fixed; their microbenchmark improvements are not evidence that
the entire priced regression is fixed.

## Investigation

All microbenchmarks ran under the machine lock, unattended and headless, on the local M4.
The sweep fixtures explicitly initialized two compute workers. These are local measurements,
not a hosted CPU-quota measurement. Each reported median uses seventeen fresh mutable fixtures;
immutable admitted geometry and spatial inputs are shared outside the interval. No full tick or
frame was timed. Command application is excluded from the sweep geometry/departure intervals.

The historical observation fixture paused for the recipe's settle_s before driving. The server's
recipe uses that field to end a settled post-impact run and starts driving immediately. Shared
`headless_test::prepare_tracked_ram` now matches that start. This deliberately changes the
functional observation's inputs; its new results are not a before/after physics comparison with
sessions 2 or 3. It still uses the offline composition plus the shipped carrier adapter, so it is
not an exact reconstruction of the owner's networked server state.

MEASURED paired check of immediate disconnection against standing-first ordering, using the
historical paused fixture and two compute workers:

| Fixture tick | Standing first, departure ms | Immediate disconnection, departure ms |
|---|---:|---:|
| 1265 | 0.783 | 0.830 |
| 1266 | 2.642 | 2.733 |

Its geometry medians at the latter tick were 5.043 and 5.057 ms respectively. This fixture does
not implicate the ordering change in the owner's much larger regression. It does not establish
that the ordering change is cheap for every reachable topology. Permanent collider geometry was
not independently bisected in an equivalent owner-state reproduction; attributing the complete
regression to either session-3 commit would exceed the evidence.

MEASURED immediate-drive geometry on identical fresh inputs, original serial patch reduction
versus worker-batched body-patch reduction, with the same worker-batched preload in both:

| Fixture tick | Patches | Casts | Joint candidates | Serial ms | Parallel ms |
|---|---:|---:|---:|---:|---:|
| 1162, contact | 1 | 34 | 3258 | 1.038 | 1.047 |
| 1163, +1 | 2 | 71 | 3258 | 10.205 | 10.229 |
| 1164, +2 | 8 | 249 | 16290 | 4.247 | 4.106 |

A paired repeat gave 4.423 / 4.165 ms at +2. The preload-heavy +1 sample remains about
10.2 ms: parallel body cuts do not remove that cost. A broader hull-query parallel prototype
was rejected: the paused fixture's busiest geometry median rose from 5.057 to 7.432 ms.
Hull merging retains source order; only the independent body-patch resolution is dispatched
in worker-sized batches after shared preload, with ordered effects unchanged.

## b22 findings

- Parallel-work, sweep intake: body patches resolve independently after preload; collider patches
  of one body still merge first. Results preserve body order before the shared state commit.
- Parallel-work, preload: replace one task per part with worker-sized batches, preserving part
  order. MEASURED chosen granularity: two batches for the 2048-singleton fixture, median 0.398 ms.
  There is no measured old/new preload speedup: task-limit trials all used the same worker-sized
  granularity because the pinned Bevy implementation uses the worker-based chunk size for all those limits. Their spread is
  repeat noise, not a comparison with the former per-part dispatcher.
- Parallel-work, landings: each owner's worker updates its seams, routes fragments and constructs
  split children; ordered application owns only ledger entries, messages and structural changes.
- Parallel-work, fresh colliders: worker preparation caches contact-shape mass inside the pinned
  Collider. Existing backend insertion/update hooks consume that cache; changed shape, scale or
  density cannot use an obsolete value. The explicit occupied mass overrides remain unchanged.
  MEASURED 600 identical convex collider insertions, seventeen fresh app fixtures: 0.787 ms without
  preparation, 0.498 ms cached. The insertion and mutation witness checks the backend result and
  subsequent density, scale and shape replacement. The local patch inventory and upstream draft
  document its contract; nothing was sent externally.
- Testing: the upper-pair and law-installer fixtures scale from the authored rubble floor, so
  collider retention no longer assumes a unit cube is resolved rubble.
- Code-branching: the design describes the common volume route and server rubble retention;
  dust-only hull cuts and a mass floor are explicitly rejected.
- Brevity: one resolved-contact helper retains the four distinct force assertions; one ordered
  loop computes telemetry and momentum; collider comments each own one invariant; both ram
  observations share their setup.

## Lifecycle observations

MEASURED counters below are `colliders created/deleted; bodies created; entities spawned/despawned`.
The immediate-drive serial and parallel observations agree at every recorded fixed boundary,
including the shared carrier adapter. This is a one-time refactor comparison, not a retained
historical-output assertion. All per-tick observations are in
[rubble-session4-counts.json](rubble-session4-counts.json).

| Fixture tick | Serial patches | Parallel patches |
|---|---|---|
| 1162, contact | 136/2; 1; 138/2 | 136/2; 1; 138/2 |
| 1163, +1 | 966/51; 15; 996/53 | 966/51; 15; 996/53 |
| 1164, +2 | 6/4; 18; 42/8 | 6/4; 18; 42/8 |
| 1165, +3 | 0/0; 8; 16/2 | 0/0; 8; 16/2 |
| Through +320 | 1108/57; 42; 1192/65 | 1108/57; 42; 1192/65 |

The historical paused fixture reproduces session 3's creation peak: 882 colliders at contact +2,
then zero creates / 22 deletes at +3; totals 1091 creates / 85 deletes, 58 bodies and
1207 spawns / 105 despawns. Reverting only immediate disconnection retains 1091 creations but
has 93 deletions, 51 bodies and 1193 spawns / 108 despawns. These are observations of changed
physics events, not a collider representation change or a conservation obligation.

No temporary benchmark, serial alternative, environment selector or timing probe remains.
Validation uses formatting, CI clippy, construction/debris/headless targeted tests and the
whole-tree citation, UI, network-boundary and bake guards, followed by one full library suite.
