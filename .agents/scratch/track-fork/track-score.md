# track-score: the scorecard both track arms are judged by

Subject: the video game's tank driving physics (the track belt, the suspension and their contact
with terrain, static obstacles and rubble), game state only.

**Seam contract (for the two arms that merge this):** `arm.rs`'s signatures are unchanged; one
function is new, `collider_role(world, collider) -> Option<&'static str>`; and `belt_forces` now
returns only what the arm applies outside Avian's solver, because the card reads every solver
contact, friction included, through `ContactTransfers`. Session 3 left `arm.rs` untouched.

## What it is

One `#[ignore]` runner drives the shipped tank through the shipped plugin and writes one card:

```
scripts/serial-cargo test --lib --features dev_ui,bitprobe track_scorecard -- --ignored --nocapture
```

It writes `target/track-scorecard.json` and `target/track-scorecard.md` under the crate root
after every line, then fails if a law check failed. A whole card is 45 fresh boots, about
950 simulated seconds, and 2.5 minutes of wall time on the local Mac (MEASURED 135 to 166 s).

Files:
- `src/headless_test/track_scorecard.rs`: the card, the world fixtures and the readers. An arm
  never edits it.
- `src/headless_test/track_scorecard/arm.rs`: **the seam**, the only file an arm edits. It holds
  what belongs to one track model:
  - `set_ground`: make the height field one ground type;
  - `belt_forces`: the forces the model applied to the hull outside Avian's solver this tick,
    per world body (a solver arm returns none for contacts that live in the solver);
  - `collider_role`: the role of each collider the model spawns on the tank (the boat: bump
    stop, side plate; a solver arm adds belt segment, road wheel and so on);
  - `work`: the model's own counts per tick;
  - `CLAIMS_PIECE_REACTION`: whether the model claims a loaded piece takes the equal and
    opposite reaction.
- `src/headless_test.rs`: one line, `mod track_scorecard;`.

### What the card reads, and why it is arm-agnostic
- **In:** `TankCommand` only (throttle, steer), the shipped input. Each scenario boots a fresh
  app and places the hull before the clock starts, so each arm begins from its own spawn state.
  The duel partner is despawned, so the tank is alone and its work counts are its own.
- **Out:** Avian body state (pose, velocity, mass, inertia, centre of mass), every solver
  contact's momentum for the step (`contact_transfer::ContactTransfers::solver_groups`, normal
  and friction, over the step length), construction facts (`RigGeom`, the tank spec, the
  transmission's steering radii, the spawned colliders' shapes and poses), and the seam's
  `belt_forces`. `ContactGraph` is read only to count solver pairs and points.
- **Collider names are roles**, read from what spawned each collider: the seam names its own
  (`collider_role`); a remaining collider on the vehicle layer is a hull proxy, named by the
  authored proxy node it hangs under (`hull proxy Hull_Collider`); anything else is reported
  as unclassified, with its parent's name.

### The world each scenario stands on
- **Terrain:** a height grid at the shipped cell spacing (Fixture: 513 samples over 750 m gives
  1500 m / 1024 ≈ 1.465 m cells), with its heightfield collider, on an empty map placement.
- **Steps, walls, curbs, bars, trenches, ledges:** static Avian cuboids made as the map makes a
  graybox block (`RigidBody::Static`, layer `Terrain` against all, the shipped `Brick` friction).
  The boat sees them through its solid path; a solver arm meets them natively.
- **Rubble:** dynamic `Brick` cuboid pieces, 0.8 × 0.5 × 0.6 m (Fixture: cube root of volume
  0.62 m, above the 0.5 m size floor), shipped density and friction, the debris layers.

## The card (Fixture dimensions in the code; law checks marked)

