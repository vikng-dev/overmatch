# Rubble settlement: slice 0, experiment 0b

Subject: Overmatch's environment destruction and driving physics; game state only.
Lane `lane/settle-probe`, continued from `0f041da0`; production base
`ceac5e06734dc356d7340aeeef80855506196d77`. Only this report is retained.

**Binary verdict: FAIL. Stop before slice 1.** Pre-contact release recovers finite
mass at first touch, but the real parked gate fails in all three poses. Departures
leave in both directions, with qualifications below; the side hit fails its dynamic
twin's exit-velocity comparison. The earlier A/B passes were false positives from
stopping at the pile's front. They do not count toward this verdict.

All positions, dimensions, durations, selection rules and comparison gates are
**Fixture** inputs. Observations, states, counts and elapsed tick costs below are
**MEASURED**; rates, differences, percentages, unit conversions, accelerations,
quantiles and engineering estimates are **DERIVED**. Source/configuration facts are
identified as such. The gate is Fixture travel below 3 cm over 10 s and the hull's
island asleep at window end, after reaching the control's parking position.
Hosted cost is **unmeasured**.

## Method and deciding runs

The final matrix has 36 full-app runs: control and corrected B, three house shifts,
forward and reverse departures, three repetitions each. Every recorded state column
and piece-state digest agrees exactly across the three repetitions of each
arm/pose/direction; forward/reverse prefixes agree through parking. Only timing
columns differ. Control's observed position and velocity spread is zero at the
recorded f32 precision. Side-hit B and its dynamic twin each have three repetitions;
two diagnostic side arms have three each too. A further three control and three B
reverse runs record piece velocity vectors. Their original state columns agree
exactly with the primary traces; their timings are excluded from the cost tables.
No deciding conclusion relies on a single run.

Full-app runs mount the shipped simulation through
`frozen_rubble::{flat_world,fell_house}`, with shipped vehicle, house and material
data. Compute/async/IO pools are initialized to Fixture 2/1/1 threads before boot.
Every execution uses `OVERMATCH_UNATTENDED=1 OVERMATCH_MUTE=1`. All builds use
`scripts/serial-cargo` with `--features dev_ui,bitprobe`; measurements use its local
`edit` profile and the shipped physics substeps. Engine APIs were checked in the
vendored Avian source and local versioned Bevy source. No agents, web search,
GitHub writes, windows, sound, merges or pushes.

Recipe: house 40 m ahead, lateral shifts +0.05/-0.05/-0.10 m; collapse for 10 s,
wait up to 60 s for no awake loose pieces; drive toward the pieces' initial mean Z
for at most 20 s; brake for 3 s using `transmission::park_engage_speed`; release both
controls and observe 10 s; then drive for 12 s. Forward and reverse use independent
replays from collapse. The parked window begins before its first tick. Approach is
the driving portion before root Z reaches its starting Z + 32 m; climb is the rest
of the driving portion, including any obstruction. These are phase labels, not
claims that the vehicle reached the pile's top.

Control retains shipped freeze/thaw. B disables both at entry, before the house is
spawned; it imports no frozen snapshot. B runs for the entire collapse-to-departure
sequence. No production setting, material or physical parameter is retuned.

## What the corrected stand-in actually does

A piece can settle after physics Writeback and construction SplitSet if it passes
the installed `RestTest` now and has a feasible wrench on touching fixed supports.
There is no rest timer. The switch writes `RigidBody::Static` and zero linear/angular
velocity, never a pose. Static-static pairs disappear, so support collider/body
identities, contact points, normals and combined friction survive in the probe.
Loss or reattachment of a support releases its dependents before collision detection.
Retired bodies leave the probe's index.

