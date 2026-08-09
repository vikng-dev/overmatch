# Coincident same-primitive shells: alternative laws and recommendation

## Executive judgment

The clean law is not recoverable from the current `FaceHit` stream alone. At that interface, these
two cases can be made observationally identical:

- two real shells of one primitive close at one `t`, so the correct depth delta is `-2`;
- one real exit is reported by two incident triangles while a second shell remains inside, so the
  correct delta is `-1`.

The `t`, winding sign, and normal can all be identical. `triangle` is diagnostic-only and carries no
adjacency or shell identity. Therefore every always-successful walk-only rule is unsafe: choosing
`-2` can erase the remaining shell's line-of-sight thickness; choosing `-1` retains the reported
fail-closed residual. This is an information deficit, not an algebra problem.

My recommended resolving law is **certified surface identity plus signed surface deltas**:

1. Reuse the exact welded, edge-connected shell topology already constructed by
   `bake::manifold_gate`; stop discarding its shell roots.
2. Carry a stable surface/shell key through collection **and through restart seeds**. The least
   invasive representation may be one collider entity per connected shell, letting today's
   `PrimitiveKey` slot carry the identity without adding an `on_edge` field to every `FaceHit`.
3. Behind the collector seam, turn triangle claims into canonical oriented surface crossings with
   an orientation-aware exact/symbolic edge-and-vertex rule. Raw triangle multiplicity never reaches
   the walk.
4. Within each already bounded/anchored candidate cluster, count signed crossings per certified
   surface and apply the checked net field delta atomically. The anchor window finds candidates; it
   is not permission to equate two nonincident surfaces.
5. Preserve the order and metric chord of distinct surfaces inside the window, or fail closed if
   their order cannot be represented. Only a proven common edge/vertex contact may reduce to a
   zero-measure touch.

This adds no normal tolerance and makes triangle valence unable to control armor. It has a larger
architectural blast radius than a walk-only patch, but it is the first candidate that both resolves
the measured residual and can defend a literal "never erase armor" claim on adversarial valid
meshes. If that scope is too large for this round, the only safe low-blast fallback is a structured
`AmbiguousCoincidence` error; retaining fail-closed behavior is better than inferring multiplicity.

## What the current branch establishes

- During this brainstorm the parallel round advanced from the committed previous-hit single-linkage
  law to a dirty bounded-chain implementation. Its two ceilings are a total span measured from the
  cluster anchor and a same-primitive pairwise-coincidence check; the dirty tests cover the bridge
  counterexample. This analysis consumes that bounded result and never assumes an unbounded chain.
- `walk_ray` currently reduces every cluster to `(has_entry, has_exit)` per `PrimitiveKey`, hence at
  most one toggle. `Field` can represent depth greater than one, but the reducer destroys
  multiplicity before `Field` sees it.
- `collect::cross_triangle` deliberately lets both incident triangles claim an exact shared edge.
  That is a safe completeness bias: duplicates are preferable to a dropped armor crossing. It also
  proves that raw hit count is not surface count.
- `FaceHit::triangle` is explicitly diagnostic-only. Promoting its numeric value into a topology
  tie-break would make re-triangulation change protection.
- `bake::manifold_gate` already does most of the hard domain work generically from data: exact-
  position welding, directed-edge closure and winding checks, union-find by shared welded edge, and
  positive signed volume per connected shell. The shell root is then discarded. Persisting it is a
  deepening opportunity, not a new asset-specific rule.
- The reported residual rate is **MEASURED (reported)** at roughly 1-2 failures per 10^7 rays. The
  example stream `8.1927 in, 8.2123 in, 8.2235 out, 8.2235 out` is likewise **MEASURED
  (reported)**. The failure is rare, but its safety posture matters more than its frequency.
- The live parallel implementation opens a cluster at its first face and closes it at its last,
  conservatively stretching ordinary material presence. It still reduces one primitive's pairwise-
  coincident entry plus exit to `Touch`; that remaining quotient is where a true sub-window shell
  can still be erased unless contact provenance distinguishes it from a graze.

The parallel clustering fix changes only the outer batching law and adds its own bounded-span
parameter. The surface proposal here adds no further numeric knob. Surface reduction must consume a
fixed bounded/anchored cluster and must never extend it through pairing, normal similarity,
topology adjacency, or global conservation.

## Safety standard used here

"No free penetration" means more than eventually reaching depth zero:

- A successful interpretation may not shorten any real ballistic volume's line-of-sight thickness.
- It may not lower the union field's `max(material_factor)` integral over any interval.
- It may not delete a protective boundary law by pretending a positive-width shell was a touch.
- Missing or contradictory topology is an error. Final-balance conservation is not evidence that a
  missing surface existed.