0. **Where the tank's colliders sit: observation, construction facts.** Parked 2 s on flat
   ground; every colliding collider on the hull body, by role, in the hull frame (forward is
   −z), heights above the ground: the nose (most forward vertex) and its height, the lowest
   point, and the reach below h (the most forward point at or below h, for h = 0.2 to 1.2 m),
   which says what of the tank a step of that height meets first on a level approach. Laterally,
   the most forward point of each role between the belt lanes, in them and outboard of them, at
   heights up to the walls' 4 m (a 2 cm grid of rays cast rearward): what a wall over that strip
   meets first. A lane is one shoe's span plus 0.05 m a side, the same strip as line 4b's
   one-lane walls. The belt's rest outline (every route circle at the outer face) is a row of
   its own.
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
   ahead of the belt's front point, full throttle from rest).
   **4b. Walls: observation.** The owner's front-wheel case: a wall 4 m tall and 2 m thick, its
   face 2 m ahead of the belt's front point, 8 s of full-throttle push from rest. Three walls:
   full width (10 m), and one belt lane wide under each belt in turn (the shoe's span plus
   0.05 m a side, Fixture: it covers the approach's lateral drift, under 1 mm per 10 m in
   line 6, and stays clear of the gap between the lanes). Reported: the first touch by role,
   the wall's force on the hull split per solver contact into normal and friction (vertical
   parts), the belt channel's vertical force, the hull's rise and pitch range.
5. **Trenches: observation.** A 2 m deep cut between two static plateaus at 0.25, 0.5, 0.75 and
   1.25 × the road-wheel span: crossed or not, the sink, the pitch range.
6. **Curb and ledge: observation.** A 0.15 m curb under the right shoe's outer edge, driven along
   at 0.3 throttle; the right shoe's centre line over a 1 m ledge, parked. Roll, rise, drift.
7. **Ride: washboard observation plus a passivity law check.** 30 bars 0.08 m tall, 1.2 m apart,
   at full throttle: pitch, roll and vertical-speed RMS. Kick: parked, 0.3 rad/s pitch and
   0.3 m/s heave, no input for 4 s: with no input the system takes no energy in, so the last
   second's peak hull kinetic energy must fall below the first second's.
8. **Rubble: observation, law check where the arm claims it.** Three cases, each parked 4 s:
   a 50-piece pile ahead, driven onto at 0.6 throttle; the same pile at full throttle; and one
   layer of pieces under each track's road wheels, the tank placed on it at rest. Rise, vertical
   jitter, the rest-pose belt bottom's depth inside pieces, the largest piece speed while
   driving and parked, the share of the pieces' support the belt channel carries, how many
   belt-loaded pieces are asleep, and the reaction ratio
   `(world support on the awake pieces − their weight) / (the pieces' support on the hull)`. It
   is 1 when every loaded piece takes the equal and opposite reaction, and 0 when the pieces
   hold the tank up unloaded.
9. **Work per tank per tick: observation, MEASURED counts.** Production `Work` counters, the
   seam's counts and Avian solver contacts with the tank as a party, on open ground and parked
   in each rubble case. No timing.

## Baseline: `lane/rubble`, the boat

All numbers MEASURED by the card on `lane/rubble` (the boat), 2026-11-08; the shipped tank is
the fixture. The full card, with every row and note, is `target/track-scorecard.md` after a run;
the tables below are its headline columns. Lines 1 to 7 read the same to the last digit before
and after the switch to `ContactTransfers`, except the wall's vertical force (line 4), which
now includes friction.

### 0. Where the tank's colliders sit (construction facts)

Hull frame, forward is −z; heights above the flat ground the parked tank rests on (the ground
sits at hull-frame y 0.0597 m; pitch 0.36°).

| collider | parts | nose z | nose height | lowest height (at z) | reach below 0.2 / 0.4 / 0.6 / 0.8 m |
|---|---|---|---|---|---|
| belt outline (rest pose) | 2 | −2.993 | 0.709 | 0.000 (−1.862) | −2.461 / −2.867 / −2.979 / −2.993 |
| side plate | 4 | −2.992 | 0.676 | 0.000 (1.743) | −2.461 / −2.861 / −2.975 / −2.992 |
| bump stop | 2 | −2.943 | 0.736 | 0.208 (2.389) | — / −2.789 / −2.926 / −2.943 |
| hull proxy Hull_Collider | 2 | −3.147 | 0.714 | 0.430 (−2.490) | — / — / −2.883 / −3.147 |
| hull proxy Turret_Collider | 1 | −1.274 | 1.840 | 1.840 (−1.274) | — / — / — / — |

Laterally (the lanes: left x −1.957 .. −1.132 m, right x 1.132 .. 1.957 m), the most forward
point of each role at heights up to 4 m, as z at height:

| collider | between the lanes | in the lanes | outboard |
|---|---|---|---|
| belt outline (rest pose) | — | −2.993 at 0.71 m | — |
| side plate | — | −2.992 at 0.68 m | — |
| bump stop | — | −2.942 at 0.73 m | — |
| hull proxy Hull_Collider | **−3.147 at 0.99 m** | −2.271 at 1.27 m | — |
| hull proxy Turret_Collider | −1.274 at 2.24 m | — | — |

So the hull proxy's nose is between the lanes; inside a lane the belt's front wrap leads, the
side plate level with it (1 mm behind), the bump stop 0.051 m behind, and the proxy 0.722 m
behind. Nothing of the tank reaches outboard of the lanes.

### Fact check: the hull proxy's nose against the drawn hull and the belt's front wrap

Code and asset facts only. Sources: the card's line 0 (the spawned colliders and `RigGeom`) and
the shipped render asset `assets/tiger_1/tiger_1.glb`, read with a stdlib glTF parser (no
Blender needed). The glb's `Hull` frame is the hull body's frame: the glb's `Hull_Collider` mesh
and the spawned proxy agree to the millimetre (nose −3.147, lowest 0.430 at −2.490, reach below
0.6 m −2.883). "Drawn hull" is the union of the `Hull_*` plate meshes (`Hull_LFP_*`,
`Hull_UFP_*`, `Hull_Side_*`, `Hull_Top`, `Hull_Bottom`, `Hull_Rear`); `Hull_Decor` (fittings)
is listed apart. All MEASURED; offsets are DERIVED as differences of those.

| part | nose z | nose band (heights) | lowest in the front metre (height, z) | reach below 0.4 / 0.6 / 0.8 m |
|---|---|---|---|---|
| hull proxy `Hull_Collider` | −3.147 | 0.714 .. 1.105 | 0.430, −2.490 | — / −2.883 / −3.147 |
| drawn hull plates | −3.147 | 0.711 .. 1.119 | 0.424, −2.529 | — / −2.974 / −3.147 |
| drawn `Hull_Decor` | −3.124 (at 0.86 m) | — | 0.391, −2.533 | −2.558 / −2.827 / −3.081 |
| belt front wrap (rest outline, outer face) | −2.993 | at 0.709 | 0.000 | −2.867 / −2.979 / −2.993 |
| drawn `Sprocket_L` outline | −2.984 | at 0.729 | 0.277, −2.572 | −2.822 / −2.969 / −2.984 |

- **The proxy's nose against the belt's front wrap:** 0.154 m ahead of it, at the same height
  (the proxy's nose band starts 0.005 m above the wrap's most forward point). Against the drawn
  sprocket outline: 0.163 m ahead.
- **The proxy against the drawn hull at the nose:** it follows the drawn nose. Same most forward
  point (−3.147); nose band within 0.003 m at the bottom and 0.014 m at the top; lowest front
  edge 0.006 m higher and 0.039 m further back. One place differs: at 0.6 m the drawn lower
  front plate reaches 0.091 m further forward than the proxy (−2.974 against −2.883).
