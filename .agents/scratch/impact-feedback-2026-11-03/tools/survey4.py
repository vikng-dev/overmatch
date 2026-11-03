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


KINDS = {"mud": (3, 0.8), "gravel": (0, 0.8), "dirt": (1, 0.4)}
for kind, (slot, least) in KINDS.items():
    found = []
    for iz in range(-720, 721, 15):
        for ix in range(-720, 721, 15):
            x, z = float(ix), float(iz)
            w = weights(x, z)
            if w[slot] < least or slope(x, z) > 0.2 or near_object(x, z, 25.0):
                continue
            gy = height(x, z)
            per = {500.0: [], 1000.0: []}
            for deg in range(0, 360, 15):
                dx, dz = math.sin(math.radians(deg)), math.cos(math.radians(deg))
                for dist in per:
                    ex, ez = x + dx * dist, z + dz * dist
                    if abs(ex) > 735 or abs(ez) > 735 or slope(ex, ez) > 0.1 or near_object(ex, ez, 10.0):
                        continue
                    ey = height(ex, ez) + 2.3
                    if not clear_line(ex, ez, ey, x, z, gy + 0.5):
                        continue
                    per[dist].append((round(backdrop(ex, ez, ey, x, z, gy, dist), 1), deg))
            if per[500.0] and per[1000.0]:
                b5, d5 = max(per[500.0])
                b10, d10 = max(per[1000.0])
                found.append((round(min(b5, b10), 1), x, z, round(gy, 1), b5, d5, b10, d10, tuple(round(v, 2) for v in w)))
    found.sort(reverse=True)
    print(kind, len(found))
    for f in found[:6]:
        print("  backdrop", f[0], "site", f[1], f[2], "ground", f[3], "b500", f[4], "at", f[5], "b1000", f[6], "at", f[7], "w", f[8])