- Re-triangulation, triangle order, face valence, and authoring order cannot change the answer.

Failing closed satisfies that standard. Fabricating armor can prevent free penetration, but it
violates no-fabricated-events, damage attribution, spall, and physical gaps, so it is not an
acceptable general repair.

## The impossibility witness for walk-only laws

Suppose the walk is already at depth two. In the next anchored cluster it receives two same-sign
exit hits with the same `t` and normal.

- Geometry A has two disconnected shells whose real exit surfaces coincide. Correct result:
  `depth 2 -> 0` **DERIVED from the two shell crossings**.
- Geometry B has one shell exiting on a face diagonal claimed by two triangles; the other shell
  exits later. Correct result: `depth 2 -> 1` **DERIVED from the one surface crossing**.

Both produce the same current interface values. If a reducer applies `-2`, Geometry B closes early
and the later line-of-sight thickness disappears. A later underflow does not save the law: put the
remaining exit beyond the corridor, or drop it in the collector, and the inferred close turns what
should be `IncompleteCorridor` into success. If the reducer applies `-1`, Geometry A retains the
known residual.

The mixed-sign version is equally sharp: an entry and exit inside one numeric window can be a true
corner touch or a positive-width shell thinner than that window. Without topological provenance,
successful cancellation defines an implicit minimum-feature authoring law. No such law is currently
gated, and an adversary can put arbitrarily many real surfaces inside the anchored window.

## Candidate audit

### 1. Raw signed multiset: `delta = entries - exits`

This is the proposed net law applied directly to `FaceHit`s.

- **Can erase cost:** yes. In Geometry B, two triangle claims become two exits and close the outer
  presence early. Unequal tessellation at entry and exit also makes a duplicated shell fail to
  unwind symmetrically.
- **Adversarial mesh:** face valence becomes an integer armor control up to the collector's hit cap.
  Adding triangles can raise or lower inferred depth without changing the surface.
- **Corner clustering:** a high-valence vertex becomes many entries/exits, not one contact.
- **Verdict:** correct only after a deeper module certifies one item per real surface crossing.

### 2. State-aware saturation: close all current depth

For an exit-only cluster, use `delta = -current_depth`; infer the symmetric entry multiplicity from
later exits.

- **Can erase cost:** yes, by construction. One duplicated inner exit closes every enclosing shell.
- **Adversarial mesh:** a single engineered exit cluster can erase an arbitrary remaining chord.
- **Corner clustering:** mixed signs can still cancel, but homogeneous corner claims remain unsafe.
- **Verdict:** reject.

### 3. Greedy pairing, FIFO/LIFO stacks, or parity

Pair entry and exit reports by order, normal, or minimum cardinality; alternatively flip parity on an
odd number of reports.

- **Can erase cost:** yes. Pairing cannot recover unknown surface multiplicity. Two diagonal claims
  and two real coincident surfaces are both even; FIFO versus LIFO changes shell ownership but not
  the missing fact.
- **Adversarial mesh:** engineered duplicates steer the pairing and can create an air interval where
  one legal interpretation remains occupied.
- **Corner clustering:** parity handles some touches accidentally and fails as soon as valence
  changes.
- **Verdict:** reject.

### 4. Global depth-delta conservation / feasible-path reconstruction

Enumerate latent surface counts under each raw count, require nonnegative prefixes and final depth
zero, and accept only a unique binary-presence history.

- **Attraction:** it resolves the reported `E, E, X, X-at-one-t` stream without a new field or knob;
  final balance forces the last raw pair to mean two exits.
- **Can erase cost:** yes if incomplete interpretations are discarded. Geometry B with the later
  exit missing has only one balanced interpretation: treat the duplicate as two real exits. The
  solver silently repairs a dropped surface and succeeds early. If incomplete interpretations are
  retained as ambiguity, the measured residual is ambiguous too and the solver becomes the
  fail-closed law below.
- **Adversarial mesh:** many reports create a large interpretation lattice. Minimum-union solutions
  undercharge; maximum-union solutions fabricate armor and events.
- **Corner clustering:** uncertified mixed entry/exit clusters still confuse tangency with a thin
  shell.
- **Verdict:** useful as defensive validation after surface certification, not as the source of
  truth.

### 5. Conservative maximum-occupancy envelope

When interpretations disagree, charge every interval occupied by any interpretation.

- **Can erase scalar cost:** no, relative to the captured reports.
- **But:** it fabricates material in real air, can emit damage/spall/ricochet belonging to no surface,
  and makes adversarial face count increase protection. It also masks missing topology instead of
  reporting it.
