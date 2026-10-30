"""Side-by-side and difference images for each capture pair, and their MEASURED pixel stats.

sbs.py <png-dir> <out-dir> [left right]: for every <png-dir>/<left>/<name>.png with a twin in
<png-dir>/<right>/ (default arms: before, after), writes JPEGs of both frames (quality 92) to
<out-dir>/<left>/ and <out-dir>/<right>/, <out-dir>/side-by-side/<name>.jpg (left arm left, half
size), <out-dir>/difference/<name>.jpg (per-pixel largest channel change x8 in red over the left
frame, half size) and <out-dir>/comparison.json. Every stat is taken on the unmodified PNGs.
"""
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image

pngs, out = Path(sys.argv[1]), Path(sys.argv[2])
left, right = (sys.argv[3], sys.argv[4]) if len(sys.argv) > 4 else ("before", "after")
for sub in (left, right, "side-by-side", "difference"):
    (out / sub).mkdir(parents=True, exist_ok=True)
stats = {}
for left_path in sorted((pngs / left).glob("*.png")):
    right_path = pngs / right / left_path.name
    if not right_path.exists():
        continue
    name = left_path.stem
    a = Image.open(left_path).convert("RGB")
    b = Image.open(right_path).convert("RGB")
    a.save(out / left / f"{name}.jpg", quality=92)
    b.save(out / right / f"{name}.jpg", quality=92)
    w, h = a.size
    half = (w // 2, h // 2)
    sbs = Image.new("RGB", (w, h // 2), (0, 0, 0))
    sbs.paste(a.resize(half), (0, 0))
    sbs.paste(b.resize(half), (w // 2, 0))
    sbs.save(out / "side-by-side" / f"{name}.jpg", quality=90)
    da = np.asarray(a, dtype=np.int16)
    db = np.asarray(b, dtype=np.int16)
    delta = np.abs(da - db).max(axis=2)
    grey = (np.asarray(a.convert("L"), dtype=np.float32) * 0.35).astype(np.uint8)
    vis = np.stack([grey] * 3, axis=-1)
    vis[..., 0] = np.maximum(vis[..., 0], np.clip(delta * 8, 0, 255).astype(np.uint8))
    Image.fromarray(vis).resize(half).save(out / "difference" / f"{name}.jpg", quality=90)
    stats[name] = {
        "pixels": int(delta.size),
        "differing": int(np.count_nonzero(delta)),
        "over_8": int(np.count_nonzero(delta > 8)),
        "over_32": int(np.count_nonzero(delta > 32)),
        "max_channel_delta": int(delta.max()),
        "mean_channel_delta": round(float(np.abs(da - db).mean()), 4),
    }
(out / "comparison.json").write_text(json.dumps(stats, indent=2) + "\n")
print(json.dumps(stats, indent=2))
