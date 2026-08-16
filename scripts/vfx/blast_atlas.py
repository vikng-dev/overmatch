# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow", "numpy"]
# ///
"""Re-atlas a source explosion flipbook into an `assets/vfx/` blast-core atlas.

Source: the Unity Labs Paris "Free VFX image sequences" flipbooks (CC0; see
`assets/vfx/cc.txt`), 5x5 grids in a 1024 px sheet — so a source cell is 204.8
px and NO integer crop reproduces it. Each cell is resampled from its exact
fractional box into an integer output cell, which is also where the frame
window is applied: only the fireball frames are kept, the smoke tail a muzzle
blast does not want is dropped.

Two defects of the source are fixed here, both invisible until a sampler
interpolates:
  * the fully transparent region carries flat grey RGB (47,47,47 in
    Explosion00) — bilinear taps straddling the sprite edge would drag that
    grey into the visible fringe as a halo. RGB is edge-extended (alpha-bled)
    out of the visible texels, and zeroed beyond the bleed radius so any tap
    that reaches further adds black, never grey;
  * the sprites are COLORED fire, while the billboard pipeline expects
    grayscale-with-alpha (`src/vfx/billboard.rs` recolors the sprite signal
    through a per-effect gradient LUT). RGB is collapsed to relative luminance
    in linear light, then re-encoded to sRGB — the flash LUT rebuilds the
    white-hot-core-to-orange-edge ramp from that one channel.

Frames are also re-centred on the cell. Both sequences were authored as a
rising ground explosion, so the fireball sits low in the cell and climbs
(Explosion01 travels a quarter of a cell over the window) — on a muzzle
billboard that reads as the blast crawling off the bore. Centring hands the
vertical motion back to the billboard's own world-space `drift`, where it is
physical.

Alpha is left STRAIGHT (the shader reads `tex.r * tex.a`).

Usage:
    uv run scripts/vfx/blast_atlas.py <source.tga> <out.png>
"""

from __future__ import annotations

import argparse
import sys

import numpy as np
from PIL import Image

# The frames a muzzle blast reads: the fireball from ignition to the beat it
# rolls fully into smoke. Beyond frame 11 both source sequences are a slow
# smoke fade with no hot texels left (measured: the fraction of texels above
# 0.6 signal falls under 0.5% by frame 11 and is zero by frame 14).
FRAME_WINDOW = (0, 12)
# Output grid — INTEGER cells, unlike the 204.8 px source.
OUT_COLS, OUT_ROWS = 4, 3
CELL_PX = 256
# How far RGB is edge-extended into the transparent region (texels). Well past
# any bilinear tap; the atlases carry no mip chain.
BLEED_PX = 12
# Alpha above which a texel counts as sprite for the centring bounding box.
# Low enough to include the smoke fringe, high enough to ignore the tail of the
# source's own antialiasing.
VISIBLE_ALPHA = 0.03


def srgb_to_linear(x: np.ndarray) -> np.ndarray:
    return np.where(x <= 0.04045, x / 12.92, ((x + 0.055) / 1.055) ** 2.4)


def linear_to_srgb(x: np.ndarray) -> np.ndarray:
    return np.where(x <= 0.0031308, x * 12.92, 1.055 * x ** (1 / 2.4) - 0.055)


def luminance(rgb: np.ndarray) -> np.ndarray:
    """Relative luminance of sRGB-encoded `rgb` (0..1), returned sRGB-encoded.

    Weighting in linear light and re-encoding is what keeps the mid-tone fire
    at the brightness it reads at; weighting the encoded values directly
    darkens everything that is not already black or white.
    """
    linear = srgb_to_linear(rgb)
    return linear_to_srgb(linear @ np.array([0.2126, 0.7152, 0.0722], np.float32))