- **Corner clustering:** tangent fans can grow phantom micro-armor.
- **Verdict:** reject under the full section 13 contract.

### 6. Exact rational `t` ordering before clustering

Recompute plane intersections over the original f32 dyadics and compare rationals before rounding.

- **Can erase cost alone:** yes. It distinguishes two near surfaces whose f32 `t` collided, but it
  cannot distinguish two truly coincident shells from two triangle claims on one edge.
- **Adversarial mesh:** arbitrarily many exact surfaces can share one rational `t`, or distinct exact
  rationals can fit inside the anchor window.
- **Corner clustering:** processing plane order without coherent edge/vertex ownership can turn one
  zero-measure corner into transient entry/exit intervals.
- **Verdict:** valuable supporting evidence and a deterministic ordering tool, never the surface
  law by itself. If two distinct exact orders collapse into one public f32 `t`, represent the order
  explicitly or fail closed.

### 7. Exact plane or exact-normal keys

Group faces by bit-equal plane equations or normals instead of a tolerance.

- **Can erase cost:** yes. Distinct parallel/coincident shells can have identical planes and normals;
  one curved or merely re-triangulated surface can produce different normalized bits.
- **Adversarial mesh:** alternating exact and one-bit-perturbed normals controls group count.
- **Corner clustering:** sharp edges intentionally have several normals at one contact.
- **Verdict:** reject.

### 8. Normal-grouping tolerance (known route)

- **Can erase cost:** yes. Two real parallel surfaces fall in one group; a curved fan can split one
  surface into many groups. Either error can close a remaining shell early once depth is greater than
  one.
- **Adversarial mesh:** any angular threshold admits surfaces engineered just inside it, and
  single-linkage in normal space merely recreates the chaining defect. Anchoring the normal group
  bounds the damage but does not supply identity.
- **Corner clustering:** this is precisely where normals vary most while the contact remains one.
- **Knob/blast:** adds a new angular law to justify; structural test blast is modest.
- **Verdict:** reject as a safety law.

### 9. `on_edge` on `FaceHit` plus a collector tie-break (known route)

- **Can erase cost:** a coherent whole-fan implementation need not, but the boolean itself is
  insufficient. At a non-coplanar tangent edge, one incident face is entry and one exit; choosing an
  arbitrary winner fabricates an unbalanced crossing. A high-valence vertex must be classified as a
  single coherent one-sided perturbation, not independent triangle winners.
- **Adversarial mesh:** valid manifold fans are manageable; a T-junction is missing adjacency, not a
  tie, and must remain a bake error rather than be repaired.
- **Corner clustering:** works only with orientation-aware edge/vertex semantics.
- **Knob/blast:** no new numeric knob, but leaks collector implementation into the walk interface and
  touches every struct literal. **MEASURED by current inspection**, the walk test file has 18 direct
  `FaceHit` literals in addition to its helpers.
- **Verdict:** viable machinery, shallow interface. Keep the evidence private instead.

### 10. Persist connected-shell identity or split colliders by shell

The bake gate already computes the exact edge-connected roots needed here. Preserve them as
`SurfaceKey`s, or spawn one query-only collider entity per root so `PrimitiveKey` already names one
shell.

- **Can erase cost:** not for the residual. Triangle duplicates remain within one shell; distinct
  shells remain distinct even at identical `t`. It is not sufficient alone for a positive-width
  entry/exit pair of the same shell inside the numeric window; exact contact/order evidence must
  supplement it.
- **Adversarial mesh:** arbitrarily many disconnected shells remain separate. Edge-connected roots
  are essential: vertex-connected components would wrongly merge two legal shells touching at one
  vertex.
- **Corner clustering:** one shell's incident fan can reduce to a touch; two shells at a junction do
  not lose multiplicity.
- **Knob/blast:** no new knob. Exposing a new key field is high blast; shell-per-collider reuses the
  existing entity slot but changes spawn counts, query candidates, goldens, and possibly cost.
- **Restart interaction:** this identity must survive `initial_presence`. Hiding shell identity only
  inside the collector is incomplete: a corridor restarting inside two shells currently seeds the
  primitive at depth one, so two later exits still underflow or invite unsafe inference.
- **Verdict:** necessary domain identity and a strong architecture, but pair it with canonical
  contact ownership.

### 11. Exact/symbolic canonical collector ownership

Use adaptive-exact projected predicates and one orientation-aware simulation-of-simplicity rule for
the whole welded edge/vertex fan. Emit one canonical oriented crossing per certified shell contact,
with all incident normals retained for the boundary aggregate.

