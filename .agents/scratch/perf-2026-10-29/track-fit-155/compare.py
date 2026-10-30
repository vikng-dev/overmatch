"""Compare unmodified capture PNGs, including every RGB pixel in a requested belt rectangle.

Requires Pillow and NumPy. Coordinates and tolerance are capture configuration (ASSUMED),
not tank-specific behavior or a system gate. All output statistics are MEASURED pixel values.
"""

import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image


def stats(delta, tolerance):
    pixel = delta.max(axis=2)
    return {
        "pixels": int(pixel.size),
        "max_channel_delta": int(delta.max()),
        "mean_channel_delta": float(delta.mean()),
        "p99_pixel_delta": float(np.percentile(pixel, 99)),
        "pixels_over_tolerance": int(np.count_nonzero(pixel > tolerance)),
        "percent_within_tolerance": float(100 * np.mean(pixel <= tolerance)),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("--crop", required=True, help="left,top,right,bottom, excluding right/bottom")
    parser.add_argument("--tolerance", type=int, default=8)
    args = parser.parse_args()
    left, top, right, bottom = map(int, args.crop.split(","))
    results = {"tolerance": args.tolerance, "crop": [left, top, right, bottom], "frames": {}}
    before = sorted(args.before.glob("*.png"))
    assert before, "no baseline captures"
    assert {p.name for p in before} == {p.name for p in args.after.glob("*.png")}
    for path in before:
        a = np.asarray(Image.open(path).convert("RGB"), dtype=np.int16)
        b = np.asarray(Image.open(args.after / path.name).convert("RGB"), dtype=np.int16)
        assert a.shape == b.shape
        assert 0 <= left < right <= a.shape[1] and 0 <= top < bottom <= a.shape[0]
        delta = np.abs(a - b)
        results["frames"][path.name] = {
            "whole_frame": stats(delta, args.tolerance),
            "belt_rectangle": stats(delta[top:bottom, left:right], args.tolerance),
        }
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
