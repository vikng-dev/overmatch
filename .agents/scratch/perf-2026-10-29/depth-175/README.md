# Scene depth copied from the main pass, issue #175

The camera no longer draws the scene twice. The depth prepass's draw, clear and copy are skipped,
and the main pass's own depth is copied into the prepass depth texture after the opaque pass. Two
things read that texture: the soft fade of the VFX sprites, and the hull marks (the visual layer
of the game's damage model), whose fade also hides a mark behind a nearer surface. The mechanism
and its invariants are in `src/scene_depth.rs`. `SPIKE_ABLATE=depth_copy` selects bevy's own
prepass again, for A/B pricing. No frame time was measured here.

## What to look at

Each JPEG puts **before** (bevy's prepass, `SPIKE_ABLATE=depth_copy`) on the left and **after**
(the copy, the default) on the right. Frames are unmodified apart from JPEG compression; the
statistics in `diff.json` come from the lossless PNGs.

- `marks-18/21/24s`: close-ups of the three hull marks. The 18 s frame shows the hole's lower edge
  fading where the fender stands in front of the face.
- `scene-16s`: the struck tank seen from its far side. All three marks are on the face turned
  away from the camera, so the hull stands between the eye and every mark. None may show.
- `scene-17.08/17.2/17.45s`: dust from four terrain impacts at 17 s (three at 88 mm, one at
  7.92 mm, fixture data). The dust stands against the terrain (left) and against the hull
  (centre); one plume rises behind the hull.
- `control-no-copy/`: the copy route (left) against a build with only the copy removed (right).
  Without the copy, the hidden marks draw through the hull as black squares, and the close-up
  marks turn into black squares. The captures therefore detect a wrong depth source.

## Results (MEASURED)

Every RGB pixel of each 1600x900 frame (1,440,000 pixels):

| MSAA | Frames | Differing pixels | Over 8/255 | Max channel delta |
|---|---:|---:|---:|---:|
| 4x (default) | 7 | 0 in every frame | 0 | 0 |
| 2x | 7 | 0 in every frame | 0 | 0 |
| off | marks 18/21/24 s | 0, 0, 4 | 0 | 1 |
| off | scene 16/17.08/17.2/17.45 s | 68, 125, 127, 75 | 1 per frame | 11 |

- **Noise floor (MEASURED):** two identical runs of one route at MSAA off differ by 0 to 1 pixel
  per frame.
- **Where the MSAA-off differences are:** scattered single pixels on the distant terrain ridge
  near the horizon. None fall on a sprite, a mark or a tank. See
  `msaa-off/scene-00017.2000s-ridge-crop-4x.png`: before, after, and the differing pixels in red.
- **Cause (ASSUMED, not isolated):** where far terrain surfaces overlap at grazing angles, depth
  ties resolve differently when the main pass tests against its own depth instead of against
  the prepass's.
- **Route check (MEASURED, `SPIKE_PHASE_COUNTS=1`):**
  - Every after-run log has no per-view `opaque_prepass` census line for the camera.
  - Every before-run log has 3 or 4 such lines.
  - Every log has `settings: loaded`, three hull marks struck, and no panic or validation line.

## Not captured

- **The kill cam lesson.** It draws the pen cam's `LookMaterial` with an opaque blend through the
  main camera. That material opts out of the prepass, so after this change it enters the copied
  depth, and sprites and marks fade against it. The difference is intended and was not captured.

## Reproduce

These are offline runs with a hidden window and muted sound: one
`/usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock` per run, `target/debug/overmatch`. Each
MSAA rung has its own `OVERMATCH_CONFIG_DIR` holding `(version: 1, msaa: X4|X2|Off,)`.

```sh
common="OVERMATCH_OFFLINE=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_MUTE=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
  OVERMATCH_TARGET_AHEAD=1 OVERMATCH_HULL_MARKS_DEMO=1 OVERMATCH_CAPTURE_SIZE=1600x900 \
  OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_GIZMOS=0 OVERMATCH_CAPTURE_STEP_S=0.0078125 \
  SPIKE_PHASE_COUNTS=1"
# before: add SPIKE_ABLATE=depth_copy
env $common OVERMATCH_CAPTURE=marks.png OVERMATCH_CAPTURE_AT=18,21,24 <lockf …> target/debug/overmatch
env $common OVERMATCH_CAPTURE=scene.png OVERMATCH_CAPTURE_AT=16,17.08,17.2,17.45 \
  OVERMATCH_CAPTURE_FOLLOW=2.0,3.5,-26 \
  "OVERMATCH_CAPTURE_IMPACT=-406.5,-462,0.088@17;-409.5,-460.2,0.088@17;-412.5,-450,0.088@17;-408,-461,0.0079@17" \
  <lockf …> target/debug/overmatch
python3 compare.py before/ after/ --sbs out/
```

- `OVERMATCH_CAPTURE_IMPACT` is a capture lever added with this change (`src/dev_capture.rs`).
- A capture run now seeds the VFX's cosmetic randomness with a fixed value, so the two routes draw
  the same sprites.
