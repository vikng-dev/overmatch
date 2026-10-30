"""compare.py <caps dir> <out dir>: per-scene changed share (any channel past 12/255) between
before/<scene>.png and after/<scene>.png, side-by-side JPEGs, and every frame as JPEG."""
import glob, os, sys
import numpy as np
from PIL import Image

caps, out = sys.argv[1], sys.argv[2]
for arm in ("before", "after"):
    os.makedirs(f"{out}/{arm}", exist_ok=True)
    for png in sorted(glob.glob(f"{caps}/{arm}/*.png")):
        name = os.path.splitext(os.path.basename(png))[0]
        Image.open(png).convert("RGB").save(f"{out}/{arm}/{name}.jpg", quality=85)
os.makedirs(f"{out}/compare", exist_ok=True)
for png in sorted(glob.glob(f"{caps}/after/*.png")):
    name = os.path.splitext(os.path.basename(png))[0]
    before = f"{caps}/before/{name}.png"
    if not os.path.exists(before):
        continue
    a = np.asarray(Image.open(before).convert("RGB")).astype(np.int16)
    b = np.asarray(Image.open(png).convert("RGB")).astype(np.int16)
    changed = (np.abs(a - b) > 12).any(axis=2).mean()
    print(f"{name}: {100 * changed:.1f} % of pixels changed past 12/255")
    w, h = a.shape[1], a.shape[0]
    side = Image.new("RGB", (w, h))
    side.paste(Image.fromarray(a.astype(np.uint8)).resize((w // 2, h // 2)), (0, h // 4))
    side.paste(Image.fromarray(b.astype(np.uint8)).resize((w // 2, h // 2)), (w // 2, h // 4))
    side.save(f"{out}/compare/{name}.jpg", quality=85)
