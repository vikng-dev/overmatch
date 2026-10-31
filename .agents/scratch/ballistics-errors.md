# Ballistics error outcomes

Subject: the game's damage model, game state only. This lane keeps the current envelope paths,
including the no-envelope fallback, for the adopted walk redesign. It changes no overrun accounting
or next-step pose prediction.

The pure core retains structured contradictions for the fuzzer and intermediate corridor extension.
Every gameplay consumer makes those contradictions fatal through `WalkError::panic`, on authority
and replicas alike. No walk failure produces an armour mark, verdict, guessed membership or missing
dose. The fuzzer records each panic's reproducing ray and continues its survey.

- `BadFactor`: removed; the material registry refuses nonfinite or negative `resistance.Effective` at load, naming its actual file, substance, field and repair. Invalid programmer-built materials panic at table construction.
- `SubstanceFactorConflict`: removed; one load-validated registry identity owns one factor, and duplicate substance declarations are refused by the registry parser. An inconsistent adapter-built table panics at construction.
- `RepeatedVolume`: removed; the spec's keyed declarations refuse duplicate authored volumes. Runtime entity rows are assembled once per entity; a duplicate adapter row panics at construction.
- `UncertifiedStart`: removed; the asset door certifies meshes and spawn constructs certified trimeshes or stored-face convex geometry. Unreadable bindings panic. Containment probes span their finite loaded geometry without independent candidate, total-disc-face or length quotas.
- `UnknownVolume`: a gameplay panic naming the spawned-collider-to-volume binding invariant.
- `CollectorFailed`: a gameplay panic naming the resolved-pose and certified-geometry binding invariant.
- `BadCorridor`: a gameplay panic naming finite parameters and a unit axis; nonfinite hits are checked before clipping can discard them.
- `UnexpectedExit`: a gameplay panic naming shell alternation from established membership.
- `UnexpectedEntry`: a gameplay panic naming certified embedded-shell alternation.
- `UnattributedRun`: a gameplay panic naming the common crossing source of the field and presence.
- `IncompleteCorridor`: extend an intermediate corridor; if the closing corridor still lacks an exit, a gameplay panic naming that exit invariant.
- `CorridorMismatch`: a gameplay panic naming the exact `begin`/`finish` corridor contract.
- `DegenerateEntryNormal`: a gameplay panic naming the opposing-entry-normal aggregate invariant.
- `CorridorOverflow`: a gameplay panic at the existing face ceiling; no claim of a global load-time proof is made for arbitrarily overlapping candidates.
- `UnrepresentableChord`: removed; symbolic side orders bit-equal contacts within a shell (outside: enter then leave; inside: leave then enter), with contact identity as the final order. Distinct shells remain one atomic field batch. The limiting tangent chord has zero measure, with no epsilon material or energy.
- `UnsupportedControl`: a gameplay panic naming the driver's implemented-control contract.
- `DrainExhausted`: a gameplay panic naming the existing step crossing/overrun limits; accounting itself is unchanged.

Item-1 evidence (MEASURED): formatting and CI clippy pass; ballistics passes 330 tests, material
parser 19, pen-cam 169, citation gate 3, and the affected sandbox log test 1. The CI-scale fuzzer
covers 10,000 rays with zero reported walk errors; its injected-binding test also proves that it
records multiple panic reproducers and completes the survey. No game window or capture was used.
