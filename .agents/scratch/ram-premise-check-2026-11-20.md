# Ram premise check

Subject: the game's building model (game state only): a brick house when a heavy vehicle drives into
it. Nothing was built or run. Citations are `main` unless a lane is named. MEASURED figures name who
measured them; DERIVED ones show their arithmetic. Asset counts are DERIVED by a script over
`assets/constructions/house_1/house_1.construction.ron`.

## 1. The fixture cannot answer "does the whole house fall"

Verified:
- House: 4.0 m across the gable, 6.0 m along the ridge, walls 0.25 m, so 3.5 m between the long
  walls (`tools/constructions/house_1.house.ron:11-14`). The lab drives along −Z into the 4.0 m
  gable wall and down the 6 m length (`lane/ram-plug:src/net/debris_lab/scene.rs:88-101`).
- Track edges at x = ±1.88 m (MEASURED, ram-plug report). The long walls occupy |x| 1.75 to 2.0 m:
  each track edge meets a long wall on its centre line (1.875 m) and pushes it end-on.
- All 994 brick chunks hold matter inside |x| < 1.88 m: the vehicle's outline covers every wall.
- 160 brick chunks lie wholly below grade (the 0.45 m foundation). "160 of 1,122 standing" is that
  number: nothing above ground stands, on `main` or in either round. House 25.8 t; vehicle 57 t.

**Believable for this house:** all of it down: nothing above ground is outside the vehicle's path or
supported outside it. Only the order is wrong: down in ticks 43 to 49, 0.1 s (MEASURED, ram-plug),
while the hull needs 6 m / 8 m/s = 0.75 s (DERIVED) to reach the far wall. The end state carries no
information.