The local rest law balances gravity and the current external contact wrench with
unilateral point forces: `normal >= 0`, `|tangent| <= mu * normal`. It adds no patch
area, adhesion or invented torsional strength. It is a circular Coulomb feasibility
stand-in, not `parts::Assessment`'s authored-face elastic distribution. Projected
FISTA has a Resolution cap of 2,048 iterations and uses the sourced
`parts::LOAD_TOLERANCE = 1e-5` for force/weight and moment/(weight * contact reach).
Acceptance checks the **projected, cone-feasible forces**, not the extrapolated
iterate. A capped solve is refused as unheld; refusal does not prove mathematical
infeasibility. The support pass propagates each local solve's chosen reactions; it
does not search every globally feasible load-sharing distribution or solve contact
compliance. These are conservative refusals for the selected distribution. Production
needs an explicit numerical-status result and an owned reaction-distribution law.

Present loads come from signed `ContactTransfers::solver_groups()` impulses divided
by their own step length, implicit belt forces, and each body's retained last
reading while its source sleeps. This cache covers candidates as well as already
settled bodies. Settled bodies' reactions propagate through the retained support
graph into lower pieces: turning an upper piece static must not erase its weight or
the tank's press. The graph is acyclic because a new rest depends only on an older
fixed layer. Prospective settles are judged together; rejected candidates' actual
dynamic loads are restored and the remaining proposal is rejudged before committing.
An arriving load rejudges its support path too. These serial dependencies belong to
the contact group, not to independent piles.

Two incomplete screens are excluded from the deciding tables. Raw manifold normals
on the flat heightfield included nearly horizontal normals (MEASURED minimum upward
component 0.004219 among 305 retained points on one 21,309.826 kg group). They made
an artificial brace that accepted MEASURED arriving momentum 406,243 N·s. The final
stand-in reads the actual terrain triangle normal through `HeightGrid::cast_ray`
for `FieldSurface` contacts; other supports use their manifold normals. This changes
only the rest verdict, not the shipped dynamic solver. Another screen dropped
static-to-static reaction loads; it is excluded too. Neither is evidence of a
physical impossibility of loaded settlement.

### Release before first contact

The production sweep was tried first through its existing `SourceShape`/`SourceTree`
bounds API, which is used directly. Its complete `Geometry::swept/resolve` path
cannot accept a settled target and point-rest wrench: it requires authored
`Structure`/`State` chunks, joints and directional cuts. No fake zero-area joints
or invented authored construction were supplied to it.

For every awake, non-fixed body failing the installed rest test, the probe builds
an **expanded AABB**, not a shape cast. It merges all actual collider boxes and the
production source bounds, expanding by `dt * |v| + dt * radius * |omega| + tolerance`.
The radius encloses the colliders about their COM; all fixture collision margins
are zero. This contains the translated/rotated shape and Avian's speculative reach
for this step. Avian creates its ordinary contact rows from the current poses and
velocities before substep acceleration; therefore this query cannot miss that
first solver touch in this fixture. It can release early on empty box corners or
separating compound features. Those false positives are a limitation and a cost,
not a claim of exact occupied first-touch geometry.

At the source-box point nearest the target COM, let
`u = v + omega cross (point - source_COM)`, `n = u / |u|`.
The estimated arriving impulse and one-tick force are
`J_arrive = m_source * |u|`, `F_arrive = n * J_arrive / dt`, applied at the box witness
projected onto the target box. For translation this is the body's momentum along
its approach. It is a conservative full-source momentum budget, not a reduced-mass
impact solve. The rest/support-path verdict receives this force and its moment;
a failed piece and its dependency closure become dynamic **before BroadPhase**.
Release assigns no velocity or extra impulse. The real contact solver then sees
its original finite mass/inertia. A released piece cannot re-settle in that tick;
if moving, it reaches further pieces at the next tick.

DERIVED flat-ground bracket: with sourced pair friction 0.5700877, gravity 9.81 m/s²
and step 1/64 s, an unloaded 59 kg piece's horizontal hold impulse is at most
`mu * m_piece * g * dt = 5.16 N·s`. A 57,000 kg hull at the sourced 0.15 m/s rest
boundary has 8,550 N·s: it frees that piece by a large margin. Masses in the actual
probe are read from `ComputedMass`; there is no vehicle-name branch. Disposable
brackets exercised gravity hold, heavy arrival and a Fixture 0.1 m/s neighbour
passing the actual rest predicate. A slow neighbour is not a sweep source merely
because multiplying its speed by mass could exceed a light piece's impulse bound.
Downward compression and genuine bracing still receive the full wrench verdict.