- **The collider that met the 0.4 m step first is a bump stop** (the role name replaces the
  session-1 report's `Tiger I (A)`). Its nose: −2.943 at 0.736 m, 0.050 m behind the belt
  wrap's most forward point and 0.027 m higher. Its reach below 0.4 m is −2.789, 0.078 m behind
  the belt outline's −2.867 at that height. Against the proxy: 0.204 m behind the proxy's nose.
- **What the run shows beside the geometry:** below 0.6 m the belt outline leads both rigid
  colliders at rest (by 0.078 m at 0.4 m over the bump stop, by 0.096 m at 0.6 m over the
  proxy), yet the run records the bump stop first at the 0.4 m step and the proxy first at the
  0.6 m step, with the belt channel not touching either face first. Code fact, not diagnosed
  here: the boat's belt did not meet those step faces first, where its rest outline sits.

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
  the belt at 0.2 m; a bump stop at 0.4 m; the hull proxy `Hull_Collider` at 0.6 m and above.
- So the climb limit here is set by where the hull proxy's nose sits relative to the belt's
  run-up (line 0 and the fact check above), a construction fact, not by grip.

### 4b. Walls

Vertical forces upward positive, in kN; "last 2 s" is the mean over the push's last 2 s.

| wall | first touch | solver colliders over the push | normal force, last 2 s | normal vertical: last 2 s, peak up | friction vertical: last 2 s, peak up | belt channel vertical, last 2 s | rise | pitch range |
|---|---|---|---|---|---|---|---|---|
| full width, 10 m | hull proxy Hull_Collider | hull proxy Hull_Collider | 289.6 | −0.45, 14.8 | −72.7, 0 | 0 (never touched) | 3.7 mm | −0.45° .. 2.64° |
| left belt lane only | belt, bump stop (same tick) | bump stop | 266.3 | 0.0, 13.3 | −81.1, 150.5 | +0.93 | 3.7 mm | −0.64° .. 2.64° |
| right belt lane only | belt, bump stop (same tick) | bump stop | 266.2 | 0.0, 18.3 | −81.1, 150.3 | +0.95 | 3.7 mm | −0.64° .. 2.64° |

- **The owner's front-wheel case, on real geometry:** with the wall one lane wide, the belt and
  the bump stop meet it on the same tick, and only the bump stop stays in solver contact over
  the push. The two lanes agree to within 0.1 % on the normal and friction forces; the
  normal's upward peak differs (13.3 against 18.3 kN).
- **No climb in any case.** The hull rises 3.7 mm at most; the 2.64° nose-up peak is the launch
  squat, the same in every case. Over the last 2 s the wall's friction pushes the hull down
  (−73 to −81 kN) while the normal force is horizontal. The one-lane walls show a transient
  upward friction peak of 150 kN that does not last.
- **The belt channel carries almost no vertical force** off a one-lane wall: about +0.9 kN
  over the last 2 s, against the bump stop's 266 kN normal push.
- **Where the parts end up (DERIVED from the hull's end position and line 0, at the rest
  pose):** the hull origin stops 2.888 m short of the wall face, which puts the bump stop's
  nose 0.054 m and the belt outline's front wrap 0.105 m past the face. Against the full-width
  wall the proxy's nose ends at the face (−3.1475 against −3.147).

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

| measure | pile ahead, 0.6 throttle | pile ahead, full throttle | layer under the tracks |
|---|---|---|---|
| reached the pile's centre in 15 s | no: hull centre 0.16 m past the near edge | no: 1.84 m past | — (placed on it) |
| hull rise while parked | 0.044 m | 0.086 m | 0.500 m (the layer is 0.502 m) |
| vertical jitter, vertical speed (RMS) | 1.1 mm, 0.014 m/s | 2.4 mm, 0.028 m/s | 0.1 mm, 0.009 m/s |
| sink of the rest-pose belt bottom into pieces | 0 | 0 | 0.2 mm mean, 0.4 mm max |
| largest piece speed, driving / parked | 2.89 / 0.019 m/s | 3.58 / 0.063 m/s | — / 0.070 m/s |
| support from pieces, share of the hull's weight | 25 % | 49 % | 100 % |
| of that, through the belt channel | 0.005 % | 0.0005 % | 92 % |
| belt-loaded pieces asleep (most at once) | 0 | 0 | 0 |
| reaction ratio (RMS) | 1.000 (0.004) | 1.000 (0.006) | 0.081 (0.067) |

The reader's control (the settling, unloaded pile's world support over its awake weight) reads
1.0000 (RMS 0.0000).

- **On the pile ahead the belt carries almost none of the piece support**, at either throttle.
  The pieces hold the tank through its rigid solver colliders, whose contacts are symmetric:
  the reaction ratio reads 1.000.
- **The layer under the tracks loads the belt:** the belt channel carries 92 % of the pieces'
  support, and the reaction ratio falls to 0.08. Every belt-loaded piece is awake, so this is
  not sleep: the world under the pieces carries their weight plus 8 % of the load the belt puts
  on them. The boat does not claim the reaction (`track::rubble` clamps a resting piece's
  normal reaction by design), so this is an observation, not a failure.
- **The solver reading.** The card now reads `ContactTransfers`, the game's one owner of the
  momentum the solver moved, friction included. Avian 0.7's `ContactPoint::normal_impulse` adds
  the accumulated impulse after both the solve and the relax pass of every substep, so at rest
  it reads twice the applied impulse (the first control read 2.0005); the session-1 stand-in
  read 0.97 where `ContactTransfers` reads 1.000.

### 9. Work per tank per tick (MEASURED counts)

