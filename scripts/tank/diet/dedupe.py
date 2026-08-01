"""Point duplicate nodes at one shared mesh, then garbage-collect what that orphans.

Usage: dedupe.py <target.glb> <node_name>=<mesh_name> ...
       dedupe.py selftest

The right-hand side is named, not indexed, so the mapping in the caller reads as
"the coax barrel now uses the hull barrel's mesh" and cannot silently follow an
index that moved.

THE CHECK COMPARES DATA, NOT COUNTS. Repointing is destructive — the losing mesh is
garbage-collected on the next line — so "identical" here means every attribute element
and every index of every primitive decodes to the same values, and the two primitives
resolve to the same effective material. Counts are reported, never trusted: two meshes
with equal triangle and vertex counts and different vertices would pass a count check,
get merged, and silently swap one model for another with nothing to notice it later.

Comparison is on DECODED elements, so identical geometry sitting at different buffer
offsets, in differently-strided bufferViews, or behind different accessor indices still
compares equal — which is the whole point, since a duplicate mesh never shares storage
with its twin. `selftest` proves both directions.
"""

import copy
import hashlib
import json
import os
import struct
import sys
import tempfile

import glblib


class Mismatch(Exception):
    """The two meshes are not the same geometry. Nothing has been written."""


# --- comparison -----------------------------------------------------------------


def accessor_digest(g, idx):
    """A stable hash of an accessor's LOGICAL elements — offset-, stride- and index-blind.

    Floats hash by their exact bit pattern; integers hash by value, so the same index list
    stored as uint16 and as uint32 compares equal.
    """
    acc = g.gltf["accessors"][idx]
    n = glblib.NCOMP[acc["type"]]
    is_float = acc["componentType"] == 5126
    h = hashlib.sha256()
    h.update(f"{acc['type']}|{acc['count']}|{'f' if is_float else 'i'}|"
             f"{bool(acc.get('normalized'))}|".encode())
    for el in g.read_accessor(idx):
        vals = el if isinstance(el, tuple) else (el,)
        if is_float:
            h.update(struct.pack("<" + "f" * n, *vals))
        else:
            h.update((",".join(str(v) for v in vals) + ";").encode())
    return h.hexdigest()[:16]


def material_key(gl, mi):
    """Effective material identity: the definition itself, not its index.

    Names are dropped and texture references are resolved to (image, sampler), so two
    materials that render identically compare equal even when they are separate entries.
    """
    if mi is None:
        return None

    def resolve(obj, key=None):
        if isinstance(obj, dict):
            out = {}
            for k, v in obj.items():
                if k == "name":
                    continue
                out[k] = resolve(v, k)
            if key and key.lower().endswith("texture") and "index" in obj:
                t = gl.get("textures", [])[obj["index"]]
                out["index"] = ["tex", t.get("source"), t.get("sampler")]
            return out
        if isinstance(obj, list):
            return [resolve(v) for v in obj]
        return obj

    return json.dumps(resolve(copy.deepcopy(gl["materials"][mi])), sort_keys=True)


def sig(g, mi):
    """Per-primitive geometry signature: decoded attribute and index data, plus material."""
    gl = g.gltf
    out = []
    for p in gl["meshes"][mi]["primitives"]:
        out.append(
            {
                "mode": p.get("mode", 4),
                "attrs": {k: accessor_digest(g, a) for k, a in sorted(p["attributes"].items())},
                "indices": accessor_digest(g, p["indices"]) if "indices" in p else None,
                "material": material_key(gl, p.get("material")),
                "tris": glblib.tri_count(g, p),
                "verts": glblib.vert_count(g, p),
            }
        )
    return out


