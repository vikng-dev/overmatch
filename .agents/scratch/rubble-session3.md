# Permanent chunk colliders

Subject: the game's environment destruction and damage models; game state only.

## Zero separation

Decision: keep the 1 mm inset as the chunk's only contact and ballistic hull, standing and freed.
Occupied geometry continues to supply mass, inertia and centre. The inset is a numerical
resolution, with the smallest-hull contraction cap still applying.

MEASURED reproduction: run the existing
`a_chunk_freed_in_place_takes_no_energy_from_its_neighbours` witness with only `LOOSE_GAP_M`
changed to zero. Its first sampled release, chunk 29 at the origin on tick zero, has kinetic
energy 0.23900023 J while gravity did -4.1571803 J of work. The test fails. This is the release
sample introduced by “buildings: a freed chunk weighs its whole hull; only its contact hull
stands inside it”, retained through subsequent module renaming.

Pinned source trace, DERIVED:

- Avian 0.7.0, `collision/narrow_phase/system_param.rs`, computes prediction distance from
  speculative motion, contact tolerance and collision margins. It calls the shape query before
  adding collision margins to penetration. Neither margin supplies or corrects the normal.
- Avian's `collision/collider/parry/contact_query.rs::contact_manifolds` creates fresh Parry
  manifolds, calls `DefaultQueryDispatcher`, then rotates the returned normal into world space.
  Its validity check accepts any normalized normal; it cannot check the source cut plane.
- Cargo.lock pins Parry 0.27.0. Convex-polyhedron pairs reach
  `query/contact_manifolds/contact_manifolds_pfm_pfm.rs::contact_manifold_pfm_pfm`, then
  `query/contact/contact_support_map_support_map.rs::contact_support_map_support_map_with_params`.
  GJK supplies the direction used to select both support features and clip contact points.
- `query/gjk/gjk.rs::closest_points` normalizes the closest Minkowski-simplex vector. At a shared
  face its true length is zero: rounding changes its direction disproportionately. GJK either
  returns a direction on stagnation or classifies a distance within `eps_tol` as intersection
  and invokes EPA. `eps_tol` is ten machine epsilons (DERIVED from the pinned implementation).
  EPA's zero-dimensional simplex case returns the Y axis, and its planar-simplex path builds
  opposing faces; these degeneracies do not encode the construction's known cut normal.
  The reproduction establishes the energy failure, not which GJK/EPA exit each pair takes.

There is no exposed tolerance that substitutes the true face normal. Lowering prediction distance
cannot fix an already touching pair. A negative collision margin changes depth after the erroneous
normal has been chosen. Choosing and validating a supporting face would require additional shape
work or a broader Parry algorithm change; no small, local, equal-work correction was established.
No engine patch or external issue was created.

The known cut plane is insufficient as a one-time hint: Avian rebuilds these manifolds on every
narrow-phase pass, with no persisted Parry normal. A game contact hook or a custom normal-constraint
lookup would execute for pairs each physics tick. Persisting and injecting the hint requires a new
contact-cache mechanism. Both fail the owner's requirement that this alternative remove work.

## Permanent ownership

A chunk collider uses the chunk's own occupied centroid as its permanent local frame. Standing
placement and each departure change the frame's transform relative to its owner, not its shape.
Fresh geometry and occupied mass properties prepare independently on workers; existing chunk
entities prepare neither again. Ordered structural application changes hierarchy, pose and collision
layers, removes the standing sweep hook and preserves material and occupied mass properties.
Freed pieces use ordinary Avian contacts. The standing hook still suppresses speculative braking
only where an actual prepared sweep owns the point.

The separate occupied sensor is gone. The common collider belongs to both the debris and armour
query layers; its contact mask still excludes tank armour volumes. The game's rounds therefore
meet the same inset hull the contact solver uses.

MEASURED validation: formatting passes; 132 targeted, headless and whole-tree guard tests pass,
with seven manual tests ignored. The existing no-energy-gain, occupied mass/inertia, production
round-march, rotated-parent reuse, later split and shipped law-installer witnesses pass. The reuse
witness observes zero collider insertions during both handoffs; fresh departures have exactly one
collider per chunk. The retired exact ballistic hull and sensor-only mass tests are deleted with
that representation.

MEASURED lifecycle observations use the same transport-free census as session 2. All cells are
`colliders created/deleted; bodies created; entities spawned/despawned`. These are event counts,
not duration measurements or obligations on subsequent physics. Contact is at fixture tick 1263.

| Tick | Before | One permanent collider | Immediate disconnection |
|---|---|---|---|
| 1263, contact | 131/2; 0; 131/2 | 131/2; 0; 131/2 | 131/2; 0; 131/2 |
| 1264, +1 | 78/10; 0; 78/10 | 78/10; 0; 78/10 | 78/10; 0; 78/10 |
| 1265, +2 | 1098/51; 7; 1112/53 | 882/51; 3; 888/51 | 882/51; 3; 888/51 |
| 1266, +3 | 691/22; 20; 731/27 | 0/30; 21; 42/33 | 0/22; 21; 42/25 |
| 1301, +38 | 0/0; 0; 0/0 | 0/0; 0; 0/0 | 0/0; 2; 4/1 |
| Through +320 | 1998/85; 56; 2110/105 | 1091/93; 51; 1193/108 | 1091/85; 58; 1207/105 |

Per-tick counts: `evidence/rubble/rubble-session3-counts.json` (orchestrator scratchpad copy); the original and sweepfix runs remain in
`evidence/rubble/rubble-session2-counts.json` (orchestrator scratchpad copy). The permanent collider changes
contact geometry and allocation order, so later departures can move; subtracting totals does
not imply identical physical events.

CI clippy passes. After extracting the reuse witness's collider query to meet the function-length
limit, that witness and all three citation guards pass again (four tests).

## Cut before standing reconciliation

The cut now determines the remainder's connectivity immediately, before reconciliation can build
standing colliders. Swept matter inherits the hull exchange. Other chunks already disconnected
from every ground path start at rest as free bodies; support that still needs a later force solve
continues through the normal law pass. There is no speculative release of future failures.

D3 sees all fragments leaving in this cut event, including those disconnected indirectly. If its
resolution grouping combines driven and stationary matter, the resulting rigid field conserves
their combined linear and angular momentum using cached occupied mass properties. Pure driven
or stationary groups need no mass-matrix reduction. This projection can dissipate energy, never
supply it. Workers prepare fragments independently; ordered application retains fragment order
for their departures, decor and ledger entries.

The creation peak remains 882 colliders at tick 1265. Every retained chunk still needs its one
collider; immediate disconnection removes an unnecessary standing phase only when the cut
already proves support is gone. It cannot anticipate subsequent load failures. The census has
the same 1,091 total creations after this ordering change; later body/deletion counts move as
physical contacts evolve. No timing or identical-output claim is made.

Final source gates: formatting and CI clippy pass; 135 targeted tests and whole-tree guards pass,
with four manual tests ignored; the final transport-free census passes. The new cut-off-matter
witness proves immediate departure, absence of a standing collider and agreement with the shared
ledger. The absorption witness independently accounts for linear/angular momentum and kinetic
energy. The final full library suite was stopped at the 60-minute session cap; it did not complete. No test failure was reported before termination. The required completed full-suite run remains outstanding.
