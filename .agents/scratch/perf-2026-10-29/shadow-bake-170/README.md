# The terrain's sun shadow as a baked field, issue #170 (step 1: the terrain layer)

`src/terrain_shadow.rs` owns the bake. The vendored `bevy_pbr`
(`vendor/bevy_pbr-0.19.1-scalar-math/OVERMATCH_PATCH.md`) owns the sampling. This file holds the
evidence for the owner to judge by eye. No frame time was measured.

## What changed in the picture

- **Before** (`SPIKE_ABLATE=terrain-field`): the terrain casts into the sun's cascades, as shipped.
- **After** (default): no terrain level casts. Every lit fragment (ground, tanks, trees, houses)
  multiplies the cascade shadow by the baked field's visibility at its own position, inside the
  cascades' envelope. The penumbra is the sun's disc past the occluder's edge: half lit on the
  occluder's line, and wider the further away the occluder is.
- **Both:** the track-print relief's sun term (#161) is folded into the sun's shadow through
  `PbrInput::directional_shadow_factor` instead of re-evaluating the sun and subtracting it.

## The finding that frames every capture

On Kalinovo, under the shipped 17° sun, the terrain shades almost nothing: MEASURED 7 of 1,050,625
grid nodes are in the terrain's shadow (bake log). The steepest slope on the map that rises toward
the sun is DERIVED 18.3° (from a bilinear resample of the heightmap, by the lane that started this
work; not re-derived), one degree steeper than the sun. So at the shipped sun, before and after
should match everywhere, and this map cannot show the hull-down and ridgeline cases.

To show them, the captures marked `8deg` lower the sun with a dev-only evaluation lever,
`OVERMATCH_SUN_ELEVATION=8` (`world::sun_elevation_deg`). It moves the light, the sky and the bake
together. The illuminance is not rebalanced, so the scene reads darker. At 8° the field shades
MEASURED 19,057 nodes.

## Captures

Both arms ran the same binary (`scripts/serial-cargo build --features dev_ui,bitprobe --bin
overmatch`, dev profile). Each run was offline, hidden, muted, with a fresh config directory,
1920x1080, no UI or gizmos, a fixed step of `0.0078125 s`, a frame at 10 s, and one
`/usr/bin/lockf` per run: `capture.sh <binary> <png-dir> before|after [scene …]`. The scenes and
their levers (ASSUMED capture configuration) are in `capture.sh`:

| Scene | What it shows |
|---|---|
| `duel` | the default spawn and camera |
| `hulldown-side-8deg`, `hulldown-high-8deg` | a tank at the edge of a ridge's shadow, from the side and from above |
| `ridgeline-8deg`, `ridgeline-17deg` | the same ridge from further back, at a low sun and at the shipped sun |
| `treeline-17deg`, `treeline-8deg` | the forest view of #157 |
| `prints-mud-*` | the track-print demo on mud: the drive (38.5, 40.5, 42.5 s) and the pivot (44.5, 46.5, 48.5 s), each at Lighting, Parallax and Sun Shadow |

The tank spot was picked from an independent estimate of the 8° shadow (a numpy march over the
heightmap). There, the top of the ridge's shadow stands about 1.35 m above the ground: the hull
sits at the shadow's edge, with its deck and turret above it.

Committed: `before/` and `after/` (JPEG, quality 92), `side-by-side/` (before on the left, after on
the right), `difference/` (each pixel's largest channel change ×8, in red over the before frame,
at half size), and `comparison.json`. `sbs.py` made all of them from the unmodified PNGs.
`fold-161/` is the same comparison for the fold-in alone: `main` is the binary of this branch's
base (`8f116cc`), `folded` is this branch's raster arm.

## What to look at

- **`duel`, `ridgeline-17deg`, `treeline-*`:** these match. The differences are thin lines on the
  tank's edges: the tank's spawn frame follows asynchronous asset loading, so its settled pose
  varies by a hair between runs.
- **`hulldown-*-8deg`:** the tank reads the same in both arms, with the ground around its hull in
  the ridge's shadow and its deck and turret in the sun. The ground shadow's far edge reaches
  further in the field.
- **`ridgeline-8deg`:** the field shades visibly more ground than the raster does. An independent
  estimate agrees with the field, not with the raster (the starting lane's numpy march, not re-run
  here). The march shades 17,755 nodes at 8°,
  against the field's 19,057; the march walks only the diagonal nodes, so it can only find fewer.
  Half of those nodes lie less than 0.17 m below the shadow's top (DERIVED percentiles 10/25/50/
  75/90 from the march: 0.015, 0.046, 0.167, 0.446 and 0.821 m). The likely cause, which was not
  isolated: the raster arm looks up each receiver offset along its normal by
  `SUN_SHADOW_NORMAL_BIAS` (2.6) cascade texels, and that can hide a shadow this shallow. So at a
  grazing sun the field draws long, shallow shadows that the raster path did not. Whether they
  should read this dark is the owner's call. The sun's-disc penumbra already softens them in
  proportion to their distance from the crest.
