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
| 3 | Weight check, per change | main | one chunk to dust, 1 003 chunks | p50 1 193–1 201 µs, p95 2 080–2 096 µs | no change: 0 | passed, 3 runs | #189 |
| 3 | Weight check, long-wall shot | main | 204 chunks to dust at once | p50 1 344–1 391 µs, p95 1 407–1 449 µs (71 965 steps) | as above | passed, 3 runs | #189 |
| 3 | Weight check, then the gables | main | 118 more chunks | p50 12 314–12 534 µs, p95 12 727–12 791 µs | as above | passed, 3 runs | #189 |
| 4 | Sleeping pieces | main | 200 / 1 000 asleep, tank parked | +92 / +393 µs p50 a tick (0.4 µs a piece) | parked, no pieces: p50 908–928 µs | passed, 3 runs | #106, #189 |
| 4 | A pile held awake | main | tank parked on 120 pieces, 82 awake | +938 to +979 µs p50 a tick (drive phase +296) | parked, no pieces | passed, 3 runs | #106, #188 |
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
- **Window mode.** The beds are headless (`OVERMATCH_WINDOW=headless`): no window, no GPU. No
  client frame was measured (items 2 and 5 below say why).

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
