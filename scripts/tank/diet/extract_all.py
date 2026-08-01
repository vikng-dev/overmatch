"""Extract every VISIBLE primitive of the tank into one geometry-only glb, world-baked.

Mirrors the visibility rules in src/: `_Collider` / `_Ballistic` nodes are hidden by
src/tank/view.rs, `Link` / `Link_Box` by src/track/link_view.rs (the shoe is instanced
onto the belt instead, so it is re-added once by --with-link).

Each output mesh is named "<node>::<material>" so a render can be traced back.
"""

import math
import os
import struct
import sys

import glblib

SRC, OUT = sys.argv[1], sys.argv[2]
WITH_LINK = "--with-link" in sys.argv
ONLY = None
if "--only" in sys.argv:
    ONLY = set(sys.argv[sys.argv.index("--only") + 1].split(","))
NODES = None
if "--nodes" in sys.argv:
    NODES = set(sys.argv[sys.argv.index("--nodes") + 1].split(","))

g = glblib.Glb.load(SRC)
gl = g.gltf


def mat_mul(a, b):
    return [
        sum(a[r * 4 + k] * b[k * 4 + c] for k in range(4)) for r in range(4) for c in range(4)
    ]


def node_matrix(n):
    if "matrix" in n:  # glTF column-major
        m = n["matrix"]
        return [m[c * 4 + r] for r in range(4) for c in range(4)]
    t = n.get("translation", [0, 0, 0])
    q = n.get("rotation", [0, 0, 0, 1])
    s = n.get("scale", [1, 1, 1])
    x, y, z, w = q
    rot = [
        1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w), 0,
        2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w), 0,
        2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y), 0,
        0, 0, 0, 1,
    ]
    for c in range(3):
        for r in range(3):
            rot[r * 4 + c] *= s[c]
    rot[3], rot[7], rot[11] = t[0], t[1], t[2]
    return [
        rot[0], rot[1], rot[2], t[0],
        rot[4], rot[5], rot[6], t[1],
        rot[8], rot[9], rot[10], t[2],
        0, 0, 0, 1,
    ]


def xform(m, p, w=1.0):
    return tuple(
        m[r * 4 + 0] * p[0] + m[r * 4 + 1] * p[1] + m[r * 4 + 2] * p[2] + m[r * 4 + 3] * w
        for r in range(3)
    )


def hidden(name):
    if os.environ.get("SHOW_HIDDEN"):
        return False
    if name.endswith("_Collider") or name.endswith("_Ballistic"):
        return True
    if name in ("Link_Box",):
        return True
    if name == "Link":
        return not WITH_LINK
    return False


out = glblib.Glb({}, b"")
out.gltf = {
    "asset": {"version": "2.0"},
    "scene": 0,
    "scenes": [{"nodes": []}],
    "nodes": [],
    "meshes": [],
    "buffers": [{"byteLength": 0}],
    "bufferViews": [],
    "accessors": [],
}

IDENT = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]
stack = [(i, IDENT) for i in gl["scenes"][gl.get("scene", 0)]["nodes"]]
total_t = 0
while stack:
    ni, parent = stack.pop()
    n = gl["nodes"][ni]
    world = mat_mul(parent, node_matrix(n))
    name = n.get("name", "?")
    for c in n.get("children", []):
        stack.append((c, world))
    if n.get("mesh") is None or hidden(name):
        continue
    if NODES and name not in NODES:
        continue
    for prim in gl["meshes"][n["mesh"]]["primitives"]:
        mat = prim.get("material")
        mname = gl["materials"][mat].get("name") if mat is not None else "none"
        if ONLY and mname not in ONLY:
            continue
        pos = [xform(world, p) for p in g.read_accessor(prim["attributes"]["POSITION"])]
        nor = [xform(world, p, 0.0) for p in g.read_accessor(prim["attributes"]["NORMAL"])]
        nor = [
            (v[0] / L, v[1] / L, v[2] / L) if (L := math.sqrt(sum(c * c for c in v))) else (0, 1, 0)
            for v in nor
        ]
        uv = g.read_accessor(prim["attributes"]["TEXCOORD_0"])
        idx = g.read_accessor(prim["indices"])
        a_pos = out.add_accessor(
            out.add_bufferview(b"".join(struct.pack("<3f", *v) for v in pos), 34962),
            5126, "VEC3", len(pos),
            (
                [min(v[i] for v in pos) for i in range(3)],
                [max(v[i] for v in pos) for i in range(3)],
            ),
        )
        a_nor = out.add_accessor(
            out.add_bufferview(b"".join(struct.pack("<3f", *v) for v in nor), 34962),
            5126, "VEC3", len(nor),
        )
        a_uv = out.add_accessor(
            out.add_bufferview(b"".join(struct.pack("<2f", *v) for v in uv), 34962),
            5126, "VEC2", len(uv),
        )
        a_idx = out.add_accessor(
            out.add_bufferview(b"".join(struct.pack("<I", v) for v in idx), 34963),
            5125, "SCALAR", len(idx),
        )
        mi = len(out.gltf["meshes"])
        out.gltf["meshes"].append({
            "name": f"{name}::{mname}",
            "primitives": [{
                "attributes": {"POSITION": a_pos, "NORMAL": a_nor, "TEXCOORD_0": a_uv},
                "indices": a_idx,
            }],
        })
        out.gltf["nodes"].append({"mesh": mi, "name": f"{name}::{mname}"})
        out.gltf["scenes"][0]["nodes"].append(len(out.gltf["nodes"]) - 1)
        total_t += len(idx) // 3

out.sync_buffer_len()
out.save(OUT)
print(f"{OUT}: {len(out.gltf['meshes'])} primitives, {total_t} tris")
