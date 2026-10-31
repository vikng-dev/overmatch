# Track lane 3 first-part review

Subject: the game's tank driving physics and the game's damage model, game state only.

Reviewed `00abd492`, `26ea0844` and `ba8726b0` against issue #238 and the error-handling,
code-branching and base-truths lenses. No introduced P1 or P2 finding.

The friction gate still compares the resolved coefficient and combine rule with the material
registry. Side plates join the bump stops' material class; neither bypasses the assertion. The
remaining named track material is an existing limitation assigned to the material-pair lane.

For the game's damage model, the record refresh follows changed geometry, not a changed march
law. The diff from the previous green record baseline to the side-plate lane changes vehicle
assets and the geometry-dependent field-cast fixtures, but no march implementation. The new
records retain their verdicts. The restart-at-rest example is a demonstration fixture, not a law
gate: its contract is a delay restarted at a later plate and a burst after the round stops. With
the new wheel geometry the previous line no longer demonstrates that contract. Moving the line
down (DERIVED displacement: 0.1 m from the scenario coordinates) restores the example; retaining
the old line under the old description would mislabel it. Its new record stops in the lower side
(MEASURED chord in the recorded game state: 36.433575 mm).

P3s and inherited limitations:

- The pen-cam renderer has a fixed field capacity (MEASURED from `PICTURE_FIELDS`: four), and
  `Pictures::of` and `look::pack` truncate beyond it. Retargeting the restart-at-rest demonstration
  does not establish that arbitrary gameplay reports fit the renderer. A general renderer fix
  belongs to the pen-cam owner; increasing a constant just until this vehicle's line fits would
  not establish a class invariant.
- `damage_cam::wire`'s recorded-size table predates the refreshed records; its values need a
  deliberate remeasurement when that table is next maintained.
- The pivot speed bound and named track material already recorded in `track-t2-review.md` remain
  with their owners. The marker-model consumer prose is now accurate.

Validation: formatting, CI clippy, the pen-cam tests, field-cast tests and doc-citation tests.