## Requirement 1: climb, brake, park — FAIL in every pose

Root world Z / Y, m. Start is immediately after braking; end is after the parked
window. The control's run-to-run spread is zero in every coordinate below.

| Shift, m | Arm | At brake Z / Y | Park start Z / Y | Window end Z / Y | Peak drive Y |
|---|---|---|---|---|---|
| 0.05 | control | 40.000404 / 0.327364 | 39.993046 / 0.271490 | 39.943092 / 0.197323 | 0.331004 |
| 0.05 | B | 40.000237 / 0.796787 | 40.287636 / 0.606070 | 40.400040 / 0.553024 | 0.796787 |
| -0.05 | control | 40.014923 / 0.496652 | 40.685635 / 0.437783 | 40.634342 / 0.390391 | 0.496652 |
| -0.05 | B | 40.003696 / 0.417212 | 38.236460 / 0.042685 | 36.476470 / -0.004600 | 0.417212 |
| -0.1 | control | 40.012040 / 0.900305 | 40.107254 / 0.802813 | 39.992016 / 0.708815 | 0.992421 |
| -0.1 | B | 38.420517 / 0.659009 | 38.471280 / 0.227910 | 38.464516 / 0.142972 | 0.762950 |

The first B pose reaches the pile centre and climbs higher than control, but does
not stay still. The second reaches about Z 40.004 m and then rolls back during
braking to Z 38.236 m; it retreats further during parking and sleeps after leaving
the intended parking region. The third hits the drive cap at Z 38.421 m, short of
control's Z 40.012 m before braking: the bank remains an obstruction. Its later
sleep/travel measurement is not a successful park at the control's position.
No B terminal parking position matches the control's zero observed spread.

`f_under` is fixed loose pieces / loose pieces whose **body origins** lie within
the hull body's collider-union AABB in XZ and below its top. This Fixture spatial
census is not a mass fraction or a direct-contact-only census. It changes as pieces
move, split or leave the footprint. The hold verdict itself includes the entire
support path, including loads outside that census. Fractions are at brake / park
start / window end. Travel is net 3-D root displacement, not integrated speed.

| Shift, m | Arm | Park travel, cm | Hull island asleep | f_under: brake / start / end |
|---|---|---|---|---|
| 0.05 | control | 10.1244 | no | 0/13 / 0/13 / 0/12 |
| 0.05 | B | 18.0386 | no | 3/22 / 5/24 / 2/21 |
| -0.05 | control | 8.6111 | no | 0/26 / 0/23 / 0/22 |
| -0.05 | B | 176.4383 | yes | 1/7 / 0/3 / 1/1 |
| -0.1 | control | 15.4789 | no | 0/12 / 1/13 / 0/13 |
| -0.1 | B | 8.5207 | no | 2/7 / 1/5 / 0/5 |

A sleeping endpoint in the second pose is a failure, not a substitute for the
travel/position requirements. The first and third keep switching under the hull
through the window. Time from brake to the **last observed** under-hull settle is
DERIVED 12.984 / 8.031 / 12.984 s. There is no accepted loaded parking plateau:
the last settle does not mean a piece stayed settled thereafter, and the second
pose's final 1/1 follows the tank's retreat.

## Requirement 2: drive off — leaving passes; complete gate FAIL

Every departure leaves, with no persistent stop against settled matter. The first
pose provides actual departures from the loaded pile. The second already rolled
back and the third never reached the control's parking position, so those runs
cannot certify the requested full loaded-park-then-depart sequence.

For Fixture 12 s departures, peak deceleration is DERIVED
`max((|v_before| - |v_after|) / dt)`, including vertical landing motion; it is not
an instantaneous impact acceleration. Piece speed is loose-body linear speed,
not the speed of a spinning corner. Each cell is control / B.