| case | belt contacts | belt force applications | belt near colliders | belt candidates | slab checks | dynamic contacts | solver pairs, points |
|---|---|---|---|---|---|---|---|
| driving on open ground | 786 | 1572 | 9 | 0 | 0 | 0 | 0, 0 |
| parked on the pile ahead, 0.6 throttle | 848 | 1695 | 36 | 27 | 388 | 27 | 9, 27.6 |
| parked on the pile ahead, full throttle | 772 | 1544 | 38 | 29 | 388 | 43 | 9, 28.9 |
| parked on a layer under its tracks | 693 | 1387 | 23 | 14 | 1358 | 693 | 28, 111 |

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
| 8 rubble, pile ahead | 50 Brick pieces 0.8 × 0.5 × 0.6 m, 5 × 6 under 4 × 5, near edge 6 m ahead of F; 0.6 and full throttle | the rubble yard, `x=-13,z=1` (108 shipped brick chunks) | as is, a heavier pile |
| 8 rubble, layer under | one row of the same pieces under each track's road wheels, the tank placed on top | none | — |

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
- **Solver contacts are read through `ContactTransfers`** (the coordinator's decision), the
  game's one owner of the momentum the solver moved, friction included; force is that momentum
  over the step length. So `belt_forces` carries only what an arm applies outside the solver,
  and the card counts no solver contact twice.
- **Collider roles come from what spawned the collider:** the seam's `collider_role` names the
  arm's own (markers `BumpStop`, `SidePlate` for the boat); the proxies are recognized by the
  vehicle layer and named by their authored proxy node; an unclassified collider is reported as
  such rather than folded into another role.
- **Ballistic volumes are left out of line 0:** they have no collision filters and meet nothing
  in the world, so they cannot be what meets a step.
- **The one-lane walls** span the shoe plus 0.05 m a side, one constant (`LANE_MARGIN`) shared
  with line 0's lane strips, so the two read against each other. The walls moved out of line 4
  into their own line 4b, so their columns (a force split the steps do not have) make one table;
  the full-width wall is its first row, unchanged.
- **The force split is per solver contact** (along the contact normal, and the rest as
  friction). The arm's own channel is reported unsplit, because the seam's `WorldForce` carries
  no normal and the contract stays unchanged.
- **Line 0's lateral strips use rays**, a 2 cm grid (a `Resolution`), cast rearward at each
  role's colliders; a ray beside a part's outline is a miss, not a failure.
- **The rubble cases share one parked measurement**, so the three rows are comparable column by
  column, and each adds its own work row.
- **The layer under the tracks** is one row of pieces per track, centred on the shoe's centre
  column and spanning the road wheels, the tank placed on it at rest: the plainest pile that
  loads the belt, with nothing for the hull proxy to rest on.
- **The fact check reads the render asset with a stdlib glTF parser** rather than Blender: it is
  read only, needs no window, and the parse is checked against the spawned proxy, which it
  matches to the millimetre.
- **The transmission state** (gear, detent) is read directly: the drivetrain is common to both
  arms. An arm that changes `TankTransmission` updates the card's two reads.
- **Steeds' L** is the mean road-wheel span, the construction's own nominal contact length.
- **No line-filter switch:** the whole card runs in about 2.5 minutes, so a lever with no other
  user was not added.
- **`headless_test` was not run as a suite:** the change adds no production collider or body;
  its fixtures live only in the ignored card.

## Left for a later session

- **Why the belt does not meet the 0.4 and 0.6 m step faces first** where its rest outline leads
  the rigid colliders (line 0 against line 4). It is a fact about the boat, for whoever owns it;
  the card only reports it.
- **The one-lane wall's end pose:** at the rest pose, the bump stop's nose ends 0.054 m and the
  belt's front wrap 0.105 m past the wall face (4b). Whether that is real overlap or a pose
  the rest geometry does not capture (the parts' live positions) is not measured here; a
  reading of the bump stop's contact depth would settle it.
- **Sandbox twins** for the curb, the ledge and the 4 m wall, not built this session. The card
  places the curb and the ledge from the rig's own geometry (the right shoe's grip columns),
  but the sandbox course is built at startup, before `RigGeom` exists, into one analytic field.
  So the twins need a `RigGeom`-gated course pass that rebuilds the boat's field, not just
  course data with Tiger numbers. Better still: the card's fixture list shared as data with a
  sandbox spot, so the owner drives the identical course.
- **Running the card on both arms** is the arms' work: each merges `lane/track-score` and edits
  `arm.rs` (and `collider_role`, for the colliders it spawns).
