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

## Acceptance checkpoint

The tracked recipe is now an ordinary CI test. The criterion's hand fixture has an analytical
plane normal, keeping convex-query normal roundoff out of its exact equality assertion; the
separate occupancy and shipped-plugin witnesses use the actual query normal.

- DERIVED and MEASURED hand threshold: three 16 N anchors, 128 kg source, one-second interval:
  0.375 m/s. Equality holds; just above breaks. Doubling both seam strengths moves it to
  0.75 m/s; an 8 N drive moves the original to 0.3125 m/s. Strong internal seams carry nothing.
- MEASURED hand exchange: three 10 kg chunks, 100 kg source at 2 m/s leave at
  1.5384615384615383 m/s; exchanged momentum is 46.153846153846146 N s.
- DERIVED shipped wall threshold, independently enumerated then MEASURED by below/above trials:
  the first patch has 12 chunks and 28 intact cut joints, 158400.16 N directional capacity,
  a 57000 kg source and a 0.015625 s interval. Without drive: 0.043421097 m/s. With the
  observed 6207.18 N inward drive: 0.041719563 m/s. Both thresholds bracket hold versus break.
- MEASURED ram: first contact and first hole are driving tick 1134, zero added ticks; 49 chunks
  are swept and absent by contact + 2. First exchange speed is 10.885338 -> 10.832401 m/s,
  exchanging 3017.9602 N s. Subsequent exchanges are 10.834826 -> 10.803365 m/s
  (1793.6361 N s), then 9.965262 -> 9.841336 m/s (7159.489 N s). Every source delta-p
  balances actual outgoing occupied momentum within the stated floating-point allowances.
- MEASURED pre-tightening work, `(bounds nodes, candidate casts, cut capacity evaluations,
  convex-cast/contact API calls)`: contact `(6024, 2211, 28, 2223)`, next
  `(6321, 2247, 30, 2254)`, next `(8739, 2461, 79, 2501)`. This excludes spatial broad-phase
  calls, internal GJK iterations, membership-search arithmetic, departure creation and structure.
- MEASURED house before contact: 1122 chunks, 3258 joints, zero cracked and zero absent.
  This is the existing initial structure calculation, before parts merges.

MEASURED gates: eight focused acceptance witnesses pass; 114 module/caller/acceptance and
whole-tree guard tests pass, two observational beds ignored. Formatting and CI clippy pass.

The plastic exchange is not the whole post-contact slowdown. MEASURED remaining placed-owner
wall impulses on the next two ticks are approximately `(-2504,40390,46744)` and
`(50970,24700,205199)` N s. The source's final velocity at contact + 2 is
`(1.6111904,0.07137566,-2.166457)` m/s, below its immediate post-sweep speed. This retained
contact response remains visible for the orchestrator's play/capture assessment; it was neither
removed nor attributed to patch mass. Debris routing still belongs to decor. New source bodies
must have their physics mass prepared before this pre-physics hook; decor should construct that
response at spawn when reconciling its cluster path.
