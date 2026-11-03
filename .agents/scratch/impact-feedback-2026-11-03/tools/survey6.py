#!/usr/bin/env python3
"""Fourth survey pass: strike sites whose read stands against ground, not the map's dark clear
colour: for each candidate and bearing, how many metres above the strike's ground the terrain behind
it covers from a gun-height eye, with a clear line. Usage: survey4.py <impact-stills dir> <level.json>"""
import json
import math
import sys

S = sys.argv[1]
LEVEL = sys.argv[2]
N = 1024
HALF = 750.0
CELL = 2 * HALF / N
masks = open(f"{S}/masks.rgb", "rb").read()
hbytes = open(f"{S}/height.gray16", "rb").read()


def idx(x, z):
    return min(max(int((x + HALF) / CELL), 0), N - 1), min(max(int((HALF - z) / CELL), 0), N - 1)


def height(x, z):
    c, r = idx(x, z)
    o = (r * N + c) * 2
    return (hbytes[o] << 8 | hbytes[o + 1]) / 65535.0 * 50.0


def weights(x, z):
    c, r = idx(x, z)
    o = (r * N + c) * 3
    R, G, B = masks[o] / 255.0, masks[o + 1] / 255.0, masks[o + 2] / 255.0
    base = max(0.0, 1.0 - min(R + G + B, 1.0))
    total = base + R + G + B
    return base / total, R / total, G / total, B / total


level = json.load(open(LEVEL))
BUCKET = 32.0
grid = {}
for i in level["instances"]:
    x, z = i["translation"][0], i["translation"][2]
    grid.setdefault((int(x // BUCKET), int(z // BUCKET)), []).append((x, z))


def near_object(x, z, r):
    gx, gz = int(x // BUCKET), int(z // BUCKET)
    reach = int(r // BUCKET) + 1
    for i in range(gx - reach, gx + reach + 1):
        for j in range(gz - reach, gz + reach + 1):
            for ox, oz in grid.get((i, j), ()):
                if (ox - x) ** 2 + (oz - z) ** 2 < r * r:
                    return True
    return False


def clear_line(ax, az, ay, bx, bz, by, skip_end=30.0):
    d = math.hypot(bx - ax, bz - az)
    steps = int(d / 4.0)
    for s in range(1, steps):
        t = s / steps
        x, z = ax + (bx - ax) * t, az + (bz - az) * t
        if height(x, z) > ay + (by - ay) * t - 0.3:
            return False
        if t * d > skip_end and (1 - t) * d > skip_end and near_object(x, z, 4.0):
            return False
    return True


def slope(x, z, r=6.0):
    return max(abs(height(x + r, z) - height(x - r, z)), abs(height(x, z + r) - height(x, z - r))) / (2 * r)


def backdrop(ex, ez, ey, x, z, gy, dist):
    """Metres above the strike's ground that terrain behind it covers along this line."""
    dx, dz = (x - ex) / dist, (z - ez) / dist
    best = -1e9
    for s in range(10, 900, 8):
        px, pz = x + dx * s, z + dz * s
        if abs(px) > 749 or abs(pz) > 749:
            break
        best = max(best, (height(px, pz) - ey) / (dist + s))
    return ey + best * dist - gy



for (x, z) in [(55.0, 390.0), (45.0, 385.0), (65.0, 395.0)]:
    gy = height(x, z)
    print(x, z, "ground", round(gy, 1), "w", tuple(round(v, 2) for v in weights(x, z)))
    for deg in range(150, 330, 10):
        dx, dz = math.sin(math.radians(deg)), math.cos(math.radians(deg))
        row = []
        for dist in (500.0, 1000.0):
            ex, ez = x + dx * dist, z + dz * dist
            if abs(ex) > 735 or abs(ez) > 735:
                row.append((dist, "off"))
                continue
            ey = height(ex, ez) + 2.3
            ok = clear_line(ex, ez, ey, x, z, gy + 0.5)
            row.append((dist, ok, round(backdrop(ex, ez, ey, x, z, gy, dist), 1), near_object(ex, ez, 10.0), round(slope(ex, ez), 2)))
        print("  ", deg, row)
