# track-score: the scorecard both track arms are judged by

Subject: the video game's tank driving physics (the track belt, the suspension and their contact
with terrain, static obstacles and rubble), game state only.

## What it is

One `#[ignore]` runner drives the shipped tank through the shipped plugin and writes one card:

```
scripts/serial-cargo test --lib --features dev_ui,bitprobe track_scorecard -- --ignored --nocapture
```

It writes `target/track-scorecard.json` and `target/track-scorecard.md` under the crate root
after every line, then fails if a law check failed. A whole card is 38 fresh boots, about
900 simulated seconds, and 2.5 minutes of wall time on the local Mac (MEASURED 135 to 166 s).

Files:
- `src/headless_test/track_scorecard.rs`: the card, the world fixtures and the readers. An arm
  never edits it.
- `src/headless_test/track_scorecard/arm.rs`: **the seam**, the only file an arm edits. It holds
  what belongs to one track model:
  - `set_ground`: make the height field one ground type;
  - `belt_forces`: the forces the model applied to the hull outside Avian's solver this tick,
    per world body;
  - `work`: the model's own counts per tick;
  - `CLAIMS_PIECE_REACTION`: whether the model claims a loaded piece takes the equal and
    opposite reaction.
- `src/headless_test.rs`: one line, `mod track_scorecard;`.

### What the card reads, and why it is arm-agnostic
- **In:** `TankCommand` only (throttle, steer), the shipped input. Each scenario boots a fresh
  app and places the hull before the clock starts, so each arm begins from its own spawn state.
  The duel partner is despawned, so the tank is alone and its work counts are its own.