def bleed(gray: np.ndarray, alpha: np.ndarray, radius: int) -> np.ndarray:
    """Edge-extend `gray` out of the visible texels, zero beyond `radius`.

    One dilation pass per texel of radius: each still-unknown texel takes the
    mean of its known 4-neighbours. Cheap, and the result only has to be
    plausible — it is never seen at alpha 1, only dragged in by a filter tap.
    """
    known = alpha > 0
    out = np.where(known, gray, 0.0)
    for _ in range(radius):
        if known.all():
            break
        padded_v = np.pad(out, 1)
        padded_k = np.pad(known, 1).astype(np.float32)
        total = (
            padded_v[:-2, 1:-1]
            + padded_v[2:, 1:-1]
            + padded_v[1:-1, :-2]
            + padded_v[1:-1, 2:]
        )
        count = (
            padded_k[:-2, 1:-1]
            + padded_k[2:, 1:-1]
            + padded_k[1:-1, :-2]
            + padded_k[1:-1, 2:]
        )
        fillable = ~known & (count > 0)
        out = np.where(fillable, total / np.maximum(count, 1.0), out)
        known = known | fillable
    return out


def recentre(cell: np.ndarray) -> np.ndarray:
    """Shift `cell` so its visible bounding box is centred, without clipping."""
    rows, cols = np.nonzero(cell[..., 3] > VISIBLE_ALPHA)
    if rows.size == 0:
        return cell
    shifts = []
    for axis, (lo, hi) in enumerate(((rows.min(), rows.max()), (cols.min(), cols.max()))):
        want = round((cell.shape[axis] - 1 - hi - lo) / 2)
        shifts.append(int(np.clip(want, -lo, cell.shape[axis] - 1 - hi)))
    return np.roll(cell, shifts, axis=(0, 1))


def source_cell(image: Image.Image, cols: int, rows: int, index: int) -> Image.Image:
    """Resample source frame `index` out of its exact (fractional) box."""
    cell_w, cell_h = image.width / cols, image.height / rows
    row, col = divmod(index, cols)
    box = (col * cell_w, row * cell_h, (col + 1) * cell_w, (row + 1) * cell_h)
    return image.resize((CELL_PX, CELL_PX), Image.LANCZOS, box=box)


def build(source: Image.Image, cols: int, rows: int) -> Image.Image:
    first, last = FRAME_WINDOW
    frames = list(range(first, last))
    capacity = OUT_COLS * OUT_ROWS
    if len(frames) > capacity:
        sys.exit(f"{len(frames)} frames do not fit a {OUT_COLS}x{OUT_ROWS} grid")
    # A short window pads by repeating its last frame: the grid arithmetic in
    # `vfx_billboard.wgsl` has no notion of a partial row.
    frames += [frames[-1]] * (capacity - len(frames))

    sheet = Image.new("RGBA", (OUT_COLS * CELL_PX, OUT_ROWS * CELL_PX))
    for slot, frame in enumerate(frames):
        cell = np.asarray(source_cell(source, cols, rows, frame), np.float32) / 255.0
        # Centred before the bleed: `recentre` rolls the cell, so whatever the
        # source left outside the sprite wraps to the far edge — and the bleed
        # is what overwrites every one of those texels.
        cell = recentre(cell)
        alpha = cell[..., 3]
        gray = bleed(luminance(cell[..., :3]), alpha, BLEED_PX)
        packed = np.stack([gray, gray, gray, alpha], -1)
        packed = np.clip(packed * 255.0, 0, 255).round().astype(np.uint8)
        row, col = divmod(slot, OUT_COLS)
        sheet.paste(Image.fromarray(packed, "RGBA"), (col * CELL_PX, row * CELL_PX))
    return sheet


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", help="source flipbook sheet (TGA/PNG)")
    parser.add_argument("out", help="destination atlas PNG")
    parser.add_argument("--cols", type=int, default=5, help="source grid columns")
    parser.add_argument("--rows", type=int, default=5, help="source grid rows")
    args = parser.parse_args()

    source = Image.open(args.source).convert("RGBA")
    build(source, args.cols, args.rows).save(args.out, optimize=True)
    print(f"{args.out}: {OUT_COLS}x{OUT_ROWS} of {CELL_PX} px, frames {FRAME_WINDOW}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
