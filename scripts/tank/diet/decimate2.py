"""Blender headless: weld -> planar dissolve -> quadric collapse to a triangle budget.

The planar pass spends none of the budget on flat plate faces (they dissolve exactly),
so the collapse pass only has to approximate the curved features (pin bosses, guide horn).

Usage: blender -b -P decimate2.py -- <in.glb> <out.glb> <target_tris> [smooth_deg] [planar_deg]
"""

import sys

import bmesh
import bpy

argv = sys.argv[sys.argv.index("--") + 1 :]
IN, OUT, TARGET = argv[0], argv[1], int(argv[2])
ANGLE = float(argv[3]) if len(argv) > 3 else 30.0
PLANAR = float(argv[4]) if len(argv) > 4 else 1.0
D2R = 3.141592653589793 / 180.0

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=IN, merge_vertices=True)
objs = [o for o in bpy.context.scene.objects if o.type == "MESH"]
assert len(objs) == 1
ob = objs[0]
bpy.context.view_layer.objects.active = ob
ob.select_set(True)


def counts(o):
    dg = bpy.context.evaluated_depsgraph_get()
    ev = o.evaluated_get(dg)
    d = ev.to_mesh()
    bm = bmesh.new()
    bm.from_mesh(d)
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    t, v = len(bm.faces), len(bm.verts)
    bm.free()
    ev.to_mesh_clear()
    return t, v


print("[dec2] imported: %d tris / %d verts" % counts(ob))

bpy.ops.object.mode_set(mode="EDIT")
bm = bmesh.from_edit_mesh(ob.data)
bmesh.ops.remove_doubles(bm, verts=bm.verts[:], dist=1e-5)
bmesh.update_edit_mesh(ob.data)
bpy.ops.mesh.select_all(action="SELECT")
bpy.ops.mesh.quads_convert_to_tris(quad_method="BEAUTY", ngon_method="BEAUTY")
# Planar pass: dissolve coplanar triangles into ngons, then re-triangulate. Flat faces
# come back as a minimal fan instead of the authored grid.
bpy.ops.mesh.dissolve_limited(angle_limit=PLANAR * D2R, delimit={"UV"})
bpy.ops.mesh.select_all(action="SELECT")
bpy.ops.mesh.quads_convert_to_tris(quad_method="BEAUTY", ngon_method="BEAUTY")
bpy.ops.object.mode_set(mode="OBJECT")
print("[dec2] welded+planar: %d tris / %d verts" % counts(ob))

mod = ob.modifiers.new("Decimate", "DECIMATE")
mod.decimate_type = "COLLAPSE"
mod.use_collapse_triangulate = True

lo, hi, best = 0.0, 1.0, None
for _ in range(24):
    mid = (lo + hi) / 2
    mod.ratio = mid
    bpy.context.view_layer.update()
    t, v = counts(ob)
    if t <= TARGET:
        best = (mid, t, v)
        lo = mid
    else:
        hi = mid
    if best and TARGET * 0.94 <= best[1] <= TARGET:
        break

if best is None:
    # The collapse floor is above the budget. Report it rather than silently shipping the
    # undecimated mesh — this is the outcome `probe.py` explains, and on the MG barrel it is
    # the outcome that actually happened.
    mod.ratio = 0.001
    bpy.context.view_layer.update()
    t, v = counts(ob)
    raise SystemExit(
        f"[dec2] cannot reach {TARGET} tris: the collapse floor is {t} tris / {v} verts. "
        f"Run probe.py on this mesh - a boundary loop or a shell count is holding it up."
    )
mod.ratio = best[0]
bpy.context.view_layer.update()
print("[dec2] ratio=%.5f -> %d tris / %d verts" % best)
bpy.ops.object.modifier_apply(modifier=mod.name)

bpy.ops.object.shade_smooth()
try:
    bpy.ops.object.shade_smooth_by_angle(angle=ANGLE * D2R)
except AttributeError:
    pass

print("[dec2] final: %d tris / %d verts" % counts(ob))
bpy.ops.export_scene.gltf(
    filepath=OUT,
    export_format="GLB",
    export_apply=True,
    export_normals=True,
    export_texcoords=True,
    export_tangents=False,
    export_materials="NONE",
    export_yup=True,
)
print("[dec2] wrote " + OUT)