- **Out:** Avian body state (pose, velocity, mass, inertia, centre of mass), Avian's solver
  contacts (`ContactGraph`, total normal impulse per point), construction facts (`RigGeom`, the
  tank spec, the transmission's steering radii), and the seam's `belt_forces`.
- **One gap, for now:** the card reads solver normals only (the last substep's impulse times
  the substep count). Avian's contact point keeps no friction total, and its `normal_impulse`
  double-counts (see line 8). Until the card reads `ContactTransfers` (left below), a solver
  arm returns its own friction from `belt_forces`, or the wall line's upward force and the
  turning moments read normals only.

### The world each scenario stands on
- **Terrain:** a height grid at the shipped cell spacing (Fixture: 513 samples over 750 m gives
  1500 m / 1024 ≈ 1.465 m cells), with its heightfield collider, on an empty map placement.
- **Steps, walls, curbs, bars, trenches, ledges:** static Avian cuboids made as the map makes a
  graybox block (`RigidBody::Static`, layer `Terrain` against all, the shipped `Brick` friction).
  The boat sees them through its solid path; a solver arm meets them natively.
- **Rubble:** dynamic `Brick` cuboid pieces, 0.8 × 0.5 × 0.6 m (Fixture: cube root of volume
  0.62 m, above the 0.5 m size floor), shipped density and friction, the debris layers.

## The card (Fixture dimensions in the code; law checks marked)

1. **Holding on a slope: law check.** Per shipped ground, the limit is
   `min(atan μ, asin(2 F_brake k_static / W))` from the ground's grip and the tank's brake data.
   Slopes sit a tenth of the limit below and above it (the shipped brake test's margin rule).
   Below: parked 60 s, holds iff the last second's travel and speed are under 0.1 % of g·dt.
   Above: 8 s, moves iff both exceed it. Spring-back and creep are observations.
2. **Turning: observation.** Wide and tight detents after a 3 s and a 10 s run-up, plus a
   neutral pivot. Achieved radius over geared radius, yaw rate, and the yaw moments of every
   world force on the hull, split into longitudinal and lateral parts, against Steeds' μWL/4
   (μ the ground's grip, W the weight, L the mean road-wheel span).
3. **Acceleration and top speed per ground: observation.** Full throttle 40 s from rest.
4. **Climbing: observation.** A step sweep of 0.2 to 1.2 m from a fixed approach (the face 6 m
   ahead of the belt's front point, full throttle from rest), and the owner's wall case: a 4 m
   wall 2 m ahead, 8 s of push; the wall's upward force on the hull, the hull's rise and pitch,
   and which tank parts met the wall.
5. **Trenches: observation.** A 2 m deep cut between two static plateaus at 0.25, 0.5, 0.75 and
   1.25 × the road-wheel span: crossed or not, the sink, the pitch range.
6. **Curb and ledge: observation.** A 0.15 m curb under the right shoe's outer edge, driven along
   at 0.3 throttle; the right shoe's centre line over a 1 m ledge, parked. Roll, rise, drift.
7. **Ride: washboard observation plus a passivity law check.** 30 bars 0.08 m tall, 1.2 m apart,
   at full throttle: pitch, roll and vertical-speed RMS. Kick: parked, 0.3 rad/s pitch and
   0.3 m/s heave, no input for 4 s: with no input the system takes no energy in, so the last
   second's peak hull kinetic energy must fall below the first second's.
8. **Rubble: observation, law check where the arm claims it.** A 50-piece pile, driven onto at
   0.6 throttle, then parked 4 s: rise, vertical jitter, the rest-pose belt bottom's depth inside
   pieces, the largest piece speed while driving and parked, and the reaction ratio
   `(world support on the pieces − their weight) / (the pieces' support on the hull)`. It is 1
   when every loaded piece takes the equal and opposite reaction, and 0 when the pieces hold the
   tank up unloaded.
9. **Work per tank per tick: observation, MEASURED counts.** Production `Work` counters, the
   seam's counts and Avian solver contacts with the tank as a party, on open ground and parked
   on the pile. No timing.

## Baseline: `lane/rubble`, the boat

All numbers MEASURED by the card on `lane/rubble` (the boat), 2026-11-08; the shipped tank is
the fixture. The full card, with every row and note, is `target/track-scorecard.md` after a run;
the tables below are its headline columns.

### 1. Slope hold: law check PASS on all five grounds

| ground | binding limit | below: settle, last-second speed | above: last-second travel |
|---|---|---|---|
| concrete | grip 30.96° | 0.123 m, 4.2e-7 m/s | 2.58 m |
| dirt | grip 22.68° | 0.105 m, 2.9e-7 m/s | 0.186 m |
| gravel | brake 31.00° | 0.078 m, 9.3e-7 m/s | 10.8 m |
| mud | grip 15.48° | 0.088 m, 1.6e-7 m/s | 0.043 m |
| sand | grip 28.50° | 0.100 m, 7.6e-7 m/s | 0.124 m |

The settle below the limit (78 to 123 mm, spring-back plus the first suspension settle) is an
observation.

### 2. Turning (base ground gravel, μ 0.712)

| run | gear, detent | speed m/s | yaw rad/s | R / R_geared | lateral moment / Steeds |
|---|---|---|---|---|---|
| wide, 3 s run-up | F6 wide | 4.92 | 0.0506 | 1.28 | 0.25 |
| tight, 3 s run-up | F6 tight | 4.89 | 0.149 | 1.27 | 0.64 |
| wide, 10 s run-up | F8 wide | 9.26 | 0.0417 | 1.35 | 0.17 |
| tight, 10 s run-up | F8 tight | 8.32 | 0.112 | 1.33 | 0.46 |
| neutral pivot | F4 tight | 0 | 0.129 | — | 0.91 |

The longitudinal and lateral moments cancel to within 1 to 5 % in every run, which checks the
card's force reader. The achieved-to-geared ratio of 1.27 to 1.35 matches the 1.31 recorded
earlier. Steeds' μWL/4 (W·L from construction) is 359 kN·m; only the pivot comes near it.

### 3. Acceleration and top speed

| ground | v at 5 s | v at 10 s | v at 40 s | final gear |
|---|---|---|---|---|
| concrete | 4.98 | 7.39 | 10.63 | F8 |
| dirt | 4.81 | 6.70 | 10.34 | F8 |
| gravel | 4.76 | 6.99 | 10.58 | F8 |
| mud | 3.00 | 3.23 | 3.23 | F5 |
| sand | 3.28 | 4.55 | 4.83 | F6 |

### 4. Climbing
- Steps of 0.2 and 0.4 m are climbed; 0.6 m and higher are not. The first part to meet a step:
  the belt at 0.2 m; an unnamed tank child collider (a bump stop or side plate, reported under
  the tank's node name) at 0.4 m; the hull proxy `Hull_Collider` at 0.6 m and above.
- **The owner's wall case does not occur on this tank.** Against the 4 m wall the hull proxy
  meets the wall first, and the belt never touches it over 8 s of push. The upward force from
  the wall is solver normals only: a 14.8 kN peak, about zero (−0.45 kN) over the last 2 s. The
  hull neither rises nor pitches.
- So the climb limit here is set by where the hull proxy's nose sits relative to the belt's
  run-up, a construction fact, not by grip. Whether the shipped proxy's nose is right is a
  question for the asset, not for either arm.

### 5. Trenches

Road-wheel span 3.605 m (construction), a 2 m deep cut.

| width | width / span | crossed | time s | sink m | pitch range |
|---|---|---|---|---|---|
| 0.90 m | 0.25 | yes | 5.33 | 0.060 | −1.3° .. 4.4° |
| 1.80 m | 0.50 | yes | 5.56 | 0.130 | −3.7° .. 5.5° |
| 2.70 m | 0.75 | yes | 6.13 | 0.219 | −3.9° .. 8.9° |
| 4.51 m | 1.25 | no: nose down in the cut | — | 0.943 | −33.0° .. 2.7° |

### 6. Curb and ledge
- Curb under the right shoe's outer edge at 0.3 throttle: roll 0.30° (RMS 0.24°), rise 6 mm,
  no lateral drift (0.8 mm over 10.6 m).
- Right shoe's centre line over a 1 m ledge, parked: roll −3.8°, the hull 88 mm lower, no slide
  (5 mm).

### 7. Ride: passivity law check PASS
- Washboard at 7.4 m/s: pitch RMS 0.16°, roll RMS 1e-5°, vertical speed RMS 0.023 m/s.
- Kick of 11.9 kJ: half-second peaks 10240, 2435, 637, 366, 163, 42, 15, 11 J. The energy falls
  by about 3 orders in 4 s, monotonically.

### 8. Rubble

| measure | value |
|---|---|
| reached the pile's centre | no: the hull centre stopped 0.16 m past the pile's near edge after 15 s at 0.6 throttle |
| hull rise while parked | 0.044 m |
| vertical jitter while parked | 1.1 mm RMS, vertical speed 0.014 m/s RMS |
| sink of the rest-pose belt bottom into pieces | 0 |
| largest piece speed | 2.89 m/s driving, 0.019 m/s parked |
| support from pieces | 29 % of the hull's weight, of which the belt channel carries 0.004 % |
| reader control, unloaded pile | world support / weight = 1.0000 |
| reaction ratio | 0.97 (RMS 0.045): symmetric within 5 % |

- **On this pile the belt carries almost none of the piece support.** The pieces hold the tank
  through its rigid solver colliders (bump stops, side plates, hull proxies), whose contacts are
  symmetric, so the reaction ratio is 0.97. The boat's clamped belt reaction is therefore not
  exercised by this fixture.
- **The solver reading.** Avian 0.7's `ContactPoint::normal_impulse` adds the accumulated
  impulse after both the solve and the relax pass of every substep, so at rest it reads twice
  the applied impulse: the card's first control read 2.0005 × the pile's weight. The game
  already knows this and owns the exact reading: `contact_transfer::ContactTransfers` records
  the momentum the solver moved, friction included (its module doc and
  `the_reading_is_the_momentum_the_solver_moved`). The card today reads the last substep's
  impulse times the substep count, exact at rest (the control now reads 1.0000) and approximate
  in transients. Switching to `ContactTransfers` is the first item left.

### 9. Work per tank per tick (MEASURED counts)

| case | belt contacts | belt force applications | belt near colliders | belt candidates | slab checks | dynamic contacts | solver pairs, points |
|---|---|---|---|---|---|---|---|
| driving on open ground | 786 | 1572 | 9 | 0 | 0 | 0 | 0, 0 |
| parked on the pile | 848 | 1695 | 36 | 27 | 388 | 27 | 9, 27.6 |

On open ground the boat makes 786 contacts per tank per tick, against the 540 the study
inferred from code. The terrain facet work itself has no counter (`Work` covers colliders
only), so the contact count is the card's proxy for it.

## Scenario note: driving the same fixtures by eye

The card's fixtures are placed relative to the hull's start (origin, facing −z) and the belt's
front point F (the most forward point of the rest belt, `front_z`). The track sandbox
(`track_sandbox`, `dev_ui`) already has a near twin of most lines, built from its course data
`assets/dev/track_sandbox.ron`; its blocks are the boat's analytic course blocks, not Avian
statics, so a solver arm meets them only once they become colliders.

| Card line | Card fixture | Nearest sandbox spot | To match the card by eye |
|---|---|---|---|
| 1 slope | a plane at limit ± 10 %, parked uphill, per ground | the 20° pad, `SANDBOX_HARNESS="pose=slope_up,…"` | set `slope_pad_deg` to the card's angle |
| 2 turning | flat, 3 s or 10 s run-up, then steer 0.475 (wide) or 1.0 (tight) for 12 s | the runway, `pose=runway,throttle=1,t2=192,steer2=1` | as is |
| 3 acceleration | flat, full throttle 40 s | the runway, `pose=runway,throttle=1` | as is |
| 4 steps | a 10 × 20 m plateau 0.2 to 1.2 m tall, face 6 m ahead of F | the course step (`step_height_m` 0.45, `step_z_m` −72) | set `step_height_m` |
| 4 wall | a 10 m wide, 2 m thick, 4 m tall wall, face 2 m ahead of F | none | a step with `step_height_m: 4.0` |
| 5 trenches | a 2 m deep cut between two plateaus, 0.25 to 1.25 × the road-wheel span, near edge 6 m ahead of F | the course trenches (2.2, 5.0 and 10 m) | set `trenches[].width_m` |
| 6 curb | a 0.3 m wide, 0.15 m tall strip under the right shoe's outer edge, from 5 m ahead of F | none | — |
| 6 ledge | a 1 m plateau whose edge runs under the right shoe's centre line, parked | none | — |
| 7 washboard | 30 bars 0.08 m tall, 0.4 m long, 1.2 m apart, from 40 m ahead of F | the course washboards | set `washboards` |
| 8 rubble | 50 Brick pieces 0.8 × 0.5 × 0.6 m, 5 × 6 under 4 × 5, near edge 6 m ahead of F | the rubble yard, `x=-13,z=1` (108 shipped brick chunks) | as is, a heavier pile |

The curb, the ledge and the 4 m wall have no sandbox twin yet.

## Decisions taken

- **The card is a child of `headless_test`**, so it reuses the shared boot, lease and drive
  helpers. Its seam is one file, `arm.rs`; an arm merges the card and edits only that file.
- **Each scenario boots a fresh app and places the hull before the clock starts**, so every arm
  begins from its own spawn state and the card resets no arm-internal state. The duel partner
  is despawned, so the work counts are one tank's.
- **The world:** a height grid at the shipped cell spacing; fixtures as the map makes graybox
  blocks; pieces of the shipped `Brick` substance (density and friction from the registry), so
  no fixture states a material number of its own.
- **The slope check** reuses the shipped brake test's limit and its 10 % margin rule, and its
  0.1 % of g·dt precision.
- **The passivity check** asks only that the last second's peak kinetic energy fall below the
  first second's. The decay rate is an observation.
- **The reaction check** holds to 5 % (a `Resolution` margin for solver iteration) and is a law
  check only where an arm sets `CLAIMS_PIECE_REACTION`; the boat does not claim it.
- **The friction gap:** avian 0.7 keeps no total friction impulse per contact, so the card reads
  solver normals and a solver arm adds its own friction through `belt_forces`.
- **The transmission state** (gear, detent) is read directly: the drivetrain is common to both
  arms. An arm that changes `TankTransmission` updates the card's two reads.
- **Steeds' L** is the mean road-wheel span, the construction's own nominal contact length.
- **No line-filter switch:** the whole card runs in about 2.5 minutes, so a lever with no other
  user was not added.
- **`headless_test` was not run as a suite:** the change adds no production collider or body;
  its fixtures live only in the ignored card.

## Left for a later session

- **Read solver contacts through `contact_transfer::ContactTransfers::solver_groups`.** It is
  the game's one owner of the momentum the solver moved, friction included. It replaces the
  card's own last-substep approximation and closes the friction gap, so a solver arm would no
  longer need to report its friction through `arm::belt_forces`.
- **Collider names.** A tank child collider with no `Name` is reported under the tank's node
  name (`Tiger I (A)` in the step line). Naming by collider role would make `first_touch`
  clearer.
- **The wall case.** On this tank the hull proxy meets a wall before the belt, so the owner's
  front-wheel case never runs. It needs a tank whose wheel leads, or the shipped proxy's nose
  checked against the drawn hull.
- **The parked-on-rubble case.** At 0.6 throttle the hull never reached the pile's centre. A
  pile built under the parked tank, or a full-throttle approach, would load the belt on pieces;
  today's fixture loads the bump stops.
- **Sandbox twins** for the curb, the ledge and the 4 m wall; better, the card's fixture list
  shared as data with a sandbox spot, so the owner drives the identical course.
- **Running the card on both arms** is the arms' work: each merges `lane/track-score` and edits
  `arm.rs`.
