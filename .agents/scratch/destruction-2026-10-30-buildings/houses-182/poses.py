"""Capture poses for issue 182: houses of the shipped map, their world frames, terrain heights."""
import json, math, sys
import numpy as np
from PIL import Image

root = sys.argv[1]
level = json.load(open(f"{root}/assets/maps/kalinovo/level.json"))
img = np.array(Image.open(f"{root}/assets/maps/kalinovo/terrain_height.png")).astype(np.float64)
n = img.shape[0]
c0 = 749.81689453
step = 2 * c0 / (n - 1)


def height(x, z):
    col = (x + c0) / step
    row = (c0 - z) / step
    c, r = int(math.floor(col)), int(math.floor(row))
    fc, fr = col - c, row - r
    h = lambda rr, cc: img[min(max(rr, 0), n - 1), min(max(cc, 0), n - 1)] / 65535.0 * 50.0
    return (h(r, c) * (1 - fc) * (1 - fr) + h(r, c + 1) * fc * (1 - fr)
            + h(r + 1, c) * (1 - fc) * fr + h(r + 1, c + 1) * fc * fr)


def rot(q, v):
    x, y, z, w = q
    # v' = q v q*
    u = np.array([x, y, z]); v = np.array(v, dtype=float)
    return 2 * np.dot(u, v) * u + (w * w - np.dot(u, u)) * v + 2 * w * np.cross(u, v)


houses = [i for i in level["instances"] if i["prototype"] == "house_proxy"]


def frame(inst):
    x, _, z = inst["translation"]
    t = np.array([x, height(x, z), z])
    s = inst["scale"][0]
    q = inst["rotation"]
    return lambda p: t + rot(q, np.array(p, dtype=float) * s)


if __name__ == "__main__" and sys.argv[2] == "pose":
    # pose <house id> <local eye x,y,z> <local target x,y,z>: a capture pose in world metres.
    by_id = {i["id"]: i for i in houses}
    f = frame(by_id[sys.argv[3]])
    p = [float(v) for v in sys.argv[4].split(",")]
    q = [float(v) for v in sys.argv[5].split(",")]
    print(",".join(f"{v:.2f}" for v in list(f(p)) + list(f(q))))
elif __name__ == "__main__" and sys.argv[2] == "ground":
    # ground <x,z>: the terrain height there.
    x, z = (float(v) for v in sys.argv[3].split(","))
    print(f"{height(x, z):.3f}")
elif __name__ == "__main__":
    target = np.array([float(a) for a in sys.argv[2].split(",")])
    best = sorted(houses, key=lambda i: (i["translation"][0] - target[0]) ** 2 + (i["translation"][2] - target[1]) ** 2)[:3]
    for inst in best:
        f = frame(inst)
        print(inst["id"], "scale", inst["scale"][0], "origin", np.round(f([0, 0, 0]), 3),
              "east window centre", np.round(f([2.0, 1.46, 1.5]), 3),
              "local +x dir", np.round(f([1, 0, 0]) - f([0, 0, 0]), 3))
