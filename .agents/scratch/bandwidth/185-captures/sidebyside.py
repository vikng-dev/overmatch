"""sidebyside.py <before dir> <after dir> <out dir>: for every remote-*.png present in both capture
directories, a JPEG with the before frame on the left and the after frame on the right, each at
full size, labelled; plus every frame as its own JPEG under before/ and after/."""

import glob
import os
import sys

from PIL import Image, ImageDraw

before_dir, after_dir, out = sys.argv[1:4]
for arm, src in (("before", before_dir), ("after", after_dir)):
    os.makedirs(f"{out}/{arm}", exist_ok=True)
    for png in sorted(glob.glob(f"{src}/remote-*.png")):
        name = os.path.splitext(os.path.basename(png))[0]
        Image.open(png).convert("RGB").save(f"{out}/{arm}/{name}.jpg", quality=85)
os.makedirs(f"{out}/compare", exist_ok=True)
for png in sorted(glob.glob(f"{after_dir}/remote-*.png")):
    name = os.path.splitext(os.path.basename(png))[0]
    before = f"{before_dir}/{name}.png"
    if not os.path.exists(before):
        continue
    a = Image.open(before).convert("RGB")
    b = Image.open(png).convert("RGB")
    side = Image.new("RGB", (a.width + b.width, max(a.height, b.height)))
    side.paste(a, (0, 0))
    side.paste(b, (a.width, 0))
    draw = ImageDraw.Draw(side)
    draw.text((12, 12), f"BEFORE (main)  {name}", fill=(255, 255, 255))
    draw.text((a.width + 12, 12), f"AFTER (#185 wins 1-4)  {name}", fill=(255, 255, 255))
    side.save(f"{out}/compare/{name}.jpg", quality=85)
    print(f"{out}/compare/{name}.jpg")
