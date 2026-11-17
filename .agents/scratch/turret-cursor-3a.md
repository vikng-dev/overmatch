# Timing slice 3a: remote turrets on the view cursor (#330), captures for #187 and #132

Subject: the game's netcode presentation of turrets, guns and rounds. No damage-model state is
read or changed; "fire" and "tracer" name rounds as presented.

## Remote turrets

Before (main 8337127d): `ServoAngles` replicated plainly (`src/net/protocol.rs:892`) and was
applied on arrival. `apply_servo_angles` (`protocol.rs:675`, FixedUpdate `GameplaySet`,
`protocol.rs:1043`) wrote each non-owner's `ServoCommand.target`; `drive_servos`' published
branch (`src/tank/servo.rs:410`, `:434`) set `current = target`; `interpolate_servos`
(`servo.rs:622`) blended on the client's fixed overstep. A remote turret ran on the client's fixed
clock at arrival time while its hull was drawn at the view cursor.

After: `ServoAngles` is interpolated at the view cursor like the hull pose
(`protocol.rs:879`, shorter-arc `servo_angles_lerp` at `protocol.rs:760`). `net::rig::present_aim`
(`src/net/rig.rs:90`, scheduled by `present_streamed_aim` at `rig.rs:81`, mounted in
`install_replica_consumers` at `src/net/client.rs:632`) runs in Update after
`InterpolationSystems::All` and the rig attach, before the new `tank::ServoView` set, and writes
each non-owner's `RemoteServos` slots as `ServoState::presented(angle)`. `drive_servos` returns
before integrating a streamed replica (`servo.rs:420`); `interpolate_servos` (`servo.rs:628`)
draws a presented state exactly. The rig attach now also waits for `ServoAngles`, which rides the
same init snapshot. The own turret is unchanged.

Deleted: `apply_servo_angles` and its registration; the published branch of `drive_servos`; the
tests `public_servo_angles_skip_the_tank_that_holds_the_input_slot` and
`a_replica_consumes_truth_instead_of_simulating_a_second_drive` (replaced by
`net::rig::tests::the_stream_poses_every_replica_but_the_controlled_and_the_launched` and
`tank::servo::tests::the_fixed_step_never_moves_a_streamed_replica`); lens backlog row net-22.
New end-to-end witness: `net::tests::stream_presence::tests::a_replica_turret_is_drawn_on_its_hull_cursor`
(hull and turret counter-rotate on the production view clock; the world bearing holds every frame).

Wire: no `WIRE_TYPE_DEFS` type changed; the pins pass unchanged and `PROTOCOL_REV` stays 105.

## Capture tooling

`scripts/shot/run-turret-cursor.py fire|pivot --seed N --out DIR`: one dedicated server
(`scripts/harness/flat-patch.ron`), two `OVERMATCH_WINDOW=hidden` clients over real UDP, each
receiving through the seeded 80 ms / 10 ms conditioner, `OVERMATCH_UNATTENDED=1`,
`OVERMATCH_MUTE=1`. Additions it needs: `Drive::Pivot { steer }` and `Aim::World` in the harness
grammar; a drawn-bore (`bore`) field on trace frame rows and a tick-truth bore on tick rows; the
fired direction `d` on the server's shot-trace `fire` row; a client shot-trace `drawn` row (a
shell's first three drawn stations beside its shooter's drawn bore).

Harness fix: a scripted client never laid its own turret. `buffer_input` wrote only the wire
command, so the own tank's `TankCommand` (which `aim::drive_aim_servos` lays the own servos from)
held no aim and the own turret sat at rest on its own client while the server traversed (MEASURED
in a first pilot: own drawn bore 0.00° for 2800 ticks against a 5.75°/s server traverse). It now
writes the script's aim and range there, as a player's aim commit and range dial do. Every earlier
scripted capture of the own turret measured a turret at rest.

## #187: own and remote barrel against the fired bore

Summaries: `.agents/scratch/turret-cursor-3a/fire-{1,2,3}.txt`. The shooter traverses at its
idle-powered full rate (MEASURED 5.75°/s, stationary) and fires every 196 ticks; ten shots per run
leave mid-traverse, two at rest after it. "Drawn" is the first frame that draws the tracer's
first segment. MEASURED, degrees, over the ten mid-traverse shots per run:

