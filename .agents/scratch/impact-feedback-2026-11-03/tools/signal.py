#!/usr/bin/env python3
"""Throwaway: the powder cells' r*a signal: quad fraction above each threshold, bbox height/width,
with a 2% row/column occupancy so stray specks do not set the box."""
import subprocess

B = "/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-2"
raw = subprocess.check_output(["magick", f"{B}/assets/vfx/impact_dust.png", "-depth", "8", "rgba:-"])
n = int((len(raw) / 4) ** 0.5)
cell = n // 2
print("atlas", n)
for name, (cx, cy) in {"0": (0, 0), "1": (1, 0), "2": (0, 1)}.items():
    sig = []
    for y in range(cell):
        row = []
        for x in range(cell):
            o = ((y + cy * cell) * n + x + cx * cell) * 4
            row.append(raw[o] / 255 * raw[o + 3] / 255)
        sig.append(row)
    for t in (0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35):
        rows = [sum(v > t for v in r) / cell for r in sig]
        cols = [sum(sig[y][x] > t for y in range(cell)) / cell for x in range(cell)]
        h = sum(r > 0.02 for r in rows) / cell
        w = sum(c > 0.02 for c in cols) / cell
        cover = sum(sum(v > t for v in r) for r in sig) / cell**2
        print(f"cell {name} above {t:.2f}: cover {cover:.2f} height {h:.2f} width {w:.2f}")
