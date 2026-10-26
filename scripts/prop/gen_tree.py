"""gen_tree.py — the headless tree generator, run inside Blender.

    blender --background --factory-startup --python scripts/prop/gen_tree.py -- \\
        --species norway_spruce --seed 1 --out assets/trees/fir_1/fir_1.blend

Writes `<stem>.blend`, holding the slice contract's node tree, and `<stem>.tree.ron` beside it.
The species row comes from `assets/trees/species.ron` (`--table` to point elsewhere); the plan
every mesh is built from is `scripts/prop/plan.py`'s, a function of (species row, seed).

THE NODE TREE (slice contract §2, authored Z up; the export turns it +Y up):
    <id>                      an empty at the origin
      trunk_00                a tapered open tube, origin at its own bottom centre
        cap_bottom_00         a disc at the segment's bottom, facing down
        cap_top_00            a disc at the segment's top, facing up
        canopy_00             every branch card whose base lies in this segment's span (optional)
        trunk_01              placed `height_m[0]` up the parent's axis
          ...
Every node under the root carries the custom properties `tree_segment` (int) and `tree_part`
(`trunk` | `cap_bottom` | `cap_top` | `canopy`); the build exports them as glTF extras. The root
carries `tree_species`, `tree_seed` and `tree_height_m` so a reader of the blend alone knows what
it holds.

A placeholder look: untextured bark, cut-wood and needle materials; a branch is one flat kite in
the whorl's plane. Determinism: the same (species, seed) plans the same meshes; the `.blend` bytes
still differ run to run (Blender writes memory addresses into the file), the exported glb does not.
"""

from __future__ import annotations

import argparse
import math
import os
import sys

import bpy

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import plan  # noqa: E402  — the path above is what makes this importable
import ron  # noqa: E402

#: Sides of a trunk tube and a cap disc. Placeholder resolution.
SIDES = 12

#: Base colours (linear RGB) of the three placeholder materials.
MATERIALS = {
    "Bark": (0.16, 0.10, 0.06),
    "CutWood": (0.62, 0.47, 0.26),
    "Needles": (0.05, 0.16, 0.05),
}


def material(name: str):
    colour = MATERIALS[name] + (1.0,)
    mat = bpy.data.materials.new(name)
    if mat.node_tree is None:
        mat.use_nodes = True
    bsdf = mat.node_tree.nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = colour
    bsdf.inputs["Roughness"].default_value = 1.0
    mat.diffuse_color = colour
    mat.roughness = 1.0
    # The cards are seen from both sides; the exporter writes `doubleSided` from this flag.
    mat.use_backface_culling = name != "Needles"
    return mat


def mesh_object(name: str, verts, faces, uvs, mat, smooth: bool, parent, location, props):
    """One object holding a fresh mesh, linked, parented and tagged."""
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata([tuple(v) for v in verts], [], [tuple(f) for f in faces])
    mesh.update()
    if len(mesh.polygons) != len(faces):
        raise SystemExit("{}: {} face(s) went in and {} came out".format(
            name, len(faces), len(mesh.polygons)))
    for polygon in mesh.polygons:
        polygon.use_smooth = smooth
    layer = mesh.uv_layers.new(name="UVMap")
    flat = []
    for face in faces:
        for vertex in face:
            flat.extend(uvs[vertex])
    layer.uv.foreach_set("vector", flat)
    mesh.materials.append(mat)
    obj = bpy.data.objects.new(name, mesh)
    if obj.name != name or mesh.name != name:
        raise SystemExit("{}: Blender renamed the object or its mesh".format(name))
    bpy.context.scene.collection.objects.link(obj)
    obj.parent = parent
    obj.location = location
    for key, value in props.items():
        obj[key] = value
    return obj


def tube(r_bottom: float, r_top: float, length: float):
    """An open tapered tube from z = 0 to z = `length`, `SIDES` quads, smooth."""
    verts, uvs = [], []
    for ring, (radius, z) in enumerate(((r_bottom, 0.0), (r_top, length))):
        for side in range(SIDES):
            angle = 2.0 * math.pi * side / SIDES
            verts.append((radius * math.cos(angle), radius * math.sin(angle), z))
            uvs.append((side / SIDES, float(ring)))
    faces = []
    for side in range(SIDES):
        nxt = (side + 1) % SIDES
        faces.append((side, nxt, SIDES + nxt, SIDES + side))
    return verts, faces, uvs


def disc(radius: float, z: float, facing_up: bool):
    """A fan disc at height `z`; the winding faces +Z when `facing_up`, else -Z."""
    verts = [(0.0, 0.0, z)]
    uvs = [(0.5, 0.5)]
    for side in range(SIDES):
        angle = 2.0 * math.pi * side / SIDES
        verts.append((radius * math.cos(angle), radius * math.sin(angle), z))
        uvs.append((0.5 + 0.5 * math.cos(angle), 0.5 + 0.5 * math.sin(angle)))
    faces = []
    for side in range(SIDES):
        a, b = 1 + side, 1 + (side + 1) % SIDES
        faces.append((0, a, b) if facing_up else (0, b, a))
    return verts, faces, uvs