| Shift, m | Direction | Hull delta Z, m: C / B | Peak deceleration, m/s²: C / B | Peak piece speed, m/s: C / B |
|---|---|---|---|---|
| 0.05 | forward | 28.060 / 70.008 | 37.541 / 3.122 | 10.476 / 8.743 |
| 0.05 | reverse | -24.724 / -23.776 | 2.034 / 56.820 | 2.288 / 3.940 |
| -0.05 | forward | 53.091 / 49.264 | 9.039 / 10.616 | 7.927 / 4.661 |
| -0.05 | reverse | -24.492 / -24.653 | 3.625 / 0.242 | 3.166 / 1.232 |
| -0.1 | forward | 36.959 / 40.894 | 17.887 / 14.199 | 8.072 / 3.446 |
| -0.1 | reverse | -24.367 / -24.635 | 10.915 / 2.115 | 2.885 / 1.119 |

The first reverse B departure has the outlier: peak piece speed 3.939811 versus
2.287812 m/s. Three supplemental vector runs, all identical and matching the
primary traces, identify previously settled matter moving
`(-0.548668, -1.563434, -3.574457) m/s` at root height 2.182677 m: backward and
downward, not an upward throw at that speed. Control's speed maximum is
`(-0.500771, -2.225872, -0.169719) m/s`. Maximum upward loose-piece velocity anywhere
in these departures is 1.267519 m/s in B versus 1.133243 m/s in control. Thus the
outlier is a fall/drag disturbance, and an unqualified matching-disturbance claim
would still be false; the trajectories and pile histories differ.

B's 56.820 m/s² reverse peak accompanies the hull's downward velocity changing
from -1.365 to +0.193 m/s while backward velocity changes from -2.760 to -2.203 m/s.
It is a landing/bounce, not a tank stopped at a static wall. Neither a release nor
a settle writes an upward launch impulse. These observations do not certify every
individual piece's trajectory against its control counterpart.

## Requirement 3: repeated side hit — FAIL, without a one-tick static wall

Physics-only fixture: the shipped physics extensions, contact-transfer reader,
Brick density/friction, a Fixture 0.4 m cube (DERIVED mass 118.08 kg), and a Fixture
100 kg, 0.4 m mover starting at X -1.5 m at 3 m/s. The mover has zero gravity scale;
both bodies can still receive ground collision impulses. The target first rests
for 10 s. The twin remains dynamic; neither app installs shipped freeze/thaw. The
probe selector adds no game-specific contact-reduction component to this fixture.

All side traces are identical across three repetitions. B releases at tick 23,
before the first contact at tick 24. The target is dynamic during that solve and
leaves it with X velocity 1.805923 m/s (twin 1.815836 m/s), rather than remaining
motionless as in the earlier one-tick-wall experiment. Exit is the first step after
that contact for which the pair is no longer touching.

| Arm | Exit tick | Mover exit velocity XYZ, m/s |
|---|---|---|
| twin | 28 | (0.205189, 0.466703, 0.039567) |
| wake | 28 | (0.205185, 0.466698, 0.039569) |
| reset | 29 | (0.738826, 0.010680, 0.010040) |
| B | 30 | (0.739328, 0.019300, 0.004055) |

B minus twin is DERIVED `(0.534139, -0.447402, -0.035512) m/s`, norm 0.697663 m/s,
against the twin's zero run-to-run spread. This fails the requested equivalence.
At the first solve, the mover's pair/ground X impulses are MEASURED
-260.170/-19.751 N·s in the twin and -225.210/-0.818 N·s in B. Ground Y impulse is
30.772 versus 1.311 N·s. The finite-mass pair and ground response both changed;
this is not an unapplied impulse or an after-the-fact static release.