def require_identical(g, label, old, keep):
    """Raise Mismatch unless every primitive of both meshes carries the same data."""
    a, b = sig(g, old), sig(g, keep)
    if len(a) != len(b):
        raise Mismatch(f"{label}: primitive count differs ({len(a)} vs {len(b)})")
    for i, (pa, pb) in enumerate(zip(a, b)):
        if pa["mode"] != pb["mode"]:
            raise Mismatch(f"{label}: primitive {i} mode differs ({pa['mode']} vs {pb['mode']})")
        if (pa["tris"], pa["verts"]) != (pb["tris"], pb["verts"]):
            raise Mismatch(
                f"{label}: primitive {i} counts differ "
                f"({pa['tris']}t/{pa['verts']}v vs {pb['tris']}t/{pb['verts']}v)"
            )
        if set(pa["attrs"]) != set(pb["attrs"]):
            raise Mismatch(
                f"{label}: primitive {i} attribute sets differ "
                f"({sorted(pa['attrs'])} vs {sorted(pb['attrs'])})"
            )
        for k in sorted(pa["attrs"]):
            if pa["attrs"][k] != pb["attrs"][k]:
                raise Mismatch(
                    f"{label}: primitive {i} {k} data differs "
                    f"({pa['attrs'][k]} vs {pb['attrs'][k]}) over "
                    f"{pa['verts']} vertices — counts match, contents do not"
                )
        if pa["indices"] != pb["indices"]:
            raise Mismatch(f"{label}: primitive {i} index data differs")
        if pa["material"] != pb["material"]:
            raise Mismatch(f"{label}: primitive {i} effective material differs")
    return b


# --- the tool -------------------------------------------------------------------


def dedupe(target, specs, quiet=False):
    def say(s):
        if not quiet:
            print(s)

    g = glblib.Glb.load(target)
    gl = g.gltf
    mesh_by_name = {}
    for i, m in enumerate(gl["meshes"]):
        mesh_by_name.setdefault(m.get("name"), i)
    node_by_name = {n.get("name"): i for i, n in enumerate(gl["nodes"])}

    for spec in specs:
        node_name, mesh_name = spec.split("=", 1)
        ni = node_by_name[node_name]
        keep = mesh_by_name[mesh_name]
        old = gl["nodes"][ni]["mesh"]
        if old == keep:
            say(f"{node_name}: already on mesh {keep}")
            continue
        b = require_identical(g, node_name, old, keep)
        gl["nodes"][ni]["mesh"] = keep
        say(
            f"{node_name}: mesh {old} '{gl['meshes'][old].get('name')}' -> "
            f"{keep} '{mesh_name}'  ({sum(x['tris'] for x in b)} tris, data-verified)"
        )

    # --- GC ---------------------------------------------------------------------
    used_meshes = sorted({n["mesh"] for n in gl["nodes"] if n.get("mesh") is not None})
    drop_meshes = [i for i in range(len(gl["meshes"])) if i not in set(used_meshes)]
    mesh_map = {old: new for new, old in enumerate(used_meshes)}
    gl["meshes"] = [gl["meshes"][i] for i in used_meshes]
    for n in gl["nodes"]:
        if n.get("mesh") is not None:
            n["mesh"] = mesh_map[n["mesh"]]

    used_mats = sorted(
        {
            p["material"]
            for m in gl["meshes"]
            for p in m["primitives"]
            if p.get("material") is not None
        }
    )
    drop_mats = [i for i in range(len(gl["materials"])) if i not in set(used_mats)]
    mat_map = {old: new for new, old in enumerate(used_mats)}
    dropped_mat_names = [gl["materials"][i].get("name") for i in drop_mats]
    gl["materials"] = [gl["materials"][i] for i in used_mats]
    for m in gl["meshes"]:
        for p in m["primitives"]:
            if p.get("material") is not None:
                p["material"] = mat_map[p["material"]]

    say(f"dropped {len(drop_meshes)} orphan meshes: {drop_meshes}")
    say(f"dropped {len(drop_mats)} orphan materials: {list(zip(drop_mats, dropped_mat_names))}")
    say(f"meshes {len(gl['meshes'])}  materials {len(gl['materials'])}")

    g.sync_buffer_len()
    g.save(target, json_pad_to=glblib.JSON_PAD)
    say(f"wrote {target}")
    return g


# --- selftest -------------------------------------------------------------------

_TRI = [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)]