def cards(branches, z_local: float):
    """Every branch of one whorl as a kite in its own vertical plane: a stem-width base at the
    trunk, the full width at 40 % of the length, a point at the tip. Three triangles each."""
    verts, faces, uvs = [], [], []
    kite_uv = [(0.5, 0.0), (0.0, 0.4), (0.5, 1.0), (1.0, 0.4), (0.4, 0.0), (0.6, 0.0)]
    for branch in branches:
        along = (math.cos(branch["azimuth"]) * math.cos(branch["droop"]),
                 math.sin(branch["azimuth"]) * math.cos(branch["droop"]),
                 -math.sin(branch["droop"]))
        side = (-math.sin(branch["azimuth"]), math.cos(branch["azimuth"]), 0.0)
        length, width = branch["length_m"], branch["width_m"]

        def at(t, s):
            return (along[0] * t * length + side[0] * s * width,
                    along[1] * t * length + side[1] * s * width,
                    z_local + along[2] * t * length + side[2] * s * width)

        base = len(verts)
        verts += [at(0.0, 0.0), at(0.4, -0.5), at(1.0, 0.0), at(0.4, 0.5),
                  at(0.0, -0.08), at(0.0, 0.08)]
        uvs += kite_uv
        faces += [(base + 4, base + 1, base + 3, base + 5),
                  (base + 1, base + 2, base + 3)]
    return verts, faces, uvs


def build(stem: str, species: str, seed: int, row: dict) -> dict:
    """The whole tree into the open (empty) scene. Returns what the `.tree.ron` records."""
    sampled = plan.sample(row, seed)
    height, dbh, rng = sampled["height_m"], sampled["dbh_m"], sampled["rng"]
    segment_rows = plan.segments(height, dbh)
    whorl_rows = plan.whorls(height, row, rng)
    bark, cut, needles = material("Bark"), material("CutWood"), material("Needles")

    root = bpy.data.objects.new(stem, None)
    bpy.context.scene.collection.objects.link(root)
    root["tree_species"] = species
    root["tree_seed"] = seed
    root["tree_height_m"] = height

    parent, z = root, 0.0
    for index, segment in enumerate(segment_rows):
        tag = "{:02d}".format(index)
        length = segment["height_m"]
        verts, faces, uvs = tube(segment["r_bottom_m"], segment["r_top_m"], length)
        trunk = mesh_object(
            "trunk_" + tag, verts, faces, uvs, bark, True, parent,
            (0.0, 0.0, 0.0 if index == 0 else segment_rows[index - 1]["height_m"]),
            {"tree_segment": index, "tree_part": "trunk"},
        )
        for part, radius, at_z, up in (("cap_bottom", segment["r_bottom_m"], 0.0, False),
                                       ("cap_top", segment["r_top_m"], length, True)):
            verts, faces, uvs = disc(radius, 0.0, up)
            mesh_object(part + "_" + tag, verts, faces, uvs, cut, False, trunk, (0.0, 0.0, at_z),
                        {"tree_segment": index, "tree_part": part})
        branches = [
            (whorl["z_m"] - z, whorl["branches"])
            for whorl in whorl_rows if z <= whorl["z_m"] < z + length
        ]
        if branches:
            verts, faces, uvs = [], [], []
            for z_local, whorl_branches in branches:
                v, f, u = cards(whorl_branches, z_local)
                offset = len(verts)
                verts += v
                uvs += u
                faces += [tuple(offset + i for i in face) for face in f]
            mesh_object("canopy_" + tag, verts, faces, uvs, needles, False, trunk,
                        (0.0, 0.0, 0.0), {"tree_segment": index, "tree_part": "canopy"})
        parent, z = trunk, plan.round_m(z + length)
    return {"species": species, "dbh_m": dbh, "height_m": height, "segments": segment_rows}


def main(argv) -> int:
    parser = argparse.ArgumentParser(prog="gen_tree.py", allow_abbrev=False)
    parser.add_argument("--species", required=True, help="a key of the species table")
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--out", required=True, help="assets/trees/<id>/<id>.blend")
    parser.add_argument("--table", default=None,
                        help="the species table; assets/trees/species.ron by default")
    arguments = parser.parse_args(argv)
    out = os.path.abspath(arguments.out)
    if not out.endswith(".blend"):
        raise SystemExit("--out names a .blend: {}".format(out))
    stem = os.path.basename(out)[:-len(".blend")]
    table = arguments.table or os.path.join(
        os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
        "assets", "trees", "species.ron")
    species = ron.load(table)
    if arguments.species not in species:
        raise SystemExit("{} has no species {!r}; it has {}".format(
            table, arguments.species, ", ".join(sorted(species))))
    row = species[arguments.species]

    # The scene the tree is built into is the factory one, emptied: no startup file and no user
    # preferences reach it, so nothing of this machine's configuration reaches the export.
    bpy.ops.wm.read_homefile(use_empty=True, use_factory_startup=True)
    record = build(stem, arguments.species, arguments.seed, row)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    bpy.ops.wm.save_as_mainfile(filepath=out)
    header = ("Generated by scripts/prop/gen_tree.py: species {}, seed {}, height {} m, dbh {} m "
              "(sampled from assets/trees/species.ron). Regenerate rather than edit.").format(
                  arguments.species, arguments.seed, record["height_m"], record["dbh_m"])
    with open(out[:-len(".blend")] + ".tree.ron", "w", encoding="utf-8") as handle:
        handle.write(plan.tree_ron(record["species"], record["segments"], header))
    print("gen_tree ▸ {} — {} m, dbh {} m, {} segment(s), {} whorl card set(s)".format(
        out, record["height_m"], record["dbh_m"], len(record["segments"]),
        sum(1 for obj in bpy.data.objects if obj.name.startswith("canopy_"))), flush=True)
    return 0


if __name__ == "__main__":
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    code = main(argv)
    sys.stdout.flush()
    os._exit(code)
