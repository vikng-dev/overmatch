"""repair_source.py — fix the two defects in `assets/tiger_1/tiger_1.blend` AT THE SOURCE.

    blender -b assets/tiger_1/tiger_1.blend -P .agents/blender/repair_source.py
    blender -b assets/tiger_1/tiger_1.blend -P .agents/blender/repair_source.py -- --dry-run

THIS IS THE ONLY SCRIPT IN THE REPO THAT SAVES THE .BLEND. `export_tiger.py` never does — it
swaps `Link`'s object data for the length of the export and puts it back — and that guarantee is
load-bearing, because the artist's file is untracked and 145 MB. This one saves, once, explicitly,
after every check below has passed.

WHY IT EXISTS
-------------
Two properties the shipped `tiger_1.glb` has, and the `.blend` did not:

  * THE MG DEDUPE. The coax and hull MG34 are the same model, imported twice: two objects each
    for the barrel, the body and the magazine, pointing at six mesh datablocks that hold the same
    geometry, wearing eight materials that are four materials twice. Blender has no notion of two
    objects sharing one glTF mesh, so the exporter emitted all six — 67 meshes and 15 materials
    against the shipped 64 and 11.
  * BACK-FACE CULLING. glTF's default is single-sided; the exporter writes `doubleSided: true`
    for every material whose `use_backface_culling` is off (`__gather_double_sided` in
    `io_scene_gltf2/blender/exp/material/materials.py` — `not use_backface_culling` is the whole
    rule, and a material WITH the flag omits the key entirely, which is the glTF default). Every
    material in the file had it off, so every material shipped double-sided. The measurement that
    says culling is safe on this model was taken by the retired asset-diet pass (deleted with
    ADR 0033's branch; renders kept at `.agents/scratch/asset-diet-renders/`): 115 red pixels in
    128 M over 32 camera positions at 2 000 px, all coincident faces rather than holes.

Both were previously REPLAYED onto every raw export by a surgery stage in `export_tiger.py`,
using that pass's glb-surgery scripts. That stage is gone. A pipeline stage
that repairs its input on every run is a workaround for a bad source; the source is now correct,
and the export is a plain export again.

WHAT "IDENTICAL" MEANS HERE — DATA, NOT NAMES
---------------------------------------------
Repointing is destructive: the losing mesh is removed on the next line and its material with it.
So `Object_0.001` and `Object_0.002` are not merged because they are named like a pair or because
they have the same vertex count — every attribute of both meshes is decoded and hashed
(positions, edges, corner-to-vertex map, face offsets, and every generic attribute: UVs, material
indices, sharp flags, colour layers), plus the corner normals the exporter actually writes. Two
meshes with equal counts and different vertices would pass a count check, get merged, and silently
swap one model for another with nothing left to notice it. Same standard, and the same reasoning,
as the retired glb-side dedupe check it replaced.

Materials are compared the same way: node type, every node input default, every link, and the
image datablock behind any texture node. A material that merely LOOKS like its twin in the
outliner does not get merged.

The pairs are named by OBJECT, not by datablock, so the table reads as the thing it means — "the
coax barrel now uses the hull barrel's mesh" — and the material mapping is DERIVED from the two
objects' slots rather than written down twice.

IDEMPOTENT. A second run finds the merges already done and the flags already set, reports that,
and does not save. Nothing here is a one-shot that corrupts the file if it runs twice.

REFUSES LOUDLY. Any mismatch raises `Refused`, which derives from `SystemExit` because that is
the only exception class `blender -b -P` reports through its exit code (an unhandled `Exception`
prints a traceback and still exits 0 — measured, see `export_tiger.ExportError`). Nothing is
written on any refusal path: every check runs before the first mutation.
"""

import hashlib
import struct
import sys

import bpy

