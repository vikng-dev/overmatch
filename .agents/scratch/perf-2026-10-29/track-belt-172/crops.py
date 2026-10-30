"""Crop the committed evidence out of the full capture runs, pixel for pixel (no scaling).

Usage: python3 -I crops.py <capture-root> <out-dir>

For each still, the baseline run is the one closer to the candidate (`matrix.py`'s
`closest_cross`): the baseline itself takes more than one picture run to run. Rectangles are
capture configuration (ASSUMED): the belt rectangle of #155 for the near-camera scenes, the tank's
box for the far one.
"""

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

BELT = (235, 225, 1065, 460)
TANK_FAR = (560, 290, 760, 400)
STILLS = [
    ("near", "belts-00012.5000s.png", BELT),
    ("near", "belts-00014.0000s.png", BELT),
    ("far", "belts-00014.0000s.png", TANK_FAR),
    ("xray", "belts-00012.5000s.png", BELT),
    ("printed", "belts-00016.0000s.png", BELT),
]


def main():
    root, out = Path(sys.argv[1]), Path(sys.argv[2])
    matrix = json.loads((root / "matrix.json").read_text())
    for scene, frame, rect in STILLS:
        pairs = matrix[f"{scene}/{frame}"]["pairs"]
        baseline = min(("before", "before2"), key=lambda b: pairs[f"{b}-after"]["pixels_differing"])
        stem = f"{scene}-{frame.removeprefix('belts-').removesuffix('s.png')}"
        crops = {}
        for tag, run in (("before", baseline), ("after", "after")):
            image = Image.open(root / run / scene / frame).convert("RGB").crop(rect)
            image.save(out / f"{stem}-{tag}.png")
            crops[tag] = np.asarray(image, dtype=np.int16)
        delta = np.clip(np.abs(crops["after"] - crops["before"]) * 8, 0, 255).astype(np.uint8)
        Image.fromarray(delta).save(out / f"{stem}-difference-8x.png")
        print(stem, "baseline run:", baseline)


if __name__ == "__main__":
    main()