The diagnostic `wake` arm merely wakes the dynamic twin at tick 23 and stays close
to the twin. `reset` restores that twin's original rested pose, zeros its velocities
and reinserts Dynamic at tick 23, rebuilding its contacts without ever making it
static. It reproduces most of B's changed response. This identifies contact-cache/
pose history as a major cause, not just the wake timer. It does not isolate every
float-scale pose difference from the cache rebuild. Avian's body-type insertion
removes old pairs and re-derives collider state; pre-contact release alone does not
preserve the never-settled body's response. B later meets the piece again and ends
the short fixture with it settled; the primary exit comparison above is before
that later interaction.

## Requirement 4: churn and local cost — FAIL as a quiet/cost-saving result

All-piece switches per second, pooled by elapsed phase ticks across the three poses
and repetitions. A retired body is not a release. Forward/reverse prefixes are
identical; both are included. These are actual populations, not matched workloads.

| Phase | Control settles / releases per s | B settles / releases per s |
|---|---|---|
| approach | 0.000 / 0.000 | 0.000 / 0.000 |
| climb | 0.539 / 1.257 | 214.814 / 213.694 |
| brake | 0.000 / 0.000 | 359.444 / 355.889 |
| park | 0.833 / 0.400 | 198.667 / 198.433 |
| off_f | 2.167 / 0.611 | 266.667 / 265.722 |
| off_r | 1.500 / 0.500 | 116.222 / 115.028 |

B work below counts actual full-wrench evaluations (including support-path nodes
and rejected-proposal rejudgements), mover reach queries, and source-target AABB
pair evaluations. Cells are mean / maximum per tick. Control performs zero added
probe verdicts or reach queries; its shipped construction sweep remains installed
in both arms and is not included in these added-work counters.

| Phase | B wrench verdicts/tick | B mover sweeps/tick | B box pairs/tick |
|---|---|---|---|
| approach | 7.000 / 7 | 0.924 / 1 | 6.469 / 7 |
| climb | 38.977 / 165 | 19.560 / 48 | 97.729 / 474 |
| brake | 76.326 / 183 | 10.083 / 31 | 140.530 / 502 |
| park | 92.927 / 204 | 1.379 / 14 | 31.185 / 218 |
| off_f | 89.549 / 258 | 3.602 / 19 | 81.168 / 338 |
| off_r | 86.873 / 190 | 1.924 / 14 | 64.914 / 341 |

Whole `App::update` timing, exactly one fixed tick per call. CSV observation, vector
census and output are outside the timed update. Mean / nearest-rank p99, ms:

| Phase | Control mean / p99, ms | B mean / p99, ms |
|---|---|---|
| approach | 1.201 / 1.894 | 2.028 / 2.588 |
| climb | 7.766 / 10.933 | 16.929 / 38.151 |
| brake | 8.684 / 11.403 | 25.018 / 47.892 |
| park | 8.538 / 11.033 | 20.116 / 36.831 |
| off_f | 6.835 / 10.719 | 18.555 / 42.074 |
| off_r | 6.740 / 10.802 | 13.795 / 40.756 |

Parked cost by pose (control / B mean, then control / B p99, ms):

| Shift, m | Mean C / B, ms | p99 C / B, ms |
|---|---|---|
| 0.05 | 8.238 / 27.666 | 10.064 / 39.582 |
| -0.05 | 9.780 / 17.219 | 11.447 / 33.224 |
| -0.1 | 7.595 / 15.463 | 9.000 / 24.177 |

The parked table has 11,520 timed samples per arm. The control timings come from
the control matrix; corrected B changes only the disposable B systems. Control's
production behavior and measured trajectories are unchanged. B prices a serial,
cold feasibility solve with allocations, brute-force box pairing and no changed-load
verdict cache. Independent piles could be evaluated by ownership in production;
this timing is not that implementation's price. Conversely, a cache cannot erase
loads that really change at every switch. Hosted throughput remains unmeasured.

Under the parked hull, pieces switching more than twice and their worst individual
switch count are:

