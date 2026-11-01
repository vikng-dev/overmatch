# Driving sweep 1: review and disposition

Subject: the game's driving, tracks and running gear. References to the game's damage model
below mean game state only: rounds, component facets and crew HP pools.

Base `6dadb0e7`; branch `lane/sweep-drive-1`, bay-2. MEASURED start: 2026-11-01 09:56:58 +0700.
The lane used no agents, captures, pricing runs, deployments, API calls, pushes or external writes.
The forbidden damage-history documents were not opened and no damage-topic web search was made.

## Sweep-net-4 review

| ID | Severity | Disposition |
|---|---|---|
| N4-terminate-install | P2 | Fixed in `b0e094c3`: a missing winit delegate, an unexpected existing native handler or a failed method install now refuses startup. Verified the delegate contract against local winit 0.30.13 source. `0d553bac` corrects this lane's initial over-broad assertion: windowless roots mount the shared quit helper too; the native hook is required only when `WinitPlugin` is mounted. The composed headless tests caught that mistake. |
| N4-quit-wait | P2 | Fixed in `8f8649f0`: an update mode that can strand cancelled termination is refused rather than warned about. The composition test exercises the guard, not just its predicate. |
| N4-beacon-cause | P2 | Fixed in `e9375397`: DNS failure and empty resolution results carry typed `BeaconError` variants; the DNS cause remains `io::Error` until visible worker handling. |
| N4-load-shape | P2 | Fixed in `cb0d0623`: a missing replicated load used to invent round index zero, and only arming logged the shape mismatch. Arming and arrival folding now visibly refuse a typed slot-count fault. A rejected arrival leaves the last complete ledger and its loaded indices intact. Tests exercise malformed initial arming and a later consumption arriving without its load. |
| Ledger shrink | No finding | Agree with the outside-input classification. The server ledger only appends within one generation; a shorter received record is malformed peer state, not a local invariant breach. The logged replacement is deliberate recovery. Replicon's per-entity stale-mutation rejection rules out ordinary out-of-order mutation delivery as the explanation. |
| Debris carrier | No finding | Agree: the carrier itself has `WallPiece` and `DebrisPose`; naming and skipping that carrier is necessary. A non-carrier lacking its required pose still panics. |
| Required loads | No finding | Agree with requiring the component. Verified local bevy_replicon 0.44.2 source: `server::collect_changes` moves mutations into updates on component insertion/removal or initial replication to keep an entity atomic; `client::apply_changes` applies the entity component array before flushing it. The two components share their spawn and disclosure scope. This does not justify a default for malformed vector contents. |
| UI cardinality | No finding | The changed reads distinguish named pre-spawn/windowless absence from duplicate or missing constructed nodes. The kill-camera panel composition fix is necessary beside its required-node reads. |

No additional P1 was found in sweep-net-4. Its two quit disagreements are rejected for the
reasons above. Its existing destruction-area handoff remains with that area.

## Driving inventory

