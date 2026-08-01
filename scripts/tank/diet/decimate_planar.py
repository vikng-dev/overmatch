"""Blender headless: weld -> PLANAR dissolve -> (optional collapse) -> triangulate -> export.

The planar ("dissolve") decimator merges faces whose normals differ by less than an angle
limit into ngons, then the export re-triangulates them. Unlike the quadric collapse in
`decimate.py` it never MOVES a vertex: every surviving position is an authored position, so
flat plate faces cost a fan instead of a grid and nothing on a curved feature drifts. That is
why its output is the one Yan called "practically indistinguishable" where collapse output at
a comparable budget was rejected as mangled.

Usage:
    blender -b -P decimate_planar.py -- <in.glb> <out.glb> <angle_deg> [collapse_tris] [smooth_deg]

`collapse_tris` adds a quadric-collapse pass ON TOP of the planar result, for the distance
tiers only. It moves vertices and drops the authored split normals (hence `smooth_deg`, which
rebuilds hard edges from a dihedral angle). Leave it off for anything a player sees up close.

Environment:
    DELIMIT          comma list of UV,SHARP,SEAM,MATERIAL,NORMAL   (default "UV,SHARP")
    ALL_BOUNDARIES   1 to dissolve verts along ngon perimeters too  (default 0)
    WELD             merge-by-distance radius in metres             (default 1e-5)

THE WELD IS NOT OPTIONAL. The glTF importer's `merge_vertices=True` does NOT merge this
mesh -- the shoe arrives fully split at 10 530 verts for 5 552 tris and stays there. A split
vertex means the two faces sharing an edge do not share vertices, so the dissolve has no
shared edge to dissolve across and the pass barely moves the count. `remove_doubles` at 1e-5
takes it to 2 748 verts / 1 closed manifold shell, which is what makes the planar pass work
at all. See the README for the measured before/after.
"""

import os
import sys

import bmesh
import bpy

argv = sys.argv[sys.argv.index("--") + 1 :]
IN, OUT = argv[0], argv[1]
ANGLE = float(argv[2])
COLLAPSE = int(argv[3]) if len(argv) > 3 and argv[3] not in ("-", "") else None
SMOOTH = float(argv[4]) if len(argv) > 4 else 30.0

DELIMIT = {s.strip().upper() for s in os.environ.get("DELIMIT", "UV,SHARP").split(",") if s.strip()}
ALL_BOUNDARIES = os.environ.get("ALL_BOUNDARIES", "0") not in ("0", "", "false", "False")
WELD = float(os.environ.get("WELD", "1e-5"))
D2R = 3.141592653589793 / 180.0

bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=IN, merge_vertices=True)
objs = [o for o in bpy.context.scene.objects if o.type == "MESH"]
assert len(objs) == 1, f"expected 1 mesh, got {len(objs)}"
ob = objs[0]
bpy.context.view_layer.objects.active = ob
ob.select_set(True)


def stats(o):
    """Ngon faces, verts, and the TRIANGLE count the exporter will actually write."""
    dg = bpy.context.evaluated_depsgraph_get()
    ev = o.evaluated_get(dg)
    d = ev.to_mesh()
    faces, verts = len(d.polygons), len(d.vertices)
    bm = bmesh.new()
    bm.from_mesh(d)
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    tris = len(bm.faces)
    bm.free()
    ev.to_mesh_clear()
    return faces, verts, tris


def say(tag, o):
    f, v, t = stats(o)
    print(
        f"[planar] {tag}: {t} tris / {v} verts / {f} faces "
        f"(custom normals: {o.data.has_custom_normals})"
    )
    return f, v, t


say("imported", ob)

# --- weld ------------------------------------------------------------------
# Merge coincident verts. The importer does not do this (see the module docstring), and
# without it the dissolve has no shared edges to work across.
bm = bmesh.new()
bm.from_mesh(ob.data)
bmesh.ops.remove_doubles(bm, verts=bm.verts[:], dist=WELD)
bm.to_mesh(ob.data)
bm.free()
ob.data.update()
say("welded", ob)

# --- planar dissolve -------------------------------------------------------
mod = ob.modifiers.new("Planar", "DECIMATE")
mod.decimate_type = "DISSOLVE"
mod.angle_limit = ANGLE * D2R
mod.delimit = DELIMIT
mod.use_dissolve_boundaries = ALL_BOUNDARIES
bpy.context.view_layer.update()
say(f"planar {ANGLE:g}deg delimit={sorted(DELIMIT)} all_boundaries={ALL_BOUNDARIES}", ob)
bpy.ops.object.modifier_apply(modifier=mod.name)

# --- optional collapse pass (distance tiers only) --------------------------
if COLLAPSE is not None:
    # Triangulate first: the quadric metric wants a clean triangle field, not ngons.
    bm = bmesh.new()
    bm.from_mesh(ob.data)
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    bm.to_mesh(ob.data)
    bm.free()
    ob.data.update()

    mod = ob.modifiers.new("Collapse", "DECIMATE")
    mod.decimate_type = "COLLAPSE"
    mod.use_collapse_triangulate = True
    lo, hi, best = 0.0, 1.0, None
    for _ in range(24):
        mid = (lo + hi) / 2
        mod.ratio = mid
        bpy.context.view_layer.update()
        t = stats(ob)[2]
        if t <= COLLAPSE:
            best = (mid, t)
            lo = mid
        else:
            hi = mid
        if best and COLLAPSE * 0.94 <= best[1] <= COLLAPSE:
            break
    if best is None:
        mod.ratio = 0.001
        bpy.context.view_layer.update()
        raise SystemExit(
            f"[planar] cannot reach {COLLAPSE} tris: the collapse floor is {stats(ob)[2]} tris."
        )
    mod.ratio = best[0]
    bpy.context.view_layer.update()
    print(f"[planar] collapse ratio={best[0]:.6f} -> {best[1]} tris")
    bpy.ops.object.modifier_apply(modifier=mod.name)

    # The collapse decimator drops custom split normals, so re-shade or the result ships
    # fully smooth and every plate edge reads as a soft roll.
    bpy.ops.object.shade_smooth()
    try:
        bpy.ops.object.shade_smooth_by_angle(angle=SMOOTH * D2R)
    except AttributeError:
        pass
    say(f"collapsed to {COLLAPSE} + reshade {SMOOTH:g}deg", ob)

# --- triangulate explicitly ------------------------------------------------
# The exporter would do this anyway; doing it here means the count printed above is the
# count that ships, and the tessellation is ours rather than the exporter's.
bm = bmesh.new()
bm.from_mesh(ob.data)
bmesh.ops.triangulate(bm, faces=bm.faces[:])
bm.to_mesh(ob.data)
bm.free()
ob.data.update()
f, v, t = say("final", ob)

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
print(f"[planar] wrote {OUT}: {t} tris / {v} verts")