**The collapse is not only this fixture.** `lane/rubble-staged` rammed the long wall of a 12 × 16 m
hall (`ruins_hall`, 796 chunks, cross walls at z = ±3 m: a 6 m room for a 3.76 m vehicle) on main
`efe66d31`, three runs: 0 chunks standing above grade after one ram (MEASURED,
`lane/rubble-staged:.agents/scratch/rubble-ruins-experiment.md:328`; by eye "walls and roof collapse
across the footprint", `:244`). It predates the belt fix and counts placed ownership: strong
evidence, not proof. **The question needs** a struck wall with a metre or more clear of each track,
no wall parallel to the path within the tracks, and roof members bearing on walls outside the path.

| Fixture in the tree | Clear each side | Cost | Limit |
|---|---|---|---|
| `house_1`, long wall (6 m) | (6.0 − 3.76) / 2 = 1.12 m (DERIVED) | lab yaw, about 15 lines (`construction::spawn` takes an isometry) | 3.5 m inside, then the far wall |
| `ruins_hall` (12 × 16 m, eaves 3.15 m DERIVED, walls 0.25 m) | 16 m wall, 6 m room | `lane/rubble-staged` only: format 9, 660 changed tool lines | panels 1.04 × 0.9 m, about 430 kg (DERIVED) |

## 2. The common root

**"Impulse over one step against static capacity" is not a load measure for an impact:**
1. *The duration is a solver constant.* `read_contacts` reads `-impulse / dt` (`breakage.rs:1066`).
   The same stop at 128 Hz is twice the force: a Resolution number sets a physical load.
2. *It is filed as a dead load.* The part law has two channels (`fracture/parts.rs:1616-1620`): a
   sustained field fails a bond on stress alone; a pulse must also pay the interface's fracture
   work. A contact's whole transfer enters as `press` (`breakage.rs:1123-1124`, `parts.rs:1251`).
   A dead load outlives the first failure, so one elastic solve fails every overstressed interface
   (287 joints, up to 143 times capacity, MEASURED ram-plug). An impact's load ends when its path breaks.
3. *Nothing sustains it.* 9.77 MN (MEASURED, ram-plug tick 43) is 17 times the weight of the only
   heavy body present (57,000 × 9.81 = 0.56 MN, DERIVED).
4. *It is circular.* The impulse stops the hull on matter assumed immovable: the verdict it then decides.

**What bounds an impact here.** Three of the usual four bounds do not bind:
- *Wave reach:* √(G/ρ) = √(3.44e9 / 1845) = 1,365 m/s: 4.4 ms across the house, 21 m a tick (DERIVED).
- *Energy:* the hull holds ½ × 57,000 × 8² = 1.82 MJ; all 3,177 joints at the design's 1.8 J each
  cost about 5.7 kJ (DERIVED).
- *Momentum:* a plastic exchange with the whole house leaves 57 / 82.8 × 8 = 5.5 m/s (DERIVED).
- *Strength over impedance* alone localises: a joint passes motion only up to strength / ρc (the
  design's §3). Round 2's missing truth stands; its error was to leave the other readings in place.

**One event, four readings,** each turning momentum into a force; a fifth is nearly right:
| Reading | Where | What it does with a stop |
|---|---|---|
| Hertz demand | `sweep.rs:363-364` | arriving momentum as an elastic peak force, against a static cut |
| Held gate | `sweep.rs:377`, `sweep_intake.rs:93` | "this body alone cannot break it" becomes a rigid, unlimited contact |
| `read_contacts` | `breakage.rs:1066` | whole transfer / dt as a sustained press |
| `pushes` | `sweep_intake.rs:313-339`, `:645` | transfer less the pair's own approach, / dt, as next tick's drive |
| `crumble::landings` | `crumble.rs:419-424`, doc `:14-19` | splits closing momentum from the held remainder; caps crack work at the kinetic energy lost |

**The law: two channels, never converted into each other.**
- *Force.* Standing matter is loaded statically only by forces that persist through the step: the
  weights and drives of the bodies bearing on it, passed through any loose bodies between, known
  before the solve (`SweepSource::drive`, `sweep_intake.rs:730`, gravity included). A body presses
  with no more than the forces acting on it. The static cut and the part law judge this as today;
  a slow push still fails a whole panel, as real masonry does.
- *Momentum.* A body arriving faster than a joint can pass frees the matter in its path, with what
  it carries or pushes counted as part of it, in one plastic exchange. If it cannot (a buried core,
  a light piece alone), it is held: a real stop, loading the structure by the force channel only.

It replaces the Hertz demand (a speed test), `read_contacts`' awake press (the bounded drive) and
`pushes` (the pusher's bounded drive; later the carried cast); the held gate keeps its meaning. Two
cases, because two physical quantities; four mechanisms exist because each mixes them. A piece
between hull and wall is the same law with the right striker, but separate code (round 1: +564).

## 3. The roof

Construction data (`house_1.house.ron:22-36`, `:103-126`; asset counts DERIVED):
- 26 boards, 0.2 × 0.05 m, laid along the ridge; 98 chunks, three of them single 6.6 m chunks;
  774 kg (7.6 kN). "The house has no rafters" (`:28`): no ridge beam, purlin or tie.
- Board to plate and to board: `nailed`, 3.24 kPa in tension and shear (`attachments.ron:50-65`):
  tension Derived from an ASSUMED rafter nailing density and a figure "CLAIMED, not read"; shear an
  Unknown StandIn. 3.75 m² to the plates (12.2 kN), 8.33 m² between boards (27 kN). Board to gable:
  `resting`, 2.19 m², no tension.

Intact, the roof bears believably and does not hang: weight in compression on plates and gables;
outward thrust per side 3.8 kN × (2.08 / 2) / 1.56 = 2.5 kN against 6.1 kN of nailed shear (DERIVED).
The refuter agrees: with a hull-sized opening the house stood at 1,044 chunks for 0.84 s.

With a gable and the long walls' ends gone, bending has only an edge-nailed seam 50 mm deep to run
through. One 6.6 m board (29.6 kg) held at one end: 290 N × 3.3 m = 958 N·m against
3,240 Pa × 6.6 m × (0.05 m)² / 6 = 8.9 N·m, 107 times; 13 boards on one seam, about 1,400 times
(DERIVED). **1,058 is a lever ratio, not a broken law.** Verdict: a **data fault** (construction
facts), and not the cause of the collapse. A real roof has rafter pairs on the plates and stands on
two long walls without a gable. Fix the spec when a fixture is adopted; leave the law alone.

## 4. Options for the owner

The last estimates of this kind ran 2.2 times over (260 → 564, MEASURED wall-yield); each "read as" applies it.

**A. Fit the fixture, then split force from momentum (recommended).**
- *Experiment, no law:* from `lane/ram-plug`, yaw the house so the vehicle rams the 6 m wall (about
  15 lab lines). Three arms, two runs each: today; the 8-line arm; the arm with both static readings
  bounded by the forces behind them (press per body ≤ its drive, push ≤ the pusher's drive; about
  20 lines, analogue `pushes` at 27).
- *Refuted if,* in the third arm: under half the house stands; or forward speed goes below zero; or
  the lengthwise peak is still in the hundreds of m/s².
- *Build on `main` if it holds:* a speed gate replaces the Hertz demand in `resolve` (+25 / −65, the
  design's count); `read_contacts`' awake press becomes the bounded drive (+30 / −10, analogue the
  landing split); `pushes` bounded (+5). Production +60 / −75, read as +60 to +130 added. Tests:
  the Hertz threshold tests out (about 85 lines), two in.
- *Deletes:* `peak_force` and `peak_speed` in the sweep, `PatchLoad::threshold`,
  `SweepSource::effective_mass`, the `/ dt` press. *Not in it:* the carried cast (design estimate
  +150, read as +150 to +330): only if the fitted fixture still shows a stop on small pieces.
- *The owner sees:* a tank-sized hole in the long wall; the tank keeps rolling, slower by under a
  metre a second; the gable walls and the roof over them stand; boards over the hole sag or drop.

**B. Take the arm's result by the smallest sound route; leave collapse for a bigger fixture.**
- The arm as written is a patch: it sits inside the Hertz gate, uses the sliding row for every joint
  mode, and has no bearing flood or energy test. Rebuilt soundly it is A's speed gate alone:
  +25 / −65 production, read as +25 to +55 added.
- *Limit:* here the "no rebound" is paid for by the collapse: with both static readings off, the hull
  rebounds at −1.81 m/s and stalls (MEASURED, ram-plug). It may return the day the readings are fixed.
- *Refuter:* already run (ram-plug): jolt 529 → 142 m/s², no reversal. *The owner sees:* no bounce;
  the house still falls whole; the crossing takes 6.4 s, not 4.5 s: the tank climbs the whole house.

**C. Cap the solver's contact impulse at the cut's capacity.** Covers chains of any depth with no
prediction. The fork has no such field (`ContactPoint::normal_impulse` is a readout,
`vendor/avian3d-0.8.0-overmatch/src/collision/contact_types/mod.rs:677-715`): a fork divergence
needing a seam; size unknown. Fallback if chains keep refuting.
**D. Adopt the hall as the standing fixture.** 660 tool lines and format 9, rebuilt clean from
`lane/rubble-staged`. Not needed to decide A; needed before any urban-map claim.

**Decision:** A, experiment first: about 35 throwaway lines, and the first test in three rounds
that can tell a hole from a collapse.

## Not verified

Which joint read 1,058 times capacity (the lever arithmetic is an order check); that the 160
standing chunks are the foundation's (counts match); whether the hall result survives the belt fix;
whether the hull's own 4.5 MN press on the wall's lower courses recurs on a fitted fixture.
