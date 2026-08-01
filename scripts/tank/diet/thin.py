"""Find primitives that would hole out if back-face culling were turned on.

Back-face culling is only safe on geometry that is CLOSED: every edge shared by exactly
two triangles, so the inside is never seen. A single-layer sheet (fender, skirt, flag)
drawn with culling shows nothing from behind.

Per primitive, after welding by position (the Tiger is authored fully split, so raw indices
make every edge look like a boundary):
  boundary   edges with exactly one adjacent triangle
  nonmanif   edges with three or more
  volume     signed volume by the divergence theorem
  solidity   |volume| / (area * mean_extent) - ~0.15..0.5 for a closed solid, ~0 for a sheet
"""

import sys
from collections import defaultdict

import glblib

g = glblib.Glb.load(sys.argv[1])
gl = g.gltf
node_of = {}
for n in gl["nodes"]:
    if n.get("mesh") is not None:
        node_of.setdefault(n["mesh"], []).append(n.get("name"))


def hidden(name):
    return name.endswith("_Collider") or name.endswith("_Ballistic")


def analyse(prim):
    pos = g.read_accessor(prim["attributes"]["POSITION"])
    idx = g.read_accessor(prim["indices"])
    key = {}
    remap = []
    for p in pos:
        k = tuple(round(c, 6) for c in p)
        if k not in key:
            key[k] = len(key)
        remap.append(key[k])
    edges = defaultdict(int)
    vol = 0.0
    area = 0.0
    for i in range(0, len(idx), 3):
        a, b, c = (remap[idx[i]], remap[idx[i + 1]], remap[idx[i + 2]])
        if a == b or b == c or a == c:
            continue
        for e in ((a, b), (b, c), (c, a)):
            edges[tuple(sorted(e))] += 1
        pa, pb, pc = pos[idx[i]], pos[idx[i + 1]], pos[idx[i + 2]]
        vol += (
            pa[0] * (pb[1] * pc[2] - pb[2] * pc[1])
            - pa[1] * (pb[0] * pc[2] - pb[2] * pc[0])
            + pa[2] * (pb[0] * pc[1] - pb[1] * pc[0])
        ) / 6.0
        ux, uy, uz = (pb[j] - pa[j] for j in range(3))
        vx, vy, vz = (pc[j] - pa[j] for j in range(3))
        cx, cy, cz = uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx
        area += 0.5 * (cx * cx + cy * cy + cz * cz) ** 0.5
    boundary = sum(1 for v in edges.values() if v == 1)
    nonman = sum(1 for v in edges.values() if v > 2)
    ext = [max(p[j] for p in pos) - min(p[j] for p in pos) for j in range(3)]
    mean_ext = sum(ext) / 3
    solidity = abs(vol) / (area * mean_ext) if area * mean_ext else 0.0
    return boundary, nonman, len(edges), abs(vol), area, solidity


print(f"{'node':<26} {'mat':<24} {'tris':>6} {'bnd':>5} {'nonm':>5} {'solidity':>9}  verdict")
rows = []
for mi, m in enumerate(gl["meshes"]):
    names = node_of.get(mi, ["<orphan>"])
    if all(hidden(n or "") for n in names):
        continue
    for prim in m["primitives"]:
        b, nm, ne, vol, area, sol = analyse(prim)
        mat = prim.get("material")
        mname = gl["materials"][mat].get("name") if mat is not None else "-"
        risky = sol < 0.02 or (b / ne if ne else 0) > 0.10
        rows.append((names[0], mname, mat, glblib.tri_count(g, prim), b, nm, sol, risky))

for r in sorted(rows, key=lambda r: (r[7], -r[3]), reverse=True):
    print(
        f"{r[0]:<26} {str(r[1]):<24} {r[3]:>6} {r[4]:>5} {r[5]:>5} {r[6]:>9.4f}  "
        f"{'THIN/OPEN - keep doubleSided' if r[7] else 'closed - cull ok'}"
    )

risky_mats = sorted({r[2] for r in rows if r[7] and r[2] is not None})
print()
print(f"materials touching thin/open geometry: {risky_mats}")
