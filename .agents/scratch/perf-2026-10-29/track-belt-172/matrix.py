"""Pairwise whole-frame pixel comparison of two baseline and two candidate capture runs.

Usage: python3 -I matrix.py <root>   (root holds before/, before2/, after/, after2/)

The baseline itself differs run to run (asynchronous asset loading moves the frame the rigs bind
on), so every candidate frame is judged against the baseline-vs-baseline control. Requires Pillow
and NumPy. All output statistics are MEASURED pixel values; the 8/255 tolerance is the #155
comparison's (ASSUMED).
"""

import itertools
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

RUNS = ["before", "before2", "after", "after2"]


def load(path):
    return np.asarray(Image.open(path).convert("RGB"), dtype=np.int16)


def stats(a, b):
    delta = np.abs(a - b).max(axis=2)
    return {
        "pixels_differing": int(np.count_nonzero(delta)),
        "pixels_over_8": int(np.count_nonzero(delta > 8)),
        "max_channel_delta": int(delta.max()),
    }


def main():
    root = Path(sys.argv[1])
    out = {}
    for scene in sorted(p.name for p in (root / "before").iterdir() if p.is_dir()):
        for frame in sorted(p.name for p in (root / "before" / scene).glob("*.png")):
            images = {run: load(root / run / scene / frame) for run in RUNS}
            pairs = {
                f"{x}-{y}": stats(images[x], images[y])
                for x, y in itertools.combinations(RUNS, 2)
            }
            cross = [pairs[k] for k in pairs if k.startswith("before") and "-after" in k]
            out[f"{scene}/{frame}"] = {
                "baseline_control": pairs["before-before2"],
                "candidate_control": pairs["after-after2"],
                "closest_cross": min(cross, key=lambda s: s["pixels_differing"]),
                "pairs": pairs,
            }
    print(json.dumps(out, indent=2))


if __name__ == "__main__":
    main()
