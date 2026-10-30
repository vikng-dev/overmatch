"""census.py — the view glb's primitives, their materials, and the meshes a merge by moving part
would leave. Read-only; stdlib only. Every node name comes in as an argument (the sheet's roles);
nothing here names a vehicle.

    python3 -I census.py <id>.glb <id>.lod.json --moving N... --hidden N...

--moving   the nodes that move on their own (servos, weapon barrels, roadwheels, drive wheels)
--hidden   the nodes no camera draws (colliders, shadow proxies, the link template)

A primitive belongs to its nearest --moving ancestor, or to its scene root. "Render material" is
counted three ways: every distinct glTF material; factor-only materials collapsed into one per cull
mode; factor-only materials collapsed into one.
"""

import argparse
import collections
import json
import struct


def gltf_json(path):
    with open(path, "rb") as f:
        f.read(12)
        length, _ = struct.unpack("<II", f.read(8))
        return json.loads(f.read(length))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("glb")
    ap.add_argument("certificate")
    ap.add_argument("--moving", nargs="*", default=[])
    ap.add_argument("--hidden", nargs="*", default=[])
    args = ap.parse_args()
    j = gltf_json(args.glb)
    chains = json.load(open(args.certificate))["chains"]
    nodes, mats = j["nodes"], j["materials"]
    moving, hidden = set(args.moving), set(args.hidden)

    textured = {
        m["name"]
        for m in mats
        if "normalTexture" in m
        or any(k.endswith("Texture") for k in m.get("pbrMetallicRoughness", {}))
    }
    referenced = {n["mesh"] for n in nodes if "mesh" in n}
    rung_names = {r["mesh"] for c in chains.values() for r in c["rungs"]}
    total = no_mat = 0
    no_mat_by = collections.Counter()
    for mi, m in enumerate(j["meshes"]):
        role = "source" if mi in referenced else ("rung" if m["name"] in rung_names else "other")
        for p in m["primitives"]:
            total += 1
            if "material" not in p:
                no_mat += 1
                no_mat_by[role] += 1
    print(f"primitives {total}; without a glTF material {no_mat} = {dict(no_mat_by)}")
    print(f"materials {len(mats)}; textured {sorted(textured)}")

    parent = {c: i for i, n in enumerate(nodes) for c in n.get("children", [])}

    def part_of(ni):
        while nodes[ni].get("name") not in moving and ni in parent:
            ni = parent[ni]
        return nodes[ni].get("name")

    def key(name, rule):
        m = next(x for x in mats if x["name"] == name)
        if rule == 0 or name in textured:
            return name
        return "factor/2-sided" if rule == 1 and m.get("doubleSided") else "factor"

    parts = collections.defaultdict(list)
    for ni, n in enumerate(nodes):
        if "mesh" not in n or n.get("name") in hidden:
            continue
        m = j["meshes"][n["mesh"]]
        for pi, p in enumerate(m["primitives"]):
            rungs = len(chains.get(f"{m['name']}#{pi}", {}).get("rungs", []))
            parts[part_of(ni)].append((mats[p["material"]]["name"], rungs))
    print("part | drawn primitives | + rung siblings | meshes: glTF material | factor per cull | factor")
    sums = [0] * 5
    for part, items in parts.items():
        row = [len(items), sum(r for _, r in items)] + [
            len({key(name, rule) for name, _ in items}) for rule in range(3)
        ]
        sums = [a + b for a, b in zip(sums, row)]
        print(part, "|", " | ".join(map(str, row)), "|", dict(collections.Counter(n for n, _ in items)))
    print("TOTAL |", " | ".join(map(str, sums)))


if __name__ == "__main__":
    main()
