# Boundary-fix lane review

Subject: Overmatch, a video game; discussion of damage, crew and rounds concerns only the game's
damage model and game state. Crew are HP pools at stations; the terms in AGENTS.md apply.

## Sweep-load checkpoint

Reviewed every hunk of `29614433..7e073952` and the additional sweep-load commits already present
in this bay (`711792b4`, `a8405f29`, `46253a1b`), against correctness and all three review lenses.
The bay started clean on `lane/boundary-fix-1101` at `46253a1b`; those existing commits are retained.
No subagents, captures, pricing runs or damage-topic web searches were used.

No P1 finding. P2 findings fixed before the boundary items:

- Error handling: `spec::duration` accepted finite delays outside `Duration` or the weapon gate's
  tick-count representation. The load gate now refuses both, naming the authored field. The
  existing reload/swap mutation battery includes these cases. The tick domain uses the default
  fixed timestep, the same timestep the game's compositions use.
- Base truths: `tools::buildings::tests::a_house_the_generator_cannot_lay_out_is_refused` assumed
  that every shipped house had east-wall openings. Its final mutations use labelled, constructed
  fixture data, so changes to any shipped house's openings cannot break the refusal test.

P3 findings retained for the owner:

- `settings::store::location` still links the deleted `reset` function in its doc.
- `TankSpec::capabilities` says `{}` gates no capability, but `validate` requires every capability
  to be keyed; `[]` is the ungated requirement.
- The recoil-spring test's rationale was attached to the adjacent primary-count test in the last
  sweep-load commit; each test should carry its own rationale.
- `assets::asset_root`'s error tells the operator to set `BEVY_ASSET_ROOT` as a bypass, but it still
  calls `current_exe` before resolving that override. Its deliberate refusal without an executable
  path is preserved; the remedy in the diagnostic should match that behavior.
- `assets::tests::the_selected_tank_derives_its_trio_paths` pins the selected vehicle's literal
  paths. The fixture is labelled, but a selection change should ideally exercise the generic
  path relation instead of requiring new expected strings.

Code branching: no new P1/P2 finding. The removed switches have no remaining live callers.
The two known walk-test failures belong to `walk-w3` and are excluded from this review.

## Boundary items

### Driving

- `contact_side` asserts the spawn-sized grip slabs in every build; neither its docs nor
  `GripElements::for_links` describes lost traction as an intended release outcome.
- Removed the authored speed and force caps, their fixture data and their consumers. Powered
  speed follows the gear ladder and fuel governor; static gear reserve uses engine torque bounded
  by the authored clutch capacity. Contact grip bounds traction. External load may drive belts
  beyond governed speed; the existing mechanical over-rev guard still bounds the crank.
- The parking-brake command blend uses the selected ladder's DERIVED governed top-gear speed as
  its scale. It does not limit belt speed. The now-identical pre/post-clamp bitprobe columns were
  reduced to the actual next-speed columns.
- The neutral-pivot test uses an ASSUMED class acceptance bound of half the vehicle's own
  belt-kinematic yaw ceiling, rather than a fixed yaw measured on the seed vehicle. The wide-turn
  test also derives its expected speed and radius from a labelled lab fixture.
- Updated all cited live-tense drivetrain docs; `SteeringSpec`'s hybrid wording was already removed
  by sweep-load at the starting revision.
- Gates: formatting, workspace CI clippy, and the targeted track/spec/bake/bitprobe/pivot and
  doc-citation tests passed (MEASURED: 230 tests). No full suite or pricing was run.
- Pivot evidence: MEASURED mean yaw magnitude 0.1312 rad/s; DERIVED kinematic ceiling
  0.1884 rad/s, DERIVED delivered share 0.6962. The belts counter-rotated.

### Game damage-model cam

- Removed `Knobs::orbit`, its parameters and cache key, and its test leg. The player's
  reduce-motion setting and the camera's automatic motion remain their live paths.
- Removed `ViewRoot`, its field, component inserts and dead checks; every `ViewMesh` is the
  stage view by construction.
