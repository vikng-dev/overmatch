"""Compare two capture directories frame by frame, every RGBA channel, no tolerance.

Requires Pillow and NumPy. Prints one JSON object: per frame, the count of pixels that differ in any
channel and the largest channel difference; totals at the end. Both directories must hold the same
frame names.
"""

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image


def main():
    a_dir, b_dir = Path(sys.argv[1]), Path(sys.argv[2])
    a_names = sorted(p.name for p in a_dir.glob("*.png"))
    b_names = sorted(p.name for p in b_dir.glob("*.png"))
    assert a_names, "no frames"
    assert a_names == b_names, "the directories hold different frame sets"
    frames = {}
    for name in a_names:
        a = np.asarray(Image.open(a_dir / name).convert("RGBA"), dtype=np.int16)
        b = np.asarray(Image.open(b_dir / name).convert("RGBA"), dtype=np.int16)
        assert a.shape == b.shape, f"{name}: sizes differ"
        delta = np.abs(a - b)
        frames[name] = {
            "pixels_differing": int(np.count_nonzero(delta.max(axis=2))),
            "max_channel_delta": int(delta.max()),
        }
    print(
        json.dumps(
            {
                "frames": frames,
                "frame_count": len(frames),
                "frames_identical": sum(f["pixels_differing"] == 0 for f in frames.values()),
                "pixels_differing_total": sum(f["pixels_differing"] for f in frames.values()),
                "max_channel_delta": max(f["max_channel_delta"] for f in frames.values()),
            },
            indent=1,
        )
    )


if __name__ == "__main__":
    main()