- **Can erase cost:** no, if the proof obligation is met: only duplicate representation of one
  certified contact is removed; distinct shells and distinct nonincident contacts remain ordered.
  Ambiguous or nonrepresentable order is an error.
- **Adversarial mesh:** triangle valence does not affect multiplicity. Valid many-shell geometry
  produces many surface crossings. Invalid T-junction/non-manifold topology is already rejected by
  the bake gate and must never be normalized here.
- **Corner clustering:** a global one-sided symbolic ray makes an exact edge/vertex touch coherent.
  A naive triangle-id or lexicographic winner does not.
- **Knob/blast:** no new physical knob; substantial implementation proof and semantic test work, but
  raw feature data can remain private.
- **Verdict:** the strongest collector half of the winning law.

### 12. Exact containment or winding-number reconstruction

Probe primitive occupancy between candidate boundaries with an exact winding oracle.

- **Can erase cost:** an exact oracle need not; a floating solid-angle sum needs a tolerance and can
  misclassify. Any finite before/after probe can hop over an engineered thin shell.
- **Adversarial mesh:** exact winding handles duplicates poorly unless shell identity is still known;
  malformed/self-intersecting inputs require explicit rejection.
- **Corner clustering:** can certify the two sides of a junction, but at much higher computation and
  interface cost than using the topology already built at bake.
- **Verdict:** theoretically sound if exact, operationally inferior.

### 13. Ambiguity rejection / three-valued occupancy

If the current interface admits both an armor-preserving and armor-erasing interpretation, return a
structured error before changing the field.

- **Can erase cost:** no successful result is produced.
- **Adversarial mesh:** bounded anchored clusters remain bounded work; arbitrary face density yields
  one error rather than an inferred topology.
- **Corner clustering:** proven mixed-sign touches can remain touches; uncertified multiplicity fails
  closed.
- **Knob/blast:** no new knob and low blast.
- **Verdict:** the only fully safe walk-only law, but it does not lower the reported failure rate.

## Ranking

The criteria are lexicographic: safety first, then no new knobs, then low existing-test blast.
"Resolves" means it can turn the measured residual into a successful, fully charged walk.

| Rank | Candidate | Safety | New knob | Test/architecture blast | Resolves |
|---|---|---|---|---|---|
| 1 | Certified shell/contact identity + canonical crossings + checked signed delta | Highest on valid gated meshes; ambiguity errors otherwise | No | Medium-high | Yes |
| 2 | Ambiguity rejection | Highest | No | Low | No |
| 3 | Shell identity alone, plus defensive validation | High for the reported class; incomplete for same-shell sub-window contacts | No | Medium-high | Yes for this class |
| 4 | Exact/symbolic collector ownership without restart-visible shell identity | High on the first corridor; incomplete at seeded restarts | No | Medium | Yes, incompletely |
| 5 | Exact containment oracle | Potentially high if truly exact | No if exact | High | Yes |
| 6 | `on_edge` field plus coherent whole-fan tie-break | Potentially high | No | High structural | Yes |
| 7 | Exact rational `t` only | Neutral; identity ambiguity remains | No | Medium | Only rounding subset |
| 8 | Normal tolerance | Low under adversarial parallel/fan geometry | Yes | Medium | Sometimes |
| 9 | Global conservation / feasible-path inference | Low when a surface is missing | No | Low-medium | Sometimes |
| 10 | Raw net, parity, greedy pairing, or close-all saturation | Lowest | No | Low | Sometimes, unsafely |

If scope is strictly constrained to `walk.rs` and today's interface, rank 2 is the answer. Among laws
that genuinely resolve the residual, rank 1 is the only recommendation I would ship as a general
law.

## Recommended module and interface

The deep module should be the adapter from authored, gated triangle data to **canonical oriented
surface crossings**. Its interface should expose what the walk needs and nothing about how a
triangle edge tested:

- stable volume and surface/shell identity;
- ordered distance/contact identity;
- winding direction and aggregate boundary normal;
- diagnostic source faces;
- explicit structured failure for uncertified topology or unrepresentable ordering.

`on_edge`, barycentric zeros, adaptive predicates, welded half-edges, and symbolic perturbation are
implementation. The walk's interface begins at certified crossings. It groups them only inside the
already bounded/anchored candidate range, applies signed deltas atomically at a proven common
contact, unions shell presence back to primitive/entity presence, and integrates
`max(material_factor)` over every distinct ordered chord.

Surface identity must also be part of the restart contract. Two implementation shapes are honest:

- make `SurfaceKey` explicit in crossings and `initial_presence`; or
- split each bake shell into its own query-only collider entity synchronously at spawn, so the
  existing `PrimitiveKey` is already a surface key and the sim/view rule remains intact.