#: The duplicate MG objects and the objects whose datablocks they should be sharing, BY OBJECT
#: NAME. The right-hand side is the one that survives — the hull MG's `Object_*.002` meshes and
#: `Material.006`..`.009` — because that is the set the shipped glb kept when the dedupe was still
#: glb surgery (the retired asset-diet pass), so the repaired export reproduces the
#: shipped bytes rather than an equivalent-but-different file.
MG_MERGES = (
    ("Coax_MG_Barrel_Visual", "Hull_MG_Barrel_Visual"),
    ("Coax_MG_Body_Visual", "Hull_MG_Body_Visual"),
    ("Coax_MG__Mag_Visual", "Hull_MG_Mag_Visual"),
)


class Refused(SystemExit):
    """A check failed. NOTHING has been written — the .blend on disk is untouched."""


# ── data equality ────────────────────────────────────────────────────────────────────────────────


def _feed(digest, values):
    """Hash a flat sequence of floats or ints. Floats by exact bit pattern, so no epsilon."""
    for value in values:
        if isinstance(value, float):
            digest.update(struct.pack("<d", value))
        else:
            digest.update(f"{value};".encode())


def mesh_digest(mesh):
    """A dict of per-aspect digests of everything the exporter reads off `mesh`.

    Split per aspect rather than rolled into one hash so a refusal can say WHICH aspect differs;
    that is the difference between "these are not the same mesh" and a debuggable message.
    """
    out = {
        "counts": f"{len(mesh.vertices)}v/{len(mesh.edges)}e/"
                  f"{len(mesh.loops)}l/{len(mesh.polygons)}p",
    }

    positions = [0.0] * (len(mesh.vertices) * 3)
    mesh.vertices.foreach_get("co", positions)
    edges = [0] * (len(mesh.edges) * 2)
    mesh.edges.foreach_get("vertices", edges)
    corners = [0] * len(mesh.loops)
    mesh.loops.foreach_get("vertex_index", corners)
    starts = [0] * len(mesh.polygons)
    mesh.polygons.foreach_get("loop_start", starts)
    totals = [0] * len(mesh.polygons)
    mesh.polygons.foreach_get("loop_total", totals)

    for key, values in (
        ("positions", positions),
        ("edges", edges),
        ("corners", corners),
        ("faces", starts + totals),
    ):
        digest = hashlib.sha256()
        _feed(digest, values)
        out[key] = digest.hexdigest()[:16]

    # Generic attributes cover the rest of what a glTF export consumes — UV layers, per-face
    # material indices, sharp flags, colour layers — and cover it WITHOUT this script having to
    # enumerate them, so an attribute the artist adds later is compared instead of ignored.
    for attribute in mesh.attributes:
        if attribute.name in {"position", ".edge_verts", ".corner_vert"}:
            continue
        field = {
            "FLOAT": ("value", 1), "INT": ("value", 1), "INT8": ("value", 1),
            "BOOLEAN": ("value", 1), "FLOAT2": ("vector", 2), "FLOAT_VECTOR": ("vector", 3),
            "FLOAT_COLOR": ("color", 4), "BYTE_COLOR": ("color", 4), "QUATERNION": ("value", 4),
            "INT32_2D": ("value", 2), "INT16_2D": ("value", 2), "FLOAT4X4": ("value", 16),
        }.get(attribute.data_type)
        if field is None:
            raise Refused(
                f"repair_source: mesh `{mesh.name}` carries attribute `{attribute.name}` of "
                f"unknown type {attribute.data_type} — this script cannot prove it equal, so it "
                f"will not merge the mesh. Teach `mesh_digest` the type."
            )
        prop, dimension = field
        buffer = [0.0] * (len(attribute.data) * dimension)
        attribute.data.foreach_get(prop, buffer)
        digest = hashlib.sha256()
        _feed(digest, buffer)
        out[f"attr:{attribute.name}"] = f"{attribute.domain}/{attribute.data_type}/" \
                                        f"{digest.hexdigest()[:16]}"

    # Corner normals are derived rather than stored, but they are what lands in the glb's NORMAL
    # accessor, so two meshes that differ only in shading would still be a visible swap.
    normals = [0.0] * (len(mesh.loops) * 3)
    mesh.corner_normals.foreach_get("vector", normals)
    digest = hashlib.sha256()
    _feed(digest, normals)
    out["corner_normals"] = digest.hexdigest()[:16]
    return out


