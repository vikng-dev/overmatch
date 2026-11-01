# Dev sweep review

Subject: the game’s dev tooling; rounds, crew and impacts mentioned here are the game’s damage
model and game state only. Lane `lane/sweep-dev-1`, starting at `48f9df93`.
MEASURED clock start: 2026-11-01 07:58:39 +07; DERIVED deadline: 09:28:39 +07.
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

- **D-CI fixed:** CI tests now use `--features dev_ui,bitprobe`, matching clippy and agents.
  DERIVED from the cfg/module and test inventory: the old featureless step omitted
  `bitprobe::tests::every_seam_payload_is_as_wide_as_its_header_schema` (one test), the optional
  UI compilation paths, and the three required-feature bin test targets (which declare no tests).
  Sandbox core tests were already compiled by the old library and are retained.
  Cache decision: change only the codegen family from `codegen` to `codegen-dev-tools`; check-only
  stays unchanged. Its first fill is cold; subsequent test jobs can restore codegen artifacts for
  the actual feature set. CI duration is UNMEASURED: the cold fill adds optional UI/flate2 codegen,
  and warm runs include the schema test and instrumented paths. No timing claim is made.
- **D-FEATURE-P3 fixed:** sandbox modules now require `dev_ui`, like their bins. Their featureless
  panel-width alternative, dead-code allowances and inner panel gates are deleted, with their
  outdated comments. No workflow was triggered or pushed.
  MEASURED gates: formatting, workspace clippy and 24 sandbox/bitprobe/citation tests passed;
  one opt-in wall survey stayed ignored.

- **D-RIG fixed / partly already fixed:** `RigGeom::build`, called while the bake prepares
  `TrackColliders` before publishing the blueprint, now refuses an unreachable authored loop or
  insufficient envelope travel on either side, naming the spec file, fields and repair. The
  sandbox’s warning-only path and rigid-pad substitution are deleted. Invalid live operator
  tuning gets a typed refusal and leaves the accepted rig in place. No per-frame validation cost
  is added to idle panels. The seed-specific upper tooth-count knob is removed; the generic pin
  polygon requires at least three teeth. Duplicate sprung/fixed role addresses refuse at the spec.
  Already fixed in the starting tree: zero teeth refuse in the spec, and the shared hard-stop
  collider constructor panics on a degenerate hull while the bake prepares it.
- **D-GEAR fixed:** delete `gear_slot` and its name-pattern tests. Binding and mesh layering read
  the spec’s roadwheel/sprocket/idler lists. Each roadwheel carrier keeps its authored address;
  the view binds that exact carrier, requiring its scene/bake rest pose to agree. There is no
  nearest-station guess, warning-only missing carrier, zero-tooth clamp or zero-travel fallback.
  Mesh role addresses are scoped to the actual hull subtree, so a world object with a matching
  name cannot become running gear. Arbitrary renamed addresses and synthetic invalid rigs are
  covered. No vehicle asset, physical parameter or collider geometry changed.
  MEASURED gates: formatting, workspace clippy and 64 focused rig/spec/sandbox/headless/citation
  tests passed; 4 opt-in measurement/survey beds stayed ignored. The headless sandbox boot passed.

- **D-SANDBOX-P3 fixed:** range camera/gun/label/legend cardinality and selected-round indices
  are explicit construction invariants; panel rows cannot truncate an admitted weapon’s round
  list. The spec already bounds stowed rounds to a u8 index. Duplicate primary windows/cursors
  fail; no window/cursor is the named GPU-less harness state. Egui initialization and target
  rebuild gaps are named, and duplicate contexts or hulls fail. The range’s fixture plates are
  described as thickness fixtures rather than a model replacement still waiting to happen.
- **D-TRACK-P3 fixed:** built-rig readers require the same-flush hull; pre-rig mesh tagging and
  pending shoe assets remain named absences. Ready draw assets and retained belt entities cannot
  silently disappear. Pristine rubble fixture chunks must be convex solids and the yard requires
  a breakable element. Three zero-vector substitutions now require the valid pose/route invariant.
- **D-HARNESS-P3 fixed:** all float keys refuse nonfinite values, commands must be normalized,
  and the tick budget must be positive and representable. Its output opens at mount before rig
  construction; creation/write/final-flush I/O failures retain typed path/operation errors and
  end visibly. The writer transfers once into the recording state.
- **D-BITPROBE-P3 fixed:** the seam payload and writer accept an array sized by the header’s
  seam-name table. Wrong seam arity is unrepresentable in every build, rather than debug-only.
- **Additional capture HUD P3 fixed:** absent possession and a weapon that does not chamber an
  independent cue are named; admitted indices, weapon load slots and resolved fuze modes are
  invariants, with no dropped conversion or missing-load case.
  MEASURED final gates: formatting, workspace clippy and 59 targeted sandbox, capture, bitprobe,
  window, font, headless and citation tests passed; 4 opt-in beds stayed ignored.
  Diagnosis: adding PrimaryWindow to the range’s cursor query suppressed the GPU-less fixture’s
  standalone locked cursor. The existing headless range-explanation test reproduced this alone
  and passed after reverting only that filter. The one-cursor invariant remains; no gate test
  was edited. Duplicate cameras and invalid selection have synthetic regression tests.

- **D-CONTROLS-P3 fixed:** delete the F follow-freeze key, CameraFollow resource, reader branches
  and obsolete argument-count allowance. Orbit placement always follows its controlled rig, with
  the existing captive and tankless states intact. G retains its gizmo/capture inspection user.
  Delete the retired harness model input arm and its unused numeric model tag; unknown model
  keys now take the same refusal as every unknown key. No tracked producer used that input.
  The client-composition comment and local debug-menu draft now describe the current surface.
  The quality standard’s CI commands name the selected workspace feature set.
- **D-MARCH already fixed:** at the starting commit the armour range has no MarchMode, Demo arm,
  T key or panel toggle. Its march uses the game’s physics tick. No extra cadence was introduced.

## Handoff

All inventory groups in this lane’s share have a disposition above. No disagreement with a
remaining in-scope finding was needed. Older claims marked already fixed were checked against
source rather than counted as new changes. The base-truths reread found and removed the
asset-specific tooth-count ceiling and the unscoped world-name classification; the I/O and vector
findings are recorded with their fixes above.

The wall implementation and its subtree remain part 2’s explicit share, including its run_laws
migration and wall-only failure sites. The ballistics-owned scene/tracer calibre split and impact
emission-geometry P2s remain assigned to that sweep. ASSUMED view data still needs the owner’s
batched eye review; this lane takes no captures or pricing measurements. No deployment, remote
mutation, push, main merge or bay release was performed.

MEASURED final control/doc gates: formatting, workspace CI clippy and 22 focused camera, debug,
harness, font and citation tests passed. The preceding invariant batch passed 59 tests and the
script batch passed 51 tests; these are separate focused groups, not a unique-test total.
MEASURED final gate clock check: 2026-11-01 08:56:35 +07. DERIVED elapsed since dispatch:
57 minutes 56 seconds, within the 90-minute lane budget. Work ends because this share is handled,
not because the clock expired. The full suite remains CI’s once-after-push gate.
