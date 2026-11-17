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

After: `ServoAngles` is interpolated at the view cursor like the hull pose (`protocol.rs:881`),
linearly in the authority's own coordinates (`servo_angles_lerp`, `protocol.rs:762`). The wire
invariant stated beside it: `advance_servo` accumulates `current` without normalizing
(`servo.rs:606`, continuous mounts) and clamps a limited mount inside its stops (`servo.rs:607-615`),
and `publish_servo_angles` sends `current()` unchanged, so a straight blend follows the mount's
travel; a shorter-arc blend would cross a stop between −170° and +170°. `net::rig::present_aim`
(scheduled by `present_streamed_aim`, mounted in `install_replica_consumers` at
`src/net/client.rs:632`) runs in Update after `InterpolationSystems::All` and the rig attach,
before the `tank::ServoView` set, and writes every attached remote aim rig's `RemoteServos` slots
as `ServoState::presented(angle)`. In the game's damage-model presentation, a knocked-out tank's
launched turret keeps following its replicated `LaunchedTurretPose`, applied on arrival in
FixedUpdate. `drive_servos` returns before integrating a streamed replica; `interpolate_servos`
draws a presented state exactly. The rig attach also waits for `ServoAngles`, which rides the same
init snapshot. The own turret is unchanged.

Deleted: `apply_servo_angles` and its registration; the published branch of `drive_servos`; the
tests `public_servo_angles_skip_the_tank_that_holds_the_input_slot`,
`a_replica_consumes_truth_instead_of_simulating_a_second_drive` and the source-scan
`replicated_rig_preserves_join_in_progress_authoritative_sim_snapshots`; lens backlog row net-22.

Tests (each names the invariant a mutant breaks):
- `net::tests::stream_presence::tests::a_replica_barrel_is_drawn_where_the_stream_puts_it`: the
  shipped glb bound by `bind_tank_view` on a replica attached by the production composition; hull
  yaw, turret yaw and gun pitch streamed; frames 0.37 ticks apart, a hold and a forward cut; the
  propagated muzzle view node's direction against the bore composed from the stream at that
  frame's cursor with the servo pose law. Removing `.before(ServoView)` fails it (MEASURED: 0.0010
  rad off at cursor 160.05).
- `…::a_replica_turret_is_sampled_on_its_hull_cursor` (the earlier state check, kept narrower).
- `…::a_replica_body_waits_for_its_crank` and
  `tank::spawn::proxy_tests::a_replica_attaches_around_its_join_in_progress_snapshot` replace the
  source scan: the attach waits for `EngineRpm`, and a nondefault crank, drivetrain, weapon gate
  and grip field survive attachment, with `RemoteServos` and no `TankServos`.
- `net::protocol::tests::the_cursor_aim_blend_follows_the_mounts_travel` (a continuous ±π crossing
  and a −170°/+170° limited swing) and
  `tank::servo::tests::a_continuous_mount_counts_its_travel_through_a_half_turn`.

Server work: `publish_servo_angles` iterates tanks on the compute pool; the trace's `record_tick`
prepares each tank's row on the pool and writes the rows in entity order.

Wire: no `WIRE_TYPE_DEFS` type changed; the pins pass unchanged and `PROTOCOL_REV` stays 105.

## Capture tooling

`scripts/shot/run-turret-cursor.py fire|pivot --seed N --out DIR`: one dedicated server
(`scripts/harness/flat-patch.ron`), two `OVERMATCH_WINDOW=hidden` clients over real UDP, each
receiving through the seeded conditioner the runner's `CONDITIONER` names (80 ms, 10 ms),
`OVERMATCH_UNATTENDED=1`, `OVERMATCH_MUTE=1`. Additions it needs: `Drive::Pivot { steer }` and
`Aim::World` in the harness grammar, and `Aim::Sweep` now carries its radius and rate as script
data; a drawn-bore (`bore`) field on trace frame rows and the skeleton bore on tick rows (the
authority's on the server, the client's presentation sample on a client); the fired direction `d`
on the server's shot-trace `fire` row; a client shot-trace `drawn` row (a shell's first three drawn
stations beside its shooter's drawn bore).

Fire workload: the aim sweeps round the subject at 0.08 rad/s (4.6°/s), under the shooter's
powered traverse, so every shot leaves mid-traverse at a rate the scenario sets. A `Drive::Fire`
primary now clicks when the own tank's primary reads ready (`fire_interval: 1`), as a player waits
for READY; clicks end 40° of sweep before the observer's bearing, so no round is fired at it. A
missing truth value prints `n/a` and leaves the statistics, which report their count.

Harness fix: a scripted client never laid its own turret. `buffer_input` wrote only the wire
command, so the own tank's `TankCommand` (which `aim::drive_aim_servos` lays the own servos from)
held no aim and the own turret sat at rest on its own client while the server traversed (MEASURED
in a first pilot: own drawn bore 0.00° for 2800 ticks against a 5.75°/s server traverse). It now
writes the script's aim and range there, as a player's aim commit and range dial do. Earlier
captures that drew the own turret through the network `OVERMATCH_HARNESS` path measured a turret
at rest; own-client visual and perf workloads under a script now move the own servos too.

## #187: own and remote barrel against the fired bore

Summaries: `.agents/scratch/turret-cursor-3a/fire-{1,2,3}.txt`, after the linear blend. Every
click fired (16, 17, 16 shots; server fire gaps 198–199 ticks, MEASURED), all mid-traverse at
4.71–4.81°/s mean. "Drawn" is the first frame that draws the tracer's first segment. MEASURED,
degrees:

