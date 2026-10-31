# Shipped-wall far-topple fixture

Subject: the game's environment destruction model, game state only. These are tests of wall state under a blast field, not empirical breach validation or any tank/crew HP change.

## Inputs and the two seams checked

The fixture reads the sandbox's free-standing east-wall cut of the shipped house: its actual chunks, masses, joints and anchors, with connections to the other walls omitted by the existing `wall_cut` adapter. Input energies are ASSUMED fixture data: the study's 1.47 MJ burst, equivalent to DERIVED about 0.351 kg TNT, a bare 5 kg TNT-equivalent comparison and a larger bare 50 kg comparison. The source height is ASSUMED 1.45 m above the cut's foundation plane. Distances are burst-centre to front face, not charge-surface gaps.

Two tests have different contracts:

- `the_sampled_wall_reports_its_far_toppling_limit` uses the sandbox's real `apply_deposit` path: angular rays, intercepted shares, grouped loads, wear/local cracks, structural check, weight check and fall. Its class assertion is that a source outside the finite field leaves no deposit or structural change. It prints the measured curve and its reversals. It does **not** claim that this sampler supplies monotone loads.
- `a_fixed_far_face_loading_topples_monotonically_and_stops` checks Law 3 with fixed centroid quadrature on the same cut's coplanar front caps. Each actual cap area takes the same field's local specific impulse and peak pressure; no angular ray can miss a cap or move its quadrature point. These are pressure/impulse inputs to the existing structural law, with no wear-energy input. The far regime begins at the cut's DERIVED bounding diagonal, 6.67968 m computed from the loaded fixture's bounds. The class assertions are monotonicity of toppling in that regime and finite reach. No measured crossing or required outcome for the shipped asset is asserted.

"Topple" means an overturning `Cause::Push` section failure accompanied by released chunks. A local `Cause::Punch` is not counted as toppling. The fixed-face fixture is a section-law input, not an alternate production surface finder. It does not prove whole-pipeline monotonicity, and passing it does not close the sampled-path finding below.

## MEASURED results

Sampling interval: ASSUMED 0.25 m. Each source pose starts from a fresh wall.

| Path and fixture | MEASURED result |
|---|---|
| Production sampler, study burst | No topple observed over 0.5–23.5 m; no positive crossing measured. At the study's DERIVED 13.8 m, no chunks are released. |
| Production sampler, bare 5 kg comparison | Topples through 3.5 m, holds at 3.75 m, topples again at 4 m, then holds from 4.25 m through the sampled 56.5 m. At 4.25 m nine chunks are released by other mechanisms; they are not a topple. The final sampled topple is at 4 m, but this is **not a monotone cutoff**. |
| Fixed-face Law 3, study burst | No topple in the far regime. |
| Fixed-face Law 3, bare 5 kg comparison | No topple in the far regime. |
| Fixed-face Law 3, bare 50 kg comparison | Monotone far-regime curve; last sampled topple 26.42968 m, first hold 26.67968 m. This observed crossing keeps the fixture informative without making any particular distance or outcome a pass condition. |

The brief's requested positive study-burst limit therefore cannot honestly be reported as 13.8 m, or as any other measured crossing: none was observed in its sampled standoff interval. It is a negative measurement, not a measured zero-metre limit. Contact and smaller standoffs were not measured by this fixture.

## Why the study's DERIVED 13.8 m is not the current wall's prediction

The source-fit audit corrected the far reflected-pressure coefficients and recomputed the study's disc-wall arithmetic. That calculation still uses a capacity of 314 N s m. Its source is the wall prototype report's `H >= sqrt(2 I G_f A)` model, which explicitly records its omission of the wall's own weight. The current `section_failure` uses inertia about the hinge and adds the gravitational work needed to lift the held centre of mass over that hinge. The current open wall also is not the study's filled disc.

MEASURED in the diagnostic: the fixed-face comparison's failing section has capacity 3158.2644 N s m, with demand 3900.914 at 3.75 m and 3751.5376 at 4 m. The sampled comparison at 4 m has demand 3435.4788 and capacity 3200.4922 N s m. These are fixture readings, not material calibration targets. Merely restoring the sourced far pressure fit cannot make a calculation carrying the old capacity predict the current law.

The old narrative is `.agents/scratch/destruction-2026-10-30-buildings/wall-prototype/README.md`, "Structural check". Its old measurement is evidence about that prototype, not a current invariant. The current law and the measured fixture win where the prose differs.

## Sampling diagnosis and outstanding owner decision

The original pass/fail loop ran twice and deterministically failed on the bare comparison holding at 3.75 m and toppling at 4 m. It was minimised to those two fresh source poses. Ranked probes distinguished increasing total moment, early topology changes and changing sampled loading:

1. **Increasing total angular impulse was ruled out.** MEASURED about the reference base point: the sampled comparison's moment is approximately `(-4326.435, 31.034, -57.955)` N s m at 3.75 m and `(-4088.629, 58.606, 7.217)` at 4 m. Its main component decreases, while its smaller components change substantially.
2. **Wear removal and local cracking were ruled out as the initial cause.** No chunk is dusted at either pose. Re-running `structural` on an intact wall with the original sampled loads preserves the same hold/topple reversal and the same first failing section at 4 m. The legacy energy paths do not explain this counterexample.
3. **Changing sampled loads is supported.** At 3.75 m the field records MEASURED 229 ray hits and 195 chunk loads; at 4 m, 205 hits and 182 loads. Despite the smaller total moment, MEASURED 37 chunks become newly loaded and 111 receive larger individual blows at the farther pose. The current section law derives its turning axis and hinge from that vector loading; those inputs are not ordered merely because range increased.
4. **A stable face-footprint probe removes this pair's reversal.** On the same front caps with the same pressure/impulse functions, the fixed-face loads topple at both poses and decrease in demand against unchanged capacity, as recorded above. This is a controlled section-law fixture. It does not establish an exact all-face visibility integral or remove the production sampler's changing footprint.

**Decision:** do not tune material strength, lower a section capacity, increase ray count to fit one wall, or silently choose a new production surface finder in this material-data lane. The production counterexample is **not fixed**. The owner-selected representation work in [#204](https://github.com/vikng-dev/overmatch/issues/204) must account for coverage, directional symmetry and the sensitivity of the hinge reading when it settles how the one field reaches surfaces. The monotonicity requirement remains open for the full sampled path; only the fixed-face Law 3 contract is green. No assertion was weakened while calling the original bug fixed.

The committed measurement can be rerun with unattended/headless settings and `scripts/serial-cargo test --lib --features dev_ui,bitprobe the_sampled_wall_reports -- --nocapture`. Its transition log prints the counterexample. The monotone law fixture is `a_fixed_far_face_loading_topples_monotonically_and_stops`. Temporary diagnostic instrumentation was removed after the probes; the production field and section laws were not changed to conceal the result.

## Validation

MEASURED: all 141 targeted building, sandbox-wall, material, shipped-spec and doc-citation checks pass. Formatting and locked workspace/all-target CI clippy with `dev_ui,bitprobe` and warnings denied pass. This includes the monotone fixed-face fixture and the production sampler's finite-reach measurement, not a claim that its known reversal is fixed. Runs used the build queue with unattended/headless settings. No game window, capture, frame/tick timing or pricing run was taken, and no collider or body was added.
