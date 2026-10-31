# Track lane 4 review

Subject: the game's tank driving physics, game state only.

Reviewed `7a0c0636`, `f668b319`, `9713f921` and `7ca401d3` against all three review lenses and
correctness, including the vendored Avian friction combination and impulse consumers.

- **P2, fixed: data-dependent friction gate.**
  `rubble::tests::the_belt_grips_by_the_material_pair_on_ground_and_pieces` required different
  extrema in the shipped ground table and nonzero friction on the selected vehicle. Both are
  valid data choices, not class invariants. Labelled synthetic track and ground coefficients now
  exercise the pair ratio and the shared piece reaction without constraining shipped data.
- **Coefficient ownership, accepted.** Each column computes its material pair once; the emitted
  `BeltContact::mu` feeds the hull's Coulomb limit and `rubble::contact_impulse`'s reaction clamp.
  Neither consumer combines the pair again. Calibration's absent grip is named support-only.
- **Tuned material row, accepted as a provisional limitation, not a steel measurement.**
  `TrackSteel` explicitly states `Tuned` and names the missing truth as grouser engagement (#146).
  DERIVED: retaining the ground pair of 0.9 with a ground row of 0.5 needs 1.62. This also reaches
  the rigid bump stops and side plates: on brick's 0.65 it gives about 1.026; on a 0.5 solid it
  gives 0.9. A coefficient above unity is supported by the Coulomb solver and introduces no
  mathematical instability by itself. The piece drag still cannot accelerate its body past the
  belt in its own direction; the rigid-contact tests check pieces are not thrown. These facts do
  not certify realistic steel friction. Decision: retain the explicitly provisional tune until
  #146 supplies the construction and ground truth, rather than invent another contact rule here.
- **P3, inherited and still open:** clipped-rectangle support uses a deepest-point resultant;
  partial-width contact averages spring support instead of modelling a rigid shoe across its
  width; reaction velocities depend on sequential tank order; the driving-rise acceptance band
  is broader than the derived share law. See `track-t3b-review.md` for the bounds and evidence.
- **P3, test coverage:** the side-plate physical test checks bounded piece response rather than
  certifying the material coefficient against a measured solid-contact experiment. #146 needs
  such evidence as well as ground driving measurements.

MEASURED again on this lane before changing the fixture: top speed 10.684 m/s, spread 0.0023 m/s;
neutral pivot −0.1338 rad/s (ratio 0.7100 of its kinematic ceiling). The original sliding fixture
reproduced the DERIVED pair ratio 1.414214; the synthetic replacement MEASURED 2.000000 against
the DERIVED ratio 2.000000. No driving law or material
value changed in this review. The extrapolation horizon's documented exclusion of rubble remains
part of the net area's N1 acceleration-bound finding; it is not a general acceleration certificate.
