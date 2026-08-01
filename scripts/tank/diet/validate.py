"""Structural validation of a glb: every reference resolves and every view is in bounds."""

import sys

import glblib

g = glblib.Glb.load(sys.argv[1])
gl = g.gltf
errs = []


def chk(cond, msg):
    if not cond:
        errs.append(msg)


buflen = gl["buffers"][0]["byteLength"]
chk(buflen == len(g.bin), f"buffer byteLength {buflen} != bin chunk {len(g.bin)}")

for i, bv in enumerate(gl["bufferViews"]):
    end = bv.get("byteOffset", 0) + bv["byteLength"]
    chk(end <= len(g.bin), f"bufferView {i} ends at {end} > bin {len(g.bin)}")

for i, a in enumerate(gl["accessors"]):
    if "bufferView" not in a:
        continue
    bv = gl["bufferViews"][a["bufferView"]]
    n = glblib.NCOMP[a["type"]]
    size = glblib.COMP[a["componentType"]][1]
    stride = bv.get("byteStride") or n * size
    need = a.get("byteOffset", 0) + (a["count"] - 1) * stride + n * size
    chk(need <= bv["byteLength"], f"accessor {i} needs {need} > bufferView {bv['byteLength']}")

nmesh = len(gl["meshes"])
nmat = len(gl.get("materials", []))
nacc = len(gl["accessors"])
for i, n in enumerate(gl["nodes"]):
    if n.get("mesh") is not None:
        chk(0 <= n["mesh"] < nmesh, f"node {i} mesh {n['mesh']} out of range")
    for c in n.get("children", []):
        chk(0 <= c < len(gl["nodes"]), f"node {i} child {c} out of range")

for mi, m in enumerate(gl["meshes"]):
    for pi, p in enumerate(m["primitives"]):
        for k, a in p["attributes"].items():
            chk(0 <= a < nacc, f"mesh {mi}/{pi} attr {k} accessor {a} out of range")
        if "indices" in p:
            chk(0 <= p["indices"] < nacc, f"mesh {mi}/{pi} indices out of range")
            pos = gl["accessors"][p["attributes"]["POSITION"]]["count"]
            mx = max(g.read_accessor(p["indices"]))
            chk(mx < pos, f"mesh {mi}/{pi} index {mx} >= vertex count {pos}")
        if p.get("material") is not None:
            chk(0 <= p["material"] < nmat, f"mesh {mi}/{pi} material out of range")
        for k in ("POSITION", "NORMAL", "TEXCOORD_0"):
            if k in p["attributes"]:
                c = gl["accessors"][p["attributes"][k]]["count"]
                chk(
                    c == gl["accessors"][p["attributes"]["POSITION"]]["count"],
                    f"mesh {mi}/{pi} attr {k} count {c} != POSITION count",
                )

for t in gl.get("textures", []):
    src = t.get("extensions", {}).get("KHR_texture_basisu", {}).get("source", t.get("source"))
    chk(src is not None and src < len(gl["images"]), f"texture source {src} out of range")

reachable = set()
stack = list(gl["scenes"][gl.get("scene", 0)]["nodes"])
while stack:
    i = stack.pop()
    if i in reachable:
        continue
    reachable.add(i)
    stack += gl["nodes"][i].get("children", [])
used_meshes = {gl["nodes"][i]["mesh"] for i in reachable if gl["nodes"][i].get("mesh") is not None}
orphan = sorted(set(range(nmesh)) - used_meshes)
used_mats = {
    p.get("material")
    for mi in used_meshes
    for p in gl["meshes"][mi]["primitives"]
    if p.get("material") is not None
}
orphan_mats = sorted(set(range(nmat)) - used_mats)

print(f"{sys.argv[1]}")
print(f"  nodes reachable from scene 0: {len(reachable)}/{len(gl['nodes'])}")
print(f"  meshes used by scene: {len(used_meshes)}/{nmesh}  orphan meshes: {orphan}")
print(f"  materials used: {len(used_mats)}/{nmat}  orphan materials: {orphan_mats}")
print(f"  json chunk {g.json_chunk_len} B, bin {len(g.bin)} B")
if errs:
    print("  ERRORS:")
    for e in errs:
        print("   -", e)
    sys.exit(1)
print("  OK - all references resolve, all views in bounds")
