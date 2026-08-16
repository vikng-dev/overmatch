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
    Explosion00) — filter taps straddling the sprite edge would drag that grey
    into the visible fringe as a halo. RGB is edge-extended (alpha-bled) out of
    the visible texels across [`BLEED_PX`], and zeroed beyond it so any tap
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
physical. The centre tracked is the SIGNAL centroid (`luminance x alpha`, what
the shader reads), and the shift applied is a degree-[`FIT_DEGREE`] fit of that
track rather than the track itself: the authored climb is smooth, so the fit
carries it while the frame-to-frame centroid noise a changing fireball shape
produces stays out of the shift and is not injected as sprite motion. Both
excursions are measured and printed per run.

Output is KTX2 (UASTC 4x4 + zstd) with a MIP CHAIN — bevy's PNG loader produces
a single mip level, and a blast quad is rendered at every viewer distance, so a
mipless atlas shimmers under minification. The chain is box-filtered and
stopped at [`MIP_SMALLEST`]: a box filter halving a 2^n cell never reaches
across a cell boundary, and the stop keeps the smallest level's cell wide
enough that a bilinear tap at a cell edge stays inside its own frame.

Alpha is left STRAIGHT (the shader reads `tex.r * tex.a`).

Usage:
    uv run scripts/vfx/blast_atlas.py <source.tga> <out.ktx2>
    (needs basisu: brew install basis_universal, or set OVERMATCH_BASISU)
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

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
# How far RGB is edge-extended into the transparent region (texels). One texel
# of the deepest generated mip covers 32 base texels (see MIP_SMALLEST), so a
# shorter bleed would let that level average black RGB into the sprite fringe.
BLEED_PX = 32
# Alpha above which a texel counts as sprite for the anti-clipping bounding box.
# Low enough to include the smoke fringe, high enough to ignore the tail of the
# source's own antialiasing.
VISIBLE_ALPHA = 0.03
# Degree of the polynomial fit through the centroid track (see the module doc).
# The authored motion is a single smooth climb; a quadratic carries it and
# rejects everything faster.
FIT_DEGREE = 2
# Mip-chain floor handed to basisu: MEASURED, this stops the 1024x768 chain at
# 32x24 (6 levels) — an 8 px cell. Deeper levels are where a bilinear tap at a
# cell edge starts reaching into the neighbouring frame, which no amount of
# edge-extend inside a cell can fix.
MIP_SMALLEST = 32


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


def centroid(gray: np.ndarray, alpha: np.ndarray) -> np.ndarray:
    """(row, col) centroid of the SIGNAL the shader reads (`luminance x alpha`).

    A bounding box tracks the outermost wisp of smoke; this tracks where the
    fireball's brightness actually is, which is what the eye locks onto.
    """
    weight = gray * alpha
    total = weight.sum()
    if total <= 0:
        return np.array([(gray.shape[0] - 1) / 2.0, (gray.shape[1] - 1) / 2.0])
    axes = [np.arange(size, dtype=np.float64) for size in weight.shape]
    return np.array([weight.sum(1) @ axes[0], weight.sum(0) @ axes[1]]) / total


def visible_bounds(alpha: np.ndarray) -> list[tuple[int, int]]:
    """Per-axis (lo, hi) of the visible texels — the anti-clipping limit on a shift."""
    rows, cols = np.nonzero(alpha > VISIBLE_ALPHA)
    if rows.size == 0:
        return [(0, size - 1) for size in alpha.shape]
    return [(int(rows.min()), int(rows.max())), (int(cols.min()), int(cols.max()))]


def excursion(track: np.ndarray, centre: float) -> tuple[float, float]:
    """(max offset from `centre`, max frame-to-frame step) of a centroid track, px."""
    offset = float(np.abs(track - centre).max())
    step = float(np.abs(np.diff(track, axis=0)).max()) if len(track) > 1 else 0.0
    return offset, step


def source_cell(image: Image.Image, cols: int, rows: int, index: int) -> Image.Image:
    """Resample source frame `index` out of its exact (fractional) box."""
    cell_w, cell_h = image.width / cols, image.height / rows
    row, col = divmod(index, cols)
    box = (col * cell_w, row * cell_h, (col + 1) * cell_w, (row + 1) * cell_h)
    return image.resize((CELL_PX, CELL_PX), Image.LANCZOS, box=box)