| Shift, m | Pieces switching >2 | Most switches/piece | Under-hull switch events | Ticks with those switches / 640 | Last settle since brake, s |
|---|---|---|---|---|---|
| 0.05 | 18 | 424 | 3039 | 640 | 12.984375 |
| -0.05 | 2 | 293 | 576 | 307 | 8.031250 |
| -0.1 | 5 | 450 | 1363 | 607 | 12.984375 |

## Requirement 5: feel in numbers — FAIL as a seat-and-stop result

Pitch/roll are peak-to-peak world upright angles during parking (YXZ convention).
Vertical range includes the position immediately before the first parked tick;
vertical speed is the largest absolute root Y speed after that start. These are
observations, not proposed regression targets.

| Shift, m | Arm | Pitch / roll range, degrees | Net Y / Y range, mm | Peak absolute Y speed, m/s |
|---|---|---|---|---|
| 0.05 | control | 2.314 / 1.157 | -74.167 / 75.971 | 0.225508 |
| 0.05 | B | 1.480 / 0.679 | -53.045 / 56.880 | 0.101950 |
| -0.05 | control | 0.291 / 1.205 | -47.391 / 47.917 | 0.070401 |
| -0.05 | B | 1.890 / 0.352 | -47.285 / 48.727 | 0.057981 |
| -0.1 | control | 1.268 / 0.381 | -93.998 / 94.572 | 0.082045 |
| -0.1 | B | 2.658 / 0.860 | -84.938 / 91.630 | 0.145151 |

Switch-associated root dips, mm; positive is downward, negative upward. Every event
under the hull has its same-tick and following-tick dip in
`/tmp/settle0b-switch-dips.csv`, including event kind and piece identity. Multiple
pieces switching in one tick share that tick's observed hull motion. The table
summarizes the complete parked event records, not a selected example.

| Shift, m | Parked under-hull events | Same-tick dip range, mm | Following-tick dip range, mm |
|---|---|---|---|
| 0.05 | 3039 | -1.815 to 1.403 | -1.815 to 1.403 |
| -0.05 | 576 | -0.272 to 0.398 | -1.120 to 0.398 |
| -0.1 | 1363 | -2.265 to 1.578 | -2.265 to 1.578 |

A post-step settle/release follows that tick's motion; pre-contact release precedes
it. These are temporal associations, not isolated causal displacement per switch.
The first pose drifts mainly laterally/longitudinally while switching every parked
tick; the third sinks about 85 mm and remains awake. The second becomes quiet only
after retreating. Smaller peak vertical speed in some B rows does not make those
outcomes a successful quiet park.

## Corrected design premises and production interfaces

- `src/construction/fracture/parts.rs` assesses authored face joints, with area,
  compliance and face geometry, not arbitrary point rests. A zero-area fake joint
  is not the same law. A point-rest interface must return reactions as well as a
  verdict, so settled upper loads reach lower supports; it needs explicit numerical
  status and reusable workspace, without mutating an authored `Structure`.
- `src/construction/breakage/sweep_intake.rs` excludes `Piece` both as owners and in
  its contact hook. `sweep::resolve` applies Hertz/drive demand to directional
  translation cuts, not a full support wrench. Its existing-gravity preload
  contract does not implement arbitrary live `mu(N+W)` hold. Source occupied bounds
  are directly reusable, as this experiment demonstrates; target geometry,
  support-path judgement and finite-mass release/contact-history handling are new
  interfaces, not a filter widening.
- Solver manifold normals need not be the actual supporting surface normal, as
  the flat-heightfield screen demonstrates. A retained rest needs a geometric
  witness, identity/pose/shape invalidation, pair friction and last sleeping press.
  `HeightGrid` already supplies terrain triangle normals; general collider support
  features still need an owned adapter.
