"""Per-frame RGB diff of two capture directories (same file names), plus optional side-by-side
and difference images. Statistics are MEASURED pixel values; the diff points where to look, it is
not a gate."""

import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image


def main():
    p = argparse.ArgumentParser()
    p.add_argument("before", type=Path)
    p.add_argument("after", type=Path)
    p.add_argument("--sbs", type=Path, help="write side-by-side JPEGs and diff PNGs here")
    p.add_argument("--label", default="")
    args = p.parse_args()
    out = {}
    for a_path in sorted(args.before.glob("*.png")):
        b_path = args.after / a_path.name
        a = np.asarray(Image.open(a_path).convert("RGB"), dtype=np.int16)
        b = np.asarray(Image.open(b_path).convert("RGB"), dtype=np.int16)
        d = np.abs(a - b)
        px = d.max(axis=2)
        out[a_path.name] = {
            "pixels": int(px.size),
            "differing": int(np.count_nonzero(px)),
            "over_8": int(np.count_nonzero(px > 8)),
            "max_channel_delta": int(d.max()),
            "mean_channel_delta": float(d.mean()),
        }
        if args.sbs:
            args.sbs.mkdir(parents=True, exist_ok=True)
            h, w, _ = a.shape
            gap = 8
            sbs = np.full((h, 2 * w + gap, 3), 255, dtype=np.uint8)
            sbs[:, :w] = a.astype(np.uint8)
            sbs[:, w + gap :] = b.astype(np.uint8)
            stem = args.label + a_path.stem
            Image.fromarray(sbs).save(args.sbs / f"{stem}-before-left-after-right.jpg", quality=90)
            if out[a_path.name]["differing"]:
                mask = np.zeros((h, w, 3), dtype=np.uint8)
                mask[..., 0] = np.clip(px * 8, 0, 255)
                grey = (a.mean(axis=2) * 0.35).astype(np.uint8)
                vis = np.stack([np.maximum(grey, mask[..., 0]), grey, grey], axis=2)
                Image.fromarray(vis).save(args.sbs / f"{stem}-difference-8x.png")
    print(json.dumps(out, indent=1))


if __name__ == "__main__":
    main()