The second avoids fixture-wide `FaceHit` field churn but has spawn/query/golden consequences. Either
is safer than reconstructing seeded depth from future exits.

The exact/symbolic rule has four proof obligations:

- a transverse shared edge of one shell emits exactly one crossing, never zero and never two;
- a tangent edge/vertex applies one coherent one-sided perturbation, so entry and exit cancel as a
  touch rather than leaving one winner;
- two edge-connected shell roots are never merged merely because their `t` or normals coincide;
- nonincident positive-width surfaces inside the anchor window remain ordered and charged, or the
  collector fails closed.

This composes with the parallel cluster rewrite: the anchored window bounds candidate work and
prevents bridges; certified contact identity decides equivalence. No per-surface grouping may widen
the outer cluster.

## Sharp regression suite

The winning law should not ship without all of these:

1. **Reported residual verbatim.** One original primitive contains two legal connected shells with
   `8.1927 in, 8.2123 in, 8.2235 out, 8.2235 out` (**MEASURED, reported**). Assert both exits are
   retained, the surface depths close, and the primitive's union span ends at the shared exit. The
   expected union chord is `8.2235 - 8.1927` **DERIVED from the reported crossings**, multiplied by
   the material factor for exact penetration cost.
2. **The indistinguishable duplicate counterexample.** Begin inside two shells. One physical exit is
   claimed by a face diagonal or high-valence fan; the other exits later. Assert canonicalization
   emits one crossing for the first surface, presence remains open, and the later chord is fully
   charged. Remove the later exit and require a structured incomplete-topology error.
3. **Two legal shells touching at one vertex.** Build two outward closed shells in one glTF primitive
   that share only one welded vertex and arrange coincident exits. The bake roots must remain
   distinct, collection must emit two exits, and the walk must close both. This catches accidental
   use of vertex-connected components.
4. **Shared-edge canonical ownership.** Every transverse ray on a triangulation diagonal emits
   exactly one canonical surface crossing across triangle permutations and re-triangulations.
5. **Non-coplanar tangent edge and high-valence vertex.** Mixed entry/exit claims at one certified
   contact produce no interval, boundary event, cost, or damage; incident-face order and valence do
   not matter.
6. **Thin shell inside one anchor window.** Entry and exit are nonincident certified features with a
   positive representable separation smaller than the topology candidate window. The chord must be
   charged and its protective boundary retained, or resolution must fail closed; successful Touch
   with zero cost is forbidden.
7. **Unrepresentable exact order.** Two distinct rational intersections round to the same public f32
   `t`. Assert the implementation either carries their order and charges the chord or returns a named
   ambiguity error, never successful zero cost.
8. **Dense anchored adversary.** Put many real shell surfaces, duplicate triangle claims, tangents,
   and unrelated-volume faces inside one anchored window. Every certified surface contributes its
   signed multiplicity; triangle density and permutation cannot lower cost. Crossing the hit cap is
   a loud overflow, not truncation.
9. **Anchored anti-bridge.** Keep the parallel round's high-factor thin-plate tests: unrelated or
   tangent faces may each be close to a neighbor but cannot bridge an entry and exit outside one
   anchor window. The full high-factor chord survives.
10. **Seeded restart at depth greater than one.** Restart a corridor while it is inside two shells of
    one original primitive, then close both at one contact. The seed must carry both surface keys;
    no inferred close-all rule is allowed.
11. **Malformed topology remains loud.** T-junction, missing face, inverted winding, same-wound
    duplicate face, exit below zero, and open-at-corridor-end all remain structured errors. In
    particular, canonical ownership never repairs a T-junction.
12. **Union laws survive.** Retessellation and whole-volume duplication leave cost/presence
    bit-identical; adding a lower-factor volume cannot reduce a higher-factor chord; exact abutment
    between different factors creates no phantom air or buried-surface event.
13. **Small-stream property oracle.** Exhaustively enumerate short certified signed-crossing streams
    and compare the walk against direct per-surface interval union. Metamorphic duplication or
    reordering may preserve output or produce a fail-closed ambiguity, but may never produce a
    successful lower cost.

## Bottom line

Do not apply a raw net delta to `FaceHit`; that converts triangle multiplicity into an armor law. Do
not use final conservation to invent missing exits. Count toggles per **certified surface**, not per
primitive and not per triangle, and make that identity survive collection, clustering, and restart
seeding. The existing bake gate already computes the right generic starting identity. Pair it with
exact, orientation-aware contact ownership; let the bounded/anchored cluster limit the search; and
fail closed wherever exact provenance cannot decide.
