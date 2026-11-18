# Cruise as desired speed

Fixture numbers below come from `assets/tiger_1/tiger_1.tank.ron`, `assets/terrain/ground_types.ron` and `assets/driving/*.ron`, as an example only. Nothing was built or run; every DERIVED figure is hand arithmetic from those files.

## Verdict

- **The code does not do this today.** A cruise notch is a throttle fraction: a share of the fuel rack. It has no speed meaning.
- **Speed-hold fixes three things the owner describes:** the weak first notch, the surge after a crest, and belts running away when grip breaks.
- **It will not free a hung tank.** The one persistent stuck case measured on the current tree is a beached hull at full pedal. That is a separate cause with its own parked work.
- **Decision:** build it as a moved governor ramp plus one gear-choice rule, keep the input grammar and the wire shape, and go to four notches spaced geometrically across each tank's own gear ladder.

## 1. Input to track force today

- **Wire type:** `TankCommand { throttle, steer, .. }`, both in [-1, 1] (`src/command.rs:17-21`). The comment calls throttle "target throttle".
- **Cruise:** client-only `DriveInput` (`src/command.rs:415-426`), never replicated.
  - A tap shorter than 0.15 s steps a persistent notch; there are 3 equal notches per side (`assets/input/drive.ron`, `tap_notch` at `src/track/drive.rs:76-84`).
  - A hold reads as full axis and clears the notch on release; an opposing tap returns to neutral (`AxisLever::sample`, `src/command.rs:487-528`).
  - Steer has no notch (#381, closed).
- **Server seam:** `drive_target` copies throttle and steer into `DriveIntent::Axes` (`src/track/sim.rs:1078-1095`). `shape_drive` slews them at 4.0 per second (`src/track/drive.rs:112-125`).
- **Throttle is fuel:** `driver()` sets `propulsive = (throttle * dir).clamp(0, 1)` (`src/track/transmission.rs:1987-1991`). `engine_torques` multiplies the torque curve by it: `induced = rate * (command * curve + released * cut_drag)` (`:1148-1179`).
- **Governor:** `fuel_cut` is a proportional ramp from governed rpm over a 100 rpm band (`:837-840`, `assets/driving/engine.ron`). It is a top-speed limiter only.
- **Two more ramps of the same form exist:** the idle catch (`:1173-1178`) and the steer/shift rev-match (`:2057-2063`).
- **Gears:** `driver::choose` (`src/track/driver.rs:521-675`) prices each gear by "the pedal's own fuel" at the requested command.
  - It steps down when the engaged gear is short of the sensed load.
  - It steps up to a viable higher gear that pulls harder where the cut lands.
  - Every change pays a cut with the drive uncoupled (0.31 s on the fixture).
- **Steering:** detents at 0.15 and 0.8 of steer travel select a geared radius per gear (`update_steer_detent`, `:2017-2025`). The steering member is a bounded row tied to the crank. `side_commands` is probe telemetry only (`:1061-1062`).
- **Reverse:** the sign of throttle selects the ladder after a 0.5 s hold at rest (`swap_ladder_direction`, `:1868-1888`). An opposing request while moving is the service brake.

## 2. Meeting resistance at a partial setting

- **Speed sags.** Nothing adds fuel above idle. The Driver downshifts through paid cuts, never upshifting in a detent turn.
- **Lowest-gear limit is grip, by a wide margin.**

| Quantity (fixture) | Value | Label |
|---|---|---|
| First-gear crank-to-belt ratio | 403.9 rad/m, from 2.8 km/h at 3000 rpm | DERIVED |
| Peak tractive effort, first gear | 1850 N·m × 403.9 = 747 kN (clutch 2400 N·m not binding) | DERIVED |
| Weight | 559,170 N | MEASURED (`.agents/scratch/crush-probe-slice-0.md:19`) |
| Effort over weight | 1.34 | DERIVED |
| Grip, mud / dirt / concrete / gravel | 155 / 234 / 335 / 398 kN (grip 0.277 / 0.418 / 0.600 / 0.712) | DERIVED |
| Effort over grip | 1.9 (gravel) to 4.8 (mud) | DERIVED |
| Fourth-gear peak effort | 227 kN, the first gear where the engine limits before dirt grip | DERIVED |

- **But the first notch throws most of that away.** From rest the clutch slips at the take-up floor, `launch_omega(command)`, which is 1100 rpm at one notch (`:842-845`, `:1626-1631`). The pedal's net torque there is about 379 N·m.
  - Push from rest at one notch: about 153 kN, 0.27 of weight (DERIVED). That is 65% of dirt grip and 38% of gravel grip.
  - Slope it can start on: about 12 degrees, against the sheet's rated 35 (DERIVED, dirt rolling included).
  - Push from rest at two notches: about 441 kN, above every grip row (DERIVED).
- **One inconsistency:** a clutch already locked in first gets the idle catch, up to the idle full-load torque, between 500 and 600 rpm. So a tank that is still moving creeps strongly at 0.5 km/h, while one that stopped pushes at 0.27 of weight.
- **Flat-ground speed per notch on concrete:** 16.7 / 38.0 / 38.3 km/h (DERIVED, rolling 0.04). Notches two and three differ in acceleration only.
  - One notch on dirt is about 10.8 km/h and in mud about 3.7 km/h (DERIVED).
  - No notch is a creep, and none means the same thing on two surfaces.

## 3. Jolt and surge sources in the drivetrain

- **Crest surge.** A notch that crawled up the pile runs back up the gears to its flat-ground speed on the far side, 11 to 17 km/h on the fixture. Each upshift moves up to three gears.
- **Shift cuts under load.** Climbing from sixth to first costs two cuts with the hull uncoupled. #398 records that a change started from a stall leaves the tank unbraked for the cut (MEASURED there).
- **Belt runaway.** When grip breaks under throttle-hold, the belts run to the gear's governed speed. #374 measured a hull at rest in fourth with belts at 2.16 m/s. Re-gripping from that speed is a hit.
- **Not verified:** whether the slipping-to-locked step in section 2 is felt as a lurch.

## 4. Related work

- **#374 (open):** light pedal on a heap sat in first under 0.05 m/s for about 2,000 of 2,560 ticks (MEASURED). Cause is listed as unknown; the 0.27-of-weight push above is consistent with it. The crush probe says those stalls no longer reproduce.
- **Beaching is the measured stuck case.** At full pedal the hull proxies carry 61 to 69% of weight and the tank creeps at 0.03 m/s (`crush-probe-slice-0.md:13,40`; `rubble-bed-fair-test.md:191`). Tracks carrying a third of the weight have a third of the grip.
- **Belt-on-piece defects (open):** #353 (belt friction billed a tick late), #347 (belt force flips sign each tick on a loose piece while the clutch is out or slipping) and #351 (no rolling resistance on loose pieces).
- **Drivetrain slices:** slice 4 (belt rows, closes #353, #347, #312) and slice 5 (groups and cost) are in `.agents/scratch/drivetrain-rows-design-2026-11-17.md`. Slices 1 to 3 have reports; slice 4 has an amendment (`belt-rows-coulomb-amendment-2026-11-18.md`).
- **Owner checkpoints:** #333 (drive the gearbox over rubble, decide on crushing), #397 (upshift picks the tallest viable gear), #398 (0.547 s pedal to first force, MEASURED), #309.

## 5. Design

**Base truth.** The real mechanism is an all-speed governor: the lever sets a crank speed, and flyweights open the fuel rack in proportion to the shortfall, up to the full-load stop. At the resolution a player perceives, that is "the Driver holds the ordered speed with his foot; the engine gives up to its curve; the tracks give up to grip". The fixture's own engine historically had only a top-speed governor, so in the game the holder is the Driver crew model, not an engine part. That keeps it generic and keeps it tied to the Drive task rate.

**Control law.**
- Ordered speed `v*` comes from the lever share and the tank's engaged ladder.
- In the engaged gear, set rpm is `v* × ratio / sprocket radius`, clamped to [idle, governed].
- Rack is `clamp((set − crank) / droop, 0, 1)`, with droop the existing `governor_cut_rpm`.
- This is `fuel_cut` with its ramp moved from governed rpm to the set rpm. At full lever the set rpm is the governed rpm in every gear, so the law reduces to today's.
- Stiffness is unchanged: the ramp has the same width as today's cut, so there is no new stability regime (DERIVED: step gain about 0.63 in first gear at 64 Hz, below the explicit limit of 2).
- Speed sags at most 100 rpm in 2500, 4% of the gear's range, before the rack is full (DERIVED). Past that the engine lugs up its torque rise, then the existing short-gear downshift runs.

**Gear rule.** The Driver rides the lowest gear that reaches the order: a higher gear is offered only when the engaged gear's governed speed is below `v*`.
- At full lever every gear is needed, so today's upshift logic is untouched.
- At a creep order the tank stays in first: no cuts on a pile.
- Downshift on a short gear is unchanged.
- New clause: when the order drops below current speed, fuel cuts and the engine drags in gear. The Driver steps down when the crank reaches idle. No automatic service brake; the opposing key remains the brake.

**Take-up.** The slipping floor follows the set rpm (no higher than today's full-pedal floor, no lower than idle), so push from rest is grip-limited at every order. Belt speed can never exceed `v*`, so a slipping belt scrabbles at creep speed.

**Settings.** Four notches per side (`notch_count` 3 to 4, a Taste in `drive.ron`), spaced geometrically from the lowest gear's governed speed to the top gear's.
- Fixture forward: 2.3 / 5.9 / 14.9 / 37.8 km/h (DERIVED). Fixture reverse: 2.3 / 3.5 / 5.2 / 7.7 km/h (DERIVED).
- The lattice derives from each tank's ladder; no speed is authored.
- Hold stays full; opposing tap stays neutral; taps, tap window and slew are unchanged.
- Floor: a locked first gear cannot run below idle, 0.56 km/h on the fixture (DERIVED).

**Steering.** The detent law, radii and pivot are unchanged. The rev-match fuel takes the larger of its own request and the governor's. A geared turn's scrub is now supplied up to the full rack, so a turn at a low order holds speed where it used to sag and step down. First gear has the tightest radii, which suits ruins.

**Reverse.** Unchanged: sign selects the ladder after the same hold, and `governed_speed(reverse)` already exists (`:732-742`).

**Downhill.** The same ramp cuts fuel above the order, and motoring drag holds it: about 263 kN in first on the fixture, 0.47 of weight (DERIVED). The far side of a pile is taken at the ordered creep.

**Flat-ground feel against today.**
- Holding W: the same law, so the same drive.
- Taps: each notch is now a fixed speed on every surface, reached at full rack. Acceleration to a low notch is brisker than today's part throttle.
- Risk to feel: a creep launch on dirt reaches 0.65 m/s in about 0.16 s at the grip limit, against about 0.25 s today (DERIVED). If that reads as a lurch, the missing truth is the Driver's clutch feed, recorded as a playtest fork, not a tuned constant.

**Wire.** No shape change: `throttle: f32` still carries the lever share, and the server maps share to speed from the tank's data. Rename the doc comment to "ordered speed share". The meaning changes, so client and server must match. The `PROTOCOL_REV` comment (`src/net/protocol.rs:34`) says every wire change bumps it and does not say whether a meaning-only change counts; I would bump it.

**Build outline.** Line counts are DERIVED from grep site counts times an assumed 1 to 3 lines per site.

| Step | Where | Lines |
|---|---|---|
| Share to ordered speed, per ladder | `transmission.rs`, beside `governed_speed` | about 15 |
| Moved ramp: `fuel_cut` takes a set rpm (6 sites), `engine_torques` (6 sites), `pedal_peak` knots, take-up floor, `driver()` | `transmission.rs` | about 40 |
| Gear rule in the free launch choice and the upshift clause, plus the overrun step-down | `driver.rs` `choose` | about 25 |
| Notch count; gauge ticks and an ordered-speed readout | `drive.ron`, `drive_hud.rs` | about 11 |
| Four class-invariant tests: speed held within droop under any load below the gear's hold; full lever equals the governed law; belt speed never exceeds the order under grip loss; no gear change at an order inside first gear's range | `transmission/tests.rs`, `driver.rs` | about 120 |
| Review of 21 existing part-command fixture sites | `turn_probe.rs`, `rubble_climb.rs`, `belt_friction.rs`, tests | reported once as moved observations |

Total about 210 lines. The throttle-fraction path is deleted in the same change.

**Cheapest experiment.**
1. With no code: on `main`, tap W once and drive at a pile, then twice. One notch should push weakly from rest; two should climb and then run to near top speed on the far side. That confirms the dilemma before anything is built.
2. One lane, about 12 lines in `driver()` (`transmission.rs:1987-2006`): compute the rack from belt-speed error against `|throttle| × governed_speed`, pass full command to `choose`, and add the one-line upshift filter. Drive `overmatch --offline` across flat ground, a pile and its far side. The lane deletes the arm when it reports.

## Open

- Whether the idle-catch step in section 2 is felt.
- The overrun step-down is the only new Driver behaviour, and the least certain part of the build.
- Whether four notches is the right count: a Taste for the owner.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/src/track/transmission.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/driver.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/drive.rs
- /Users/Yan/github/vikng-dev/overmatch/src/command.rs
- /Users/Yan/github/vikng-dev/overmatch/assets/input/drive.ron
