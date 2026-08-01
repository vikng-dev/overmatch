"""Rename the single mesh + node of a geometry-only glb, and re-emit it compactly."""

import sys

import glblib

SRC, DST, NAME = sys.argv[1], sys.argv[2], sys.argv[3]
g = glblib.Glb.load(SRC)
g.gltf["meshes"][0]["name"] = NAME
for n in g.gltf["nodes"]:
    n["name"] = NAME
g.gltf["asset"]["generator"] = "overmatch asset-diet (Blender decimate + glb surgery)"
g.sync_buffer_len()
g.save(DST)
p = g.gltf["meshes"][0]["primitives"][0]
print(f"{DST}: mesh '{NAME}' {glblib.tri_count(g, p)} tris / {glblib.vert_count(g, p)} verts")
