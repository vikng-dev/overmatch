"""Count the red (visible-back-face) pixels in each backface-probe render."""

import glob
import sys

from PIL import Image

tot = 0
worst = []
for p in sorted(glob.glob(sys.argv[1])):
    im = Image.open(p).convert("RGB")
    px = im.load()
    w, h = im.size
    n = 0
    for y in range(h):
        for x in range(w):
            r, g, b = px[x, y]
            if r > 150 and g < 80 and b < 80:
                n += 1
    tot += n
    worst.append((n, p, w * h))

worst.sort(reverse=True)
for n, p, area in worst:
    if n:
        print(f"{p}: {n} red px ({100 * n / area:.3f} % of frame)")
print(f"TOTAL red pixels across {len(worst)} frames: {tot}")