- The killer card resolves a live shooter's strong `TankSpecHandle` through `Assets<TankSpec>`.
  No live shooter is a named absence. A live shooter missing its handle or loaded data is a bug.
  The regression uses a loaded spec with no file path and a distinct in-memory display name.
- A record's unknown round key is a bug. A registered round chambered by several guns is valid:
  the record carries no firing-gun key, so the card keeps the round's name and omits an ambiguous
  muzzle speed. A constructed registry with a duplicated gun exercises that case.
- The runtime-root and scenario-list docs describe their current callers and fixture consumer.

### View

- Terrain range adaptation expects both marker indices to belong to the ladder that spawned
  them. Missing terrain before spawn or in a headless composition remains a named absence.
- Both muzzle-light dressing paths resolve minimum refire interval from their loaded weapon's
  `FireCycle` and use one shadow rule: a persistent light or an isolated flash casts, a short
  repeated flash does not. An unattributed dev/test event, or a delivered event whose weapon has
  left the scene, is explicitly an isolated flash. The source-teardown case is covered at the
  actual fire observer, so a presentation cause outliving its source does not panic the view.
- Decision: ASSUMED shadow-read window 0.1 s. The prior MEASURED short/rapid-flash study remains
  the evidence; this window is an explicit presentation policy, not a claimed perceptual
  measurement. The shipped slow/rapid fixtures keep their existing shadow choices; generic
  duration/cadence cases cover a slower rapid-fire-class weapon and a persistent light.

### Net

- Authority publishing expects its startup ledger singleton. Applying an enumerated tree expects
  the same query still to contain it. No state transition is silently dropped at either site.
- A replica explicitly waits before its first ledger or between links. Duplicate active ledgers
  are a composition bug: the pinned Lightyear disconnect observer removes non-persistent
  replicated entities when the old receiver disconnects. The normal client leaves/cleans its old
  link before returning to the lobby. The replacement-ledger test still checks the state reset.
- No replicated type or protocol revision changed.

### Buildings tests

- Every shipped spec's joints are checked against the joins it actually declares; every declared
  join is removed in turn to prove its absence is refused, and contrasted with another declared
  kind when the spec supplies one. Joint graph identity is checked across kind changes.
- Idle-join and layout refusal tests use an explicitly constructed house. No shipped house must
  keep a roof-to-plate join, an unused sill pair or an opening on a particular wall.
- Attachment resolution iterates every declared row and compares its resolved bond to that row's
  own source. The unknown-kind refusal uses a separate labelled list, so adding a kind to the
  shipped table does not invalidate its absence assumption.

### Ownership and remaining work

The walk, building fracture/breakage, tree fall and ground-table implementations were left alone.
The known replay and ground-stop test failures remain with `walk-w3`; neither is counted against
sweep-load. Pricing, captures, the full CI suite, merging, pushing and bay release remain with the
orchestrator. All lane test runs use unattended/headless settings through the machine build lock.

## Final verification

- `cargo fmt --all --check`: passed before every commit.
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`:
  passed on the final source state and each earlier source batch.
- MEASURED review-fix results: all spec tests passed (25), the opening-refusal test passed, and
  doc citations passed (3).
- MEASURED driving results: the targeted track/spec/bake/bitprobe/pivot/doc-citation batch passed
  (230). The pivot evidence is recorded above.
- MEASURED remaining-item results: the cam, muzzle, terrain-marker, ledger, attachment, font and
  citation batch passed (171 game tests); the building-tool tests passed (8).
- MEASURED final view results, after adding the source-teardown case: muzzle, terrain-marker,
  font and citation checks passed (22). The earlier cam/net/building source remained unchanged.
- `git diff --check`: passed. No source changes are deferred; only the listed sweep-load P3
  follow-ups and the explicitly excluded owner work remain.

Logs: `/tmp/boundary-review-gates.log`, `/tmp/boundary-driving-gates.log`,
`/tmp/boundary-remaining-gates.log`, `/tmp/boundary-view-gates.log`.