- **`prints-mud-*`, a visible change to the track prints.** In the raster arm the terrain's shadow
  map shades every parallax-sunk print fragment, because the ground casts onto the print drawn
  below it. So near the camera, the rut floors read dark at both parallax levels, and the relief's
  own sun shadow is lost in that darkness. In the field arm, a lit node's own falling slope is
  never its occluder (module doc), so the floors are lit, and only the relief's sun shadow darkens
  them, in strips behind the walls that face away from the sun. Compare `prints-mud-00046.5000s`
  (Parallax) with `prints-mud-00048.5000s` (Sun Shadow) in `after/`. In the raster arm the dark
  floors depend on the cascade (the normal bias grows with the texel), so they are a raster
  artefact, not the relief's law. They are the shipped look, though, so the owner should judge
  the change.
- **`fold-161/`:** the fold-in is bit-identical to the base on the raster arm. All six frames have
  MEASURED 0 differing pixels. Its one change is under `Shadows Off`: there the relief's sun
  shadow now lapses with every other sun shadow (the vendored hook sits inside the
  shadows-enabled block, as upstream's does).

## Pixel results

MEASURED on the unmodified 1920x1080 PNGs, every RGB pixel, no masking (`comparison.json`), out of
2,073,600 pixels per frame. Pixel counts point to where to look; they are not a gate.

| Scene | Differing pixels | Over 8/255 | Over 32/255 | Mean channel delta |
|---|---:|---:|---:|---:|
| `duel` | 53,240 | 4,285 | 684 | 0.0685 |
| `hulldown-high-8deg` | 906,370 | 99,771 | 44,546 | 2.5091 |
| `hulldown-side-8deg` | 267,755 | 34,579 | 8,963 | 0.4814 |
| `prints-mud-00038.5000s` | 28,871 | 3,392 | 481 | 0.0466 |
| `prints-mud-00040.5000s` | 286,881 | 39,153 | 24,472 | 0.7782 |
| `prints-mud-00042.5000s` | 216,666 | 28,208 | 17,097 | 0.5409 |
| `prints-mud-00044.5000s` | 30,989 | 4,200 | 542 | 0.0542 |
| `prints-mud-00046.5000s` | 556,826 | 140,593 | 107,536 | 2.9643 |
| `prints-mud-00048.5000s` | 485,304 | 112,873 | 88,652 | 2.4125 |
| `ridgeline-17deg` | 180 | 2 | 0 | 0.0001 |
| `ridgeline-8deg` | 961,359 | 148,310 | 67,767 | 3.1583 |
| `treeline-17deg` | 269 | 10 | 0 | 0.0002 |
| `treeline-8deg` | 419 | 16 | 0 | 0.0003 |

## The bake

MEASURED from the after runs' logs, dev profile (this crate at opt-level 1), on a machine shared
with other bays' builds: 345–373 ms at the shipped sun and 654–698 ms at 8°, once at startup, on
all cores. The field is 1025² texels of `Rg32Float`: DERIVED 8.4 MB of GPU memory.

## Not in this change: step 2, the trees and houses layer

The owner's design (#170, comments of 2026-10-30): a light-space depth bake of the trees, and later
the houses, in world-anchored tiles, each redrawn when something in it changes. When a tree starts
to fall, its tile is redrawn without it in that same frame, and the tree casts live while it falls.
When it settles, the tile is redrawn with the fallen pose. Nothing of it is built yet. It needs:

- a tile atlas (a 2D array texture, one layer per world tile) with an orthographic sun projection
  per tile that covers the tile's footprint swept along the sun;
- a render path that draws one tile's static casters into its layer on demand: a new extracted
  view per dirty tile, reusing the shadow phase's depth pipeline;
- the atlas and a tile table in the view bindings, sampled in `fetch_directional_shadow` and
  combined with the cascades by occluder depth before filtering, not by multiplying two filtered
  factors;
- a static-caster role in `render_policy` (casts into the bake, not the cascades), and the
  transitions in `trees/view.rs` (`draw_trees`, the fall's first frame and its settling) that mark
  a tile dirty and move a tree between the static and live roles;
- the capture sequence across a fall (`OVERMATCH_CAPTURE_FELL`).

Its saving ceiling is small: tree casters MEASURED −0.14 ms at 2 tanks and noise at 30, house
casters noise at both (`../shadow-pricing-2026-10-30.md`).
