# Night pricing, 2026-11-02 (#202)

Subject: the game's client frame cost and dedicated-server tick cost; where it touches the damage
model, game state only (a spall cone, fragment cone or burst ring doses a tank's modules and crew
boxes, which are HP pools at stations). Labels: MEASURED (a run below), DERIVED (arithmetic on
labelled numbers), ASSUMED, CLAIMED (another record's word, not re-measured here).

Every time is from an Apple M4 (4 performance + 6 efficiency cores, 16 GB, macOS 26.5.2) and
does not transfer as a time; shares and before/after deltas do. An Edgegap figure, where given,
is DERIVED with the 3.7x host ratio of `../perf-2026-10-31/README.md` ("alpha.4 differs from
alpha.3"), itself DERIVED.

## The table

Per server tick unless marked; M4, `perf` profile; ranges over the runs. "Gates" are this table's
for a headless bed: the machine lock held, 0 `rustc` running at start, the bed finished `ok`.

| # | Item | Where | Condition | MEASURED cost | Against | Gates | Informs |
|---|---|---|---|---|---|---|---|
| 1 | Spall field: shipped sampler | `lane/night-pricing-202-fields` | cone 64 walks / ring 256 walks, a field | cone p50 633–641 µs, p95 1 117–1 122; ring p50 1 830–1 839, p95 3 026–3 066 | itself | passed, 2 runs | #204 |
| 1 | Spall field: exact visibility | same | snapped, a field | cone p50 2 398–2 405 µs, p95 6 356–6 414 (4.6x the sampler's mean); ring p50 4 550–4 553, p95 7 978–8 045 (2.1x) | the sampler, same fields | passed, 2 runs | #204 |
| 1 | Spall field: dot picture 64x8 | same | a field | cone p50 14 355–14 392 µs (20x); ring p50 17 509–17 598 (7.7x) | as above | passed, 2 runs | #204 |
| 1 | Spall field: dot picture 128x8 | same | a field | cone p50 28 600–28 648 µs (39x); ring p50 32 483–32 614 (14.5x) | as above | passed, 2 runs | #204 |
| 1 | Spall field: dot picture 256x8 | same | a field | cone p50 59 405–59 762 µs (82x); ring p50 70 698–70 937 (33x) | as above | passed, 2 runs | #204 |
| 1 | Spall field: dot picture 256, centre-only | same | a field | cone p50 12 576–12 597 µs (19x); ring p50 33 519–34 086 (16x) | as above | passed, 2 runs | #204 |
| 1 | Spall field: shipped sampler on main | main | the field casts' 24 cones, 64 walks each | p50 578–589 µs, p95 1 348–1 367 (1.07x the fields tree's, per-cone median) | the same bed on the fields tree: p50 547–552, p95 1 158–1 168 | passed, 3 runs each | #204 |
| 2 | Track prints skip still belts (client frame) | its parent, 2026-11-01 | 30 tanks at rest, visible window, `release` | ms a frame: p50 16.41–16.43, p95 29.62–29.67; not resolved: under the noise floor (the parent's own two runs differ 0.39 ms p50, 1.67 ms p95) | parent: p50 16.12–16.51, p95 29.62–31.29 | passed, A/B/B/A | #234 |
| 2 | The other speed-ups since the last pricing | their commits | sampled 30-tank server | not re-priced: each was timed before and after in its own commit (CLAIMED, item 2) | | | #186, #188 |
| 3 | Weight check, per change | main | one chunk to dust, 1 003 chunks | p50 1 193–1 201 µs, p95 2 080–2 096 µs | no change: 0 | passed, 3 runs | #189 |
| 3 | Weight check, long-wall shot | main | 204 chunks to dust at once | p50 1 344–1 391 µs, p95 1 407–1 449 µs (71 965 steps) | as above | passed, 3 runs | #189 |
| 3 | Weight check, then the gables | main | 118 more chunks | p50 12 314–12 534 µs, p95 12 727–12 791 µs | as above | passed, 3 runs | #189 |
| 4 | Sleeping pieces | main | 200 / 1 000 asleep, tank parked | +92 / +393 µs p50 a tick (0.4 µs a piece) | parked, no pieces: p50 908–928 µs | passed, 3 runs | #106, #189 |
| 4 | A pile held awake | main | tank parked on 120 pieces, 82 awake | +938 to +979 µs p50 a tick (drive phase +296) | parked, no pieces | passed, 3 runs | #106, #188 |
| 5 | The hole cap's shader cost | | | not priced: no lever puts a cap's worth of holes on screen (item 5) | | | #189 |
| 6 | Belt on a piece (R2) | main | driving over 120 pieces, 197 belt contacts p50 | +699 to +703 µs p50 a tick, of it the drive phase +167 to +171 | driving on flat ground: p50 972–985 µs | passed, 3 runs | #188 |
| 7 | Envelope scan in `crossing_reach` | deleted | | not priced: not on main | | | #209 |

## Method

- **Machine.** Each run held the machine lock (`/usr/bin/lockf -k -t 10800
  /tmp/overmatch-cargo.lock`), so no compile in another bay and no other measurement overlapped
  it; its log records the load average and the count of running `rustc` (0) at its start.
  `caffeinate -dims` and a 10 s screensaver reset ran all night. Every run was muted and
  unattended (`OVERMATCH_MUTE=1`, `OVERMATCH_UNATTENDED=1`).
- **Build.** `scripts/serial-cargo test --profile perf --locked --lib --features dev_ui,bitprobe
  --no-run`: the `perf` profile is release codegen without LTO (`Cargo.toml`), the server-tick
  method's (`../perf-2026-11-01/server-tick.md`), so a time here compares with other `perf` times
  only. The test binary was copied out of `target/` and run directly, one test thread.
- **Beds.** Each server-side price is an ignored pricing bed that asserts nothing about time:
  - `buildings::breakage::tests::the_weight_check_priced_per_change_and_after_a_long_wall_shot`
  - `headless_test::loose_pieces_priced_per_tick`
  - `ballistics::analytic_field::the_field_models_priced_per_field`, on the measuring branch
    `lane/night-pricing-202-fields` only (the two #204 prototypes are test-only code on a tree
    from 2026-10-31; see item 1).
  - `ballistics::field_casts::the_shipped_cone_sampler_priced_per_cone`, on main and on the
    measuring branch alike.
- **Window mode.** The beds are headless (`OVERMATCH_WINDOW=headless`): no window, no GPU. The
  one client frame price (item 2) ran a visible, frontmost window on the built-in 120 Hz panel,
  because a hidden window's frame loop free-runs (`scripts/perf/run-frame-sweep.sh`'s runbook);
  its gates are that runner's.

## Item 1: the spall field models (#204)

Subject: the game's damage model, game state only: how a spall cone or a burst ring doses a
tank's modules and crew boxes. **Where:** both prototypes are test-only code on 2026-10-31 trees
(`lane/fields-exact`, `lane/fields-raster`), 568 commits behind main; nothing on main calls either.
They are merged on `lane/night-pricing-202-fields`: the dot-picture prototype's tree, with the
exact prototype's `exact.rs`, its volume gather and `Env::exact` added beside it (the two files
were identical outside those). The shipped sampler priced is that tree's
(`resolve::field_direction` over the shipped directions, as `Env::sampled` walks it); main's is
priced against it at the end of this item.

**Bed:** `the_field_models_priced_per_field`, on the 49 fields of the prototypes' comparison (33
cones: the field-casts family at poses A and B and 9 fired plate cones; 16 burst rings fired by
`88mm_APHE_ring` and `88mm_HE`), each dropped where the comparison drops it. Per field, each
method's median of 7 repeats, interleaved method by method; one thread, as a throw runs on the
server. Each method's time is all of its work: the sampler's reach and walks; the exact field's
volume gather (it copies every box's trimesh in reach), clipping and integration; the picture's
axis walk, scene and drawing. Two runs (03:02, 03:03; MEASURED, `perf`).

| µs a field, run 1 / run 2 | p50 | p95 | mean | max | mean / sampler |
|---|---|---|---|---|---|
| cone, sampler (64) | 641 / 633 | 1 117 / 1 122 | 665 / 670 | 1 322 / 1 336 | 1 |
| cone, exact | 2 405 / 2 398 | 6 414 / 6 356 | 3 070 / 3 058 | 7 698 / 7 631 | 4.6 |
| cone, picture 64x8 | 14 355 / 14 392 | 16 543 / 16 704 | 13 414 / 13 482 | 18 337 / 18 443 | 20.1 |
| cone, picture 128x8 | 28 648 / 28 600 | 33 542 / 33 509 | 25 845 / 25 934 | 37 486 / 37 561 | 38.7–38.9 |
| cone, picture 256x8 | 59 405 / 59 762 | 74 021 / 74 268 | 54 444 / 54 540 | 79 621 / 79 845 | 81.4–81.8 |
| cone, picture 256 | 12 576 / 12 597 | 15 867 / 15 913 | 12 620 / 12 619 | 17 812 / 17 481 | 18.8–19.0 |
| ring, sampler (256) | 1 830 / 1 839 | 3 066 / 3 026 | 1 953 / 1 950 | 3 112 / 3 098 | 1 |
| ring, exact | 4 553 / 4 550 | 8 045 / 7 978 | 4 062 / 4 054 | 8 859 / 8 827 | 2.1 |
| ring, picture 64x8 | 17 598 / 17 509 | 22 673 / 22 705 | 15 078 / 15 064 | 23 117 / 23 037 | 7.7 |
| ring, picture 128x8 | 32 614 / 32 483 | 44 134 / 44 209 | 28 383 / 28 268 | 45 924 / 45 864 | 14.5 |
| ring, picture 256x8 | 70 937 / 70 698 | 97 339 / 97 623 | 63 853 / 63 602 | 105 241 / 104 521 | 32.6–32.7 |
| ring, picture 256 | 34 086 / 33 519 | 35 775 / 35 723 | 31 781 / 31 678 | 38 006 / 37 652 | 16.2–16.3 |

Against the 15.625 ms of a 64 Hz tick (DERIVED): a ring costs 12 % of an M4 tick under the
sampler, 29 % under the exact field and 113 % under the 64x8 picture (p50); on Edgegap's one vCPU
(DERIVED, 3.7x) about 6.8 ms, 16.8 ms and 65 ms, so the exact field alone fills a tick per
ring and the picture fills four.

- **Exact visibility is the cheaper of the two models: 2.1x the sampler on rings, 4.6x on
  cones**, and the only one within a tick on the M4 at its worst field (8.9 ms). Its accuracy is
  the prototype's record (CLAIMED there, `analytic_field.md` on `lane/fields-exact`): 105 of 106
  cone and 29 of 32 ring rows within 1 HP or 5 % of the reference aim, no knockout flip. Its own
  record names reductions not built: integrating only silhouette edges, a bake-time convex hull
  per box, and not copying trimeshes per throw.
- **The dot picture costs 8–82x the sampler at the settings that match.** Rings need 256x8 to
  match every row within one reference (CLAIMED, `analytic_field.md` on `lane/fields-raster`):
  71 ms p50, 4.5 M4 ticks a ring. 64x8 already costs 7.7x (rings) and 20x (cones); centre-only
  256 costs 16–19x and leaves the thin openings all-or-nothing. Its record names reductions not
  built (testing sub-samples only against faces whose outline crosses the dot, adaptive
  refinement).
- **The sampler's walks stay on the server either way** while the pen cam draws per-direction
  lines (#204's second question): a model's cost adds to them unless the picture changes.
- The per-field rows are in the bed's output (`fields-1.log`, `fields-2.log`, not committed). The
  costliest fields are the rings that reach the hull's interior (HE and APHE bursts at the
  driver, turret-left, oblique: picture 256x8 82–105 ms). Four rings cost the exact field
  466–472 µs (APHE rear, oblique and engine, HE right-rear; MEASURED); that their origins are
  blocked (a burst in or on a plate, decided by one walk) is ASSUMED from the prototype's rule.

**The sampler on main against the sampler above.** `ballistics::field_casts::
the_shipped_cone_sampler_priced_per_cone`, the same bed on both trees (committed on each branch):
the field casts' 24 cones (64 directions each, poses A and B), a cone's reach then every
direction's walk, as the march's `walk_field` throws it, its median of 25 repeats run in rounds.
Each binary ran against a copy of its own tree's `assets/`; three runs each, interleaved main,
fields, main, fields, main, fields (04:04; MEASURED, `perf`).

| µs a cone | p50 | p95 | mean | max |
|---|---|---|---|---|
| main | 578–589 | 1 348–1 367 | 653–655 | 1 441–1 465 |
| the fields tree (2026-10-31) | 547–552 | 1 158–1 168 | 597–602 | 1 213–1 229 |

- **Main's sampler costs about 1.07x the one item 1 priced against**: the per-cone ratio of
  medians is 1.065 at its median, 0.98 to 1.18 over the 24 cones (DERIVED). Each model's ratio
  above would read about 7 % lower against main's sampler if the model's own cost had not moved
  with it (DERIVED, ASSUMED unmoved). Between the two trees both the walk code
  (`resolve.rs`, `walk.rs`) and the probe tank's data (`tiger_1.tank.ron`, `tiger_1.sim.glb`)
  changed, so the ratio prices main against that tree, not one commit.

## Item 2: speed-ups landed since the last pricing

**Priced:** "track_prints: skip belts none of whose inputs moved; keep the stamp scratch (#234)",
the one client-frame speed-up, against its parent ("net: crew snapshot compared in place, applied
on change, swap seats found through the tank (#234)"), both at their own tree of 2026-11-01.
`scripts/serial-cargo build --locked --release --bin overmatch --features dev_ui,bitprobe` per
arm (one feature set for every agent build; the shipped build has neither feature, so the
absolute times are not the shipped build's, the difference between arms is). The scene is the
frame sweep's `m350-30`: 30 tanks at rest (28 probes), shipped shadow distance, uncapped
(`run-frame-sweep.sh`'s conditions and every gate of `frame-run.zsh`, run from the commit's tree
by a scratch runner that swaps the binary per condition). One throwaway warm run per arm (15 s,
the shader cache), then parent, commit, commit, parent (03:41–03:47; MEASURED).

| Condition | Frames | ms p50 | ms p95 | ms p99 |
|---|---|---|---|---|
| parent-1 | 3 581 | 16.51 | 29.62 | 42.05 |
| commit-1 | 3 594 | 16.43 | 29.67 | 41.87 |
| commit-2 | 3 590 | 16.41 | 29.62 | 41.35 |
| parent-2 | 3 615 | 16.12 | 31.29 | 43.57 |

Gates passed in all four: `window: visible`, effective present mode Immediate with the frame cap
off, one 120 000 mHz primary monitor and one 2560x1440 surface over the whole window, no
occlusion in it, the 3D camera drew on at least 99 % of frames, 28 probe tanks spawned, no panic.

- **Not resolved.** The commit's two runs read p50 16.41–16.43 ms and the parent's 16.12–16.51
  ms: the parent's two runs differ by 0.39 ms p50 and 1.67 ms p95, more than any commit-to-parent
  difference. The saving is under this A/B's noise floor, about 0.4 ms p50 (DERIVED from that
  spread). A frame is as long as the slower of the main world and the render pipeline; whether
  the main world bounds this one, where the saving would show, is not measured.
- **Not re-priced: the others were timed when they landed.** Each carries its own MEASURED
  before and after in its commit message (sampled 30-tank server, 60 s, thread time per system;
  whole tick A/B/B/A together), CLAIMED here and not re-measured, since a re-price needs a bed at
  each parent's tree of 2026-10-31:
  - "ballistics: a field throw reads the broad phase once for all its directions (#186)":
    `integrate_projectiles` 448 -> 220 ms of thread time, `resolve::candidates` 204 -> 0.
  - "buildings: the deposits skip a building whose ball a blast ray or a field stop misses
    (#188)": `apply_deposits` 252 -> 111 ms.
  - "buildings: the push rule takes each source's crushed radius once per material (#188)":
    `fracture::strike` 13 -> 7 ms.
  - "tank: build a tank's armour trimeshes and hull colliders once, shared by every spawn
    (#186)": `respawn_dead_bots` 97 -> 1 ms over 24 respawns, a cost per spawn.
  - The first, second and fourth with the rest of their lane's volley fixes, whole tick, as each
    of those commits states it: worst second 178.2 / 192.4 -> 107.2 / 109.0 ms.
  - Server-perf-2's four speed-ups were priced against their parent tree by "perf: price
    server-perf-2 against server-perf-1's tree (#250)" (CLAIMED there).
  - "ballistics: record live crossing evidence without cost sampling (#191)" is instrumentation
    (an optional shot trace), not a speed-up; not priced.

## Item 3: the building weight check

`fracture::weigh` on the shipped house (`house_1`, fixture data), three runs (02:17, 02:51 twice;
MEASURED, `perf`, one thread). Per change: each of the 1 003 standing unburied chunks turned to
dust alone from the intact house, then one weigh, the median of 5 repeats per chunk. After a
long-wall shot: the 204 chunks of the long wall under the roof's fixed plate turned to dust at
once, then the 118 chunks of the gables the roof rests on (the removals of
`a_nailed_roof_distinguishes_rests_from_local_fixing_failure`), 25 repeats each.

| Change | µs p50 | µs p95 | µs mean | `Weighed::steps` | Failures |
|---|---|---|---|---|---|
| One chunk to dust, over the 1 003 chunks | 1 193–1 201 | 2 080–2 096 | 1 258–1 260 | p50 42 421, p95 144 651, max 228 235 | |
| The long wall, 204 chunks at once | 1 344–1 391 | 1 407–1 449 | 1 356–1 396 | 71 965 | 0 |
| Then the gables, 118 chunks | 12 314–12 534 | 12 727–12 791 | 12 389–12 531 | 1 415 482 | 2 |

Ranges are over the three runs. The brief's "76,532 operations" for the long-wall shot (CLAIMED)
reads 71 965 `steps` on this tree.

- **Per change the check costs about one twelfth of an M4 tick**: 1.2 ms p50 of the 15.625 ms a
  64 Hz tick has (7.7 %, DERIVED), 2.1 ms at p95. On Edgegap that is about 4.4 ms p50 (DERIVED,
  3.7x), 28 % of the tick, for every deposit that turns one chunk to dust.
- **Its time hardly depends on the change's size**: one chunk 1.2 ms, 204 chunks at once 1.37 ms
  (MEASURED). What is per call and what is per touched chunk is not measured here. ASSUMED: the
  passes' whole-structure work (`loaded_graph`, `loose_groups`, `incident`) and the section read,
  which `SectionCache` repeats after any crack or fate change, so after every change.
- **The gables after the long wall are the worst case measured: 12.4 ms**, 79 % of an M4 tick and
  about three Edgegap ticks (DERIVED, 3.7x), from 1.4 M steps in the passes that open the roof's
  fixings (2 failures).

## Item 4: loose pieces, sleeping and held awake

`headless_test::loose_pieces_priced_per_tick`: the offline sim booted headless (the shipped
map, the controlled tank and the single-player duel partner), one fixed tick per `App::update`,
each window 640 ticks, three runs (02:18, 02:51 twice; MEASURED, `perf`). Pieces are fixture
boxes 0.5 x 0.25 x 0.5 m at 1 800 kg/m3, spawned at rest, under the game's rest rule
(`DebrisPiece`). The driving phase is the wall time between stamps ordered before and after
`SimPhase::DrivingForces`, an upper bound on the drive step (`DrivePhaseClock`). The bed's
default task pools use every core; the dedicated server on Edgegap has one vCPU.

| Window | Tick µs p50 | Tick µs p95 | Driving phase µs p50 | Pieces awake |
|---|---|---|---|---|
| Parked, no pieces (the baseline, 4 windows a run) | 908–928 | 946–1 132 | 183–188 | |
| Parked, a 200-piece pile 40 m ahead, all asleep | 998–1 013 | 1 055–1 067 | 183–184 | 0 of 200 |
| Parked, a 1 000-piece pile 40 m ahead, all asleep | 1 300–1 311 | 1 403–1 622 | 184–185 | 0 of 1 000 |
| Parked on a 120-piece carpet | 1 856–1 896 | 1 931–2 158 | 481 | at most 82 of 120 |

- **A sleeping piece costs about 0.4 µs a tick**: +92 µs for 200 and +393 µs for 1 000 (DERIVED,
  each window against the mean of the baselines either side, mean of three runs), 0.46 and 0.39 µs
  a piece. None of it is in the driving phase. A shipped map's bound of 86 062 building pieces
  (`debris.rs`, MEASURED there) would cost about 34 ms a tick at that rate if they all fell
  (DERIVED, linear, not measured past 1 000).
- **A pile held awake by a parked tank costs about 0.95 ms a tick** for the 120-piece carpet:
  +938 to +979 µs p50 (DERIVED), of which the driving phase is +296 µs and the rest (+650 µs) lies
  outside it (ASSUMED: the solver with 82 pieces awake in the tank's island).

## Item 5: the hole cap's shader cost

Not priced. `HOLES_PER_CHUNK` is 16 on main (`src/buildings/breakage.rs`; ASSUMED by its own
doc: "no measured cost or look sets it yet"). The hole path (`assets/shaders/building_holes.wgsl`'s
`hole_at`, asked by `building_surface.wgsl` and its prepass) compiles only into the alpha-masked
material a building with a hole wears, and runs on the faces of chunks that name a law chunk
(a whole element's faces hold none), each fragment against up to 16 capsules and the chunk's hull
planes (read from the shaders). Its cost is set by holed chunks on screen (ASSUMED). No lever puts holes there: a hole reaches the
view's table only through the authority's deposits into a building's `Breakage`, and neither the
frame sweep nor a capture plan can place a deposit, let alone the cap's 16 per chunk on every
visible chunk. Pricing it needs a dev lever that writes holes into an on-screen building's
breakage at spawn (fixture data), then the frame A/B of item 2 at 0, 4 and 16 holes a chunk.

## Item 6: the belt on a piece (tracks on rubble, step R2)

Same bed and runs: the controlled tank driven at throttle 0.35 over the 120-piece carpet, each
tick classed by whether a belt bore on a piece (a `TrackContacts` entry on a piece with load).

| Window | Ticks | Tick µs p50 | Tick µs p95 | Driving phase µs p50 | Belt contacts on pieces |
|---|---|---|---|---|---|
| Driving on flat ground before the carpet | 427 | 972–985 | 1 009–1 248 | 186–189 | 0 |
| Driving with a belt on a piece | 1 381 | 1 675–1 684 | 1 925–1 935 | 356–357 | p50 197, p95 514, mean 252 |
| Driving on flat ground after it | 201 | 976–980 | 1 021–1 048 | 187–189 | 0 |

- **A tank driving on rubble costs about 0.7 ms a tick more than on flat ground**: +699 to +703
  µs p50 (DERIVED). Of it, **the drive step's share is about 170 µs** (+167 to +171 µs of driving
  phase, DERIVED), about 0.9 µs per belt contact on a piece (DERIVED, 170 / 197, p50 over p50).
  The other ~530 µs lie outside the driving phase (ASSUMED: the pieces the belt wakes, in the
  solver).
- The drive step's rubble path (gathering the pieces, their support, `piece_impulses`,
  `RubbleBodies::record_moving_contacts`, the pushes) is not split further: the bed times the
  phase, not its functions.

## Item 7: the envelope scan in `crossing_reach`

Not on main: "ballistics: resolve the game's damage model in relative collider frames (#191)"
deleted envelope baking with `crossing_reach` (`git log -S"fn crossing_reach"`; no `crossing_reach`
in `src/`). Not priced, as the issue directs.