def material_digest(material):
    """Everything about `material` that reaches the glb: shader graph, inputs, images, blending.

    `use_backface_culling` is deliberately NOT in here. This script is about to set it on every
    material, so comparing it would only compare the state of the repair to itself.
    """
    out = {"blend": f"{getattr(material, 'blend_method', '-')}/"
                    f"{getattr(material, 'alpha_threshold', '-')}"}
    tree = material.node_tree
    if tree is None:
        out["graph"] = "no-nodes"
        return out
    nodes = []
    for node in sorted(tree.nodes, key=lambda n: (n.type, n.name)):
        inputs = []
        for socket in node.inputs:
            value = getattr(socket, "default_value", None)
            if hasattr(value, "__len__"):
                value = tuple(value)
            inputs.append(f"{socket.identifier}={value!r}")
        image = getattr(node, "image", None)
        nodes.append(f"{node.type}|{node.name}|{image.name if image else '-'}|{';'.join(inputs)}")
    links = sorted(
        f"{link.from_node.name}.{link.from_socket.identifier}->"
        f"{link.to_node.name}.{link.to_socket.identifier}"
        for link in tree.links
    )
    out["graph"] = hashlib.sha256("\n".join(nodes + links).encode()).hexdigest()[:16]
    return out


def require_identical(kind, digest, duplicate, kept):
    """Raise `Refused` unless the two datablocks hash equal in every aspect."""
    left, right = digest(duplicate), digest(kept)
    for key in sorted(set(left) | set(right)):
        if left.get(key) != right.get(key):
            raise Refused(
                f"repair_source: REFUSED — {kind} `{duplicate.name}` and `{kept.name}` differ in "
                f"{key} ({left.get(key)} vs {right.get(key)}).\n"
                f"  They are not the same {kind}, so merging them would silently replace one with "
                f"the other. If they have genuinely diverged in the blend, MG_MERGES is what has "
                f"to change — and the tank now has two different machine guns, which is an art "
                f"decision, not a pipeline one."
            )


# ── the repair ───────────────────────────────────────────────────────────────────────────────────


def _object(name):
    ob = bpy.data.objects.get(name)
    if ob is None or ob.type != "MESH":
        raise Refused(
            f"repair_source: this blend has no mesh object called `{name}` — MG_MERGES names the "
            f"objects the dedupe operates on, and one of them is missing or is not a mesh."
        )
    return ob


def plan_mg_merge():
    """Verify every pair and return `(mesh_moves, material_moves)`. Mutates NOTHING.

    `mesh_moves` is `[(object, kept_mesh)]` for the objects that still point at a duplicate;
    `material_moves` is `[(duplicate_material, kept_material)]`, DERIVED from the two meshes'
    slot lists rather than written down, so a slot the artist adds cannot silently go unmapped.
    Each duplicate material appears ONCE even though the MG wears some of them on two meshes.
    """
    mesh_moves, material_moves, mapped = [], [], {}
    for duplicate_name, kept_name in MG_MERGES:
        duplicate, kept = _object(duplicate_name), _object(kept_name)
        if duplicate.data is kept.data:
            print(f"  mg   ▸ {duplicate_name}: already shares `{kept.data.name}`")
            continue
        require_identical("mesh", mesh_digest, duplicate.data, kept.data)
        if len(duplicate.data.materials) != len(kept.data.materials):
            raise Refused(
                f"repair_source: REFUSED — `{duplicate.data.name}` has "
                f"{len(duplicate.data.materials)} material slots and `{kept.data.name}` has "
                f"{len(kept.data.materials)}. The geometry matches but the shading does not."
            )
        for slot, (old, new) in enumerate(zip(duplicate.data.materials, kept.data.materials)):
            if old is None or new is None or old is new:
                continue
            require_identical("material", material_digest, old, new)
            if mapped.setdefault(old.name, new.name) != new.name:
                raise Refused(
                    f"repair_source: REFUSED — material `{old.name}` maps to `{new.name}` here "
                    f"and to `{mapped[old.name]}` on an earlier pair. One of the MG pairs is "
                    f"misaligned."
                )
            print(f"  mg   ▸ {duplicate_name} slot {slot}: material `{old.name}` -> `{new.name}` "
                  f"(data-verified)")
            if not any(old is m for m, _ in material_moves):
                material_moves.append((old, new))
        mesh_moves.append((duplicate, kept.data))
        print(f"  mg   ▸ {duplicate_name}: mesh `{duplicate.data.name}` -> `{kept.data.name}` "
              f"({len(kept.data.polygons)} faces, data-verified)")
    return mesh_moves, material_moves


