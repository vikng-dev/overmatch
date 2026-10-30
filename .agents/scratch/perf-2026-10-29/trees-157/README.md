# A standing tree drawn as two meshes, issue #157

View only: the trees' simulation (segment colliders, pools, state, derive) is unchanged and still
built at spawn. `src/trees/view.rs` owns the mechanism; this file holds the evidence.

## What the view draws

- **Standing:** two mesh entities under the view root, the kind's merged trunk (bark) and canopy
  (needles), shared by every tree of the kind. MEASURED in the hidden run's log:
  `fir_1's view read — a standing tree draws 2 merged meshes, a fallen one 116 entities of which 57 draw`.
  Before, every standing tree kept those 116 view entities (the scene wrapper, 58 glb nodes, 57
  primitives); 27 drew and 30 caps were hidden.
- **Down:** the segmented hierarchy, caps included, rebuilt from the kind's template in the frame
  the state leaves `Standing`, in `SpawnScene` before `present_fall` poses it. The cut is the
  sim's, unchanged. A tree standing again draws the two meshes again; a settled tree stays
  segmented (no compaction in this slice).
- **B0004:** MEASURED 635 warnings per run before (`warning[B0004] … parent (the tree fir_a_0001
  entity) without InheritedVisibility`), 0 after, in both hidden runs below. The tree root now
  carries `Visibility`.

## Captures

Baseline `2e81da6` (the fell lever on top of `f832a1e`); candidate: this lane's view change. Same
assets, a fresh config directory (defaults), hidden window, offline, one
`/usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock` per run. ASSUMED capture configuration:
`1280x720`, no UI or gizmos, virtual step `0.0078125 s`, the camera held by
`OVERMATCH_CAPTURE_POSE`. Two runs of the baseline binary matched pixel for pixel (MEASURED: 0
differing pixels in all 7 frames), so the noise floor is zero.

```sh
common="OVERMATCH_OFFLINE=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_CAPTURE_SIZE=1280x720 \
  OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_GIZMOS=0 OVERMATCH_CAPTURE_STEP_S=0.0078125"
# forest
env $common OVERMATCH_CAPTURE=forest.png OVERMATCH_CAPTURE_AT=10 \
  OVERMATCH_CAPTURE_POSE=540,19,556,548,16,640 <lockf …> target/debug/overmatch
# fall: fir_c_0160 cut at segment 3 toward bin 0 at 10 s
env $common OVERMATCH_CAPTURE=fall.png OVERMATCH_CAPTURE_AT=9.5,10,10.0078125,11,12.6,14 \
  OVERMATCH_CAPTURE_POSE=545.5,33,522,556,25,592 OVERMATCH_CAPTURE_FELL=fir_c_0160,10,3,0 \
  <lockf …> target/debug/overmatch
```

Committed: `before/` and `after/` hold the forest and the fall at rest (9.5 s), the swap frame
(10.0 s, the frame of the fall's tick), mid-fall (11.0 s) and settled (14.0 s), unmodified pixels.
`comparison.json` is `../track-fit-155/compare.py before after --crop 443,40,1280,680` (the crop is
the felled tree and its shadow; the key reads `belt_rectangle`).

## Pixel results

MEASURED, every RGB pixel, no masking:

| Frame | Differing pixels | Over 8/255 | Max channel delta | Mean channel delta |
|---|---:|---:|---:|---:|
| Forest | 362 | 5 | 81 | 0.00041 |
| At rest | 421 | 9 | 61 | 0.00047 |
| Swap | 387 | 9 | 61 | 0.00044 |
| Mid-fall | 456 | 9 | 63 | 0.00047 |
| Settled | 510 | 7 | 63 | 0.00049 |

Of 921,600 pixels per frame. The differences are isolated pixels on needle-card edges and shadow
edges, spread over every tree ([forest-difference.jpg](forest-difference.jpg), red marks dilated
for visibility). Cause, DERIVED: a merged vertex is composed on the CPU (`p + t` in the view
root's frame), a segment's on the GPU (the composed affine times `p`), so the same position
rounds differently, by about one f32 step: `2^-14 m`
(6.1e-5 m) at the felled tree's coordinates (545 m, 592 m), against a pixel of about 0.085 m at
its 74 m from the camera (0.785 rad over 720 px). That flips only coverage samples within a
thousandth of a pixel of an edge. Depth ties between crossing canopy cards, now resolved within
one draw instead of across per-segment draws, are a second candidate; the captures do not
separate the two.

**The swap does not pop.** Within one run, the swap frame against the at-rest frame (MEASURED):
before, 0 differing pixels; after, 34 pixels, all on the felled tree's canopy
([swap-difference.png](swap-difference.png)), 31 by one level, 2 by four, 1 by eight. Those are
the same rounding: the felled tree changes from its merged meshes to its segments that frame.
Across runs, the swap frame's 387 differing pixels are exactly the at-rest frame's 421 less those
34 (MEASURED): once both runs draw the felled tree segmented, it matches pixel for pixel. The next
frame (10.0078 s) differs from the at-rest frame by 837 pixels before and 854 after, the fall's
first tick in both.

The 600 m cut (`TREE_VIEW_RANGE_M`) is unchanged, but is now measured to the tree's base for the
two merged meshes instead of to each segment's origin: a standing tree now appears and disappears
whole at that range, where its upper segments used to cut first in a band of about 1 m (DERIVED:
a segment origin 35 m above the base, from a camera at the base's height, sits
`sqrt(600² + 35²) − 600 ≈ 1.0 m` farther). No tree in these frames is near that range.

No frame-time measurement was taken.
