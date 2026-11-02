# Ceiling and empty station, lane 5 (#146)

Subject: the game's driving model and damage model, game state only. Crew are HP pools at
stations. No bodily effects are modelled or described here.

MEASURED lane start: 2026-11-02 20:40:07 +07. The lane owns the ceiling, construction control
rest, signed crank, stopped reversal and the inherited S3/S4 proofs. No captures, frame/tick
pricing, issue comments, push, merge or release are authorized or performed.

## Decisions and law

- The reversal lever holds both ways from arrival at rest through the stopped pause and every
  tick of the paid direction cut. Only launch uses the rollback-only lever. Beyond the dynamic
  brake capacity a sliding tank cannot promise a stop: it retains the old ladder, keeps braking
  and refreshes the pause; it does not cut while sliding.
- The Driver uses service brakes to hold the current gear's crank ceiling. No protective
  upshift returns. Without Driver work, only the independent over-rev clutch slip guard acts.
- Required construction data names a closed fuel-rack stop, centered steering return and a
  spring-engaged clutch. Gear and park remain where left. Empty-station idle fuel is rejected:
  it would invent an idle actuator absent from this construction. An already paid cut expires
  mechanically, then the clutch springs engaged even at rest; no new cut is paid without work.
- A zero-rate task cannot open the clutch. A backward crank fires nothing and has signed
  motoring drag opposing its rotation. Free-crank drag stops at zero within a discrete step;
  only external clutch work can carry it through zero. Crank guards are symmetric without a
  Driver catch. Positive catch-floor ownership is shared by pricing and the crank solve.
- The scheduler enum and field are deleted, including hill-hold and grade-limit display tags,
  trace/hash slots and HUD markers. Eight canonical fields remain: `gear`, `shift_ticks`,
  `steer_step`, `reverse`, `park`, `omega_e`, `clutch`, `reversal_hold_ticks`. Protocol revision
  77 deliberately names the wire change; pins must match the measured canonical bytes.
- The effective bristle shear distance and elastic breakaway share keep their existing values
  but become required labelled StandIn data in `assets/driving/grip.ron`. Their former comments
  overclaimed a measured pad/ground truth; the data now names the missing constitutive evidence.
- The player's notch survives an empty Driver station. Command owns that retained input;
  `drive_target` masks it while authority is absent, and staffing resumes it without a key edge.

## Reviewer dispositions

`LAW` means `driving: hold reversals and rest controls by construction (#146)`.
`INPUT` means `driving: admit reachable notches and tolerate pedal roundoff (#146)`.
Each row was traced against this lane's base before editing; duplicate reviewer findings share
one disposition. A source verification is distinguished below from an executed red witness.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude + Codex brake P1 | stopped reversal drifts in the requested downhill direction | verified red: pause/cut witness drifts at -0.044084426 m/s | LAW |
| Claude brake P2 | direction cut loses hold and bypasses C3 | verified source; same two-way pause/cut defect | LAW |
| Claude brake P2 | HillHold is derived during ordinary service braking and replicated | verified source; scheduler and readouts removed altogether | LAW |
| Claude brake P2 | released motion between solver rest and first-gear idle has no stop witness | verified source; construction-derived threshold witness added | LAW |
| Codex brake P2 | Rust authors bristle shear modulus and breakaway | verified source; required labelled constitutive data replaces both constants | LAW |
| Claude brake P3 | coast equality does not catch common service braking | verified source; comment narrowed to its terrain-lever invariant; coupling/service witnesses own the other evidence | LAW |
| Claude brake P3 | notch travel assertion gates on selector's own launch answer | verified source; level travel is unconditional | LAW |
| Claude + Codex brake P3 | TransmissionInput and lab fixture name deleted brake inputs | verified source; current input/contact contract replaces them | LAW |
| Claude brake P3 | geared-steering comment falsely excludes later brakes | verified source; comment names pre-brake prediction | LAW |
| Claude brake P3 | driver force price duplicates catch-floor predicate | verified source; `idle_floor` owns the positive floor | LAW |
| Claude brake P3 | repeated scheduler Normal writes | verified source; scheduler state and all writes deleted | LAW |
| Claude brake P3 | reversal countdown doc says only opposing motion refreshes it | verified source; current any-motion invariant stated | LAW |
| Claude brake P3 | park-speed doc says the tolerance controls entry only | verified source; shared rest/direction/launch/static use stated | LAW |
| Claude brake P3 | both-constructions reversal witness varies only transmission | verified source; matched contact/inertia/transmission pairs replace the cross-pair | LAW |
| Claude brake P3 | shipped notch proof computes rotation twice | verified source; one expression selects the fixture pose | LAW |
| Claude brake P3 | tests and contact comment name retired blend/slip terms | verified source; current regime wording replaces them | LAW |
| Claude brake P3 | HUD describes a nonexistent hill-hold latch | verified source; marker and scheduler row deleted | LAW |
| Claude brake P3 | spec, glossary, transmission inventory and architecture prose stale | verified source; current lever/inventory stated; old architecture observation explicitly historical; Blender fixture updated with spec | LAW |
| Claude + Codex brake P3 | brake record says protocol 75 instead of 76 | verified source against that lane's protocol declaration; corrected | LAW |
| Claude clutch P2 | price invents moving sub-idle launch Slip | not a remaining defect: prior gear lane already prices In as a lug; inherited direct witness retained | prior gear law |
| Claude clutch P2 | launch admission depends on prior Out | not a remaining defect: prior brake lane admits arrested In launch; inherited witness retained | prior brake law |
| Claude clutch P2 | zero-rate task opens a moving clutch | verified source; moving zero-rate witness replaces the old rest-only assertion | LAW |
| Claude clutch P2 | open clutch grants fuel without authority or a window | verified source; rev-match admission now needs authority and a paid window or active steering | LAW |
| Codex clutch P2 | algebraic notch enumeration misses accumulated f32 rounding | verified source; deferred until the primary law is committed green | carried to INPUT |
| Codex clutch P3 | exact slew tick count rejects admitted 0.64 rise taste | verified source; deferred until the primary law is committed green | carried to INPUT |
| Claude clutch P3 | free-torque wording assigns catch to combustion sources | verified source; wording names Driver task throughput | LAW |
| Claude clutch P3 | first empty-station drag sample is transient | verified source; final-second signed motoring drag replaces it | LAW |
| Claude clutch P3 | moving-lug witness starts In and misses Out landing | verified source; Out moving sub-idle entry now asserts In without fresh launch Slip | LAW |
| Claude clutch P3 | empty-station run-down conflicts with proposed idle rest | verified source/design conflict; explicit closed-rack construction decides it | LAW |
| ratchet driving-03 | empty-station lever-persistence contract unstated/unproved | verified source; command and adapter witnesses plus transition contract close the row | LAW |