def _tiny_glb(path, positions_b, indices_b=(0, 1, 2), second_material=False, mat_b=None):
    """Two one-primitive meshes, A and B, with EQUAL triangle and vertex counts.

    B's data is deliberately written at a different buffer offset, in a padded (strided)
    bufferView and behind higher accessor indices, so a check that is doing its job sees
    through all three and compares only the values.
    """
    bin_ = bytearray()

    def pos_view(verts, stride=None):
        while len(bin_) % 4:
            bin_.append(0)
        off = len(bin_)
        for v in verts:
            bin_.extend(struct.pack("<3f", *v))
            if stride:
                bin_.extend(b"\x00" * (stride - 12))
        bv = {"buffer": 0, "byteOffset": off, "byteLength": len(bin_) - off}
        if stride:
            bv["byteStride"] = stride
        return bv

    def idx_view(idx):
        while len(bin_) % 4:
            bin_.append(0)
        off = len(bin_)
        bin_.extend(struct.pack("<3H", *idx))
        return {"buffer": 0, "byteOffset": off, "byteLength": 6}

    def prim(pos_acc, idx_acc, mat):
        return {"attributes": {"POSITION": pos_acc}, "indices": idx_acc, "material": mat}

    views = [pos_view(_TRI), idx_view((0, 1, 2))]
    bin_.extend(b"\xaa" * 37)  # shove B somewhere else entirely
    views.append(pos_view(positions_b, stride=16))
    views.append(idx_view(indices_b))

    mats = [{"name": "M", "pbrMetallicRoughness": {"baseColorFactor": [1, 1, 1, 1]}}]
    if second_material:
        mats.append(
            {
                "name": "M_copy",
                "pbrMetallicRoughness": {"baseColorFactor": mat_b or [1, 1, 1, 1]},
            }
        )
    gltf = {
        "asset": {"version": "2.0"},
        "buffers": [{"byteLength": len(bin_)}],
        "bufferViews": views,
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"},
            {"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR"},
        ],
        "materials": mats,
        "meshes": [
            {"name": "MeshA", "primitives": [prim(0, 1, 0)]},
            {"name": "MeshB", "primitives": [prim(2, 3, 1 if second_material else 0)]},
        ],
        "nodes": [{"name": "NodeA", "mesh": 0}, {"name": "NodeB", "mesh": 1}],
        "scenes": [{"nodes": [0, 1]}],
        "scene": 0,
    }
    glblib.Glb(gltf, bytes(bin_)).save(path)
    return path


def selftest():
    """Prove the check refuses equal-count/different-data meshes and accepts real twins."""
    moved = [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.001)]  # 1 mm on one vertex
    cases = [
        ("twin at a different offset and stride", dict(positions_b=_TRI), None),
        ("twin behind a duplicate material entry",
         dict(positions_b=_TRI, second_material=True), None),
        ("equal counts, one vertex moved 1 mm", dict(positions_b=moved), "POSITION data differs"),
        ("equal counts, winding reversed",
         dict(positions_b=_TRI, indices_b=(0, 2, 1)), "index data differs"),
        ("same geometry, different material",
         dict(positions_b=_TRI, second_material=True, mat_b=[1, 0, 0, 1]),
         "effective material differs"),
    ]
    failures = 0
    with tempfile.TemporaryDirectory() as td:
        for i, (label, kw, want) in enumerate(cases):
            path = _tiny_glb(os.path.join(td, f"c{i}.glb"), **kw)
            try:
                g = dedupe(path, ["NodeB=MeshA"], quiet=True)
                got = None
            except Mismatch as e:
                got = str(e)
            if want is None:
                ok = got is None and len(g.gltf["meshes"]) == 1
                detail = "repointed + orphan GC'd" if ok else f"REFUSED: {got}"
            else:
                ok = got is not None and want in got
                detail = got or "ACCEPTED — would have merged two different meshes"
            failures += not ok
            print(f"{'ok  ' if ok else 'FAIL'}  {label:<42} {detail}")
    print("selftest:", "PASS" if not failures else f"{failures} FAILURES")
    return 1 if failures else 0


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "selftest":
        raise SystemExit(selftest())
    try:
        dedupe(sys.argv[1], sys.argv[2:])
    except Mismatch as e:
        print(f"REFUSED: {e}", file=sys.stderr)
        raise SystemExit(2)
