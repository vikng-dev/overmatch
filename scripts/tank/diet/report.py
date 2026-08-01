"""Per-tank geometry report for tiger_1.glb, using the same visibility rules as src/.

`src/tank/view.rs` hides every node whose name ends `_Collider` / `_Ballistic`;
`src/track/link_view.rs` hides `Link` and `Link_Box` and instead instances the `Link`
mesh LINK_COUNT x 2 times onto the belt.
"""

import sys

import glblib

LINK_COUNT = 97  # assets/tiger_1/tiger_1.tank.ron `link_count`, x2 sides

g = glblib.Glb.load(sys.argv[1])
gl = g.gltf


def hidden(name):
    return name.endswith("_Collider") or name.endswith("_Ballistic")


rows = []
for n in gl["nodes"]:
    if n.get("mesh") is None:
        continue
    name = n.get("name", "?")
    m = gl["meshes"][n["mesh"]]
    t = sum(glblib.tri_count(g, p) for p in m["primitives"])
    v = sum(glblib.vert_count(g, p) for p in m["primitives"])
    rows.append((name, n["mesh"], m.get("name"), len(m["primitives"]), t, v))

link = [r for r in rows if r[0] == "Link"][0]
body = [r for r in rows if not hidden(r[0]) and r[0] not in ("Link", "Link_Box")]
hid = [r for r in rows if hidden(r[0])]

body_t = sum(r[4] for r in body)
body_v = sum(r[5] for r in body)
belt_t = link[4] * LINK_COUNT * 2
belt_v = link[5] * LINK_COUNT * 2

print(f"file            {sys.argv[1]}")
print(f"meshes {len(gl['meshes'])}  materials {len(gl['materials'])}  nodes {len(gl['nodes'])}")
print(f"shoe (mesh {link[1]} '{link[2]}')  {link[4]} tris / {link[5]} verts")
print(
    f"visible body    {len(body)} nodes, {sum(r[3] for r in body)} prims, "
    f"{body_t} tris / {body_v} verts"
)
print(f"belt            {LINK_COUNT * 2} shoes, {belt_t} tris / {belt_v} verts")
print(f"PER TANK        {body_t + belt_t} tris / {body_v + belt_v} verts")
print(f"hidden          {len(hid)} nodes, {sum(r[4] for r in hid)} tris")
print()
print("top visible body nodes:")
for r in sorted(body, key=lambda r: -r[4])[:12]:
    print(f"  {r[0]:<28} mesh {r[1]:>2} {str(r[2]):<16} {r[4]:>6} tris {r[5]:>6} verts")

ds = [i for i, m in enumerate(gl["materials"]) if m.get("doubleSided")]
print()
print(f"doubleSided materials: {len(ds)}/{len(gl['materials'])} -> {ds}")
