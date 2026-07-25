#!/usr/bin/env python3
"""Repair T-junction cracks (open boundary edges) on tiger_1's idler wheels, headless in Blender.

Run (dry run — reports, changes nothing on disk):
    blender --background --factory-startup assets/tiger_1/tiger_1.blend \
        --python scripts/track/fix_idler_manifold.py --

Run (write the repaired scene to a NEW file — never overwrites the input):
    blender --background --factory-startup assets/tiger_1/tiger_1.blend \
        --python scripts/track/fix_idler_manifold.py -- --save /tmp/tiger_1_fixed.blend

Options after `--`:
    --save PATH     write the repaired .blend to PATH (refuses to equal the input path)
    --objects A,B   objects to inspect/repair (default: every mesh whose name contains "Idler")
    --all           inspect/repair every mesh object in the scene
    --report-only   diagnose, never modify the in-memory mesh
    --keep-ngon     leave the repaired face as an n-gon instead of re-triangulating it
    --tol F         collinearity / weld tolerance in metres (default 1e-4)

Prints one `IDLER_REPORT {json}` line with before/after stats per object.

DIAGNOSIS (Idler_L / Idler_R, tiger_1.blend, 2026-07-23)
-------------------------------------------------------
Each idler is 813 tris / 371 verts with NO duplicate vertices at all (welding at 1e-6 leaves 371),
yet 7 edges are used by exactly one face. Those 7 edges form ONE closed 7-vertex loop whose
vertices are all COLLINEAR to within 3.2e-7 m — i.e. the "hole" has zero area. It is a T-junction
crack, not a missing face:

    long side : one edge  283 -> 27              (0.041043 m), owned by triangle (27, 293, 283)
    fine side : six edges 283-284-285-287-289-291-27 (0.006841 m each), owned by the
                fine lateral rim strip (triangles 473/474/476/478/480/482)

The five interior vertices (284, 285, 287, 289, 291) sit exactly ON the long edge at t = 1/6 .. 5/6
(perpendicular deviation < 3.2e-7 m) but the long edge does not reference them, so each fine-side
edge has only one face and the surface is split along a zero-width slit.

That is why Merge by Distance cannot fix it: the vertices are NOT coincident duplicates — they are
6.8 mm apart and every one of them is a legitimate, distinct, load-bearing vertex. No threshold
below 6.8 mm merges anything, and any threshold above it would collapse real rim geometry. The
crack is a *topology* defect (an edge that must be subdivided), not a *proximity* defect.

THE REPAIR
----------
Subdivide the long edge so it acquires vertices at the interior positions, snap those new vertices
onto the existing ones, then weld. Blender's own subdivide interpolates UVs and custom split
normals along the edge, so shading and texturing are preserved. Because every inserted point is
collinear with the edge it splits, the repaired fan is exactly coplanar with the face it replaces:
signed volume, bounding box and silhouette are unchanged to float precision. Cost: +5 triangles
per idler.

GUI equivalent (Edit Mode on Idler_R, then repeat on Idler_L):
    1. Select > All by Trait > Non Manifold  (Vertex or Edge mode) — highlights the 7-edge slit.
    2. Switch to Edge mode, deselect all, and select ONLY the long edge (the one edge that spans
       the whole slit — the boundary edge with no interior vertices along it).
    3. Right-click > Subdivide, then open the operator panel and set "Number of Cuts" = 5.
    4. Select All (A), then M > By Distance with Merge Distance = 0.0001 m.
       (0.0001 m is provably safe here: the closest pair of distinct vertices in the whole idler
       mesh is 0.00095 m apart, ~10x the threshold, so nothing legitimate can collapse.)
    5. The subdivided triangle is now an 8-gon and the rest of the mesh is triangles; with it still
       selected press Ctrl+T (Face > Triangulate Faces) to keep the mesh homogeneous.
    6. Re-check with Select > All by Trait > Non Manifold: nothing should be selected.
"""

import bpy
import bmesh
import sys
import json
from collections import defaultdict
from mathutils import Vector

DEFAULT_TOL = 1e-4          # metres: both the collinearity band and the final weld distance
COLLINEAR_TOL = 1e-5        # metres: max perpendicular deviation for "this vertex lies on that edge"


# --------------------------------------------------------------------------------------- reporting