| Finding | Disposition |
|---|---|
| G1 | **Open, P1.** `GRIP_SHEAR_MODULUS_M` remains a universal solver-tuned compliance. The friction/material chain did not close this finding. |
| G2 | Already fixed in `7a0c0636`: track/ground and track/piece contacts resolve the material pair; the universal `MU` is gone. The terrain field currently represents the base ground substance; scatter colour masks are cosmetic. |
| G3 | Fixed in `74fd9ce3`: every declared station supplies its tread radius and lateral contact intervals, preserved in suspension poses and rolling presentation. |
| G4 | Already fixed by `9879b0e9`, `076e5f83` and `0ace6e15`: proxy surfaces, bump stops and side plates carry declared material response instead of the frictionless object rule. |
| G5 | **Open, P1.** `RubbleQuery::is_rubble` still admits contacts by building/piece identity. The track chain wired the law into the game, but did not make admission generic. |
| G6 | Already fixed in `b691454f`: supported normal load supplies Coulomb drag independently of normal closing velocity; `7a0c0636` supplies the same material pair on both sides. |
| G7 | Fixed in `74fd9ce3`: running-gear addresses come from construction data. Only the common shoe-template marker contract remains in `marker_model`. |
| G8 | Fixed in `74fd9ce3`: both sides retain independent routes, pin planes, station radii, travel knots, calibration and force-adapter geometry. |
| G9 | Fixed in `74fd9ce3`: each station probes the edges and middle of each declared tread span, never the gap between discs. |
| G10 | Fixed in `78255c1d`: reserve resolution is a dimensionless policy applied to the lesser of drivetrain capacity and material-pair grip capacity; the seed-sized force floor is removed. |
| G11 | Fixed in `74fd9ce3`: each side's reference tooth datum is required construction data; the runtime mesh harmonic estimator and its alternative path are deleted. |
| G12 | Fixed in `74fd9ce3`: a synthetic missing-node map checks the diagnostic contract without pinning a harmless absence in the shipped asset. |
| G13 | Fixed in `c237c7fe`: a synthetic spring/chain regime checks the chain-clamp relation. |
| G14 | Fixed in `c237c7fe`: the tested integer window is derived from droop perimeter and pitch instead of a fixed-width sweep. |
| G15 | Fixed in `c237c7fe`: the load gate compares actual pin-chord and circle-wrap demand with authored usable articulation, without the unsupported margin multiplier or directional hinge-order heuristic. |
| G16 | Fixed in `c237c7fe`: synthetic road-wheel-limited and fixed-gear-limited constructions test both belly regimes against an independent analytic circle bound and its DERIVED polygon sagitta. |
| G17 | Fixed in `151e8183`: scripted targets select engine/ammunition facets from construction data and use the current part volume pose. No seed-local aim point or ammunition fallback remains. |
| G18 | Fixed in `63e94991`: scripted stopping uses actual mass, reflected belt inertia, brakes, material grip, grade and speed, including the controller slew allowance. Unarrestable grades are named explicitly. |
| G19 | Fixed in `7efa0009`: battle evidence resolves `ShellStrike.struck` through `VolumeOf` and `BattleSlot`; proximity is not membership. Terrain evidence stays on the identity-less terrain event. |

A final G8 consumer check found the template binder still subtracting half-track separation from
its right shoe centre, which only works for mirrored planes. The final adapter change supplies
the native shoe-centre offset directly from `RigGeom`; the existing asymmetric construction test
now also checks that moving track planes cannot change the shared template's local datum. Wheel binding also uses declaration order to break coincident
axle-Z ties, matching the stable construction-data order instead of relying on ECS descendant order.
The narrow `link_view` change does not claim its remaining error-handling or alternative-path
inventory is closed. R19–R25 remain unaddressed with driving part 2/view, alongside the named P3s
below; this is an explicit exception to completing the touched-file sweep within this lane.

The required sheet fields have MEASURED sim-artifact provenance beside their values in the shipped
sheet. No physical law branches on a tank name. Fixtures for asymmetry, unequal radii, tread gaps,
material/capacity scaling, articulation and suspension regimes exercise class invariants.

## Error handling, alternatives and additional findings

- R11, R29, R30, R41 and R42 are fixed by the construction change. Invalid common-shoe dimensions
  are refused before publication; loaded view correspondence is required; the obsolete estimator
  is removed; populated dynamic-tank worlds require admitted gear and a terrain field.
- R12 was already fixed in `bf3ccc95`, and R15 in `c7fd67dc`; the stronger articulation/domain
  admission remains unconditional in this lane.
- R17 and the code-branching P1/P2 transmission overrides were already removed in `dc9233e7`.
  R18 is fixed in `78255c1d`: extreme finite inputs cannot publish overflowed/underflowed coupled
  reductions, curvatures, inertias, speeds or force capacities.
- R38 and R39 are fixed in `151e8183` with typed, visible startup refusals. R40 was already
  handled by `cf4fa4e4` through `env_parse` and `refuse_start`.
- The `PendingTankAssets` handoff is fixed in `151e8183`: the loading system requires the resource
  inserted at startup. Controlled-tank and scenario-camera reads name pre-spawn absence and reject
  duplicate possession/cardinality.
