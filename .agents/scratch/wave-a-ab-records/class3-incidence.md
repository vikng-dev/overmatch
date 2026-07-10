# Class-3 incidence A/B — avian constraint-order patch (2026-07-11)

Per the hsim session's suggested discriminator (.agents/scratch/hsim-to-wave-a.md): short
course, 80/10, SPIKE_PERTURB=0, N=12 per side, counting class-3 = pos+rot+lv+av mismatch
window persisting to trace end below rollback thresholds (fire-seeded or connect-tail-seeded).
A = main @ d7d103e (recoil-order fix IN). B = bay wave-a/integration avian-alone @ 38c31b0
(same main + avian fork override). All 24 runs clean (retry harness; zero NaN/panic greps).

| side | class-3 | specimens | other anomalies |
|------|---------|-----------|-----------------|
| A (unpatched) | 2/12 | A2 fire-seed @568, |dp| 0.230mm→1.95mm, 289 ticks to end; A4 connect-tail @291, 1.13mm→9.68mm, 564 ticks to end | A1 16-tick connect transient (closed); A12 constant-offset connect anomaly (below) |
| B (avian) | 1/12 | B12 fire-seed @555, |dp| 0.230mm→1.91mm, 296 ticks to end | B2 constant-offset connect anomaly, closed @471 |

## Conclusions

1. **The avian constraint-order patch does NOT eliminate class-3** (1/12 vs 2/12, Fisher
   p≈1.0 — no detectable effect; a 2× effect at ~15% incidence needs N≈100+/side, not worth
   the wall clock). The fire seed is IDENTICAL (0.230 mm) on patched and unpatched builds and
   matches both hsim-session specimens — a deterministic seed magnitude, one mechanism.
2. Therefore class-3 attribution shifts: NOT (primarily) solver color order. Remaining
   suspects per the hsim note: contact-restore/BVH at island change (report #5 class), or the
   shell-spawn/impulse path itself. Hand back to the divergence track with this evidence.
3. Incidence is far below the pre-recoil-fix era (2/3 rebaseline shorts on 2a482c6-era code)
   — f516fb6 (recoil order) removed the bulk of fire-edge seeding; what remains is the rarer
   0.230 mm pos-seed.
4. **NEW anomaly class (not wave-A scope): constant-offset connect runaway.** A12: |dp|
   884.760 mm CONSTANT from tick 264 to trace end (580 ticks), no rollback. B2: 858.415 mm
   constant, closed @471 (corrected). 2/24 incidence. A constant offset = a world-frame shift
   (spawn/teleport ordering?), not physics divergence; above rollback threshold yet unrolled
   (watchdog active!) for its duration → §7-adjacent connect pathology + possible watchdog
   blind spot. Flag into doc §9 update and the connect-transient investigation.

## Avian adoption picture at game level (with flat-cruise gate from earlier)

- Long course avian-patched: physics bit-exact all 1262 shared ticks, 94.69% vs 94.45%
  unpatched — NO regression.
- Short course: class-3 incidence not worse (1/12 vs 2/12).
- The patch's proof of value stays CRATE-LEVEL (cross-World bit-identical 180 ticks, RED
  fails with order-1 Gauss-Seidel divergence at tick 1) + strategic (determinism enabler).
  The live-network game instrument cannot see the wedge term (documented in ab-avian/RECORD.md).
