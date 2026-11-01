# Driving sweep 3: review and disposition

Subject: the game's driving, tracks and environment destruction, game state only. For servo
capability and weapon-clock rows, the subject is the game's damage model: crew are HP pools at
stations. This record extends [part 2](sweep-drive-2-review.md), which extends
[part 1](sweep-drive-1-review.md); their design narrative and measurements are not repeated here.

Base `4d4dce84`, branch `lane/sweep-drive-3`, bay-1. MEASURED start: 2026-11-01 12:32:09 +0700.
The lane used no agents, captures, pricing, deployments, external APIs or pushes. Builds and tests
used the shared queue, `dev_ui,bitprobe`, unattended/headless settings and the bay's warm target.

## Review of sweep-drive-2

The six commits were read against standards, their originating brief and all three lenses.
The review was performed directly: this dispatch prohibited subagents.

| Finding | Disposition |
|---|---|
| P2: `RubbleBodies::gather` interpreted any failed dynamic force query as an excluded tank, concealing a malformed admitted body | Fixed, `5c727802`: exclusions are explicit entity lists; admitted dynamic force state is required. A regression removes computed mass and requires a bug outcome. |
| P2: generic dynamic supports could lock axes, while the snapshot advanced unrestricted mass/inertia | Fixed, `8270a535`: snapshot and Avian impulse response use the same effective inertia. A locked direction at rest cannot acquire a reaction through infinity times zero. |
| P2: `WorldInstanceReady` was later than the available file gate, and vehicle detection by axle names let an export missing those names evade template refusal | Fixed, `147149b9`: the selected view's template is checked at startup. `997cc06a` adds pin-domain and decoded-mesh certification before template publication. Loading waits remain distinct from malformed loaded data. |
| Cmd+Q decision in `Plugin::finish` | Agree. Local Bevy 0.19 source shows `App::finish` runs after plugin builds; `WinitPlugin::build` installs its settings. No native window was launched to exercise Cmd+Q. |
| `Stragglers::Park` disagreement | Agree with part 2. `track_sandbox::link_view::place_links` is a live caller for the count-tuning instrument. The inventory's test-only claim is stale. |
| G1 temporary measurement edits | Confirmed: `GRIP_SHEAR_MODULUS_M` is still `0.075` in both the base and this lane. No G1 tuning or feel choice was made. |
| P3: parent scale versus baked axle position | Fixed, `6a507cf7`: use the baked rigid position and refuse scaled ancestors before publishing gear. |
| P3: scripted braking used inertia extents as a contact footprint | Fixed, `48e833c9`: sample grade over the lane projection of declared contact cylinders, including asymmetric stations and lateral spans. |
| P3: test-builder comment attached to the wrong fixture; stale claim that no shoe exports tangents | Fixed beside the related construction/template changes. |

No additional P1 was found in the part-2 diff. Its retained G1 and G5 findings were still open
at the base; their disposition is below.

## G5 and new findings

- **G5 static half fixed**, `196089be`. Implicit belt reactions join `ContactTransfers` for the
  physics step that integrates the force. The existing wall and trunk readers consume that
  reading. Static support uses solid/layer-compatible geometry; `FieldSurface` excludes the
  heightfield and authored blocks already represented by the field. Cylinder support has its
  approximation bound beside `Surface::triangles`. Kinematic surfaces retain their motion.
  Class tests cover reaction conservation against source momentum, the actual wall-law queue,
  cylinder support, field exclusion, footprint subdivision and kinematic slip.
- **Other tanks remain explicitly excluded.** The game adapter's mutable hull query and mutable
  `Without<Tank>` recipient query enforce its current write ownership. Admitting another hull
  requires a tick-start snapshot of all recipients and staged writes before physics, while
  retaining the established driving/recoil/physics order. Deleting a filter alone is incorrect.
  This optional extension was deferred in favour of the dispatched admission/error groups.
- **New P2 fixed**, `e8c65dad`: `RigGeom::validate` now certifies every station's full position.
  A finite side-plane route did not establish a finite lateral coordinate for posing/projection.
