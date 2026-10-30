"""Side-by-side stills of the #172 follow-up for the owner's eye, and the pixel counts beside them.

Usage: python3 -I sidebyside.py <before-root> <after-root> <out-dir>

Each root holds `capture.sh`'s output: `<scene>/belts-<t>s.png`. For every still this writes
`<scene>-<t>.png`: the before crop over the after crop, then their difference times 8. Crops are
capture configuration (ASSUMED): the near shots' belt rectangle of #155, the tank's box in the mid
and far shots, scaled up 3x nearest-neighbour there so a shoe is more than a pixel. Pixel counts are
over the whole frame, unscaled: pixels differing at all, pixels differing by more than 8/255 in any
channel, and the largest channel delta. They say where to look; they are not a gate.
"""

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

BELT = (235, 225, 1065, 460)
SCENES = {
    "near": (BELT, 1),
    "xray": (BELT, 1),
    "mid": ((540, 280, 880, 400), 3),
    "far": ((560, 290, 760, 400), 3),
}
TIMES = {"00012.5000": "standing", "00014.0000": "moving"}


def main():
    before, after, out = (Path(arg) for arg in sys.argv[1:4])
    out.mkdir(parents=True, exist_ok=True)
    counts = {}
    for scene, (rect, scale) in SCENES.items():
        for time, state in TIMES.items():
            name = f"belts-{time}s.png"
            frames = [np.asarray(Image.open(root / scene / name).convert("RGB"), dtype=np.int16)
                      for root in (before, after)]
            delta = np.abs(frames[1] - frames[0]).max(axis=2)
            counts[f"{scene}/{time}"] = {
                "state": state,
                "pixels_differing": int((delta > 0).sum()),
                "pixels_over_8": int((delta > 8).sum()),
                "max_channel_delta": int(delta.max()),
            }
            crops = [Image.fromarray(frame.astype(np.uint8)).crop(rect) for frame in frames]
            diff = np.clip(np.abs(np.asarray(crops[1], np.int16) - np.asarray(crops[0], np.int16))
                           * 8, 0, 255).astype(np.uint8)
            panels = [*crops, Image.fromarray(diff)]
            panels = [p.resize((p.width * scale, p.height * scale), Image.NEAREST) for p in panels]
            label = 18
            sheet = Image.new("RGB", (panels[0].width, 3 * (panels[0].height + label)), "white")
            draw = ImageDraw.Draw(sheet)
            for row, (panel, title) in enumerate(zip(panels, ("before (df4fc9a)", "after",
                                                              "difference x8"))):
                y = row * (panel.height + label)
                draw.text((4, y + 3), f"{scene}, {float(time):g} s, {state}: {title}", fill="black")
                sheet.paste(panel, (0, y + label))
            sheet.save(out / f"{scene}-{time}.png")
    (out / "counts.json").write_text(json.dumps(counts, indent=1) + "\n")
    for key, value in counts.items():
        print(key, value)


if __name__ == "__main__":
    main()