def weld_stats(obj, eps=1e-5):
    """Watertightness measured the ONLY way that is meaningful: weld by position first, then count
    how many faces use each edge. An index-based manifold check false-positives on every UV seam."""
    dg = bpy.context.evaluated_depsgraph_get()
    ev = obj.evaluated_get(dg)
    me = ev.to_mesh()
    mw = obj.matrix_world
    verts = [mw @ v.co for v in me.vertices]
    me.calc_loop_triangles()
    tris = [tuple(t.vertices) for t in me.loop_triangles]

    q = 1.0 / eps
    kmap, remap, wv = {}, [], []
    for v in verts:
        k = (round(v.x * q), round(v.y * q), round(v.z * q))
        if k not in kmap:
            kmap[k] = len(wv)
            wv.append(v)
        remap.append(kmap[k])

    edge_faces = defaultdict(int)
    kept, degen, zeroarea, vol = 0, 0, 0, 0.0
    face_keys = defaultdict(int)
    for (a, b, c) in tris:
        A, B, C = remap[a], remap[b], remap[c]
        if A == B or B == C or A == C:
            degen += 1
            continue
        kept += 1
        face_keys[tuple(sorted((A, B, C)))] += 1
        pa, pb, pc = wv[A], wv[B], wv[C]
        if (pb - pa).cross(pc - pa).length * 0.5 < 1e-12:
            zeroarea += 1
        vol += pa.dot(pb.cross(pc)) / 6.0
        for e in ((A, B), (B, C), (C, A)):
            edge_faces[tuple(sorted(e))] += 1

    boundary = [e for e, n in edge_faces.items() if n == 1]
    nonman = [e for e, n in edge_faces.items() if n > 2]
    xs = [v.x for v in wv]
    ys = [v.y for v in wv]
    zs = [v.z for v in wv]
    stats = {
        "tris": len(tris),
        "welded_verts": len(wv),
        "raw_verts": len(verts),
        "boundary_edges": len(boundary),
        "nonmanifold_edges": len(nonman),
        "degenerate_tris": degen,
        "zero_area_tris": zeroarea,
        "duplicate_faces": sum(n - 1 for n in face_keys.values() if n > 1),
        "signed_volume": round(vol, 9),
        "bbox_min": [round(min(xs), 6), round(min(ys), 6), round(min(zs), 6)],
        "bbox_max": [round(max(xs), 6), round(max(ys), 6), round(max(zs), 6)],
        "watertight": len(boundary) == 0 and len(nonman) == 0,
    }
    if boundary:
        bx = [wv[i].x for e in boundary for i in e]
        by = [wv[i].y for e in boundary for i in e]
        bz = [wv[i].z for e in boundary for i in e]
        stats["boundary_bbox_min"] = [round(min(bx), 6), round(min(by), 6), round(min(bz), 6)]
        stats["boundary_bbox_max"] = [round(max(bx), 6), round(max(by), 6), round(max(bz), 6)]
    ev.to_mesh_clear()
    return stats


# ------------------------------------------------------------------------------------------ repair

def find_tjunctions(bm, tol=COLLINEAR_TOL):
    """Return [(edge, [(t, vert), ...]), ...] for every boundary edge that has other boundary
    vertices lying on its interior — the signature of a T-junction crack."""
    boundary_edges = [e for e in bm.edges if len(e.link_faces) == 1]
    if not boundary_edges:
        return []
    bverts = {v for e in boundary_edges for v in e.verts}
    out = []
    for e in boundary_edges:
        a, b = e.verts[0].co, e.verts[1].co
        d = b - a
        L2 = d.length_squared
        if L2 <= 0.0:
            continue
        hits = []
        for v in bverts:
            if v in e.verts:
                continue
            t = (v.co - a).dot(d) / L2
            if not (1e-6 < t < 1.0 - 1e-6):
                continue
            if ((v.co - a) - d * t).length <= tol:
                hits.append((t, v))
        if hits:
            hits.sort(key=lambda h: h[0])
            out.append((e, hits))
    return out


def repair_object(obj, weld_tol=DEFAULT_TOL, keep_ngon=False):
    """Split every T-junction edge at the vertices lying on it, then weld.

    All mutation happens on a detached bmesh copy and is only written back on success, so a mesh
    whose boundary is NOT a clean T-junction crack (a genuinely missing face, a mismatched seam,
    non-uniform spacing) is reported and left completely untouched rather than mangled."""
    bm = bmesh.new()
    try:
        return _repair_bmesh(obj, bm, weld_tol, keep_ngon)
    except Exception as ex:                                    # noqa: BLE001 - reported, not raised
        return {"changed": False, "error": str(ex)}
    finally:
        if bm.is_valid:
            bm.free()


