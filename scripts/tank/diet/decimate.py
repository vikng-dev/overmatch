"""Blender headless: import a geometry-only glb, weld, decimate to a triangle budget, export.

Usage: blender -b -P decimate.py -- <in.glb> <out.glb> <target_tris> [smooth_angle_deg]
"""

import os
import sys

import bmesh
import bpy

argv = sys.argv[sys.argv.index("--") + 1 :]
IN, OUT, TARGET = argv[0], argv[1], int(argv[2])
ANGLE = float(argv[3]) if len(argv) > 3 else 30.0

# clean scene
bpy.ops.wm.read_factory_settings(use_empty=True)

bpy.ops.import_scene.gltf(filepath=IN, merge_vertices=True)
objs = [o for o in bpy.context.scene.objects if o.type == "MESH"]
assert len(objs) == 1, f"expected 1 mesh, got {len(objs)}"
ob = objs[0]
bpy.context.view_layer.objects.active = ob
ob.select_set(True)


def counts(o):
    d = o.evaluated_get(bpy.context.evaluated_depsgraph_get()).to_mesh()
    bm = bmesh.new()
    bm.from_mesh(d)
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    t, v = len(bm.faces), len(bm.verts)
    bm.free()
    o.evaluated_get(bpy.context.evaluated_depsgraph_get()).to_mesh_clear()
    return t, v


t0, v0 = counts(ob)
print(f"[decimate] imported (merged): {t0} tris / {v0} verts")

# Triangulate up front so the collapse decimator has a clean triangle field, and weld
# coincident verts that the importer's position merge missed (float slop).
bpy.ops.object.mode_set(mode="EDIT")
bm = bmesh.from_edit_mesh(ob.data)
bmesh.ops.remove_doubles(bm, verts=bm.verts[:], dist=1e-5)
bmesh.update_edit_mesh(ob.data)
bpy.ops.mesh.select_all(action="SELECT")
bpy.ops.mesh.quads_convert_to_tris(quad_method="BEAUTY", ngon_method="BEAUTY")
# The collapse decimator will not collapse a boundary loop, so an open shell sets a hard
# triangle floor. Capping the holes (never visible: they are the ends the parts socket into
# each other by) hands the quadric metric the freedom to go under it.
if os.environ.get("FILL_HOLES"):
    bpy.ops.mesh.select_all(action="SELECT")
    bpy.ops.mesh.fill_holes(sides=int(os.environ["FILL_HOLES"]))
    bpy.ops.mesh.select_all(action="SELECT")
    bpy.ops.mesh.quads_convert_to_tris(quad_method="BEAUTY", ngon_method="BEAUTY")
bpy.ops.object.mode_set(mode="OBJECT")

t1, v1 = counts(ob)
print(f"[decimate] welded+triangulated: {t1} tris / {v1} verts")

# Binary-search the decimate ratio so the applied result lands at/under the budget.
mod = ob.modifiers.new("Decimate", "DECIMATE")
mod.decimate_type = "COLLAPSE"
mod.use_collapse_triangulate = True

lo, hi = 0.0, 1.0
best = None
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
    # The budget is under the collapse FLOOR, not merely hard to hit. Blender will not
    # collapse a boundary loop, so an open or fragmented mesh stops dead at a triangle count
    # no ratio gets under. Say which, because the fix is a different budget or a different
    # mesh, never a smaller ratio.
    mod.ratio = 0.001
    bpy.context.view_layer.update()
    t, v = counts(ob)
    raise SystemExit(
        f"[decimate] cannot reach {TARGET} tris: the collapse floor is {t} tris / {v} verts. "
        f"Run probe.py on this mesh to see the shell and boundary-edge counts holding it up."
    )
mod.ratio = best[0]
bpy.context.view_layer.update()
print(f"[decimate] ratio={best[0]:.5f} -> {best[1]} tris / {best[2]} verts")

bpy.ops.object.modifier_apply(modifier=mod.name)

# Regenerate shading: the collapse decimator drops custom split normals, so rebuild hard
# edges from the dihedral angle rather than shipping a fully-smooth shoe.
bpy.ops.object.shade_smooth()
try:
    bpy.ops.object.shade_smooth_by_angle(angle=ANGLE * 3.14159265358979 / 180.0)
except AttributeError:
    for e in ob.data.edges:
        e.use_edge_sharp = False
    ob.data.use_auto_smooth = True

t2, v2 = counts(ob)
print(f"[decimate] final: {t2} tris / {v2} verts")

bpy.ops.export_scene.gltf(
    filepath=OUT,
    export_format="GLB",
    use_selection=False,
    export_apply=True,
    export_normals=True,
    export_texcoords=True,
    export_tangents=False,
    export_materials="NONE",
    export_yup=True,
)
print(f"[decimate] wrote {OUT}")