- **New P2 fixed**, `997cc06a`: template axes no longer select fallback directions; decoded
  shoes certify finite attributes/bounds, complete/indexable triangles and draw-index capacity.
  Allocator pages, retained materials and CPU backing buffers are required where their owning
  draw writes or releases them. Final review also requires the certified cache count when a live
  belt selects a rung, and finite selection distance for every admitted shoe. A malformed loaded asset cannot become a permanently empty belt.
- **New P2 fixed**, `6b5277e2`: world-print coverage used byte counters even where an otherwise
  supported raster assigns more near texels to one world texel. Wider CPU counters preserve full
  coverage; GPU formats remain unchanged. Off-map lookup uses wide subtraction before conversion.
  CPU count storage per active world tile is wider; pricing remains the orchestrator's work.

## Error-handling inventory

| IDs | Status |
|---|---|
| R01, R03–R07 | Fixed, `cf88c9e6`: complete motor-domain admission; selected source/slot and source/view lifetime required; missing volume relationships are bugs. |
| R02 | Already fixed: `requires` is mandatory authored data. |
| R08–R10 | Open, untouched: clock exhaustion still selects `freeze_exhausted`; a deliberate session outcome remains to be implemented. |
| R11–R12, R15, R17–R18 | Already fixed by the preceding sweeps; their stronger admission/size checks remain. |
| R13–R14, R16 | Fixed, `23ebb529`: finite positive calibration area; checked phase domain; shared wide material addressing; invalid contact loops cannot return a successful empty report. The validity flags and adapter skip branches are deleted. |
| R19–R25 | Part 2's unconditional faults retained, with earlier structural/file checks and decoded certification in this lane. |
| R26–R28 | Fixed, `997cc06a`: certify before publication; no permanent refusal cache; file/live count bounds and checked draw allocation; retained write targets required. |
| R29–R30, R38–R42 | Already fixed by preceding work, retained. |
| R31–R36 | Open, untouched: tank-surface membership/material/scale and print-asset gates. |
| R37 | Fixed, `6b5277e2`: every stamp requires its registered shoe. Zero press, empty footprints and off-map texels are named absences. |

## Verification

MEASURED green batches, after their relevant fixes:

- Static support/contact/destruction/driving/geometry/headless: **542 passed, 7 ignored**.
- Parent frame: **206 passed, 6 ignored**; contact footprint/battle: **213 passed, 6 ignored**.
- Force/calibration: **400 passed, 7 ignored**; full station admission: **208 passed, 6 ignored**.
- Servo/spec/bake/geometry/headless: **263 passed, 6 ignored**.
- Belt/spec/bake/sandbox/geometry/headless: **464 passed, 7 ignored**.
- Print store and citations: **27 passed, 1 ignored**.

Every implementation commit passed formatting, locked workspace/all-target CI clippy and its
targeted tests. Geometry batches included `pen_cam`, `field_casts` and `headless_test`.
The broad template batch initially exposed a malformed synthetic glTF fixture; after supplying
its required buffer view, template/citation tests passed. A contact test initially assumed full
body mass at an off-centre point; it now uses the class momentum bound, with a separate contact
subdivision test and the wall-reader integration check. No failing check remains accepted.

MEASURED after G5, headless: full throttle F8 **10.684 m/s**, last-second spread **0.0023 m/s**;
neutral pivot **−0.1338 rad/s**, **0.7100** of its DERIVED kinematic ceiling. The final whole-tree
checks and post-change measurements are recorded below after their run.

MEASURED final whole-tree/print/quit guard batch: **73 passed, 1 ignored**. The final
live-belt read, geometry/headless and whole-tree batch then passed **279 tests, 7 ignored**,
with formatting and locked workspace/all-target CI clippy clean. No full-suite run was made.

MEASURED most recent flat-ground run, after the physics/storage changes: F8 **10.684 m/s**,
spread **0.0023 m/s**; neutral pivot **−0.1338 rad/s**, ratio **0.7100**. The final retained-belt
read guards change no admitted-state geometry or force arithmetic.

MEASURED completion clock check: 2026-11-01 13:59:16 +0700. DERIVED elapsed:
87 minutes 7 seconds from the measured start. The orchestrator retains
G1 feel review, other-tank support, the open inventory rows, pricing/captures, merge and push.
