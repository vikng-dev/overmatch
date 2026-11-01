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
- **P3, fixed:** the fracture module introduction says hulls never serve a law, but Law 2 reads its
  hull. Its interface documentation now states the hull's Law 2 role.
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
- **P1, fixed:** the scab gate uses the original full chunk depth even when Law 1 has crushed
  the front. The study's executable calculation uses the remaining ligament and refuses scab
  after through-crush. The gate now subtracts the local cone-volume crater depth, read from the patch nearest the
  face centroid (ASSUMED). Depth is intensive under interception; through-crush emits no scab.
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

Remaining-ligament fix: formatting and the required locked clippy pass. **MEASURED:** 133
targeted tests pass. The cone-volume test holds at normal and oblique incidence across patch
shares; the gate test rejects both a partial crater with too short a ligament and through-crush,
without exporting phantom momentum or removing phantom rear volume. Existing slab momentum and
representation tests still pass; the crushed-volume calculation retains its original arithmetic.

## Dead-path deletion audit

- **Energy-gated pushes:** the literal `pushes.retain` disappeared with the unified `Load` in
  `c1208bc9`; its equivalent energy gate and energy-weighted mean were removed by `1a51f8b2`.
  Main already groups by chunk/face and retains loads by positive impulse, with an
  impulse-weighted centroid and independent opposite faces. No duplicate path is retained.
- **Energy-sized blast dishes:** `43146942` already replaced them with `Load::crushed` in m³,
  from Law 1; `holes_of` uses that volume for blast/scab dishes. Kinetic channels alone retain
  energy divided by cavity resistance. No blast-energy dish remains to delete.
- **Blast-to-Lamé:** removed the remaining addition of `RayHit::energy` to `Load::energy`.
  Blast face and rear-removal loads carry zero kinetic cavity energy. Pressure/impulse drive
  crush, scab and structural loading; only kinetic lines/fragments source the cavity crack law.
  Removed the obsolete blast-energy carry assertions and the synthetic blast helper's cavity
  energy. The low-pressure test now also requires no cavity-driven cracks.
- **Current energy user:** `RayHit::energy` survives solely for the wall sandbox's intercepted
  chemical-energy geometry audit. Its displayed value is explicitly an audit; the last-shot
  blast readout shows impulse. It reaches no load, pool, hole, centroid or joint stress.
- **Non-vacuous material fixture:** after the legacy route was removed, the old distant fixture
  produced no state change. Its charge is now the existing ASSUMED crushing fixture, and the
  test checks that its peak exceeds each material's crush threshold. Relabelled kinds/names
  must preserve the whole state; swapping material rows must change pool wear. No particular
  house breach, crack count or vehicle outcome is a pass condition.
- Corrected the static-tensile load diagnostic to name `bulk.tensile`, and the stale module/test
  prose that attributed blast cracking to kinetic cavity energy. Non-blast deposits avoid
  collecting rear contacts when they have no blast stops.

## Final gates and evidence

Formatting and locked workspace/all-target clippy with `dev_ui,bitprobe` and warnings denied
pass. **MEASURED:** 189 targeted tests pass, covering buildings, blast, ballistics burst/fuze/
recording, materials, sandbox-wall, headless, UI string coverage and doc citations. Four existing
measurement-only tests remain ignored (burst-law cost, forest cost, heightfield drive and reverse
throw); no pricing, capture or still was run. All test binaries ran unattended and headless under
the serial build/run lock.

**MEASURED**, final test run: chemistry split maximum relative residual **3.501061e-8**;
maximum crush-work/blast-energy fraction **2.760225e-2**; sampler maximum relative spread
**3.160834e-4** on the two existing near-face fixtures. The isolated fixture slabs retain the
recorded scab results: thicknesses **0.12 / 0.25 / 0.5 m**, masses **5.755 / 27.939 / 202.209 kg**,
speeds **14.29 / 6.19 / 1.96 m/s**, momentum **82.2 / 173.0 / 397.1 N s** out of **589.7 N s**.
These are fixture measurements, not a verified breach gate. The final diff was read again against
all three lenses: no remaining P1/P2 from the lane-5 review; the named footprint/point-sampling
approximations, transmission gap and casing-emission P3s remain as stated above.
