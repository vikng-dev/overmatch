"""Compare the follow-up capture script's near stills; counts are MEASURED.

Usage: python3 -I compare.py <shipped-root> <flipped-root> <output>
The crop and times reuse the #172 follow-up's capture configuration (ASSUMED).
"""
import importlib.util
import json
from pathlib import Path
import sys

import numpy as np
from PIL import Image, ImageDraw

spec = importlib.util.spec_from_file_location(
    "followup", Path(__file__).parents[1] / "track-belt-172/followup/sidebyside.py"
)
followup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(followup)

shipped, flipped, out = map(Path, sys.argv[1:])
out.mkdir(parents=True, exist_ok=True)
counts = {}
for time, state in followup.TIMES.items():
    name = f"belts-{time}s.png"
    frames = [Image.open(root / "near" / name).convert("RGB") for root in (shipped, flipped)]
    arrays = [np.asarray(frame).astype(np.int16) for frame in frames]
    delta = np.abs(arrays[0] - arrays[1]).max(axis=2)
    x0, y0, x1, y1 = followup.BELT
    wheel_delta = delta[y0:y1, x0:x1]
    counts[time] = {
        "state": state,
        "frame_pixels_differing": int((delta > 0).sum()),
        "frame_pixels_over_8": int((delta > 8).sum()),
        "frame_max_channel_delta": int(delta.max()),
        "wheel_belt_pixels_differing": int((wheel_delta > 0).sum()),
        "wheel_belt_max_channel_delta": int(wheel_delta.max()),
    }
    for frame, arm in zip(frames, ("shipped", "flipped")):
        frame.save(out / f"near-{time}-{arm}.png")
    panels = [frame.crop(followup.BELT) for frame in frames]
    diff = np.clip(np.abs(arrays[0] - arrays[1]) * 8, 0, 255).astype(np.uint8)
    panels.append(Image.fromarray(diff).crop(followup.BELT))
    width, height = panels[0].size
    sheet = Image.new("RGB", (width * 3, height + 24), "white")
    draw = ImageDraw.Draw(sheet)
    for column, (panel, label) in enumerate(zip(panels, ("shipped edge", "edge flipped locally", "difference x8"))):
        draw.text((column * width + 4, 5), f"{state}: {label}", fill="black")
        sheet.paste(panel, (column * width, 24))
    sheet.save(out / f"near-{time}-side-by-side.png")
(out / "counts.json").write_text(json.dumps(counts, indent=2) + "\n")
print(json.dumps(counts, indent=2))
assert all(c["wheel_belt_pixels_differing"] == 0 for c in counts.values()), counts