def apply_mg_merge(mesh_moves, material_moves):
    """Repoint, remap, then collect what that orphaned. Only ever called on a verified plan."""
    orphan_meshes = [ob.data for ob, _kept in mesh_moves]
    for ob, kept in mesh_moves:
        ob.data = kept

    # Remap BEFORE removing anything: a stray reference from somewhere this script did not look
    # (an OBJECT-linked material slot, a node group) gets moved to the survivor rather than
    # dropped on the floor when the datablock goes.
    for old, new in material_moves:
        old.user_remap(new)

    for mesh in orphan_meshes:
        _remove("mesh", bpy.data.meshes, mesh)
    for old, _new in material_moves:
        _remove("material", bpy.data.materials, old)


def _remove(kind, collection, datablock):
    """Remove an orphan, refusing if it is not actually one. Purging is not a blanket sweep."""
    if datablock.use_fake_user:
        print(f"  purge▸ clearing fake user on {kind} `{datablock.name}`")
        datablock.use_fake_user = False
    if datablock.users:
        raise Refused(
            f"repair_source: REFUSED — {kind} `{datablock.name}` still has {datablock.users} "
            f"user(s) after the merge, so something outside MG_MERGES references it. Removing it "
            f"would break that reference."
        )
    name = datablock.name
    collection.remove(datablock)
    print(f"  purge▸ removed orphan {kind} `{name}`")


def set_backface_culling():
    """Turn `use_backface_culling` on for every material. Returns the ones that changed.

    Every material, not just the eleven that ship: the flag is what makes a material export
    single-sided, so a material that is off is a `doubleSided: true` waiting for the next object
    that wears it. It also changes the VIEWPORT — solid and EEVEE now cull back faces, which is
    the point: the artist sees what the game sees.
    """
    changed = [m for m in bpy.data.materials if not m.use_backface_culling]
    for material in changed:
        material.use_backface_culling = True
    return changed


def summary(label):
    print(f"\n{label}")
    print(f"  meshes {len(bpy.data.meshes)}   materials {len(bpy.data.materials)}   "
          f"objects {len(bpy.data.objects)}")
    for material in sorted(bpy.data.materials, key=lambda m: m.name):
        print(f"    {'cull' if material.use_backface_culling else 'BOTH'}  {material.name!r} "
              f"(users {material.users})")


def repair(dry_run=False):
    if not bpy.data.filepath:
        raise Refused("repair_source: no .blend is open — run this with `blender -b <file> -P`.")
    print(f"repair ▸ {bpy.data.filepath}")
    summary("BEFORE")

    print("\nplanning the MG dedupe (every check runs before the first mutation)")
    mesh_moves, material_moves = plan_mg_merge()

    if not dry_run:
        apply_mg_merge(mesh_moves, material_moves)
    culled = set_backface_culling() if not dry_run else \
        [m for m in bpy.data.materials if not m.use_backface_culling]
    print(f"\n  cull ▸ use_backface_culling set on {len(culled)} material(s): "
          f"{', '.join(sorted(m.name for m in culled)) or 'none — already set'}")

    changed = bool(mesh_moves) or bool(culled)
    if dry_run:
        print("\nDRY RUN — nothing written." + ("" if changed else " (nothing to do either)"))
        return
    summary("AFTER")
    if not changed:
        print("\nNothing to repair; the .blend is already correct and was NOT saved.")
        return
    bpy.ops.wm.save_mainfile()
    print(f"\nSAVED {bpy.data.filepath}")


if __name__ == "__main__":
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    repair(dry_run="--dry-run" in argv)