- The dead `TrackGripElements::is_sized_for` helper is deleted. Scenario operator levers name their
  tracked script users. The code-branching claim that rubble is sandbox-only was already superseded
  by `b691454f`; the game and sandbox now share it.
- **New P1, fixed in `ffc57474`:** the game's roadwheel spin post-multiplied the authored rotation,
  reversing the hull-axis rotation under a flipped wheel node. Game and sandbox now use the same
  checked parent-space frame. A world-space rim-point regression covers flipped rest rotation,
  a rotated parent and uniform scale.
- **New P2, fixed in `ffc57474`:** running-gear adapters assumed parent-local axes were hull axes.
  Axle positions now compose the complete authored hierarchy. Non-finite, sheared or nonuniformly
  scaled parent frames are refused before play because their hull-axis rotations cannot be
  represented by one local Bevy `Transform`.
- **P3, fixed with this record:** construction comments now describe declared radii and captured
  frames, and battle ownership prose names environment/non-battle volumes accurately.
- **P3, handoff to the concurrent ballistics owner:** in the game's damage model, `ShellStrike`'s
  module comment still calls `hull_marks` its one reader. Scripted battle evidence is now another
  observer; this lane did not edit the other owner's source.
- **P3, part 2/view owner:** `link_view`'s `Stragglers::Park` and `ShoeBelt::pin` findings remain in
  portions of the module outside this narrow placement fix. The remaining untouched-file error-handling groups are explicitly left for
  driving part 2, as dispatched.

## Remaining implementation work

Part 2 took these rows; their current status is in [`sweep-drive-2-review.md`](sweep-drive-2-review.md).

G1 needs a sourced pad/ground shear-compliance input and a coupled integration that remains stable
at that compliance. Merely moving the tuned value into data would preserve the missing truth.
The current force law uses the universal length for strain saturation and damping as well as its
update; all those uses must follow the same contact truth and be tested across construction scales.

G5 needs a generic physical support representation and self-body exclusion, with a separate
representation rule preventing field-backed terrain from being counted again as a collider.
A marker deletion alone is incorrect: `Surface::triangles` currently supports boxes and convex
polyhedra, while `scatter` constructs trunk cylinders. The game adapter also excludes `Tank` from
its mutable piece-force query; admitting other tanks needs a shared tick-start snapshot and staged
reaction writes, preserving its established server ordering. These are the next lane's scope,
not closed findings.

The orchestrator owns batched pricing, captures, feel review, merging and pushing. No change here
claims measured driving feel or measured frame/tick cost.

## Verification

MEASURED: the final frame/geometry batch passed 439 tests, with 6 ignored and no failures, including
`track::`, `spec::`, `bake::`, `pen_cam`, `field_casts`, `headless_test`, `doc_citations`, `ui_ascii`
and `net_boundary`. Formatting and locked workspace/all-target CI clippy were clean. Earlier
item-specific green batches are recorded in each implementation commit.

MEASURED: the combined source batch passed 735 tests, with 7 ignored and no failures, including
all net tests, quit, scenario and battle alongside the geometry and whole-tree guards. Formatting
and locked workspace/all-target CI clippy passed.

MEASURED: the final placement-adapter geometry batch passed 439 tests, with 6 ignored and no
failures, at 2026-11-01 11:24:47 +0700. It includes the required geometry/headless checks and all
whole-tree guards. Formatting and locked workspace/all-target CI clippy passed. The subsequent prose-only radius clarification
was checked against the actual rolling expression (pin-line radius minus pin-to-inner offset),
and formatting was checked again. Locked workspace/all-target CI clippy also passed after that
clarification. Its redundant final test repeat was cancelled before it started to keep the
measured lane cutoff; no behavior changed after the green placement-adapter batch.

MEASURED completion check: 2026-11-01 11:26:03 +0700, 89 minutes 5 seconds after the start. The
lane's processes had ended and its committed worktree was clean. No push was made.