## Red loop and API evidence

MEASURED `scripts/serial-cargo test --lib --features dev_ui,bitprobe reversal_pause_and_cut -- --nocapture`
failed on the unmodified law: `reversal pause/cut drifted at -0.044084426 m/s`. The fixture
puts gravity in each new request's direction on a supported ten-degree grade. The shared clamp,
not a vehicle-specific branch, is the cause. Flat reversal witnesses cannot distinguish it.

Engine API verification used the installed, locked Bevy **0.19.1** sources (the actual patch in
`Cargo.lock`) for App world/update, World get/get_mut/entity access, Query iteration, and entity
insert/remove; avian3d **0.7.0** source for Position, Rotation and velocities. No engine API was
inferred from the older prose. The protective upshift, crank corroboration margin and old static
projection were already absent from the base's source; no alternate implementation was retained.

## S4: renewed certification

MEASURED on the revised gear price, fresh app for each ground/entry; complete pose, velocity,
transmission, shaped input and bristle reset; 90 simulated seconds of supported planar travel.
The last second has positive hull speed and positive elastic support on both sides, no cut,
clutch In and crank above the engaged-lug threshold. Every position remains inside the plane.
The fixture's equilibrium-spare precision is DERIVED `0.0001 * weight` (55.917 N here), independent
of the selector's answer; M is 3.915964 N. The larger precision covers the coupled contact solve,
not a gear preference. The first 60-second attempt refused sand's rolling entry at 104.804688 N;
only the renewed 90-second measurements below certify it.

| ground | rest entry: gear / mean m/s | rolling entry: gear / mean m/s | maximum last-second speed spread m/s | maximum absolute current spare N | best higher landing spare N | result |
|---|---|---|---|---|---|---|
| concrete | F8 / 10.650209 | F8 / 10.650208 | 0.000011 | 2.814453 | no higher ratio | converged |
| dirt | F8 / 10.569968 | F8 / 10.569969 | 0.000011 | 3.144531 | no higher ratio | converged |
| gravel | F8 / 10.610109 | F8 / 10.610108 | 0.000006 | 2.341797 | no higher ratio | converged |
| mud | F6 / 4.849176 | F6 / 4.849176 | 0.000009 | 7.804688 | -21679.164062 | converged |
| sand | F6 / 4.874709 | F7 / 6.211291 | 0.000163 | 11.281250 | -5650.050781 (F6 entry); -27344.066406 (F7 entry) | C1 vehicle-truth split |

The sand split meets the design's permitted vehicle-truth outcome: both entries sustain their
current gear, neither can afford a higher landing, and neither is a lug. C1 therefore does not
admit a lower gear just because it has more force. No ground name participates in the law or
pass condition. This replaces the old partial-teleport observational test.

