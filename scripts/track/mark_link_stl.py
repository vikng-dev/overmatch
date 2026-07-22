#!/usr/bin/env python3
"""Measure a single track-link mesh and auto-place pin-0 / pin-1 markers, headless in Blender.

Run:
    blender --background --factory-startup --python scripts/track/mark_link_stl.py -- \
        assets/tiger_1/tiger_1_track_link/files/Tiger_track.STL  out/tiger_link_marked.glb

Prints a `PIN_REPORT {json}` line and (if an output path is given) exports the mesh with two
`pin-0`/`pin-1` empties at the detected hinge line — the seed for the link glb the suspension
model's derived pitch reads (`derive::pitch_from_pins`).

FINDING (Tiger_track.STL, 2026-07-23): the raw CAD mesh is 44.5 × 11.0 × 11.0 in ARBITRARY units
with no metric grounding and no markers. Auto-detected pin spacing = 6.0 units along the travel
axis (z). Scaling that span to the RON pitch (0.130 m) implies a link width of 0.96 m and thickness
0.24 m — vs the RON's 0.79 m / 0.117 m. So the art asset's PROPORTIONS do not match the spec: it
needs a metric authoring pass (uniform scale + pin markers) before it can drive the model. That
mismatch is exactly the art↔spec gap the suspension editor exists to expose; this script is the
first automated step of closing it. Confidence: MEDIUM (pins inferred from the upper knuckle band,
not from hole-axis fitting) — treat the emitted markers as a starting point to refine in Blender.
"""

import bpy
import sys
import json
from mathutils import Vector


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if not argv:
        print("PIN_REPORT " + json.dumps({"error": "usage: <stl_in> [glb_out]"}))
        return
    stl_path = argv[0]
    out_glb = argv[1] if len(argv) > 1 else None

    bpy.ops.wm.read_factory_settings(use_empty=True)
    try:
        bpy.ops.wm.stl_import(filepath=stl_path)          # Blender 4.x+ native
    except Exception:
        bpy.ops.import_mesh.stl(filepath=stl_path)         # legacy addon

    meshes = [o for o in bpy.context.scene.objects if o.type == "MESH"]
    if not meshes:
        print("PIN_REPORT " + json.dumps({"error": "no mesh imported"}))
        return
    obj = meshes[0]
    verts = [obj.matrix_world @ v.co for v in obj.data.vertices]
    xs = [v.x for v in verts]
    ys = [v.y for v in verts]
    zs = [v.z for v in verts]
    mn = Vector((min(xs), min(ys), min(zs)))
    mx = Vector((max(xs), max(ys), max(zs)))
    dim = mx - mn

    report = {
        "verts": len(verts),
        "tris": len(obj.data.polygons),
        "dim": [round(c, 4) for c in dim],
    }

    # Heuristic frame (validated on Tiger_track.STL): x = width (largest extent), y = height with the
    # shoe/grousers low and the pin knuckles high, z = travel (pitch). Pins run parallel to x, sitting
    # in the upper knuckle band at the two ends of z.
    y_lo, y_hi = mn.y, mx.y
    upper = [v for v in verts if v.y > y_lo + 0.55 * (y_hi - y_lo)]

    def cluster_1d(vals, gap):
        vals = sorted(vals)
        groups = [[vals[0]]]
        for a in vals[1:]:
            if a - groups[-1][-1] > gap:
                groups.append([a])
            else:
                groups[-1].append(a)
        return groups

    if upper:
        groups = cluster_1d([v.z for v in upper], 0.12 * dim.z)
        groups = sorted(groups, key=len, reverse=True)[:2]
        centers = sorted(sum(g) / len(g) for g in groups)
        report["upper_vert_count"] = len(upper)
        report["z_clusters_found"] = len(groups)
        if len(centers) == 2:
            pin0_z, pin1_z = centers
            pitch = abs(pin1_z - pin0_z)
            pin_y = sum(v.y for v in upper) / len(upper)
            cx = (mn.x + mx.x) / 2
            report["pin0"] = [round(cx, 4), round(pin_y, 4), round(pin0_z, 4)]
            report["pin1"] = [round(cx, 4), round(pin_y, 4), round(pin1_z, 4)]
            report["pitch_stl_units"] = round(pitch, 4)
            report["scale_if_pitch_0.130"] = round(0.130 / pitch, 6) if pitch else None
            report["scale_if_width_0.790"] = round(0.790 / dim.x, 6)
            if pitch:
                s = 0.130 / pitch
                report["width_at_pitch_scale_m"] = round(dim.x * s, 4)   # vs RON 0.79
                report["thickness_at_pitch_scale_m"] = round(dim.y * s, 4)  # vs RON 0.117
            if out_glb:
                for name, pz in (("pin-0", pin0_z), ("pin-1", pin1_z)):
                    e = bpy.data.objects.new(name, None)
                    e.empty_display_type = "PLAIN_AXES"
                    e.empty_display_size = dim.z * 0.15
                    e.location = (cx, pin_y, pz)
                    bpy.context.scene.collection.objects.link(e)
                try:
                    bpy.ops.export_scene.gltf(filepath=out_glb, export_format="GLB")
                    report["marked_glb"] = out_glb
                except Exception as ex:
                    report["glb_export_error"] = str(ex)

    print("PIN_REPORT " + json.dumps(report))


main()
