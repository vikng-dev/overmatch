#!/usr/bin/env python3
"""extract_masks.py — cut a map's surface-weight masks out of the authored terrain TIFF.

    python3 scripts/map/extract_masks.py [--source <tif>] [--map <id>]

THE SOURCE STAYS OUT OF THE REPO, exactly like the terrain texture masters: one 4096x4096 uint16
4-channel TIFF (~113 MB) as the author exported it, default `~/Downloads/terrain_height_1500.tif`
or `$MAP_TERRAIN_TIF`. Its channels are R=height, G=recesses, B=slopes, A=lowlands.

THE OUTPUT is `assets/maps/<id>/terrain_masks.png` — 8-bit RGB, 4096x4096, carrying the three
masks in the order R=recesses (the TIFF's G), G=slopes (its B), B=lowlands (its A). The order is
the contract; the manifest block in `level.json` states the same one.

NO COLOUR MANAGEMENT. These are linear weights, not a picture: an ICC profile or an sRGB chunk
would gamma-shift every weight on decode. The written file is re-read and refused unless its chunk
list is exactly IHDR / IDAT / IEND.

PURE CHANNEL EXTRACTION — no resample, no crop, no flip. The masks share the heightmap's pixel
grid and row order, so they share its extent, sample centres and image axes. That sharing is
CHECKED before anything is written: the TIFF's R channel must equal the shipped
`terrain_height.png` bit for bit, or the masks would be silently misaligned with the terrain the
game runs on.

Quantisation to 8 bits is `round(u16 * 255 / 65535)`; the measured worst-case error over this data
is printed per run.
"""

from __future__ import annotations

import argparse
import os
import struct
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEFAULT_MAP = "kalinovo"
DEFAULT_SOURCE = Path(
    os.environ.get("MAP_TERRAIN_TIF", Path.home() / "Downloads" / "terrain_height_1500.tif")
)
HEIGHTMAP = "terrain_height.png"
MASKS = "terrain_masks.png"

# (source TIFF channel, output channel, meaning) — the whole channel contract, in output order.
CHANNELS = ((1, "R", "recesses"), (2, "G", "slopes"), (3, "B", "lowlands"))

# What a mask PNG may contain. Anything else is colour management or metadata.
ALLOWED_CHUNKS = ("IHDR", "IDAT", "IEND")

INSTALL = (
    "needs tifffile, imagecodecs and pillow — none is a repo dependency:\n"
    "    python3 -m pip install tifffile imagecodecs pillow\n"
    "PIL alone is not enough: it reports this TIFF as RGB and drops the lowlands channel with no "
    "error."
)


def imports():
    """numpy, tifffile and PIL, or the install line. tifffile needs imagecodecs for LZW."""
    try:
        import numpy
        import tifffile
        from PIL import Image
    except ImportError as missing:
        sys.exit("extract_masks: {}\n{}".format(missing, INSTALL))
    return numpy, tifffile, Image


def chunk_names(path: Path) -> list[str]:
    """The PNG's chunk types, in file order."""
    data = path.read_bytes()
    names, at = [], 8
    while at < len(data):
        length = struct.unpack(">I", data[at : at + 4])[0]
        names.append(data[at + 4 : at + 8].decode("ascii", "replace"))
        at += 12 + length
    return names


def main() -> int:
    parser = argparse.ArgumentParser(description="Extract a map's surface-weight masks.")
    parser.add_argument("--source", type=Path, default=DEFAULT_SOURCE, help="the authored TIFF")
    parser.add_argument("--map", default=DEFAULT_MAP, help="map id under assets/maps/")
    arguments = parser.parse_args()

    numpy, tifffile, Image = imports()
    map_dir = REPO / "assets" / "maps" / arguments.map
    if not map_dir.is_dir():
        return fail("no map at {}".format(map_dir))
    if not arguments.source.is_file():
        return fail("no source TIFF at {} — it is not tracked here".format(arguments.source))

    source = tifffile.imread(arguments.source)
    if source.ndim != 3 or source.shape[2] != 4 or source.dtype != numpy.uint16:
        return fail(
            "{} is {} {} — the authored export is 4096x4096x4 uint16".format(
                arguments.source, "x".join(str(n) for n in source.shape), source.dtype
            )
        )

    # THE ALIGNMENT CHECK, before anything is written: a mask grid that does not sit on the
    # heightmap's own pixels is wrong everywhere and looks wrong nowhere.
    heightmap = numpy.array(Image.open(map_dir / HEIGHTMAP))
    if heightmap.shape != source.shape[:2] or heightmap.dtype != numpy.uint16:
        return fail(
            "{} is {} {}, the TIFF is {} uint16".format(
                HEIGHTMAP, heightmap.shape, heightmap.dtype, source.shape[:2]
            )
        )
    differing = int((source[:, :, 0] != heightmap).sum())
    if differing:
        return fail(
            "the TIFF's R channel differs from the shipped {} in {} of {} pixels — the masks would"
            " be misaligned with the terrain the game runs on".format(
                HEIGHTMAP, differing, heightmap.size
            )
        )
    print("aligned ▸ TIFF R == {} over all {} pixels".format(HEIGHTMAP, heightmap.size))

    masks = numpy.empty(source.shape[:2] + (3,), numpy.uint8)
    worst = 0.0
    for out, (channel, name, meaning) in enumerate(CHANNELS):
        full = source[:, :, channel]
        quantised = ((full.astype(numpy.uint32) * 255 + 32767) // 65535).astype(numpy.uint8)
        masks[:, :, out] = quantised
        error = numpy.abs(quantised.astype(numpy.float64) / 255.0 - full / 65535.0).max()
        worst = max(worst, float(error))
        print(
            "channel ▸ {} = TIFF {} ({}): min {} max {} mean {:.2f}".format(
                name, "RGBA"[channel], meaning, quantised.min(), quantised.max(),
                float(quantised.mean()),
            )
        )
    print("quantise ▸ worst 8-bit error {:.5f} of full scale".format(worst))

    out_path = map_dir / MASKS
    Image.fromarray(masks, "RGB").save(out_path, optimize=True)
    names = chunk_names(out_path)
    stray = [name for name in names if name not in ALLOWED_CHUNKS]
    if stray:
        out_path.unlink()
        return fail("the encoder wrote {} — a mask PNG carries no colour management".format(stray))
    written = numpy.array(Image.open(out_path))
    if written.shape != masks.shape or written.dtype != numpy.uint8:
        out_path.unlink()
        return fail("re-read as {} {}, wrote {} uint8".format(written.shape, written.dtype, masks.shape))
    if not (written == masks).all():
        out_path.unlink()
        return fail("the written PNG does not re-read as the extracted masks")
    print(
        "masks   ▸ {} — {}x{} 8-bit RGB, {} bytes, {} IDAT chunk(s) and nothing else".format(
            out_path.relative_to(REPO), masks.shape[1], masks.shape[0],
            out_path.stat().st_size, names.count("IDAT"),
        )
    )
    return 0


def fail(why: str) -> int:
    print("extract_masks ▸ refused: {}".format(why), file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
