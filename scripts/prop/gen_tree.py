"""Generate a construction source and its Blender inspection view from species and seed.

The source solid and foliage share plan.py with the construction writer. The view has
one closed trunk mesh and one canopy mesh, both at the root origin; it is never a
simulation constructor. Run with Blender --background --factory-startup.
"""

from __future__ import annotations

import argparse
import math
import os
import sys

import bpy

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import build as prop_build  # noqa: E402
import plan  # noqa: E402  — the path above is what makes this importable
import ron  # noqa: E402

#: Base colours (linear RGB) of the two placeholder materials.
MATERIALS = {
    "Bark": (0.16, 0.10, 0.06),
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


def cards(branches, z_m: float):
    """Every branch of one whorl as its kite (`plan.kite`, the dressing's source too). Three
    triangles each."""
    verts, faces, uvs = [], [], []
    kite_uv = [(0.5, 0.0), (0.0, 0.4), (0.5, 1.0), (1.0, 0.4), (0.4, 0.0), (0.6, 0.0)]
    for branch in branches:
        base = len(verts)
        verts += plan.kite(branch, z_m)
        uvs += kite_uv
        faces += [(base + 4, base + 1, base + 3, base + 5),
                  (base + 1, base + 2, base + 3)]
    return verts, faces, uvs


def build(stem: str, species: str, seed: int, row: dict) -> dict:
    """The whole tree into the open (empty) scene. Returns what the `.tree.ron` records."""
    sampled = plan.sample(row, seed)
    height, dbh, rng = sampled["height_m"], sampled["dbh_m"], sampled["rng"]
    ring_rows = plan.rings(height, dbh, row["trunk"]["shape"])
    whorl_rows = plan.whorls(height, row, rng)
    root = bpy.data.objects.new(stem, None)
    bpy.context.scene.collection.objects.link(root)
    for key, value in {"tree_species": species, "tree_seed": seed,
                       "tree_height_m": height, "tree_dbh_m": dbh}.items():
        root[key] = value
    corners, faces = plan.solid(ring_rows)
    # Inverse of plan.gltf; the exporter returns this one solid to Y up.
    verts = [(x, -z, y) for x, y, z in corners]
    uvs = [(math.atan2(y, x)/(2*math.pi), z/height) for x, y, z in verts]
    mesh_object("trunk", verts, faces, uvs, material("Bark"), True, root,
                (0, 0, 0), {"construction_part": "trunk"})
    verts, faces, uvs = [], [], []
    for whorl in whorl_rows:
        v, f, u = cards(whorl["branches"], whorl["z_m"])
        offset = len(verts)
        verts += v
        uvs += u
        faces += [tuple(offset+i for i in face) for face in f]
    mesh_object("canopy", verts, faces, uvs, material("Needles"), False, root,
                (0, 0, 0), {"construction_part": "canopy"})
    return {"species": species, "dbh_m": dbh, "height_m": height}


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
    failures = prop_build.species_failures(arguments.species, row)
    if failures:
        raise SystemExit("{}: {}; author valid generation data".format(table, "; ".join(failures)))

    # The scene the tree is built into is the factory one, emptied: no startup file and no user
    # preferences reach it, so nothing of this machine's configuration reaches the export.
    bpy.ops.wm.read_homefile(use_empty=True, use_factory_startup=True)
    try:
        record = build(stem, arguments.species, arguments.seed, row)
    except ValueError as error:
        raise SystemExit("{}: species {!r}: {}".format(table, arguments.species, error)) from error
    os.makedirs(os.path.dirname(out), exist_ok=True)
    bpy.ops.wm.save_as_mainfile(filepath=out)
    header = ("DERIVED generation by scripts/prop/gen_tree.py: species {}, seed {}, height {} m, dbh {} m "
              "(sampled from assets/trees/species.ron). Regenerate rather than edit.").format(
                  arguments.species, arguments.seed, record["height_m"], record["dbh_m"])
    with open(out[:-len(".blend")] + ".tree.ron", "w", encoding="utf-8") as handle:
        handle.write(plan.tree_ron(record["species"], arguments.seed, record["height_m"], record["dbh_m"], header))
    print("gen_tree ▸ {} — {} m, dbh {} m, one convex source".format(
        out, record["height_m"], record["dbh_m"]), flush=True)
    return 0


if __name__ == "__main__":
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    code = main(argv)
    sys.stdout.flush()
    os._exit(code)
