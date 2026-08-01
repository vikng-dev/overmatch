"""probe.py — what triangle count can the collapse decimator actually REACH on this mesh?

    blender -b -P probe.py -- <geometry.glb>

Blender's collapse decimator will not collapse a boundary loop, so an open or fragmented
mesh has a hard floor that no ratio gets under. Printing the shell/boundary counts next to
the result at seven ratios is what turned "the barrel will not go under 1 000" from a
guess into the measurement in the MG commit: 0.12, 0.06, 0.03, 0.01 and 0.001 all return
exactly 1 139 triangles.
"""

import sys

import bmesh
import bpy

argv = sys.argv[sys.argv.index("--") + 1 :]
IN = argv[0]

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=IN, merge_vertices=True)
ob = [o for o in bpy.context.scene.objects if o.type == "MESH"][0]
bpy.context.view_layer.objects.active = ob
ob.select_set(True)


def counts():
    dg = bpy.context.evaluated_depsgraph_get()
    ev = ob.evaluated_get(dg)
    d = ev.to_mesh()
    bm = bmesh.new()
    bm.from_mesh(d)
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    t, v = len(bm.faces), len(bm.verts)
    bm.free()
    ev.to_mesh_clear()
    return t, v


bpy.ops.object.mode_set(mode="EDIT")
bm = bmesh.from_edit_mesh(ob.data)
bmesh.ops.remove_doubles(bm, verts=bm.verts[:], dist=1e-5)
bmesh.update_edit_mesh(ob.data)
bpy.ops.mesh.select_all(action="SELECT")
bpy.ops.mesh.quads_convert_to_tris()
bpy.ops.object.mode_set(mode="OBJECT")
print("[probe] welded: %d tris / %d verts" % counts())

# how fragmented is it?
bm = bmesh.new()
bm.from_mesh(ob.data)
seen = set()
shells = 0
for f in bm.faces:
    if f.index in seen:
        continue
    shells += 1
    stack = [f]
    while stack:
        cur = stack.pop()
        if cur.index in seen:
            continue
        seen.add(cur.index)
        for e in cur.edges:
            for nf in e.link_faces:
                if nf.index not in seen:
                    stack.append(nf)
boundary = sum(1 for e in bm.edges if len(e.link_faces) != 2)
print(f"[probe] shells={shells} boundary/non-manifold edges={boundary} of {len(bm.edges)}")
bm.free()

mod = ob.modifiers.new("D", "DECIMATE")
mod.decimate_type = "COLLAPSE"
mod.use_collapse_triangulate = True
for r in (0.5, 0.25, 0.12, 0.06, 0.03, 0.01, 0.001):
    mod.ratio = r
    bpy.context.view_layer.update()
    t, v = counts()
    print(f"[probe] ratio {r:<6} -> {t} tris / {v} verts")
