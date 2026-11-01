# Trees lane 10: review and disposition (#236)

Subject: the game's environment destruction, physics and damage model; game state only.
Review base: `2cff0cf5..7cfb90d2`, the four trees lane 9 commits, against AGENTS.md,
all three review lenses, issue #236 and its two owner comments. No agents were spawned.

## Standards review

No P1 finding. Two P2 findings, each fixed before the dynamic-body implementation:

- `buildings::attachments::Attachments::from_ron`, error handling: the new root coefficient
  was checked in authored f64 precision, then cast to f32 without checking representability.
  A future finite coefficient could become infinity or zero and fail only at tree spawn.
  `c1c8c612` refuses it at load and tests both extremes.
- `trees::fell_on_loads` (previously the contact-only reader), sleeping contacts: retained sleeping
  presses started unchanged structural runs every step. `909fbee0` makes them join a run another
  load starts, while starting none. The composed regression observes an unchanged structural-state
  change tick while the retained sleeping contact remains present. This is redundant work, not
  a missing physical wake rule; no runtime cost was measured.

P3: the root-plate prose inferred zero separation work from a source that supplies no work
measurement. The implementation retains a rigid-plastic root release, but the attachment table,
module contract and wall design now explicitly call zero work **ASSUMED**, not measured or derived.
No additional code-branching finding.

## Spec and mechanics review

The root plate is an attachment kind in the shared list. Fresh density is authored per species;
the birch transfer remains explicitly **ASSUMED**. For an upright trunk under its own weight and
an applied bending moment, the seam stress is `M/S - mg/A`; its stated strength is `Cm/S - mg/A`,
so the threshold is exactly `M = Cm` (**DERIVED**). The existing weakest-section test checks the
root and grain alternatives across sections. Unknown root separation work cannot be established
from that moment regression.

The emptied segment opens its base joint in the same TrunkState contacts read. The cut remains
the lowest opened joint, with one departure per tree. Faceted contact normals explain the old
azimuth-bin bound; taking direction from delivered momentum is correct for that collision
approximation. Dynamic launch now retains the full vector momentum and torque rather than using
the direction bin to prescribe a trajectory.

## Checkpoint comment dispositions

- Latched-stop P2: closed. A freed trunk stays dynamic, meets all bodies through the solver, and
  uses the existing debris sleep and support-removal wake. The physical-trunk regression proves
  sleep, wake and renewed descent; a contact regression proves loading and felling another tree.
- Optional spatial-query P3: closed by deleting the scripted support-query path.
- Down-state let-else P3: closed by deleting the path and its impossible-state skip. Remaining
  Standing/Down matches name actual game or receive states.
- Envelope holding P3: that symbol was already absent at the lane's base; it was not reintroduced.
- NaN-to-upright P3: closed by deleting the path sampler and its fallback.
- Unnamed view-root child skips P3: collider placement names the optional view child explicitly;
  sim membership is the stable TreeParts list, independent of view readiness.
- Frictionless butt-slide P3: closed. Gravity, geometry and the material's contact friction now
  determine translation and support through the solver.
- Least-starting-spin P3: closed. The common free-body launch takes actual delivered momentum;
  a press-only departure invents no spin. The capture cue is an explicit operator fixture blow.

## Implementation decisions and evidence

Authority segments retain their entity IDs and HP components when reparented onto the freed
body. Its polygonal density preserves the fresh mass of the circular data (**DERIVED** correction
`pi/3` for the authored twelve-sided ring). Walls and trees finalize departures together, with one
coupled kinetic budget and complementary source impulses before the next physics step.

The topology ledger carries only the state word. Each freed trunk uses the existing interpolated
debris pose stream with its map ordinal and cut; revision 55 deliberately changes compatibility.
Replicas wait for matching topology and the first presented pose, retaining standing colliders
until the interpolation cursor reaches the body's birth. They build static collision copies from
data and follow the pose stream even in a frame with no physics tick. Disconnect removes those copies. Carrier identity
refusals are typed and logged at the boundary.

The world blast reader routes its visible trunk stops to the same structural batch, preserving
terrain and wall occlusion; there is no independent per-tree blast cast. Line and casing momentum
come from delivered march records. No capture, pricing, deployment or push was performed.

## Final gates

**MEASURED:** 219 targeted tests passed, none failed; three existing headless measurement beds
remained ignored as required by the no-pricing rule. The batch included trees, buildings, debris,
network destruction and carriers, protocol wire guards, the full headless regression group,
related ballistic tree cases, collider rehoming, doc citations, UI coverage, the net boundary and
bake fixtures. Whole-tree formatting and workspace/all-target CI clippy passed with
`dev_ui,bitprobe` and warnings denied. Every run was unattended and headless through serial-cargo.
The previous review commits separately passed their own required gates.
