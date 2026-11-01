# Blast lane 4 review and lane 6 handoff

Subject: the game's environment destruction and damage model, game state only. This review concerns structure loads and physical fragment transport; crew and tank HP receiver laws are unchanged.

Reviewed baseline: `43146942` against its parent. Reviewed every changed hunk against the error-handling, code-branching and base-truths lenses, and against issue #237 and the supplied study. The standards and spec checks were performed directly; the owner prohibited delegated agents. The wall design owns the model narrative; this file records findings and dispositions.

## Standards

- **P2, fixed in `3baa249d`:** `substances::BlastFile::facts` accepted finite positive f64 data whose derived compaction strength or front speed became zero or infinity in f32. `Compaction::of` then silently gave the simulation unusable parameters. The load gate now refuses these values, naming the material file, row, field and repair. The omission/invalid-value test covers the new boundary.
- No kept alternative, per-object law, asset-specific pass condition or newly hidden failure was found in the remaining lane-4 hunks.

Standards disposition: one P2 fixed; P3 wording/validation findings are listed below.

## Spec

- **P1, fixed in `66cd0fef`:** `read_shell` used the fuze tip as the airblast origin. After energy wear was removed, an instant contact burst had a zero-area field patch and crushed nothing on that face. Required sourced `filler_setback` data now locates the filler centre; a positive filler-derived charge radius floors field distances and cone sections. The actual face point and the fuze's tip/time remain their own quantities. Both a kernel contact test and the live observer/deposit test pass.
- **P2, fixed in `1a51f8b2`:** the shared blast aggregation discarded a load's new crushed volume and blow when its legacy energy was zero, which the forthcoming removal of blast-to-Lame sources reaches. Blast aggregation now depends on impulse, groups by chunk and face, and preserves loads without legacy energy. Study test 9 also closes the existing energy-centroid angular-impulse gap, including opposing faces whose resultant cancels.
- Law 1's pressure gate, pulse factor and compaction speed match the stated formula. The blast wears volume alone. Kinetic lines and fields convert their energy through the existing cavity resistance into that same m3 pool; no extra energy wear pool remains.
- The stronger ray-interception fixture is justified. Its pass condition is whether an unshadowed field reaches and crushes a chunk, so its peak must exceed that material's threshold. The charge is labeled fixture data; neither the material strength nor a tank-specific pass condition was altered to make it pass.

Spec disposition: one P1 and one P2 fixed. Casing ejection momentum and study test 5 are implemented in the final lane commit.

## P3s and decisions

- **Rate applicability, named explicitly:** the source rate is a fact of a measurement; using its measured dynamic strength as one effective plateau is an ASSUMED model approximation. The source audit's tangential wood measurement is MEASURED 33.38 MPa at 2400/s, with static MEASURED 5.60 MPa. The study's recalled CLAIMED 3.8 MPa describes a different static datum and is not evidence for replacing this row. Retain the sourced row. A per-pulse rate law and separation of intrinsic rate effects from front inertia remain missing validation truths; none was invented here. The module now names the fixed-reference-rate invariant and points to the wall design; the source audit no longer claims the block is unused.
- **Representation claim, corrected and narrowed:** the old display rounded relative spread to four decimal places and could not support the literal MEASURED bound "at most 3e-4". MEASURED maximum with scientific reporting is **3.160834e-4**, below the existing ASSUMED test tolerance **1e-3**. The prose now uses MEASURED **3.17e-4** as a conservative reporting bound and limits the claim to the two tested near-face fixtures. Larger charges, other material rows and sparse far-face coverage remain #204's validation work. Changing the face from one block to an 8-by-8 cut gave identical reported totals for each ray count.
- **Units, corrected:** the spent-pool test and its synthetic load helper now describe crushed volume rather than an energy pool.
- **Filler geometry, explicit author data:** the shipped offsets are DERIVED cylinder-equivalent estimates with ASSUMED axial geometry, not claimed drawing measurements. Required sourced data supplies the runtime offset. Better drawings change those data without a new law or per-round branch.
- **Casing transport, one budget:** normalize the recorded angular profile to physical casing KE, independently of the HP potency multiplier. DERIVED ejection momentum is `2E/v_s`, with scalar total `M v_s`. Preserve each nonparallel fragment stop's load and its emitted direction even at a contact stop. Carried flight energy and a detailed fragment mass/shape distribution remain outside this ejection adapter; the existing ring is the angular approximation. Known chemistry that cannot pay its casing KE is refused before play. Unsourced chemistry continues to claim no airblast energy.

## Evidence and gates

MEASURED final physics tests: chemistry split maximum relative residual **3.501061e-8** (below f32 epsilon); maximum crush-work/blast-energy fraction **2.760225e-2** over the fixture sweep (below the specified half-energy audit bound); sampler maximum relative spread **3.160834e-4** over 1024, 4096, 16384 and 65536 rays. The study's approximately one-percent crush-work statement is not a general bound for the current sourced rows and this fixture sweep. Energy is audited, never used to cap or drive Law 1.

Formatting and locked workspace/all-target clippy with `dev_ui,bitprobe` and warnings denied pass. The final runtime change passed **182 targeted tests**, including building, blast, sandbox-wall, ballistics burst/fuze/recording, round-data, gun pins, materials and doc citations. The origin commit separately passed the headless coverage in a **148-pass / 3-ignored** selection. The full suite remains CI's job. All runs were unattended/headless and under the machine lock; no game window, capture, cost/tick measurement or external write was used.

No P1/P2 finding from the lane-4 diff remains open. Scab, removal of the remaining blast-to-Lame energy path, the products floor, surface representation, captures and pricing remain their assigned lanes. The repo still has no verified empirical blast-breach case; none was manufactured from a fixture.
