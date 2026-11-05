# Sweep session 2

Subject: the game's environment destruction model and server work; game state only.
Branch: `lane/sweep`. Session 1's observations remain in `report.md`.
The design narrative lives in `construction-destruction.md`, “Bodies in contact”.

## Cut-set checkpoint

The stopping demand uses the source body's own mass and its normal surface velocity over the
physics interval, plus inward drive. One patch includes all of that body's intersecting colliders
on a construction. Only its boundary joints supply capacity; internal joints remain intact. A
released patch crosses `sweep_departure` as one cluster, carrying the plastic rigid-motion exchange.
A held patch stops at its first surface and sends its stopping press through contact intake.

Deleted: the per-chunk acceleration criterion and its slow/strong-joint test; the departure loop
that made one body per chunk and separated internal patch joints. The retained geometry and
linear/angular plastic-exchange tests still check their independent contracts.

MEASURED checkpoint: 65 targeted module/caller/documentation tests passed, three observational
beds ignored. Formatting and CI workspace/all-target clippy passed. The prior broad caller run
was stopped after the hold-path failures; the corrected momentum, sleeping and common-departure
witnesses all pass. This checkpoint makes no tracked-ram acceptance claim yet.

The pre-physics hook excludes sleeping bodies (their retained contact force remains owned by
ContactTransfers), and newly spawned bodies whose computed mass awaits Avian preparation.
No material or asset data changed. No backlog row names the edited files.

Merge seams: `breakage::read_contacts` consumes held-patch presses; `sweep_intake` owns the cluster
signature and routing; the geometry cache gained immutable joint endpoint addresses. Reconcile
the departure signature with decor's size routing. The parts calculation is untouched.