| run | own barrel vs fired bore | own barrel vs tracer | own lead over cursor | remote barrel vs fired bore | remote barrel vs tracer |
|---|---|---|---|---|---|
| 1 | 0.655–1.311, mean 0.801 | 0.655–1.312, mean 0.801 | 0.548–1.225 | 0.076–0.214, mean 0.111 | 0.074–0.225, mean 0.116 |
| 2 | 0.633–2.867, mean 1.207 | 0.635–2.867, mean 1.208 | 0.539–2.780 | 0.078–0.189, mean 0.110 | 0.080–0.199, mean 0.115 |
| 3 | 0.649–1.894, mean 0.863 | 0.651–1.894, mean 0.864 | 0.536–1.797 | 0.087–0.233, mean 0.120 | 0.090–0.243, mean 0.126 |

The remote barrel against the server's bore at its own frame's cursor: −0.002 to +0.003°. The
tracer's first drawn point lies 3.91–3.92 m from the drawn muzzle in both views.

Reading: a remote barrel, drawn from the stream, sits 0.07–0.24° off its tracer: the fire composes
the muzzle after tick k−1 (`shooting.rs:304-320`) and releases at cursor ≥ k, so the drawn barrel
is one tick plus the cursor's fraction further on (one tick is 0.074° at 4.71°/s, DERIVED). The own
barrel leads the stream by 0.54–2.78°, unsteadily: in runs 1 and 3 it starts at 1.2–1.9° and
settles at 0.54–0.63°; in run 2 it returns to 1.7–1.8° mid-run and reaches 2.78° at the last shot
(MEASURED, per-shot rows). Not investigated here; the own turret is C7's.

The earlier captures (06c2ffab, a half-turn traverse at the full 5.75°/s and a 196-tick click
cadence) put the remote barrel 0.090–0.203° off its tracer; the rate differs, so the two sets are
compared per degree per second, not directly.

For C7, stated neutrally: on the stream, the own barrel would sit where the remote one does here
(about 0.1° off its tracer at this traverse rate), and own aim would answer input later by the own
lead measured here. Both gaps scale with traverse rate (DERIVED).

## #132: bore of a pivoting remote

Summaries: `.agents/scratch/turret-cursor-3a/pivot-{1,2,3}.txt`, captured at 06c2ffab. The blend
changed after them from shorter-arc to linear; the two agree whenever consecutive samples are less
than π apart, which a turret 1/64 s apart always is (DERIVED), so they stand. The subject turns in
place at full steer while holding a world point; the observer records 2292–2956 frames (6.0–7.8
ms) in the steady turn. MEASURED:

| run | hull rate | bore wobble p2p (rms) | bore vs server at cursor p2p | largest bore frame step | period |
|---|---|---|---|---|---|
| 1 | 5.81°/s | 0.0068° (0.0014) | 0.0063° | 0.0033° | none |
| 2 | 5.81°/s | 0.0067° (0.0014) | 0.0064° | 0.0032° | none |
| 3 | 5.81°/s | 0.0069° (0.0014) | 0.0064° | 0.0040° | none |

The bore no longer staircases: 0.007° peak to peak against #132's 0.79°. Not at the same speed:
#132 measured 10°/s during the aim-transport arc, and this tank's fastest turn in place here is
5.81°/s. The drawn hull matches the server's hull at the cursor within 0.0065° p2p in every run.

## The early-click shot

`.agents/scratch/turret-cursor-3a/early-click.txt`: two captures on the old click schedule (script
tick 8, every 196 script ticks). With seed 42, combat release landed on a frame that advanced 16
fixed steps, so the first click ran inside that batch; the second reached the server about 189
ticks later (DERIVED from frame times) and never fired, inside the 192-tick reload, while input
arrival answered all three clicks unrefused. With seed 41 the batch came a frame before release,
clicks were 196 ticks apart in wall time, and every click fired. The review's diagnosis holds: a
harness artefact, not an input-arrival defect. Readiness clicking removes it.

## Found, not anticipated

- View-cursor rate (issue #396): per frame, the cursor advances 0.73–0.74× (p1) to 1.20× (p99) of
  real time, wandering up to 7–9.5 ticks from a straight line over the middle third of each pivot
  run (MEASURED, three runs). A turning remote's drawn hull moves 0.26–1.28× of its mean rate from
  frame to frame.
- One 30 ms cursor hold in pivot run 1: five frames at cursor 1876, the newest confirmed tick.
- The scripted own-turret gap above.
- A one-tick fire-pose offset remains between any drawn barrel and its round (above).
- `interpolate_servos` still blends previous and current along the shorter arc on the fixed
  overstep. A presented remote state has previous = current, and one fixed tick moves no mount
  near π, so it draws the same; it is the same latent form the review named.

## Checks

`cargo fmt --all --check`; CI clippy (`--workspace --all-targets --features dev_ui,bitprobe -D
warnings`); `scripts/serial-cargo check --no-default-features --bin overmatch-server` (two pre-existing
dead-field warnings in `src/tank/model.rs`); targeted tests `net::protocol net::rig
net::tests::stream_presence tank::servo tank::spawn net::harness net::input_arrival trace::
shot_trace`, 115 passed. The full library suite runs on CI.

Captures (ignored, wiped when the bay recycles):
`/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-1/dist/turret-cursor-3a/{fire-{1,2,3},pivot-{1,2,3},early-click,early-click-2}/`.
