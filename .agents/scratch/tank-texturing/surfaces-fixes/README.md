# Tank surfaces, review fixes: parity and loading stills (#177)

Labels: MEASURED, DERIVED, ASSUMED, CLAIMED. No frame time was measured (other bays compiled and
priced in parallel). The owner judges by eye; the pixel counts only say where to look.

- **before**: `cc8682d0`, main at the lane's base (the surfaces as stage 1 shipped them).
- **after**: the lane's head, the six review fixes.

Both arms are dev builds with `--features dev_ui,bitprobe`, run on the same asset tree: hidden,
muted, a fresh config directory per run, a fixed step of 1/128 s, the duel's close view through
`OVERMATCH_CAPTURE_FOLLOW=6,2.5,-6`, the side view through `-7,2,0.5`, the sandbox's target through
`OVERMATCH_RANGE_POSE=6,4.5,-5,0,3,-12` with `OVERMATCH_RANGE_XRAY=0`.

## The look does not change

| Scene | Pixels differing, before against after (MEASURED) |
|---|---|
| `close`, the controlled tank from 6 m at 10 s, 1920x1080 | 0 of 2,073,600 |
| `side`, its flank from 7 m at 10 s | 0 |
| `duel`, the default camera at 10 s | 0 |
| the close view every 0.25 s from 0.25 to 8 s (32 frames) | 0 in 30 frames; 1 pixel in each of two (1.75 s, 2.75 s) |
| the sandbox's target every 0.25 s from 0.25 to 8 s (32 frames) | 0 |

`parity_close.jpg`, `parity_side.jpg`: before left, after right, half size.

## While the maps load

Bevy draws nothing of a material whose images are not on the GPU yet. Before, a dressed surface was
that material from the frame the view landed; after, it wears the glb's own look until its maps land.

- **The sandbox, unhindered.** It spawns its target as it starts, so the target's maps are still
  loading in its first frames. Frames 2 to 7 (0.016 to 0.055 s) differ by 3,045 to 7,767 pixels
  (MEASURED): before draws only the parts whose material names no visual (decor, track, the crew
  boxes inside the hull), after the whole tank in the glb's flat colours. Identical from 0.0625 s on. `sandbox_loading_0.031s.jpg`.
- **The duel with its maps held back.** Every tank-surface KTX2 is served through a FIFO in a
  mirrored asset root, fed 3.5 s (wall) after the binary starts. Before: the armour, the wheels and
  the barrel are missing until the maps land (2.375 s app time); after: the glb look until they land
  (1.375 s), then the livery. The two arms land at different app times because the feed is
  wall-clock. From 2.375 s on the arms are identical but for 5 pixels at 5.25 s (MEASURED).
  `held_back_1.000s.jpg` (both waiting), `held_back_1.500s.jpg` (after landed, before still bare).
- **The duel, unhindered.** Its maps land before its first pipelines are ready: no frame of the
  first 64 (to 0.5 s) differs between the arms. In both, the first two frames draw neither the
  armour nor the terrain; that is startup, not the maps.