## S3: the construction limit

MEASURED at 5 m/s entry on a ten-degree plane, with the Driver station's HP pools emptied before
the next tick. Every observed tick keeps clutch In, the original gear, no shift, no park movement,
zero induced fuel and zero Driver catch. A backward-crank mutant which fires or loses signed
motoring drag fails the direct class witness; an empty-station freewheel mutant fails this shipped
composition through both descent and the uphill turning point. First-tick force is not evidence:
the witness measures the final second's opposing force.

The requested blanket vehicle-speed bound is **not established**. The inherited over-rev guard
limits the crank, not the belts through a slipping clutch. MEASURED F6 on the 20-second descent
reaches 13.315221 m/s, with a final-second range 12.902819–13.315221 m/s. After the uphill coast,
F6 reaches -8.660275 m/s, range -8.660275–-8.247806 m/s. Both crank magnitudes remain bounded by
3100 rpm and both final drag magnitudes are 41712.507812 N, but the vehicle is still accelerating.
No finite terminal speed follows from that guard when grade force exceeds guarded drag plus
contact resistance. These runs stay on the supported plane; the first 40-second F6 attempt was
refused when it ran off the finite fixture, rather than being reported as a bound.

The lower launch gear can reach a motoring-drag equilibrium; its renewed 40-second measurements
and final gates are appended below. The decision is to preserve the accepted clutch slip guard
and report the failed F6 speed requirement. Making F6 bounded here would require another physical
loss/control truth, or a changed over-rev contract; silently adding brakes at an empty station or
clipping belt speed would violate the accepted construction rest. This is remaining acceptance
work, not a certified S3 success.

## Gates

The primary proof run passed 83 tests, including all arithmetic drivetrain proofs, fresh S3/S4
measurements and held reversals. CI clippy passed after the production law, and fmt passed after
the subsequent fixture/comment refinements. Final cross-cutting counts follow below.


### Primary step: green and committed

MEASURED renewed lower-gear S3 bounds at 40 simulated seconds:

| entry | entire observed hull-speed range m/s | final-second hull-speed range m/s | final-second mean motoring force N | maximum crank magnitude rpm |
|---|---|---|---|---|
| downhill, F4 | 1.775128–5.134676 | 1.775128–1.775237 | -69572.195312 | 3100 |
| uphill then rollback, F4 | -1.774553–5.038763 | -1.774553–-1.774450 | +69559.187500 | 3100 |

These are MEASURED bounds for the stated construction and fixture, not a new per-vehicle gate.
The F6 counterexample above prevents certification of the requested general S3 speed bound.
MEASURED held reversal, both complete constructions, flat and opposite ten-degree entries:
32 stopped ticks (DERIVED 0.5 s at the fixed clock), exactly one 20-tick paid cut, then launch.
The beyond-capacity witness retains its direction while sliding and refreshes its pause.

The law deletes the remaining scheduler enum, field, writes, readouts, HUD marker, canonical
scheduler slots, empty-station neutral retention, zero-rate clutch opening, unrestricted
open-clutch rev-match fuel and nonnegative crank restriction. The old protective upshift,
corroboration margin and static crank projection were already absent; the canonical eight-field
state projection remains. Replaced neutral/rest and zero-rate tests change with the law; the
partial-teleport S4 and transient first-drag S3 witnesses are replaced, not kept beside it.

MEASURED primary test-section line change: **+270**. Method: whole test-only headless and
transmission-test files, plus the `cfg(test)` module bodies in command, sim, lib, state hash and
protocol, compared with the lane's base. This includes the new grade/pause/cut, threshold stop,
backward rotation, rev-match authority, ceiling and lever-persistence proofs; no retired-law arm
is retained to save or inflate the count.

MEASURED final primary gates: fmt passed; CI clippy passed with `--locked --workspace --all-targets
--features dev_ui,bitprobe -- -D warnings`; targeted/cross-cutting run **391 passed, 4 ignored,
0 failed**, covering `track::`, command, drive HUD/row, trace, bitprobe, protocol, shipped headless
composition, and the whole-tree `doc_citations ui_ascii net_boundary bake::` guards. The ignored
cases were not run. The wire-type pin is `0x6d97144c0e2cc746`; the measured eight-field sample
transmission digest is 7604051283656621841. All runs used serial-cargo's machine lock, unattended
headless mode, and no capture or performance pricing. Primary gate completion: 21:22:39 +07.

The only carried implementation items after the primary green step are float32 notch admission
and the slew test's exact tick count. S3's F6 construction limit remains acceptance work for a
separate physical/control decision, not another switch in this implementation.