def _repair_bmesh(obj, bm, weld_tol, keep_ngon):
    me = obj.data
    bm.from_mesh(me)
    bm.verts.ensure_lookup_table()
    bm.edges.ensure_lookup_table()

    tri_only_before = all(len(f.verts) == 3 for f in bm.faces)
    tj = find_tjunctions(bm)
    info = {"tjunction_edges": len(tj), "cuts": [], "max_snap_m": 0.0, "changed": False}
    if not tj:
        return info

    max_snap = 0.0
    all_targets = []
    for edge, hits in tj:
        targets = [v.co.copy() for _, v in hits]
        all_targets.extend(targets)
        span = (edge.verts[1].co - edge.verts[0].co).length
        info["cuts"].append({
            "edge_len_m": round(span, 6),
            "cuts": len(hits),
            "t": [round(t, 6) for t, _ in hits],
        })
        res = bmesh.ops.subdivide_edges(bm, edges=[edge], cuts=len(hits), use_grid_fill=False)
        new_verts = [g for g in res["geom_split"] if isinstance(g, bmesh.types.BMVert)]
        # Snap each freshly created vertex onto the existing vertex it is meant to coincide with,
        # so the subsequent weld is exact regardless of whether the original spacing was uniform.
        for nv in new_verts:
            best = min(targets, key=lambda p: (p - nv.co).length)
            dist = (best - nv.co).length
            max_snap = max(max_snap, dist)
            if dist > weld_tol:
                raise RuntimeError(
                    f"{obj.name}: subdivided vertex is {dist:.6f} m from its target "
                    f"(> weld tol {weld_tol}); refusing to weld"
                )
            nv.co = best

    info["max_snap_m"] = round(max_snap, 9)
    before = len(bm.verts)
    bmesh.ops.remove_doubles(bm, verts=bm.verts[:], dist=weld_tol)
    info["verts_merged"] = before - len(bm.verts)
    info["changed"] = True

    # Subdividing one edge of a triangle turns it into an n-gon. The idler meshes are 100 %
    # triangles, so re-triangulate just the faces we touched to keep the topology homogeneous
    # (an n-gon would otherwise be triangulated later, unpredictably, by the glTF exporter).
    if tri_only_before and not keep_ngon:
        touched = []
        for f in bm.faces:
            if len(f.verts) <= 3:
                continue
            if any((v.co - t).length <= weld_tol for v in f.verts for t in all_targets):
                touched.append(f)
        if touched:
            bmesh.ops.triangulate(bm, faces=touched, quad_method="BEAUTY", ngon_method="BEAUTY")
            info["ngons_triangulated"] = len(touched)

    bm.to_mesh(me)
    me.update()
    return info


# -------------------------------------------------------------------------------------------- main

def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    save_path = None
    names = None
    all_meshes = False
    report_only = False
    keep_ngon = False
    tol = DEFAULT_TOL
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--save":
            i += 1
            save_path = argv[i]
        elif a == "--objects":
            i += 1
            names = [s.strip() for s in argv[i].split(",") if s.strip()]
        elif a == "--all":
            all_meshes = True
        elif a == "--report-only":
            report_only = True
        elif a == "--keep-ngon":
            keep_ngon = True
        elif a == "--tol":
            i += 1
            tol = float(argv[i])
        i += 1

    src = bpy.data.filepath
    if save_path and bpy.path.abspath(save_path) == bpy.path.abspath(src):
        print("IDLER_REPORT " + json.dumps({"error": "refusing to overwrite the source .blend"}))
        return

    if all_meshes:
        objs = [o for o in bpy.data.objects if o.type == "MESH"]
    elif names:
        objs = [bpy.data.objects[n] for n in names if n in bpy.data.objects and
                bpy.data.objects[n].type == "MESH"]
    else:
        objs = [o for o in bpy.data.objects if o.type == "MESH" and "idler" in o.name.lower()]

    report = {"source": src, "tol": tol, "report_only": report_only, "objects": {}}
    for o in objs:
        entry = {"before": weld_stats(o)}
        if not report_only:
            entry["repair"] = repair_object(o, weld_tol=tol, keep_ngon=keep_ngon)
            entry["after"] = weld_stats(o)
            b, a = entry["before"], entry["after"]
            entry["delta"] = {
                "tris": a["tris"] - b["tris"],
                "welded_verts": a["welded_verts"] - b["welded_verts"],
                "volume": round(a["signed_volume"] - b["signed_volume"], 12),
                "bbox_min": [round(x - y, 9) for x, y in zip(a["bbox_min"], b["bbox_min"])],
                "bbox_max": [round(x - y, 9) for x, y in zip(a["bbox_max"], b["bbox_max"])],
            }
        report["objects"][o.name] = entry

    if save_path and not report_only:
        bpy.ops.wm.save_as_mainfile(filepath=save_path, copy=True)
        report["saved"] = save_path

    print("IDLER_REPORT " + json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
