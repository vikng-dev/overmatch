"""UV integrity of an LOD against its authored source, read from the shipped glb bytes.

A decimator can leave the geometry defensible and still ruin the texture: dissolving a face
across a UV seam welds two islands together and drags the albedo over the join, and a
collapse can fold a triangle to zero UV area so it samples a single texel. Neither shows up
in a triangle count or a point-to-surface distance.

Four checks, no dependencies:

  1. TEXCOORD_0 present, and its bounding box against the source's -- a mapping that grew
     outside the authored range is a mapping that moved.
  2. UV-degenerate triangles: zero (or near-zero) area in UV space. Each one is a triangle
     that samples one texel, which reads as a flat smear.
  3. UV stretch: the longest UV edge on any triangle, against the source's longest. This is
     the check that actually catches a face dissolved across a seam -- such a face spans two
     islands, so its UV edge is a large fraction of the map while its world edge is small.
     Judged RELATIVE to the source, because the authored mesh has its own worst case and the
     question is whether the LOD made it worse, not whether it is zero.
  4. Anchored UVs: the planar dissolve never MOVES a vertex, so every vertex it keeps should
     still be an authored position carrying its authored UV. Reported as a distribution
     rather than a single worst: a handful of positions carry several authored UVs (they are
     on an island boundary) and nearest-UV matching there is ambiguous by construction, so
     one large delta is not evidence of damage -- check 3 is. A collapse tier scores low on
     this check by construction; that is why it is reported and not asserted.

Usage: uvcheck.py <ref.glb> <test.glb> [pos_tol]
"""

import sys

import glblib

REF, TEST = sys.argv[1], sys.argv[2]
TOL = float(sys.argv[3]) if len(sys.argv) > 3 else 1e-6


def read(path):
    g = glblib.Glb.load(path)
    prim = g.gltf["meshes"][0]["primitives"][0]
    attrs = prim["attributes"]
    if "TEXCOORD_0" not in attrs:
        raise SystemExit(f"FAIL {path}: no TEXCOORD_0 -- the asset ships unmapped")
    return (
        g.read_accessor(attrs["POSITION"]),
        g.read_accessor(attrs["TEXCOORD_0"]),
        g.read_accessor(prim["indices"]),
        g.gltf["meshes"][0].get("name"),
    )


rp, ruv, ridx, rname = read(REF)
tp, tuv, tidx, tname = read(TEST)
print(f"ref  '{rname}': {len(ridx) // 3} tris / {len(rp)} verts")
print(f"test '{tname}': {len(tidx) // 3} tris / {len(tp)} verts")


def bbox(uv):
    return (
        (min(u for u, _ in uv), min(v for _, v in uv)),
        (max(u for u, _ in uv), max(v for _, v in uv)),
    )


rb, tb = bbox(ruv), bbox(tuv)
print(f"1) uv bbox ref  min={tuple(round(x, 5) for x in rb[0])} max={tuple(round(x, 5) for x in rb[1])}")
print(f"   uv bbox test min={tuple(round(x, 5) for x in tb[0])} max={tuple(round(x, 5) for x in tb[1])}")
grew = max(
    max(rb[0][i] - tb[0][i] for i in range(2)),
    max(tb[1][i] - rb[1][i] for i in range(2)),
)
print(f"   test uv range exceeds ref by at most {grew:.6f} uv  {'OK' if grew <= 1e-5 else 'CHECK'}")


def degenerate(uv, idx):
    n = 0
    worst = 0.0
    for i in range(0, len(idx), 3):
        a, b, c = uv[idx[i]], uv[idx[i + 1]], uv[idx[i + 2]]
        area = abs((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])) * 0.5
        if area <= 1e-12:
            n += 1
        worst = max(worst, area)
    return n, worst


rd, _ = degenerate(ruv, ridx)
td, _ = degenerate(tuv, tidx)
print(f"2) uv-degenerate tris  ref={rd}/{len(ridx) // 3}  test={td}/{len(tidx) // 3}  "
      f"{'OK' if td <= rd else 'CHECK'}")


def max_uv_edge(uv, idx):
    def d(a, b):
        return ((a[0] - b[0]) ** 2 + (a[1] - b[1]) ** 2) ** 0.5

    out = []
    for i in range(0, len(idx), 3):
        a, b, c = uv[idx[i]], uv[idx[i + 1]], uv[idx[i + 2]]
        out.append(max(d(a, b), d(b, c), d(c, a)))
    out.sort()
    return out


re_, te_ = max_uv_edge(ruv, ridx), max_uv_edge(tuv, tidx)
print(
    f"3) longest uv edge  ref p50={re_[len(re_) // 2]:.5f} worst={re_[-1]:.5f}   "
    f"test p50={te_[len(te_) // 2]:.5f} worst={te_[-1]:.5f}   "
    f"{'OK' if te_[-1] <= re_[-1] * 1.05 else 'CHECK'}"
)

# 4) anchored UVs
buckets = {}
for p, uv in zip(rp, ruv):
    key = tuple(round(c / TOL) for c in p)
    buckets.setdefault(key, []).append(uv)

anchored, deltas = 0, []
for p, uv in zip(tp, tuv):
    key = tuple(round(c / TOL) for c in p)
    cands = buckets.get(key)
    if not cands:
        continue
    anchored += 1
    deltas.append(min(max(abs(uv[0] - c[0]), abs(uv[1] - c[1])) for c in cands))

deltas.sort()
pct = 100.0 * anchored / len(tp)
exact = sum(1 for d in deltas if d <= 1e-6)
print(f"4) test verts on an authored position: {anchored}/{len(tp)} ({pct:.1f}%)")
print(
    f"   of those, uv matches exactly: {exact}/{len(deltas)} "
    f"({100.0 * exact / max(1, len(deltas)):.2f}%)  worst={deltas[-1] if deltas else 0:.6f}"
)
