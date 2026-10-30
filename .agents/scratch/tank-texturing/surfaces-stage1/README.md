# Tank surfaces, stage 1: before and after stills (#177)

Labels: MEASURED, DERIVED, ASSUMED, CLAIMED. No frame time was measured (other bays compiled in
parallel); the orchestrator prices the shader. The owner judges by eye; the pixel numbers below only
say where to look. Design: `.agents/docs/design/tank-surfaces.md`.

- **before**: the bay at `60da0d3`, before this lane.
- **after**: this lane's working tree, the default livery (`assets/tiger_1/tiger_1.livery.ron`:
  Dunkelgelb matte, Olivgrün and Rotbraun stripes).

Both arms are dev builds with `--features dev_ui,bitprobe`. `capture.sh <bin-dir> <png-dir> <arm>
[scene …]`: hidden, muted, a fresh config directory, 1920x1080, no UI or gizmos, a fixed step of
0.0078125 s, a frame at 10 s (the sandbox at 8 s), one lock hold per scene. `sbs.py` made `before/`,
`after/` (JPEG, quality 92), `side-by-side/` (before left), `difference/` (largest channel change ×8
in red) and `comparison.json` from the PNGs.

| Scene | What it shows | Pixels changed > 8 of 255 (MEASURED) |
|---|---|---|
| `close` | the controlled Tiger from 6 m, three-quarter | 338,635 (16 %) |
| `side` | its left flank from 7 m | 480,536 (23 %) |
| `macro` | the glacis and the mantlet from about 3 m: the rolled and cast visuals under paint | 1,513,803 (73 %) |
| `mid` | three-quarter from about 24 m | 42,623 (2 %) |
| `duel` | the default spawn and camera | 54,977 (3 %) |
| `sandbox_close` | the armour sandbox's target from about 9 m, x-ray on (its default) | **0** |
| `sandbox_mid` | the same from about 22 m | **0** |

The changed pixels are the tank and the bloom around it (`difference/`); the ground and sky are
untouched beyond that halo. **The sandbox pair is byte-identical**: the x-ray dresses and restores a
surface-material mesh exactly as it did the standard one.

`after-only/`, with no before twin:

- `sandbox_close_plain`, `sandbox_mid_plain`: the sandbox's target with the x-ray off, through the
  new `OVERMATCH_RANGE_XRAY=0` capture lever, which the before binary does not have. Its before is
  the flat substance colours of the duel stills' before arm.
- `no_camouflage_close`, `no_camouflage_side`: a second livery as a data edit — the same binary, the
  livery file's `camouflage` set to `None`, nothing else changed (the file was restored after).

What to look at:

- The decor atlas (tools, fenders, lights, the stowage bin's cross) keeps its look; it gets
  materials in a later stage.
- The road wheels and sprockets take the paint, because `MildSteel`'s visual is rolled steel and
  takes paint; the tyres are the `rubber` visual. Each road wheel carries its own frame (it is an
  articulated station), so its stripes differ wheel to wheel.
- The stripes run on across the plates of one rigid part (the glacis in `macro`) and jump where two
  plates face different projection axes. The visuals' relief reads at `macro` range and is gone by
  `close`.
