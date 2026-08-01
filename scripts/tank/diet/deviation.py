"""Blender headless: point-to-surface deviation between a reference mesh and an LOD.

Answers "how far does this LOD move the surface", in millimetres, which is the number the
pixel arithmetic in the README turns into a switch distance. Reports BOTH directions,
because they catch different failures:

    test -> ref   the LOD's surface drifting off the authored one (a bulge, a cut corner)
    ref  -> test  an authored feature the LOD DELETED (a boss, a horn) -- invisible to the
                  forward direction, because every surviving LOD point can sit perfectly on
                  the reference while a whole feature is missing

Sampling is area-weighted and deterministic: samples per triangle are proportional to area
and placed on a Halton sequence warped into the triangle, so the same pair of meshes always
yields the same numbers. Every vertex is sampled too -- vertices are where the worst
deviation usually lives.

Usage: blender -b -P deviation.py -- <ref.glb> <test.glb> [target_samples]
"""

import sys

import bmesh
import bpy
from mathutils import Vector
from mathutils.bvhtree import BVHTree

argv = sys.argv[sys.argv.index("--") + 1 :]
REF, TEST = argv[0], argv[1]
NSAMP = int(argv[2]) if len(argv) > 2 else 200000


def load(path, name):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=path, merge_vertices=True)
    ob = [o for o in bpy.context.scene.objects if o.type == "MESH"][0]
    bm = bmesh.new()
    bm.from_mesh(ob.data)
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    verts = [v.co.copy() for v in bm.verts]
    tris = [tuple(v.index for v in f.verts) for f in bm.faces]
    bm.free()
    print(f"[dev] {name}: {len(tris)} tris / {len(verts)} verts  <- {path}")
    return verts, tris


def halton(i, base):
    f, r = 1.0, 0.0
    while i > 0:
        f /= base
        r += f * (i % base)
        i //= base
    return r


def sample(verts, tris, target):
    """Area-weighted deterministic surface samples, plus every vertex."""
    areas = []
    for a, b, c in tris:
        areas.append((verts[b] - verts[a]).cross(verts[c] - verts[a]).length * 0.5)
    total = sum(areas) or 1.0
    pts = list(verts)
    k = 0
    for (a, b, c), ar in zip(tris, areas):
        n = int(target * ar / total)
        va, vb, vc = verts[a], verts[b], verts[c]
        for j in range(n):
            k += 1
            # Square-to-triangle warp keeps the samples uniform over the face.
            u, v = halton(k, 2), halton(k, 3)
            su = u**0.5
            w0, w1, w2 = 1.0 - su, su * (1.0 - v), su * v
            pts.append(va * w0 + vb * w1 + vc * w2)
    return pts, total


def dists(pts, tree):
    out = []
    for p in pts:
        loc, _nor, _idx, d = tree.find_nearest(p)
        out.append(d if loc is not None else 0.0)
    return out


def summarize(tag, ds, span):
    ds = sorted(ds)
    n = len(ds)

    def q(f):
        return ds[min(n - 1, int(f * n))]

    med, p90, p99, worst = q(0.5), q(0.9), q(0.99), ds[-1]
    print(
        f"[dev] {tag:<12} n={n:>7}  median={med * 1000:8.4f} mm  p90={p90 * 1000:8.4f} mm  "
        f"p99={p99 * 1000:8.4f} mm  worst={worst * 1000:8.4f} mm  "
        f"({worst / span * 100:.3f}% of {span * 1000:.1f} mm span)"
    )
    return med, p90, p99, worst


rv, rt = load(REF, "ref ")
tv, tt = load(TEST, "test")

lo = Vector((min(v[i] for v in rv) for i in range(3)))
hi = Vector((max(v[i] for v in rv) for i in range(3)))
span = (hi - lo).length
print(f"[dev] reference bbox {tuple(round(x, 4) for x in (hi - lo))} m, diagonal {span:.4f} m")

ref_tree = BVHTree.FromPolygons([tuple(v) for v in rv], rt, all_triangles=True)
test_tree = BVHTree.FromPolygons([tuple(v) for v in tv], tt, all_triangles=True)

tp, _ = sample(tv, tt, NSAMP)
rp, _ = sample(rv, rt, NSAMP)

f = summarize("test->ref", dists(tp, ref_tree), span)
b = summarize("ref->test", dists(rp, test_tree), span)
worst = max(f[3], b[3])
print(f"[dev] SYMMETRIC worst = {worst * 1000:.4f} mm  ({worst:.7f} m)")