def build(source: Image.Image, cols: int, rows: int) -> tuple[Image.Image, str]:
    """The atlas sheet, plus the one-line centroid measurement of this build."""
    first, last = FRAME_WINDOW
    frames = list(range(first, last))
    capacity = OUT_COLS * OUT_ROWS
    if len(frames) > capacity:
        sys.exit(f"{len(frames)} frames do not fit a {OUT_COLS}x{OUT_ROWS} grid")

    cells = [
        np.asarray(source_cell(source, cols, rows, frame), np.float32) / 255.0
        for frame in frames
    ]
    grays = [luminance(cell[..., :3]) for cell in cells]
    alphas = [cell[..., 3] for cell in cells]

    # The authored track, and the smooth part of it the shift is allowed to cancel.
    centre = (CELL_PX - 1) / 2.0
    track = np.stack([centroid(g, a) for g, a in zip(grays, alphas)])
    steps = np.arange(len(track), dtype=np.float64)
    fit = np.stack(
        [
            np.polyval(np.polyfit(steps, track[:, axis], FIT_DEGREE), steps)
            for axis in range(2)
        ],
        axis=-1,
    )

    sheet = Image.new("RGBA", (OUT_COLS * CELL_PX, OUT_ROWS * CELL_PX))
    placed = []
    for index, (gray, alpha) in enumerate(zip(grays, alphas)):
        shift = tuple(
            int(np.clip(round(centre - fit[index, axis]), -lo, CELL_PX - 1 - hi))
            for axis, (lo, hi) in enumerate(visible_bounds(alpha))
        )
        gray, alpha = (np.roll(plane, shift, (0, 1)) for plane in (gray, alpha))
        # Bled AFTER the roll: the roll wraps whatever the source left outside
        # the sprite to the far edge, and the bleed is what overwrites it.
        packed = np.stack([bleed(gray, alpha, BLEED_PX)] * 3 + [alpha], -1)
        placed.append(centroid(gray, alpha))
        row, col = divmod(index, OUT_COLS)
        sheet.paste(
            Image.fromarray(np.clip(packed * 255.0, 0, 255).round().astype(np.uint8)),
            (col * CELL_PX, row * CELL_PX),
        )

    # A short window pads by repeating its last frame: the grid arithmetic in
    # `vfx_billboard.wgsl` has no notion of a partial row.
    last_cell = sheet.crop(
        (
            ((len(frames) - 1) % OUT_COLS) * CELL_PX,
            ((len(frames) - 1) // OUT_COLS) * CELL_PX,
            (((len(frames) - 1) % OUT_COLS) + 1) * CELL_PX,
            (((len(frames) - 1) // OUT_COLS) + 1) * CELL_PX,
        )
    )
    for slot in range(len(frames), capacity):
        row, col = divmod(slot, OUT_COLS)
        sheet.paste(last_cell, (col * CELL_PX, row * CELL_PX))

    was = excursion(track, centre)
    now = excursion(np.stack(placed), centre)
    report = (
        f"centroid px — authored: {was[0]:.1f} off centre, {was[1]:.1f} per frame; "
        f"residual: {now[0]:.1f} off centre, {now[1]:.1f} per frame"
    )
    return sheet, report


def encode(sheet: Image.Image, out: str) -> None:
    """Write `sheet` as a mipped UASTC KTX2 (see the module doc's mip policy)."""
    basisu = os.environ.get("OVERMATCH_BASISU", "basisu")
    if shutil.which(basisu) is None:
        sys.exit("need basisu: brew install basis_universal (or set OVERMATCH_BASISU)")
    with tempfile.TemporaryDirectory() as work:
        png = Path(work) / "atlas.png"
        sheet.save(png)
        subprocess.run(
            # `-force_alpha`: the sprite IS its alpha; never let the encoder
            # decide the sheet is opaque. `-mip_srgb` (filter in linear light)
            # matches the sRGB-encoded luminance written above.
            [
                basisu, "-uastc", "-uastc_level", "2",
                "-ktx2", "-ktx2_zstandard_level", "9",
                "-force_alpha",
                "-mipmap", "-mip_srgb", "-mip_filter", "box",
                "-mip_smallest", str(MIP_SMALLEST),
                "-file", str(png), "-output_file", out,
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", help="source flipbook sheet (TGA/PNG)")
    parser.add_argument("out", help="destination atlas KTX2")
    parser.add_argument("--cols", type=int, default=5, help="source grid columns")
    parser.add_argument("--rows", type=int, default=5, help="source grid rows")
    args = parser.parse_args()

    source = Image.open(args.source).convert("RGBA")
    sheet, report = build(source, args.cols, args.rows)
    encode(sheet, args.out)
    print(f"{args.out}: {OUT_COLS}x{OUT_ROWS} of {CELL_PX} px, frames {FRAME_WINDOW}")
    print(f"  {report}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
