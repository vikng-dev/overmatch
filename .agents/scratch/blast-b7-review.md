# Blast lane 5 review and lane 7 disposition

Subject: the game's environment destruction and damage model, game state only. Structure loads
and physical transport are in scope; crew and tank HP receiver laws are unchanged.

Baseline: the four commits `d83df63b` through `fc561faf`, against `d83df63b^`. Every changed hunk
was read against the error-handling, code-branching and base-truths lenses and against #237 and
the supplied blast study. Standards and spec were reviewed directly: the owner forbids agents.
The wall design owns the model narrative; this file records findings and dispositions.

## Standards

- **P2, fixed:** `Masonry::of` supplies `Spall::of` a static tensile strength already rounded to
  f32, although `BlastFile::facts` validates the f64 product. A new row can pass a load gate for a
  different dynamic strength from the one the simulation constructs, including underflow of its
  static operand. The simulation now constructs the validated product from the original datum; the load gate
  refuses a static tensile strength that cannot be stored as positive finite f32.
- **P3:** the fracture module introduction says hulls never serve a law, but Law 2 reads its
  hull. Correct the module interface documentation with the energy-path deletion.
- Required chemical energy removes the omitted-data sentinel. Required tensile DIFs are
  positive and finite, at least unity, with their derived dynamic strength and bar speed checked
  at load. No per-object law or alternative switch was introduced.

## Spec

- **P1, fixed:** `scab` reflects compression as tension even at an intact contact. The law now
  bounds its footprint by the exposed rear area: live intact contacts subtract their projected
  area on the rear plane. Full bonded and resting contacts emit no scab and retain their blow;
  opened joints and departed neighbours expose the surface. Both game and sandbox supply the
  current construction contacts. Propagation through contacts to a farther free surface remains
  unresolved; the local interface does not invent a reflection there.
- **P1, pending:** the scab gate uses the original full chunk depth even when Law 1 has crushed
  the front. The study's executable calculation uses the remaining ligament and refuses scab
  after through-crush. Supply that missing ligament term, naming the depth approximation.
- **P2, fixed:** the farthest corner's projection need not be the convex hull's exit behind a
  loaded point. A tapered chunk can place its supposed back point outside its hull. The calculation now locates the
  actual convex-hull exit before reading the ligament and rear contacts.
- The correction to the solid-angle mean is complete: the kernel is `L/(2 pi R^3)`, with no
  elastic Boussinesq kernel left in Law 2. The pulse gate and speed match the study. A scab's
  momentum is subtracted once; its volume is a separate zero-blow removal load.

## Model choices and P3s

- Retain the equivalent-area footprint, capped by exposed rear area, and the single back-point
  gate. These are named approximations rather than missing material or construction facts.
  In particular the thick-slab fixture's large footprint is not evidence for an invented radial
  cutoff. A spatial footprint needs validation; it belongs with #204's representation work.
- Brick's tensile DIF is explicitly **CLAIMED**, not a read measurement; wood's unity DIF is
  explicitly **ASSUMED**. Keep both provenance labels and the reference-rate applicability gap.
- The lane-6 P3s remain visible: casing rays start at the tip while the airblast starts at the
  filler centre; casing transport depends on an HP field being emitted and accepted by its law.
  Their emission adapter is outside the energy-path deletion; no HP receiver law is changed.
- A flying scab carrier and the near-contact products floor remain owner decisions. No carrier,
  alternative sampler, pricing run or capture is added here.

## Evidence

Contact fix: formatting and locked workspace/all-target clippy with `dev_ui,bitprobe` and
warnings denied pass. **MEASURED:** 96 building tests, 7 sandbox-wall tests and 3 doc-citation tests pass. The new
contact test covers bonded/resting interfaces, opened joints, dust/fallen neighbours, side
contacts and unrelated contacts. All test runs are unattended and headless.

Tensile precision fix: formatting and the required locked clippy pass. **MEASURED:** 130
building/material/sandbox-wall/doc-citation tests pass. The fractional static-strength fixture
breaks a mutant that rounds before multiplying; the extreme static-strength fixture is refused
at load even though its dynamic product fits in f32.

Hull-exit fix: formatting and the required locked clippy pass. **MEASURED:** 131 targeted tests
pass. A tapered hull at unit and doubled instance scale emits from its actual rear plane; an
anchor on that plane suppresses reflection. No collider or spawned body was added.
