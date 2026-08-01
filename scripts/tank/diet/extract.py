"""Extract one mesh primitive from a glb into a minimal standalone glb (geometry only)."""

import struct
import sys

import glblib

SRC, MESH_IDX, OUT = sys.argv[1], int(sys.argv[2]), sys.argv[3]

g = glblib.Glb.load(SRC)
mesh = g.gltf["meshes"][MESH_IDX]
prim = mesh["primitives"][0]
pos = g.read_accessor(prim["attributes"]["POSITION"])
nor = g.read_accessor(prim["attributes"]["NORMAL"])
uv = g.read_accessor(prim["attributes"]["TEXCOORD_0"])
idx = g.read_accessor(prim["indices"])

out = glblib.Glb({}, b"")
out.gltf = {
    "asset": {"version": "2.0"},
    "scene": 0,
    "scenes": [{"nodes": [0]}],
    "nodes": [{"mesh": 0, "name": mesh.get("name", "mesh")}],
    "meshes": [{"name": mesh.get("name", "mesh"), "primitives": []}],
    "buffers": [{"byteLength": 0}],
    "bufferViews": [],
    "accessors": [],
}

pb = b"".join(struct.pack("<3f", *v) for v in pos)
nb = b"".join(struct.pack("<3f", *v) for v in nor)
ub = b"".join(struct.pack("<2f", *v) for v in uv)
ib = b"".join(struct.pack("<I", v) for v in idx)

a_pos = out.add_accessor(
    out.add_bufferview(pb, target=34962),
    5126,
    "VEC3",
    len(pos),
    (
        [min(v[i] for v in pos) for i in range(3)],
        [max(v[i] for v in pos) for i in range(3)],
    ),
)
a_nor = out.add_accessor(out.add_bufferview(nb, target=34962), 5126, "VEC3", len(nor))
a_uv = out.add_accessor(out.add_bufferview(ub, target=34962), 5126, "VEC2", len(uv))
a_idx = out.add_accessor(out.add_bufferview(ib, target=34963), 5125, "SCALAR", len(idx))
out.gltf["meshes"][0]["primitives"] = [
    {
        "attributes": {"POSITION": a_pos, "NORMAL": a_nor, "TEXCOORD_0": a_uv},
        "indices": a_idx,
    }
]
out.sync_buffer_len()
out.save(OUT)
print(f"extracted {mesh.get('name')}: {len(pos)} verts / {len(idx) // 3} tris -> {OUT}")