- The claimed settling-energy bound omits angular energy. The installed rest test
  accepts either its energy condition **or** separate linear/angular speed bounds.
  Actual discarded kinetic energy is
  `E = (m|v|² + omega dot I_world omega)/2`.
  For nonnegative separate thresholds, a DERIVED upper bound on that arm is
  `(m*v_limit² + lambda_max(I)*omega_limit²)/2`, not the linear term alone.
  `avian_ext::rest_test::kinetic_speed_squared` already supplies the full kinetic
  expression. Production must expose/check the chosen bound explicitly; this
  experiment does not introduce a new settling-energy criterion.

DERIVED rough engineering ranges below are planning estimates from these seams,
not measured production sizes or an implementation plan approved by this result.
They exclude removal of the old freeze/thaw implementation.

| Needed API | Contract | Rough implementation lines |
|---|---|---:|
| Point-rest/wrench law | Points, friction, physical inertia and external wrench -> reactions plus Held/Unheld/NumericalFailure | 300–500 |
| Rest-network and sleeping-load cache | Propagate reactions; retain current sleeping presses; judge prospective fixed layers | 200–350 |
| Settled-target sweep adapter | Occupied shape/inertia, geometric first-touch witness, full support path, ordered release closure | 250–450 |
| Support-feature witnesses | Geometric normal/friction and collider generation/pose/shape validity | 100–200 |
| Contact-history handoff | Preserve or deliberately reconstruct the released body's valid manifold/warm state through body-type conversion | 200–400 |
| Full settling-energy gate | Explicit kinetic-energy value/bound, angular inertia included | 20–50 |
| Changed-input index and ownership boundary | Reuse unchanged verdicts; parallel independent groups; order effects | 100–200 |

Together: roughly **1.2–2.2k implementation lines plus 0.5–0.9k focused test lines**.
The contact-history seam also needs Avian integration; simply reusing the existing
public body-type setter is what produced the measured twin mismatch here.

## Decision

**Stop at slice 0; do not build slice 1 from this criterion.** The loaded gate fails
in all poses, two fail sleep and one sleeps after retreating, and matching first-touch
velocity still fails after timely release. The corrected rest network makes the
first pose worse than control and still produces hundreds of switches per second.
It does not supply the direction's intended cheap, quiet loaded pile.

The switching circle is real for this tested rule at the shipped integration
resolution: a slow body can pass the proposed hold and then fail after contact
loads redistribute. More eager release could change the obstructing bank, and a
contact-history handoff could address the isolated hit. Neither alone establishes
a quiet loaded park. A stricter settle criterion might suppress switches by keeping
more matter dynamic, leaving the original loaded-contact creep and cost instead.
No tested alternate criterion here fixes all three gates. The finite solver cap
and the difference between cone feasibility and the live compliant contact law
also prevent claiming a proof that no physically valid loaded rest can exist.
The result refutes this replacement criterion, not the possibility of real rubble
coming to rest. The diagnosis's integration-resolution direction retains stronger
direct evidence for addressing the original parked creep.

## Retained evidence and verification

Primary control traces: `/tmp/settle0b-control-<shift>-{f,r}{0,1,2}.csv`.
Corrected B: `/tmp/settle0b-B-<shift>-n{f,r}{0,1,2}.csv`.
Side fixtures: `/tmp/settle0b-side.csv`; vector audit:
`/tmp/settle0b-{control,B}-0.05-obsr{0,1,2}.csv`.
`/tmp/settle0b-summary.json` records derived summaries;
`/tmp/settle0b-switch-dips.csv` holds each under-hull event and its two observed dips.
Earlier plain `B-...-f/r...` traces and `screen/debug` logs are excluded screens.
Only the corrected `nf/nr` series prices and decides B.

All disposable source, test registrations, arm gates, instrumentation and runner/
analysis scripts are deleted using literal absolute paths. The bay's `target/`
is preserved. No production code or retained test is committed.

Final restored-source checks:

- `git diff --exit-code -- src vendor`: passed.
- `cargo fmt --all --check`: passed.
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`: passed.
- Headless `scripts/serial-cargo test --lib --features dev_ui,bitprobe debris::freeze::tests`: 13 passed, none failed.
- `git diff --check`: passed.

No merge or push. The commit retains only this report.