| run | own barrel vs fired bore | own barrel vs tracer | own lead over cursor | remote barrel vs fired bore | remote barrel vs tracer |
|---|---|---|---|---|---|
| 1 | 2.18–3.63, mean 2.59 | 2.18–3.64, mean 2.59 | 2.10–2.97 | 0.093–0.169, mean 0.126 | 0.090–0.177, mean 0.130 |
| 2 | 2.19–3.80, mean 2.65 | 2.19–3.80, mean 2.65 | 2.11–3.11 | 0.103–0.195, mean 0.151 | 0.106–0.203, mean 0.157 |
| 3 | 2.22–3.53, mean 2.56 | 2.21–3.54, mean 2.56 | 2.13–2.83 | 0.092–0.188, mean 0.126 | 0.091–0.197, mean 0.130 |

At rest, both views: 0.002–0.024° to the fired bore, 0.012–0.035° to the tracer. The remote
barrel against the server's bore at its own frame's cursor: −0.004 to +0.002°. The tracer's first
drawn point lies 3.91–3.92 m from the drawn muzzle in both views.

Reading: the own barrel leads the stream by 2.1–3.1° at 5.75°/s, 23–35 ticks (0.37–0.54 s,
DERIVED from rate). The first four shots of every run lead by 2.8–3.1° and the rest by about 2.1°:
in fire-1's frame rows the view cursor moves from 10–11 ticks behind the newest confirmed tick to
1–2 behind between 11 and 14 s into the run (MEASURED), and the own lead falls with it. A remote barrel, drawn from
the stream, sits 0.09–0.20° off the fired bore: the fire composes the muzzle after tick k−1
(`shooting.rs:304-320`) and releases at cursor ≥ k, so the drawn barrel is one tick plus the
cursor's fraction further on (one tick is 0.090° at this rate, DERIVED).

For C7, stated neutrally: on the stream, the own barrel would sit where the remote one does here
(about 0.1–0.2° off its tracer at this traverse rate), and own aim would answer input later by the
own lead measured here (0.37–0.54 s in this conditioner). Today the own barrel and its tracer
differ by 2.2–3.8°. Both gaps scale with traverse rate; a faster traverse (a revved engine) widens
both proportionally (DERIVED).

## #132: bore of a pivoting remote

Summaries: `.agents/scratch/turret-cursor-3a/pivot-{1,2,3}.txt`. The subject turns in place at
full steer while holding a world point; the observer records 2292–2956 frames (6.0–7.8 ms) in the
steady turn. MEASURED:

| run | hull rate | bore wobble p2p (rms) | bore vs server at cursor p2p | largest bore frame step | period |
|---|---|---|---|---|---|
| 1 | 5.81°/s | 0.0068° (0.0014) | 0.0063° | 0.0033° | none |
| 2 | 5.81°/s | 0.0067° (0.0014) | 0.0064° | 0.0032° | none |
| 3 | 5.81°/s | 0.0069° (0.0014) | 0.0064° | 0.0040° | none |

The bore no longer staircases: 0.007° peak to peak against #132's 0.79°. Not at the same speed:
#132 measured 10°/s during the aim-transport arc, and this tank's fastest turn in place here is
5.81°/s. The drawn hull matches the server's hull at the cursor within 0.0065° p2p in every run.

## Found, not anticipated

- View-cursor rate: per frame, the cursor advances 0.73–0.74× (p1) to 1.20× (p99, and p90: a
  ±20% bound reached in at least a tenth of frames) of real time, mean 65.3 ticks/s, wandering up
  to 7–9.5 ticks from a straight line over the middle third of each pivot run (MEASURED, three
  runs). A turning remote's drawn hull moves 0.26–1.28× of its mean rate from frame to frame
  (pivot runs 2 and 3; run 1 includes the hold below). A held world bearing hides this; any
  moving remote shows it.
- One 30 ms cursor hold in pivot run 1: five frames at cursor 1876, which was the newest confirmed
  tick.
- The scripted own-turret gap above.
- With clicks from script tick 8 every 196 ticks, two of three runs fired only nine shots
  mid-traverse; in the one inspected, the second click never fired. From script tick 64 every
  click fired. Not investigated.
- A one-tick fire-pose offset remains between any drawn barrel and its round (above).

## Checks

`cargo fmt --all --check`; CI clippy (`--workspace --all-targets --features dev_ui,bitprobe -D
warnings`); `scripts/serial-cargo check --no-default-features --bin overmatch-server` (two
pre-existing dead-field warnings in `src/tank/model.rs`); targeted tests `net::protocol net::rig
net::tests::stream_presence tank::servo net::harness net::input_arrival trace:: shot_trace`, 104
passed. The client/server exchange is unchanged, so the full library suite was not run.

Captures (ignored, wiped when the bay recycles):
`/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-1/dist/turret-cursor-3a/{fire,pivot}-{1,2,3}/`.
