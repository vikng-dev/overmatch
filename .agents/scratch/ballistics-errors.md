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

## Overlap review dispositions

Subject: the game's damage model, game state only. Every finding below was re-checked against the
source before editing. Field directions keep the existing first-entry stop law; the line's record
reports exposed portions. `perforated` means those exposed portions were traversed before a rest,
even if that rest is inside a hidden portion of the same volume.

- Mechanism weights: confirmed. A tied sample's maximum-factor cost share is divided among its active tied contacts; duplicating a contact cannot take energy from another sample.
- Mechanism hard-volume endpoints: confirmed. Records are built only from strongest, actually reached encounter contacts; a missing contact for a positive line deposit is an invariant panic.
- Mechanism witness re-walk: the duplicate query was confirmed; the claim that a failure silently drops the leg is obsolete after item 1. Complete resolved legs now carry the disc walk's hard presence. Genuinely uncollected open-flight/inspection legs still collect their own presence, without the removed probe-length quota.
- Mechanism field wording: confirmed. Design sections 13.2 and 13.7 now state that a weaker protruding face entered first can stop a field direction; near-equal factors do not blend entry order. No new field law was introduced.
- Doctrine duplicated visibility state: confirmed. The encounter's invariant substance set fixes each sample's strongest factor; resistance, dose weights and dosed chord now read that fact, without `hidden` metres or `shown` flags.
- Doctrine crossing convention: confirmed. Both hard volumes and witnesses report their exposed portions. Witness reporting now follows that convention across connected approach/transit legs, including a later leg wholly inside hard matter.
- Doctrine vocabulary/provenance: confirmed. The transit comment uses dose vocabulary and the `ResolutionPlan::deposits` documentation retains the invariant without its date label.

Item-3 evidence (MEASURED): formatting and CI clippy pass; the combined ballistics, citation and
pen-cam run passes 506 tests, with zero failures (334 ballistics, 3 citation checks, 169 pen-cam).
The targeted fuzzer gate again covers 10,000 rays with zero walk errors. Regressions distinguish
per-sample tie shares, buried/exposed endpoints and exact-exit membership, reuse of resolved hard
presence despite an unreadable later binding, and a witness's hidden rest across connected legs.
The pinned field-cast digest still passes. No frame/tick pricing, window or capture was run.
