"""Replace one glb primitive's geometry in place from a geometry-only glb.

The mesh name, its index in `meshes`, the node that points at it and the primitive's
MATERIAL are all left untouched — only POSITION / NORMAL / TEXCOORD_0 / indices are
repointed at freshly appended accessors. Nothing in src/ has to change.

Usage: inject.py <target.glb> <mesh_idx> <new_geometry.glb> [<mesh_idx> <new.glb> ...]
"""

import struct
import sys

import glblib

TARGET = sys.argv[1]
pairs = list(zip(sys.argv[2::2], sys.argv[3::2]))

g = glblib.Glb.load(TARGET)
pad = glblib.JSON_PAD

for mesh_ref, newpath in pairs:
    # Accept a mesh NAME as well as an index: indices move when an earlier step garbage
    # collects a duplicate, names do not.
    if mesh_ref.lstrip("-").isdigit():
        mesh_idx = int(mesh_ref)
    else:
        matches = [i for i, m in enumerate(g.gltf["meshes"]) if m.get("name") == mesh_ref]
        assert len(matches) == 1, f"mesh name {mesh_ref!r} matched {len(matches)} meshes"
        mesh_idx = matches[0]
    src = glblib.Glb.load(newpath)
    sp = src.gltf["meshes"][0]["primitives"][0]
    pos = src.read_accessor(sp["attributes"]["POSITION"])
    nor = src.read_accessor(sp["attributes"]["NORMAL"])
    uv = src.read_accessor(sp["attributes"]["TEXCOORD_0"])
    idx = src.read_accessor(sp["indices"])

    prim = g.gltf["meshes"][mesh_idx]["primitives"][0]
    before = (glblib.tri_count(g, prim), glblib.vert_count(g, prim))

    a_pos = g.add_accessor(
        g.add_bufferview(b"".join(struct.pack("<3f", *v) for v in pos), target=34962),
        5126,
        "VEC3",
        len(pos),
        (
            [min(v[i] for v in pos) for i in range(3)],
            [max(v[i] for v in pos) for i in range(3)],
        ),
    )
    a_nor = g.add_accessor(
        g.add_bufferview(b"".join(struct.pack("<3f", *v) for v in nor), target=34962),
        5126,
        "VEC3",
        len(nor),
    )
    a_uv = g.add_accessor(
        g.add_bufferview(b"".join(struct.pack("<2f", *v) for v in uv), target=34962),
        5126,
        "VEC2",
        len(uv),
    )
    # Keep indices 16-bit where they fit: the shoe is far under 65 536 verts and bevy
    # uploads whatever the accessor declares.
    if len(pos) <= 65535:
        ib = b"".join(struct.pack("<H", v) for v in idx)
        ct = 5123
    else:
        ib = b"".join(struct.pack("<I", v) for v in idx)
        ct = 5125
    a_idx = g.add_accessor(g.add_bufferview(ib, target=34963), ct, "SCALAR", len(idx))

    prim["attributes"] = {"POSITION": a_pos, "NORMAL": a_nor, "TEXCOORD_0": a_uv}
    prim["indices"] = a_idx
    after = (glblib.tri_count(g, prim), glblib.vert_count(g, prim))
    name = g.gltf["meshes"][mesh_idx].get("name")
    print(
        f"mesh {mesh_idx} '{name}': {before[0]} tris/{before[1]} verts "
        f"-> {after[0]} tris/{after[1]} verts (material {prim.get('material')})"
    )

g.sync_buffer_len()
g.save(TARGET, json_pad_to=pad)
print(f"wrote {TARGET}")
