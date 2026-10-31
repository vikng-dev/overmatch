# Keeping server-simulated debris cheap: survey and ranked menu

Subject: the game's environment destruction (walls and houses breaking into pieces), its server
physics cost only; game state, no crew. Read 2026-10-31; nothing in the repository was edited,
built or run. Number labels: MEASURED, DERIVED, ASSUMED. Source tags: [S] read in the cited
source, [S~] via a summary, [I] my inference, [?] not found. The replication and look survey
(`.agents/scratch/destruction-2026-10-30-buildings/web-survey.md`) is not repeated.

## TLDR

1. **Most "live" pieces in the measured curve are rubble the rule itself keeps waking.** At 12:2,
   6 pieces are released per second and 382 are woken per second (the run's counters). So 98 % of
   the pieces that become live are wakes. Little's law puts a live episode at ~0.9 s, and each
   frozen piece is woken again about every 1.3 s (all DERIVED, §1). The first lever is to stop
   spurious wakes, not to make moving pieces cheaper.
2. **The code reading names concrete wake sources** (§2):
   - Every freeze and every wake changes the body type. In Avian that drops all of the piece's
     contacts and wakes its island. Box2D's docs call a type change "expensive".
   - The freeze rule and Avian's own island sleep both run, on the same 0.15 m/s and 0.5 s
     thresholds, and they undo each other.
   - REST_SPEED (0.15 m/s) sits below g·dt (0.153 m/s at 64 Hz). One step without support
     therefore reads as motion and wakes what rests on it.
   - `wake_ahead` grows its wake box from every collider a tank owns, ballistic volumes
     included.
3. **Do now (same gameplay):**
   - A1: wake hysteresis plus a debris-only wake reach;
   - A2: wake by spatial query or index instead of a loop over every frozen piece;
   - A3: fix the cap's quadratic selection;
   - A4: stop cloning Avian's static tree;
   - A5: quantise a carrier's pose before comparing it;
   - A6 (larger): persistent contact manifolds in the vendored Avian.
4. **Owner's call (trade-offs):** native island sleep instead of freezing; no debris-to-debris
   collision (Jolt's documented DEBRIS layer); fine debris as client cosmetics; shorter debounce
   or low-speed damping; cheaper shapes; global substeps; merged piles; baked falls.
5. **Next lane:** instrument wakes by cause and Avian's per-phase timers, apply A1–A3, then rerun
   12:2 and 50:2 on the M4 A/B/B/A at the game's default cap 256. The curve ran with the cap off.

## 1. What the existing logs already say

New derivations from `.agents/scratch/perf-2026-10-31/` (M4, alpha.3, 29 bots).

**Freeze and wake rates per condition.** Counter deltas over the last three quarters of each
first-run window (`local-curve/c0{2..5}-*.window.log`), at one stats line per second. The counters
are MEASURED; the rates are DERIVED. Live is the README median.

| Lever | Released/s | Rest-frozen/s | Thawed/s | Frozen-cap despawns/s | Live | Wakes as share of live entries |
|---|---|---|---|---|---|---|
| 12:2 | 6.0 | 387 | 382 | 2.9 | ~350 | 98 % |
| 50:2 | 25.3 | 708 | 692 | 16.5 | ~1190 | 96 % |
| 100:2 | 50.3 | 691 | 658 | 33.4 | ~2490 | 93 % |
| 200:2 | 103 | 628 | 561 | 66.9 | ~5080 | 84 % |

- **Little's law** (L = λW, DERIVED). At 12:2, λ ≈ 388 entries/s and L ≈ 350, so a live episode
  lasts W ≈ 0.9 s, close to the 0.5 s debounce (`REST_S`) plus settling. Each frozen piece is woken
  0.75 times a second (382 / 510), about every 1.3 s. At the higher levels W grows to ~1.7, ~3.5
  and ~7.5 s.
- **Price of one wake** (DERIVED). At 12:2 a live piece costs ~8–9 µs per tick on the M4 (README),
  so one wake costs ~0.9 s × 64 × 8 µs ≈ 0.46 ms of server time. On Edgegap, at 18–20 µs per live
  piece, it costs ~1.0–1.2 ms.
  - If half the wakes were spurious, removing them would take ~175 pieces off the live count at
    12:2. That is about −1.4 ms of tick p50 on the M4 and −3 to −3.5 ms on Edgegap (DERIVED;
    the spurious share is unknown).
- **Not plausibly tank contact alone** [I]. 29 tanks each wading through a 12-piece heap once a lap
  would wake tens of pieces a second, not 382. A self-sustaining cascade (§2, F1–F3) fits better.
  The `thawed` counter does not say which wake path fired, and that is the first thing to
  instrument.
- **Frozen pieces cost money inside the step** (calibration `calib/`, M4, live cap 8192). The
  frozen-cap-1 runs give a physics slope of 1.21 µs per live piece, with an intercept of ~1.03 ms
  that matches `off` (DERIVED).
  - The no-cap runs then leave an excess of 1.63 µs per frozen piece at 200:2 and 1.88 µs at 50:2
    inside Avian's step.
  - Outside the step they leave 0.22 µs per frozen piece. That covers the wake loops and the
    command flushes that switch body types.
  - A static body with no contacts costs nothing per tick (§2), so the excess must be contact
    pairs with live pieces plus the churn [I].
- **Caveats about the lever.**
  - It drops single 12-vertex hulls beside circling bots, while the game drops multi-chunk groups.
  - It despawns frozen pieces past 512, and each despawn wakes what rested on that piece.
  - The curve ran with the live cap off (8192); the game's default is 256.
  - The census-only zero showed 0 live pieces while bots broke houses. So the game's own breakage
    adds no debris load today, and every number here prices a future in which walls fall.

## 2. How the rule meets Avian 0.7 (source read, `vendor/avian3d-0.7.0-child-pose`)

- **F1. A body-type switch is a rebuild.**
  - `remove_body_on::<Insert, RigidBody>` (`src/collision/narrow_phase/mod.rs:170`, `:465–501`)
    removes every contact pair of the body, frozen or woken, and wakes its island. A comment at
    `:489` reads "TODO: Only remove static-static contacts".
  - The proxy moves between trees (`src/collider_tree/update.rs:206`, Case 6). `SolverBody` and
    `BodyIslandNode` are added or removed.
  - A woken piece therefore restarts with cold contacts: no warm-start impulse, and new pairs found
    by the next broad phase.
  - Box2D: "changing the body type later is expensive" [S].
- **F2. The freeze and Avian's sleep run together and undo each other.**
  - `IslandPlugin` and `IslandSleepingPlugin` are in the default group
    (`src/dynamics/solver/mod.rs:69–70`). No piece carries `SleepingDisabled` (grep).
  - The rest constants equal Avian's sleep defaults (`src/debris.rs:41–47`).
  - When a piece in an island Avian has already put to sleep is frozen,
    `wake_on_replace_rigid_body` (`islands/sleeping.rs:68`, `:125`) and `remove_body_on` wake the
    whole island and zero every member's sleep timer (`:530`).
- **F3. The wake threshold sits inside the gravity step.**
  - g·dt = 9.81 / 64 = 0.153 m/s, above `REST_SPEED_M_S` = 0.15 (DERIVED).
  - `wake_unsupported` wakes a frozen piece when any collider in `RestsOn` belongs to a live piece
    moving at ≥ 0.15 m/s.
  - `RestsOn` holds every collider the piece touched, side neighbours included.
  - So a neighbour that loses support for a single step, which F1's dropped pairs can cause, counts
    as moving, and the wake propagates [I].
- **F4. The wake box is larger than anything that can touch rubble.**
  - `wake_ahead` takes the union of the AABBs of every collider the mover owns
    (`src/debris.rs:273–287`).
  - That includes the tank's ballistic and witness volumes, which are trimeshes with
    `LayerMask::NONE` (`src/tank/spawn.rs:784–791`). The fixture's barrel module volume, for
    example, overhangs the hull.
  - A world-axis box around a rotated hull also covers more ground than the hull. For an a × b
    footprint the mean area over all headings is ab + (a² + b²)/π, which is 1.8× the footprint for
    a 2:1 hull (DERIVED).
  - The design asks for a test of "approaching swept geometry"
    (`wall-destruction.md`, "Rubble settlement").
- **F5. Every tick, the wake loops visit every frozen piece.**
  - `wake_ahead` is O(F × M), for F frozen pieces and M moving bodies.
  - `wake_unsupported` is O(F × |RestsOn|).
  - The game has no frozen cap, since rubble stays all match, so F grows through a match.
- **F6. Per-piece static freezing breaks finite mass through a chain** (correctness, [I]).
  - `thaw_touched` wakes a frozen piece only for a dynamic body that is not a piece
    (`src/debris.rs:319`).
  - So a woken piece pushed into a frozen one meets infinite mass, and a tank shoving a pile
    deeper than its wake box meets a wall.
  - The design asks for waking on "a relevant load". Catto: "Sleeping must be done per island, not
    per body" [S].
- **F7. Avian clones the static tree.**
  - `optimize_trees` clones the whole BVH of any tree with a moved proxy (`tree.bvh.clone()`,
    `src/collider_tree/optimization.rs:206`), under the defaults `optimize_in_place: false` and
    `use_async_tasks: true` (`:70–74`).
  - Every freeze, wake, chunk swap or dust event adds or removes a static proxy, so the static
    tree, holding the map plus every frozen piece, is cloned in most ticks.
  - At ~100 B per proxy (ASSUMED), that is tens to a few hundred µs. It is not the frozen-piece
    excess: a clone of 3649 proxies is ~0.4 MB (DERIVED).
  - Nothing in the game runs inside `PhysicsSchedule` (grep), and lightyear's tree snapshot is off
    (`rollback_resources` defaults to false; `src/net/protocol.rs:1141`). An in-place
    optimisation is therefore safe to try.
- **F8. Every contact pair is recomputed from scratch each step.**
  - `contact_query.rs:111` and `:174` are "TODO: Reuse manifolds" comments: the call passes fresh
    manifolds and no workspace.
  - The narrow phase also clones each pair's old manifolds every step (`system_param.rs:696`).
  - Jolt skips the narrow phase for a pair that moved less than 1 mm (`mUseBodyPairContactCache`),
    and PhysX keeps contact caches [S].
- **F9. Game-side overhead in the freeze.** `freeze_rested` allocates a `Vec` for every live piece
  every tick. `past_cap` looks up each candidate with a linear `resting.iter().any`, which is
  O(L × R) per tick once over the default cap of 256. The measured runs never reached that path,
  since they ran with the cap at 8192.
- **F10. The carrier sends jitter.**
  - `publish_poses` compares raw f32 poses with `set_if_neq` (`src/net/debris_stress.rs:418–430`).
    A live piece at rest in an awake island re-sends every tick.
  - Send cost was 0.32 µs per live piece per link on the M4 (DERIVED, one link). The Edgegap
    18–20 µs per piece is client-less, so it carries no send at all.
  - At 30 links, linear scaling (ASSUMED) would make send per piece comparable to physics.

## 3. What engines and shipped games do

- **Box2D v3 (Catto)** [S]:
  - Islands link only dynamic bodies through constraints: "Static bodies are not part of the
    island graph."
  - Sleep is per island: "If you put a body to sleep but not the whole island then you will see
    shapes begin to overlap". "Any single body in an island can keep it awake" (source comment).
  - Splitting runs at most one island per step, and "In some games there may be a large pile or
    tower of rigid bodies that are in a single island ... can dominate performance."
  - Defaults: sleep threshold 0.05 m/s, time to sleep 0.5 s.
  - "changing the body type later is expensive"; "Body disabling ... generally not good for
    performance".
- **Jolt** [S]:
  - The documented layer example includes "DEBRIS - Layer for all debris dynamic bodies, we want to
    test these only against the static geometry", with its own broad-phase tree, because
    "thousands" of debris objects are expected.
  - Islands sleep whole, and sleeping bodies wake on contact with awake ones. "Removing a body
    doesn't wake neighbors": the docs say to call `ActivateBodiesInAABox` with its bounds.
  - Per-body solver overrides (`mNumVelocityStepsOverride`, `mNumPositionStepsOverride`).
  - Body-pair contact cache, on by default, below 1 mm of relative motion.
  - Defaults: 0.5 s before sleep, 0.03 m/s point-velocity threshold.
- **PhysX 5** [S]:
  - Sleep is driven by low kinetic energy held over time.
  - Per-actor `setSolverIterationCounts`, though an island runs the maximum any of its actors asks
    for.
  - `eENABLE_STABILIZATION` is for "complex interactions between large numbers of bodies", is
    "experimental" and causes "some loss of momentum".
  - Contact caches give "faster contact generation".
  - Dominance groups give a one-way interaction.
- **Rapier** [S]:
  - Bodies wake when "another non-sleeping rigid-body starts interacting with them".
  - Extra solver iterations apply to "the whole island".
  - Dominance groups.
- **UE5 Chaos** (Python API 5.5/5.7) [S]:
  - Geometry collections have `remove_on_max_sleep` with `max_sleep_time` and `removal_duration`
    (pieces shrink and leave).
  - `slow_moving_as_sleeping` exists "to help removal of jittery but not really moving objects".
  - `size_specific_data` sets collision per piece size and "can also be empty", meaning no
    collision.
  - `optimize_convexes` merges shapes into "one single convex shape".
  - Clustering joins the mass and inertia of connected pieces into "one larger single rigid body",
    and a cache allows "smooth replay of complex destruction at runtime with minimal impact on
    performance".
  - `BodyInstance` offers per-body PhysX iteration counts, a `stabilization_threshold_multiplier`
    ("risk of loss of momentum at low speeds") and `one_way_interaction`.
- **Battlefield 6** (EA, 2025-11-10) [S]:
  - "level-designed debris piles";
  - "prioritization and culling of part destruction and debris spawning" in networking.
  - The server and client split is [?].
- **THE FINALS** [?]: the GDC 2024 talk (Isaksson, "Engineering Mayhem") covers "how we manage the
  physics state of our simulated debris". Its content sits behind a Vault login and was not
  reached.
- **Red Faction: Guerrilla:** debris budget and cleanup are [?].
- **Teardown:** covered by the earlier survey; the blog now returns HTTP 466.
- **Havok:** the docs are not public [?].

**Pattern** [I]: every engine sleeps whole islands, wakes on contact begin, and never toggles a
body's type to rest it. The cost tools they offer are a debris layer that skips debris-to-debris
pairs, per-size collision or none, a contact cache, per-body or per-island iterations,
stabilisation damping, and removal on sleep for cosmetic pieces.

## 4. Ranked menu

Lever names: **moving** = solver bodies, **pairs** = contact pairs and narrow phase, **wakes** =
body-type churn, **broad** = broad-phase proxies and trees, **solver** = iterations and substeps.
Everything in A is server-only and changes no input to any client, so all of it fits one timeline.

### A. Bug or optimisation class: same gameplay, cheaper; do now

| # | What | Source | Lever | Effect on our numbers | Seen or felt? |
|---|---|---|---|---|---|
| A1 | **Stop spurious wakes.** (a) Grow the wake reach only from the mover's colliders whose layers meet `Layer::Debris`, as a hull sweep rather than a world box (F4). (b) Do not freeze a piece inside a mover's reach (no freeze-then-wake). (c) Wake dependants when a support has moved some distance since the freeze, or above a wake speed clearly over g·dt, not at ≥ 0.15 m/s (F3). (d) A/B `SleepingDisabled` on pieces, so the freeze stops waking Avian-slept islands (F2). | Box2D (type change expensive); UE `slow_moving_as_sleeping`; Avian source F1–F3 | wakes, moving | Upper bound 98 % of live entries at 12:2. Each wake ≈ 0.46 ms of M4 server time and ~1.1 ms on Edgegap. Halving the wakes is about −1.4 ms M4 and −3 ms Edgegap tick p50 at 12:2 (DERIVED; the spurious share is unknown). | No. A piece beside a tank stays live instead of cycling. (c) at centimetre scale is invisible. |
| A2 | **Wake by query, not by loop.** `wake_ahead` queries the static tree per mover, O(M log F + hits). `wake_unsupported` uses a support-to-dependants index, driven by despawned colliders and moving pieces only. | Avian's own broad phase queries only moved proxies; Jolt `ActivateBodiesInAABox` | broad (game side) | 0.22 µs per frozen piece outside the step on the M4, an upper bound (DERIVED). The game keeps rubble all match: 5000 frozen is ~1.1 ms M4 and 4–8 ms on Edgegap at the 3.7–7× host factor (DERIVED/ASSUMED). | No: the same pieces wake. |
| A3 | **Linear cap selection, no per-tick allocations** in `freeze_rested` (F9). | code | game CPU | O(L × R) to O(L) once over 256 live. Small below ~1000 live (ASSUMED). | No. |
| A4 | **No static-tree clone:** `ColliderTreeOptimization { optimize_in_place: true }`, or `use_async_tasks: false` on 1 vCPU (F7). | Avian source and docs | broad | ASSUMED 0.05–0.3 ms in each tick a static proxy changes. Read `ColliderTreeDiagnostics::optimize` before and after. | No. |
| A5 | **Quantise the carrier pose before `set_if_neq`** (1 mm / 0.1°, ASSUMED invisible) (F10). | code | replication send | Removes jitter sends from live-but-resting pieces. At 30 links per-piece send may rival physics (0.32 µs × links, ASSUMED linear). | No, within the quantum. |
| A6 | **Persistent manifolds or a body-pair cache** in the vendored Avian narrow phase (parry supports persistent manifolds; F8). Larger work, an upstream-shaped patch. | Jolt `mUseBodyPairContactCache` (1 mm), PhysX contact cache, Avian TODOs | pairs | ASSUMED, in proportion to the narrow phase's share of the step. Read `CollisionDiagnostics::narrow_phase` first. | No (≤ 1 mm). |

### B. Trade-offs: the owner's call

| # | What | Source | Lever | Effect | Seen or felt | One timeline |
|---|---|---|---|---|---|---|
| B1 | **Sleep first.** Avian island sleep rests piles: contacts kept, no type change. Freeze only an at-rest piece whose island something else keeps awake, typically a tank contact. Keep a support-loss wake, since Avian does not wake bodies on a removed static collider (`narrow_phase/mod.rs:414`) and neither does Jolt. | Catto, Jolt, Rapier (per-island sleep, wake on contact begin) | wakes, pairs | Removes most body-type churn. Risk: a pile touching a parked tank stays awake as one island, which Catto warns "can dominate". Net unknown: A/B it. | Pushing becomes more correct: a shoved chain meets finite mass (fixes F6). Otherwise none. | Fits. |
| B2 | **No debris-to-debris collision.** Pieces collide with terrain, buildings and tanks only. Variant: only pieces below a size class skip each other. | Jolt DEBRIS layer | pairs, moving | A pile piece keeps 1 support pair instead of an ASSUMED 2.5–4 (−60–75 %), and each piece becomes its own island, so native sleep works per piece. | Yes: rubble interpenetrates, no mounds; tanks crest one layer. Conflicts with "retain gameplay-relevant occupied geometry". | Fits. |
| B3 | **Fine debris client-side.** Pieces below a gameplay-effect threshold (cannot stop a round or move a tank measurably) are never server bodies. Clients seed them from the wall record. | UE abandon-level and empty size-specific collision; owner table "Fine presentation debris may fade" | moving, pairs | Proportional to the share of pieces under the threshold. That share is unknown and set by the bake's chunk sizes. | Rounds pass through them, tanks do not feel them, and each client sees them differently. | Fits only if no gameplay ever reads them; the threshold must be a generic law from data. |
| B4 | **Shorter debounce or low-speed damping.** `REST_S` 0.5 → 0.25 s, or extra damping below a speed. | PhysX stabilization; UE stabilization multiplier | moving | Little's law: at 12:2, W ≈ 0.9 s, so ≤ 0.25 s off each episode is ≤ ~28 % fewer live (DERIVED). | Slow rolls stop sooner; slightly "sticky" rubble. | Fits. |
| B5 | **Cheaper shapes.** Fewer hull vertices or boxes for small pieces, or one compound collider per group instead of child colliders. | UE `optimize_convexes`, size-specific collision | pairs, broad | Per-pair cost unknown; read narrow-phase timers first. | Box poses miss the mesh by centimetres. Group chunks also serve ballistics (`Layer::Armor`), so the shape must stay per chunk. | Fits. |
| B6 | **Global substeps** 6 → 4. Avian has no per-body budget, unlike Jolt and PhysX. | Avian `SubstepCount` | solver | −33 % of solver phases (DERIVED); the solver's share is unknown. | Changes tank and track feel: out of debris scope. | Fits. |
| B7 | **Merge settled rubble** into a static compound, a heightfield bump or an authored pile. | BF6 "level-designed debris piles" | broad, pairs | Small: frozen static pieces already cost ~0 except their pairs (F1, §1). | Heightfield or authored pile: lost pushability and shape. Exact compound: none, at a high build cost. | Fits; the heightfield option conflicts with the one-surface doctrine. |
| B8 | **Baked fall playback**, kinematic. | Chaos cache | solver | Removes solving of falls only. | The fall ignores the impulse and the tanks, and kinematic pieces push tanks with infinite mass. Conflicts with "Callers do not decide which chunks fail". | Fits: deterministic data. |
| B9 | **Remove on sleep** for gameplay rubble. | UE `remove_on_max_sleep` | all | Large. | Rubble disappears; contradicts the owner decision that rubble persists. Listed only for completeness. | Fits. |

**Not a cost lever:** dominance or one-way interaction (Avian `Dominance`, PhysX, Rapier, UE
`one_way_interaction`). The contacts are still solved; only tanks stop feeling rubble.

## 5. Recommended next lane: "debris churn"

1. **Instrument**, server stats only.
   - Split `thawed` by cause: ahead, unsupported, touched.
   - Count freezes per piece.
   - Add Avian's always-on diagnostics resources to the stats line: `CollisionDiagnostics`
     (broad, narrow, `contact_count`), `SolverDiagnostics` and `ColliderTreeDiagnostics`
     (optimize, update). Add awake `SolverBody` and `Sleeping` counts.
2. **Apply A1 (a–c), A2 and A3.**
   - Gates: the three existing `src/debris.rs` tests, plus one new test: "a piece at rest beside a
     parked mover is never frozen and woken more than once". It names the churn invariant.
3. **Run 12:2 and 50:2 on the M4, A/B/B/A,** at live cap 256 (the game default) and 8192.
   - Success: thawed/s and live at 12:2 fall, and tick p50 falls by the DERIVED order above.
   - Add A1(d) and A4 as one-line A/B arms.
4. **Then decide from the per-phase split:** A6 if the narrow phase dominates, B1 or B2 if pile
   pairs dominate.
5. **Price the game's own falls** before more lever runs: one real wall group per period instead of
   single hulls. Group bodies with child colliders cost differently.

## 6. After the switch to native sleep (2026-10-31)

The owner chose B1's first half: rubble rests through Avian's island sleep, and the freeze rule
and its cap are gone (`src/debris.rs`; decision in `wall-destruction.md`, "Rubble settlement and the
cap"). F1, F2, F3, F4, F5, F6 and F9 no longer apply. The game keeps one wake Avian lacks: a
sleeping piece wakes when a collider it touches goes. Without it, a piece stays asleep in mid-air
after its static support despawns (MEASURED: the test fails with the observer removed).

New counts, MEASURED over 10 s on a fixture lattice of 54 cubes, 0.4 m, 2 cm apart, beside a
3 × 2 × 6 m box that never sleeps (a driven tank's stand-in):

| Case | Touching the box | Awake of 54 | Active pairs | Sleeping pairs |
|---|---|---|---|---|
| No box | 0 | 0 | 329 | 54 |
| Flush against the box | 2 | 3 | 351 | 51 |
| Pressed 1 cm into the box | 15 | 18 | 366 | 36 |
| Thrown at the box, 2 m/s | 6 | 36 | 308 | 18 |
| Pressed, box free to sleep | 18 | 0 | 329 | 73 |

- **The risk B1 named is real.** An island sleeps whole, so a pile against a hull stays awake
  through every piece touching it. At the README's ~8–9 µs per live piece per M4 tick, the thrown
  case's 30 extra awake pieces cost ~0.26 ms per tick per such pile (DERIVED). Under the freeze rule
  only the 6 hull-touching pieces stayed live. Avian offers nothing per body: `Sleeping` on one body
  sleeps its whole island, the tank included. A contact that generates no constraints does not link
  islands, but then the tank passes through the rubble.
- **A sleeping body is not free (new, A7).** `ContactGraph::sleep_entity_with` moves only touching
  pairs to the sleeping set. A non-touching pair whose AABBs still overlap stays active, and the
  narrow phase recomputes its manifolds every tick (`system_param.rs`, the `active_pairs_mut` loop
  has no sleep test). The fully sleeping pile keeps 329 such pairs, ~6 per piece. A vendored-avian
  patch candidate: move a non-touching pair to the sleeping set when its other body is asleep or
  static, and back when either wakes. The per-pair price, and with it whether sleeping pieces need
  a cap, is the next pricing lane's.

The §4 B trade-offs, with what these counts add:

| # | Status and numbers |
|---|---|
| B1 | Sleep: done. The other half, freezing an at-rest piece whose island a tank keeps awake, is not done: it would bring back the type switch for exactly the thrown case above (30 of 54 pieces). |
| B2 | No debris-to-debris collision: each piece becomes its own island, so the thrown case would keep only the 6 hull-touching pieces awake, and nearly all of the 329 near pairs of a sleeping pile are piece-to-piece (DERIVED). Rubble interpenetrates. |
| B3 | Fine debris as client cosmetics: no new numbers. |
| B4 | Shorter rest: now Avian's `TimeToSleep` resource (shared with every body) or a per-piece `SleepThreshold` component. Tanks never sleep while track forces act, so a shorter global time leaves tanks unchanged (DERIVED from `track/sim.rs`: a non-zero force wakes a body). No new numbers. |
| B5 | Cheaper shapes: no new numbers; fewer hull vertices would also cheapen the near pairs above. |
| B6 | Global substeps: no new numbers. |
| B7 | Merge settled rubble: would remove a sleeping pile's near pairs (329 for 54 pieces); otherwise small. |
| B8 | Baked falls: no new numbers. |
| B9 | Remove on sleep: no new numbers; still contradicts the persistence decision. |

A4 and A5 after the switch: a static proxy now changes only when a building swaps, dusts or drops a
chunk, so A4's per-tick static-tree clone no longer runs on every freeze and wake ([I]). A5 still
applies to awake resting pieces, such as the 36 in the thrown case, whose jitter still reaches
their carriers.

## Source index

Repository (read 2026-10-31):

- `.agents/scratch/perf-2026-10-31/README.md`
- `local-curve/c02–c05-*.window.log`
- `calib/{no-frozen-cap,frozen-cap-1}.txt`
- `src/debris.rs`
- `src/net/debris_stress.rs`
- `src/buildings/breakage.rs`
- `src/tank/spawn.rs`
- `src/net/protocol.rs`
- `.agents/docs/design/wall-destruction.md`
- `vendor/avian3d-0.7.0-child-pose/src/`:
  - `dynamics/solver/islands/{mod,sleeping}.rs`
  - `dynamics/solver/mod.rs`
  - `collision/narrow_phase/{mod,system_param}.rs`
  - `collision/broad_phase/bvh_broad_phase.rs`
  - `collider_tree/{mod,tree,update,optimization}.rs`
  - `collision/collider/parry/contact_query.rs`
  - `collision/contact_types/{system_param,contact_graph}.rs`
  - `dynamics/rigid_body/{mod,sleeping}.rs`
  - `dynamics/solver/schedule.rs`
- `vendor/lightyear_avian3d-0.30.1-child-pose/src/{plugin,rollback}.rs`

Web (read 2026-10-31):

- Catto, "Simulation Islands", 2023: https://box2d.org/posts/2023/10/simulation-islands/
- Box2D v3 docs, Simulation: https://box2d.org/documentation/md_simulation.html
- Box2D source, `src/types.c` (sleepThreshold 0.05) and `include/box2d/constants.h`
  (B2_TIME_TO_SLEEP 0.5), `src/solver.c`: https://github.com/erincatto/box2d
- Jolt Physics docs: https://jrouwe.github.io/JoltPhysics/ (Sleeping, Collision Filtering, Broad
  Phase); `BodyCreationSettings`:
  https://jrouwe.github.io/JoltPhysics/class_body_creation_settings.html ;
  `Jolt/Physics/PhysicsSettings.h` (master)
- PhysX 5.4.1, Rigid Body Dynamics:
  https://nvidia-omniverse.github.io/PhysX/physx/5.4.1/docs/RigidBodyDynamics.html ; Simulation:
  https://nvidia-omniverse.github.io/PhysX/physx/5.4.1/docs/Simulation.html ; `PxSceneFlag`:
  https://nvidia-omniverse.github.io/PhysX/physx/5.4.1/_api_build/structPxSceneFlag.html ;
  stabilization threshold via physx-sys docs:
  https://docs.rs/physx-sys/latest/physx_sys/fn.PxRigidDynamic_setStabilizationThreshold_mut.html
  [S~]
- Rapier, Rigid bodies: https://rapier.rs/docs/user_guides/rust/rigid_bodies
- Epic, Destruction Overview: https://dev.epicgames.com/documentation/en-us/unreal-engine/destruction-overview
  ; Python API `GeometryCollection` 5.7:
  https://dev.epicgames.com/documentation/en-us/unreal-engine/python-api/class/GeometryCollection?application_version=5.7
  ; `BodyInstance` 5.5:
  https://dev.epicgames.com/documentation/en-us/unreal-engine/python-api/class/BodyInstance?application_version=5.5
- EA, "How Battlefield 6 redefined destruction", 2025-11-10:
  https://www.ea.com/en/news/how-battlefield-6-redefined-destruction
- GDC Vault, Isaksson, "Engineering Mayhem" (GDC 2024), abstract only:
  https://gdcvault.com/play/1034280/Engineering-Mayhem-Technical-Deep-Dive
- Not reached:
  - Teardown multiplayer blog (HTTP 466 today);
  - the content of the THE FINALS talk;
  - Red Faction: Guerrilla debris internals;
  - Havok documentation.
