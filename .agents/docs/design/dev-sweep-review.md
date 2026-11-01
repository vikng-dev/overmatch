# Dev sweep review

Subject: the game’s dev tooling; rounds, crew and impacts mentioned here are the game’s damage
model and game state only. Lane `lane/sweep-dev-1`, starting at `48f9df93`.
MEASURED clock start: 2026-11-01 07:58:39 +07; deadline: 09:28:39 +07.
No agents, captures, pricing runs, deployments or pushes.

## Review of sweep-view-2

Read the six commits after `8904a30e` against the three lenses and the #215 brief.
Standards and correctness: **P1 V-RATION, fixed**. A single global smoke counter let a shot from
another mount change this mount’s authored ration. Cadence now belongs to `(tank, weapon)`;
changing ammunition preserves it, other mounts cannot advance it, far shots still count, and
stale presentations do not. Unattributed developer shots form one named stream. Despawned
sources leave the table at the next aging pass. No wire type changes.
MEASURED validation: workspace CI clippy and formatting passed; 16 muzzle tests and 3 citation
tests passed, including interleaved sources and ammunition selection. The macOS test linker
reports its existing compact-unwind size warning; the gates exit successfully.

Spec and base truths: no additional P1/P2 found. Required gun `muzzle` blocks and ground
`dust.erodibility` refuse omission and invalid domains before play. Their values explicitly say
ASSUMED, and belong to the firing gun or the painted ground, with no calibre-based recipe.
The dust law rolls the authored count and multiplies by blended erodibility within the authored
reach. This remains provisional presentation data, not measured gas or soil physics.
`controlled_one` enforces HUD control cardinality; the reverse steering-table clamp matches the
transmission. A perforation mark reads penetrator diameter; a stop or bounce reads the flying
assembly; the record cutoff still reads the flying body. Keeping the APCR fireball independent
of the smaller penetrator is correct: the gun’s muzzle emission supplies it.

P3s retained from that review: VFX admission reads whole texture files for a prefix check;
`rounds::effects::resolve` resists rustfmt; sprite playback and fill-glow appearance remain
shared renderer facts. A misplaced terminal-type introductory sentence above
`face_contact_caliber` is a documentation P3. Owner’s batched eye review still owns calibration.
The existing ballistics P2 (`ballistics::view` choosing a scene/streak by calibre), and the impact
emission geometry P2, remain assigned to the ballistics sweep.

## Dev inventory

Disposition IDs below identify the inventory’s symbol groups (the inventory gave no numeric IDs).
The sandbox wall and its subtree belong to part 2 and are excluded from this lane.

- **D-CUE fixed / partly already fixed:** tree identity, segment count and standing state,
  and wall existence, already fail on this starting tree. Terrain misses and missing shot
  terrain/tree now fail at the cue. A wall charge without a core stating chemical energy refuses,
  and wall direction uses the authored-normal invariant. The offline tree cue requires ShotClock.
- **D-CAPTURE-P3 fixed:** Follow permits the named pre-spawn gap only until the frame target arms;
  duplicate control owners fail. Frame paths require a UTF-8 stem and .png extension; no invented
  frame name or format remains. Zero Follow offsets refuse at parse. The docs name the plan plus
  live scenario levers rather than claiming one exclusive input.
- **D-WINDOW partly already fixed, remainder fixed:** hide/pin already require the root’s single
  primary window. Non-UTF-8 window mode now uses the shared typed environment refusal rather than
  selecting the visible default.
- **New P1 D-SAVE fixed:** Bevy’s save observer logs disk errors but emits no failure to its caller.
  Counting its readback event as saved allowed a success exit with missing files. The capture now
  owns the write, counts only successful PNG saves, and emits an error exit on image I/O failure.
  A synthetic readback regression checks the saved count and error status without saving an image.
  MEASURED gates: formatting, workspace clippy, 9 capture tests, 4 window tests and 3 citation tests
  pass. No capture or still was taken.

- **D-SINK fixed:** cost and frame-cost sink creation now refuses startup with status 2. New
  same-pattern findings in `trace` and `shot_trace` are fixed too. Shared `SinkError` retains
  create/write/flush operations, path and OS cause; armed streams no longer drop write/flush
  failures, including the final buffered flush. A failure during an already-failing unwind is
  reported without masking that panic.
- **D-FRAME-P3 fixed:** primary-window readers require the windowed root’s singleton. Pending
  offscreen target information is named and bounded by an ASSUMED 30-wall-second readiness
  allowance; a target that never resolves fails instead of recording forever without a surface.
  MEASURED gates: formatting, workspace clippy and 18 focused recorder/citation tests passed.

- **D-COST-SCRIPT fixed:** readiness has a bounded wait and liveness check; both client statuses
  and a server that exits during the workload fail their scenario and the battery. Direct child
  PIDs are cleaned on exit/signals. The tracked runner holds one machine lock and forces silent,
  headless mode. Fake-process regressions cover readiness, primary and second client failures.
- **D-DISK fixed:** listing, reapers and cargo-sweep failures are visible and produce a failing
  status; a failed cargo sweep never prints `swept`. A fake inventory/cargo regression touches
  only a temporary fixture, never an actual checkout or bay target.
- **D-SCRIPT-P3 fixed:** both shot readers forgive only a torn final row, and refuse malformed
  middle rows. Delivery counts require recipient evidence. A malformed or missing sampling edge
  in a present network window now fails with its path/cause. Gatekeeper rejection gates packaging.
  LFS inventory and checkout failures refuse warming or leasing; unresolved pointer files refuse
  too. Bay-pool runtime tests were not run because they create temporary worktrees; shell syntax
  was checked instead. No release packaging, disk sweep or bay operation ran on real state.
  MEASURED gates: shell syntax, formatting, workspace clippy, 3 citations and 51 script tests pass
  (4 fake cost-runner, 1 fake disk-sweep, 26 shot join, 9 latency, 11 network-window parser).
