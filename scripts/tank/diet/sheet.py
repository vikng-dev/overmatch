"""Compose labelled contact sheets from the per-variant renders."""

import os
import sys

from PIL import Image, ImageDraw, ImageFont

OUT = sys.argv[1]
TITLE = sys.argv[2]
# remaining args: "collabel:path,path,path" per row, rows separated by args
rows = []
for spec in sys.argv[3:]:
    label, paths = spec.split(":", 1)
    rows.append((label, paths.split(",")))

FONT = None
for cand in (
    "/System/Library/Fonts/SFNSMono.ttf",
    "/System/Library/Fonts/Supplemental/Andale Mono.ttf",
    "/System/Library/Fonts/Menlo.ttc",
):
    if os.path.exists(cand):
        FONT = cand
        break


def font(sz):
    return ImageFont.truetype(FONT, sz) if FONT else ImageFont.load_default()


CELL = int(os.environ.get("CELL", 0))


def load(p):
    im = Image.open(p)
    if CELL and im.size[0] != CELL:
        im = im.resize((CELL, round(CELL * im.size[1] / im.size[0])), Image.LANCZOS)
    return im


cells = [load(p) for _, ps in rows for p in ps]
cw, ch = cells[0].size
cols = max(len(ps) for _, ps in rows)
LAB = 34
TOP = 52
sheet = Image.new("RGB", (cols * cw, TOP + len(rows) * (ch + LAB)), (24, 25, 28))
d = ImageDraw.Draw(sheet)
d.text((14, 14), TITLE, fill=(235, 235, 235), font=font(26))

for r, (label, paths) in enumerate(rows):
    y = TOP + r * (ch + LAB)
    for c, p in enumerate(paths):
        sheet.paste(load(p), (c * cw, y + LAB))
    parts = label.split("|")
    for c in range(len(paths)):
        txt = parts[c] if c < len(parts) else ""
        d.text((c * cw + 12, y + 8), txt, fill=(240, 200, 120), font=font(20))

if OUT.endswith((".jpg", ".jpeg")):
    sheet.save(OUT, quality=93, subsampling=0, optimize=True)
else:
    sheet.save(OUT, optimize=True)
print(f"{OUT}  {sheet.size[0]}x{sheet.size[1]}  {os.path.getsize(OUT) // 1024} KB")
