"""Cut every asset's error-ladder chain and write the manifest. The one entry point.

    /Applications/Blender.app/Contents/MacOS/Blender -b --factory-startup \
        --python scripts/lod/generate.py -- [--asset NAME] [--no-render-gate]

WHAT THIS IS. ADR 0033's generation stage: one global octave grid of deviation targets, a per-asset
SPARSE subset of it, triangle counts as outputs. Nothing in this file knows what a track shoe is —
the assets are rows in `config.ASSETS`, and every threshold is a constant in `config.py`.

THE SEARCH IS OVER UNIQUE INTEGER TRIANGLE TARGETS, NOT OVER RATIOS (ADR 0033 §5), and it is
EXHAUSTIVE rather than a bisection. Measured error is not monotone in collapse ratio, so a
bisection assumes exactly the property the doctrine denies and can step straight over a lower
feasible island. Instead every mesh the decimator can produce is enumerated once (see
`Candidates.enumerate_outputs`, which costs one decimation per distinct output), and each rung
scans them in ASCENDING triangle order and takes the first that clears its target — so everything
smaller has been measured and rejected, and the winner is minimal by exhaustion.

THE ORDER IS SACRED (ADR 0033 §6). Per level: decimate -> cleanup -> export -> decode the written
glb -> measure everything on those bytes. The search itself measures pre-export candidates, because
choosing which candidate to ship has to be cheap; but a chosen candidate is re-certified from the
file, and if the file disagrees with the search the level FAILS. The search is an optimiser, the
decode is the certificate. That includes L0, which is not generated but IS decoded: its identity
with the source is proven on welded topology, and its validity is measured on the shipped bytes
rather than inherited from the Blender mesh that produced them.

WHAT FAILS THE RUN, LOUDLY (ADR 0033 §10): an unexpected Blender build; a skinned or morph-target
source; a multi-material or multi-primitive source; a shipped L0 that is not the source; a level
whose certified upper bound misses its rung after export; a level that lost a component; a sliver
below the scale-aware altitude floor; a defaulted tangent; a duplicate face; a non-manifold edge; a
non-finite attribute; a flipped winding; a level that does not reproduce byte-for-byte when built
twice; and — once its threshold is ratified — a rendered difference over budget. Nothing degrades
silently.
"""

import hashlib
import json
import os
import shutil
import sys
import tempfile
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import bmesh  # noqa: E402
import bpy  # noqa: E402

import config as CONFIG  # noqa: E402
import measure as M  # noqa: E402
import render_gate  # noqa: E402


class GenerationError(SystemExit):
    """A named failure. SystemExit so `blender --background` reports it through its exit code.

    An unhandled ordinary Exception in a background Blender prints a traceback and still exits 0
    (measured), which would make a broken chain look like a good one to anything reading the code.
    """

    def __init__(self, stage, message):
        super().__init__(f"[{stage}] {message}")
        self.stage = stage


def log(line):
    print(line, flush=True)


def _exporter_version():
    """The glTF exporter add-on's version, which moves independently of Blender's own."""
    try:
        import io_scene_gltf2

        return ".".join(str(part) for part in getattr(io_scene_gltf2, "bl_info", {}).get(
            "version", ("unknown",)
        ))
    except ImportError:
        return "unknown"


def assert_toolchain():
    """Refuse to cut a corpus with an unexpected Blender. Named override for a deliberate upgrade.

    A decimator is a program, not a specification. Blender's collapse can change a tie-break in a
    point release and silently move every level in the game; recording the version afterwards says
    which build produced a chain, but only asserting it first stops two machines producing two
    different corpora with nobody the wiser. ADR 0033 makes a toolchain bump a corpus regeneration
    review, and this is where that becomes mechanical rather than remembered.
    """
    running = bpy.app.version_string.split()[0]
    if running == CONFIG.EXPECTED_BLENDER:
        return
    allowed = os.environ.get(CONFIG.BLENDER_OVERRIDE_ENV, "")
    if allowed in (running, "1", "yes", "any"):
        log(f"  toolchain: Blender {running} != pinned {CONFIG.EXPECTED_BLENDER}, allowed by "
            f"{CONFIG.BLENDER_OVERRIDE_ENV}")
        return
    raise GenerationError(
        "toolchain",
        f"this Blender is {running}, the corpus is pinned to {CONFIG.EXPECTED_BLENDER}. A "
        f"simplifier is a program: a point release can move every level in the game. Set "
        f"{CONFIG.BLENDER_OVERRIDE_ENV}={running} to regenerate the whole corpus deliberately, "
        f"and update config.EXPECTED_BLENDER with the result.",
    )


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


# ── the source ───────────────────────────────────────────────────────────────────────────────────

def refuse_unsupported(obj):
    """Every refusal the doctrine names, checked on the AUTHORED object before anything runs."""
    if obj is None or obj.type != "MESH":
        raise M.Refusal("not-a-mesh", repr(obj))
    if obj.data.shape_keys is not None:
        raise M.Refusal(
            "morph-mesh",
            f"{obj.name!r} carries shape keys; bind-pose metrics do not bound deformed error",
        )
    if any(modifier.type == "ARMATURE" for modifier in obj.modifiers) or obj.parent_type == "BONE":
        raise M.Refusal("skinned-mesh", f"{obj.name!r} is driven by an armature")
    if obj.vertex_groups and any(
        modifier.type in {"ARMATURE", "MESH_DEFORM"} for modifier in obj.modifiers
    ):
        raise M.Refusal("skinned-mesh", f"{obj.name!r} has deform vertex groups")
    materials = [slot.material for slot in obj.material_slots if slot.material is not None]
    if len(materials) > 1:
        raise M.Refusal(
            "multi-material-mesh",
            f"{obj.name!r} wears {len(materials)} materials, so it exports as "
            f"{len(materials)} primitives; the chain loader reads primitive 0 only",
        )
    if not materials:
        raise M.Refusal("no-material", f"{obj.name!r} has no material to render the gate under")
    return materials[0]


def shipped_material(l0_path, node_name):
    """The material as it SHIPS: imported back out of the L0 glb. Returns (material, provenance).

    The gate is supposed to render the bytes, materials included — taking the material from the
    .blend renders the artist's inputs, not the asset. So the shipped glb is re-imported and its
    material used instead.

    IT CAN LEGITIMATELY FAIL, and then it says so instead of pretending. The tank glb's textures are
    baked to KTX2 (`scripts/encode-tank-ktx2.sh`, via `KHR_texture_basisu`) because bevy needs mips;
    Blender's glTF importer does not read that extension, so the images can come back empty. A
    material with no image data would render a flat grey asset and the gate would go on producing
    confident numbers about a shading comparison it was no longer making. So every image node is
    checked for actual pixels, and on failure the caller is told which material it is getting.
    """
    before = set(bpy.data.objects)
    try:
        bpy.ops.import_scene.gltf(filepath=l0_path)
    except (RuntimeError, TypeError) as exc:
        return None, f"glb import failed: {exc}"
    imported = [ob for ob in bpy.data.objects if ob not in before]
    material, reason = None, "node not found in the imported glb"
    for ob in imported:
        if ob.name.split(".")[0] == node_name and ob.type == "MESH" and ob.data.materials:
            material = ob.data.materials[0]
            break
    if material is not None:
        empty = [
            node.image.name
            for node in material.node_tree.nodes
            if node.type == "TEX_IMAGE" and (node.image is None or not node.image.has_data)
        ]
        if empty:
            reason = (
                f"imported material {material.name!r} has {len(empty)} texture(s) with no pixel "
                f"data ({', '.join(empty[:3])}) — Blender cannot read the KHR_texture_basisu KTX2 "
                f"images the bake writes"
            )
            material = None
        else:
            reason = f"decoded from {os.path.basename(l0_path)} (material {material.name!r})"
    for ob in imported:
        bpy.data.objects.remove(ob, do_unlink=True)
    return material, reason


def evaluated_source(obj):
    """L0: the EVALUATED RENDER SNAPSHOT of the artist's object (ADR 0033 §1).

    Modifier stack applied, world rotation and scale baked into the coordinates (translation is not:
    the chain is about a shape, not a placement). This mesh is never written anywhere and the .blend
    is never saved — it is read-only input from first line to last.
    """
    depsgraph = bpy.context.evaluated_depsgraph_get()
    mesh = bpy.data.meshes.new_from_object(obj.evaluated_get(depsgraph), depsgraph=depsgraph)
    matrix = obj.matrix_world.copy()
    matrix.translation = (0.0, 0.0, 0.0)
    surface = M.from_bpy_mesh(mesh, matrix, name="source")
    bpy.data.meshes.remove(mesh)
    return surface


# ── candidates ───────────────────────────────────────────────────────────────────────────────────

def _fresh_copy(obj, name):
    duplicate = obj.copy()
    duplicate.data = obj.data.copy()
    duplicate.name = name
    duplicate.parent = None
    duplicate.matrix_world = obj.matrix_world.copy()
    duplicate.matrix_world.translation = (0.0, 0.0, 0.0)
    bpy.context.scene.collection.objects.link(duplicate)
    return duplicate


def _drop(obj):
    data = obj.data
    bpy.data.objects.remove(obj, do_unlink=True)
    bpy.data.meshes.remove(data)


def _evaluated_triangles(obj):
    depsgraph = bpy.context.evaluated_depsgraph_get()
    evaluated = obj.evaluated_get(depsgraph)
    mesh = evaluated.to_mesh()
    count = sum(len(polygon.vertices) - 2 for polygon in mesh.polygons)
    evaluated.to_mesh_clear()
    return count


def _fit_collapse(obj, budget):
    """Drive a quadric collapse to at most `budget` triangles. Returns the triangles it reached.

    The ratio is an internal edge-count lever, not a triangle count, so the only honest way to hit
    an integer budget is to ask the modifier — 28 halvings resolve it to 4e-9. Returns None when the
    budget is below the mesh's topology floor, which the caller treats as the end of the ladder
    rather than as an error.
    """
    modifier = obj.modifiers.new("Collapse", "DECIMATE")
    modifier.decimate_type = "COLLAPSE"
    modifier.use_collapse_triangulate = True
    low, high, best = 0.0, 1.0, None
    for _ in range(28):
        middle = (low + high) / 2.0
        modifier.ratio = middle
        bpy.context.view_layer.update()
        count = _evaluated_triangles(obj)
        if count <= budget:
            best = (middle, count)
            low = middle
        else:
            high = middle
        if best and budget * 0.99 <= best[1] <= budget:
            break
    if best is None:
        modifier.ratio = 0.0
        bpy.context.view_layer.update()
        return None
    modifier.ratio = best[0]
    bpy.context.view_layer.update()
    return best[1]


def cleanup(mesh, scale_m):
    """Dissolve degenerate faces and zero-length edges, drop loose geometry, keep it triangles.

    RUN ON EVERY GENERATED LEVEL, BEFORE EXPORT. This is the same defect class that was just
    repaired in the source by hand: a collapse can leave an edge of length ~0, and the triangle it
    belongs to has no interior, no meaningful normal and — when its UV area collapses with it — no
    tangent, so bevy hands the shader a defaulted one and the level draws a wrongly lit band while
    passing every positional check.

    The dissolve distance is scale-relative (1e-6 of the mesh's bounding diagonal, sub-micron on a
    track shoe): large enough to catch a true degenerate, far too small to move a vertex anyone
    could measure. Everything is re-certified after this pass, on the shipped bytes, so the cleanup
    cannot smuggle in a change of its own.
    """
    bm = bmesh.new()
    bm.from_mesh(mesh)
    distance = max(scale_m * 1.0e-6, 1.0e-9)
    before = len(bm.faces)
    bmesh.ops.dissolve_degenerate(bm, dist=distance, edges=bm.edges[:])
    loose_verts = [v for v in bm.verts if not v.link_faces]
    if loose_verts:
        bmesh.ops.delete(bm, geom=loose_verts, context="VERTS")
    loose_edges = [e for e in bm.edges if not e.link_faces]
    if loose_edges:
        bmesh.ops.delete(bm, geom=loose_edges, context="EDGES")
    non_triangles = [f for f in bm.faces if len(f.verts) > 3]
    if non_triangles:
        bmesh.ops.triangulate(bm, faces=non_triangles)
    bm.normal_update()
    bm.to_mesh(mesh)
    bm.free()
    mesh.update()
    return {"faces_before": before, "faces_after": len(mesh.polygons),
            "dissolve_dist_m": distance}


def _baked(obj, name):
    depsgraph = bpy.context.evaluated_depsgraph_get()
    mesh = bpy.data.meshes.new_from_object(obj.evaluated_get(depsgraph), depsgraph=depsgraph)
    mesh.name = name
    return mesh


def candidate_mesh(source_obj, budget, scale_m):
    """One candidate: collapse to `budget` triangles, then the cleanup pass.

    Returns `(mesh, reached)` or `(None, None)`, where `reached` is the count the DECIMATOR landed
    on, before cleanup. The enumeration walks the budget axis by `reached - 1`, and that step is
    only sound against the decimator's own staircase: cleanup can dissolve a degenerate afterwards
    and lower the count, and stepping from the lowered number would jump over realizable outputs.
    """
    work = _fresh_copy(source_obj, f"cand_{budget}")
    try:
        reached = _fit_collapse(work, budget)
        if reached is None:
            return None, None
        mesh = _baked(work, f"cand_{budget}_mesh")
    finally:
        _drop(work)
    cleanup(mesh, scale_m)
    return mesh, reached


class Candidates:
    """Every mesh the decimator can actually produce, and the deviation of the ones worth measuring.

    ENUMERATION, NOT BISECTION. The previous version bisected feasible/infeasible budgets and then
    walked down in 2 % steps until the first failure — which assumes the very monotonicity this
    pipeline's own design doc says does not hold. A lower feasible ISLAND below an infeasible step
    is invisible to that search, so its winner was not proven minimal and a sparse-chain skip
    decision could be made against the wrong incumbent.

    The realized outputs are enumerated exactly instead, and cheaply. `_fit_collapse(B)` returns the
    largest realizable triangle count <= B; call it R. Then for every budget in [R, B] the answer is
    also R, because R is realizable and nothing realizable lies in (R, B]. So the next distinct
    output is at budget R-1, and walking `B <- R-1` from the ceiling to the floor visits EVERY
    realizable output exactly once, at one decimation each. Roughly two hundred on the reference
    asset, and the enumeration is shared by every rung of the chain.

    Deviation is then certified in increasing triangle order and the FIRST feasible output is the
    Pareto minimum, with no monotonicity assumed anywhere: everything smaller has been measured and
    rejected. Rejections are nearly free — a single sampled point above the target ends the proof —
    which is what makes an exhaustive scan affordable.
    """

    def __init__(self, source_obj, source_surface, gates):
        self.source_obj = source_obj
        self.source = source_surface
        self.gates = gates
        self.by_tris = {}
        self.outputs = []          # realized triangle counts, ascending
        self.evaluations = 0
        self.decimations = 0

    def enumerate_outputs(self, floor_tris, ceiling_tris):
        """Every realizable output in [floor, ceiling], ascending. One decimation per output."""
        budget = ceiling_tris
        found = []
        while budget >= floor_tris:
            mesh, reached = candidate_mesh(self.source_obj, budget, self.source.diagonal)
            self.decimations += 1
            if mesh is None:
                break
            surface = M.from_bpy_mesh(mesh, None, f"cand{budget}")
            bpy.data.meshes.remove(mesh)
            # `reached` steps the budget axis (the decimator's own staircase); `surface.tri_count`
            # is what the level would actually ship, after cleanup, and is what gets certified.
            shipped_tris = surface.tri_count
            if shipped_tris not in self.by_tris:
                # THE BUDGET IS KEPT, not just the count it produced. Rebuilding a chosen level
                # means re-running the decimator, and its input is a BUDGET; feeding back the
                # post-cleanup triangle count would ask for a different mesh than the one certified.
                self.by_tris[shipped_tris] = {
                    "tris": shipped_tris, "budget": budget, "surface": surface,
                    "lo_mm": None, "up_mm": None,
                }
                found.append(shipped_tris)
            if reached <= floor_tris:
                break
            budget = reached - 1
        self.outputs = sorted(set(found))
        log(f"  enumerated {len(self.outputs)} realizable outputs in [{floor_tris}, "
            f"{ceiling_tris}] from {self.decimations} decimations")
        return self.outputs

    def deviation(self, tris, target_mm):
        """Certified deviation for the realized output at `tris`, decisive for `target_mm`.

        Cached by triangle count, which is a proxy for the mesh: the enumeration produces exactly
        one mesh per count. A cached bracket is REUSED ONLY WHEN IT DECIDES the new target — an
        upper bound under it accepts, a sampled lower bound over it rejects — because the search
        stops as soon as the current rung's question is answered and a bracket measured against a
        coarse rung can be far too loose to answer a fine one.
        """
        entry = self.by_tris[tris]
        if entry["up_mm"] is not None and (
            entry["up_mm"] <= target_mm or entry["lo_mm"] > target_mm
        ):
            return entry
        started = time.time()
        result = M.certified_deviation(
            self.source, entry["surface"],
            self.gates["deviation_tol_m"], self.gates["deviation_max_nodes_search"],
            target_mm=target_mm, rel_tol=self.gates["deviation_rel_tol_search"],
        )
        self.evaluations += 1
        entry["lo_mm"] = result["mm"]
        entry["up_mm"] = result["mm_upper"]
        log(f"    probe {tris:>5} tris  dev in [{result['mm']:.3f}, {result['mm_upper']:.3f}] mm "
            f"vs target {target_mm:.3f}  {time.time() - started:.1f}s")
        return entry


# ── export ───────────────────────────────────────────────────────────────────────────────────────

def write_level_glb(mesh, node_name, path):
    """Export one mesh alone as the glb a chain level ships as.

    No material from either direction: the slots are cleared AND `export_materials='NONE'` is
    passed, because a level wears the source's material at bind time and a second copy in these
    bytes would be a second answer to how the asset looks. `export_tangents=False` for the same
    reason the source does not carry them: they are generated at bind from the UVs, which is why
    the tangent gate is about UV area rather than about a TANGENT accessor.
    """
    mesh.materials.clear()
    # The MESH DATABLOCK NAME lands in the glb's JSON chunk, so it has to be a property of the
    # level and not of how the level was found. It used to carry the search's scratch name — a
    # shipped asset containing `cand_860_mesh`, and worse, bytes that changed whenever the search
    # landed on a different budget for geometry that was identical. Naming it after the node makes
    # the file a function of the geometry alone, which is what the manifest's hash is supposed to
    # mean and what the double-generation check is supposed to prove.
    mesh.name = node_name
    obj = bpy.data.objects.new(node_name, mesh)
    if obj.name != node_name:
        bpy.data.objects.remove(obj, do_unlink=True)
        raise GenerationError(
            "export",
            f"the blend already holds an object called {node_name!r}; Blender renamed the export "
            f"copy to {obj.name!r}, which would change the node name in {os.path.basename(path)}",
        )
    bpy.context.scene.collection.objects.link(obj)
    try:
        for other in bpy.context.view_layer.objects:
            other.select_set(False)
        obj.select_set(True)
        bpy.context.view_layer.objects.active = obj
        result = bpy.ops.export_scene.gltf(
            filepath=path,
            export_format="GLB",
            use_selection=True,
            export_materials="NONE",
            export_normals=True,
            export_texcoords=True,
            export_tangents=False,
        )
        if "FINISHED" not in result:
            raise GenerationError("export", f"export_scene.gltf returned {result} for {path}")
    finally:
        bpy.data.objects.remove(obj, do_unlink=True)
    return path


# ── certification ────────────────────────────────────────────────────────────────────────────────

def sliver_floor_m(gates, source, source_validity):
    """The floor generated levels are held to: scale-aware, and anchored to the source's own worst.

    See `config.GATES["sliver_margin_vs_source"]` for why both halves are needed.
    """
    return max(
        gates["min_altitude_frac_of_diag"] * source.diagonal,
        source_validity["min_altitude_m"] / gates["sliver_margin_vs_source"],
    )


def validity_failures(validity, source_validity, gates):
    """The structural gates, as a list of named failures. Empty means clean.

    Split out of `certify` so the SHIPPED L0 bytes go through exactly the same checks the generated
    levels do. They did not before: L0's record was copied from the Blender source, so the one level
    the whole ladder is measured against was the only one never validated as it ships.
    """
    failures = []
    if gates["components_must_match"] and validity["components"] != source_validity["components"]:
        failures.append(
            f"component count {validity['components']} != source {source_validity['components']} "
            f"— a part vanished, and a vanished part has near-zero Hausdorff distance"
        )
    if validity["slivers_below_floor"] > 0:
        failures.append(
            f"{validity['slivers_below_floor']} triangle(s) below the scale-aware altitude floor "
            f"{validity['min_altitude_floor_m'] * 1000:.5f} mm "
            f"(worst {validity['min_altitude_m'] * 1000:.6f} mm)"
        )
    for key, limit, description in (
        ("duplicate_faces", gates["max_duplicate_faces"], "duplicate face(s)"),
        ("nonfinite_attrs", gates["max_nonfinite"], "non-finite attribute component(s)"),
        ("orientation_flips", gates["max_orientation_flips"],
         "edge(s) traversed the same way by both their faces — inconsistent winding"),
        ("nonmanifold_edges", gates["max_nonmanifold_edges"],
         "edge(s) shared by more than two faces — no consistent normal or tangent frame there, "
         "and a non-watertight volume bakes to zero armour silently"),
        ("tangent_default_faces", gates["max_tangent_default_faces"],
         "face(s) with degenerate UV area would take a DEFAULTED tangent at bind"),
        ("tangent_default_verts", gates["max_tangent_default_verts"],
         "vertex/vertices whose every incident face has degenerate UV area"),
    ):
        if validity[key] > limit:
            failures.append(f"{validity[key]} {description}")
    return failures


def certify(source, shipped, target_mm, gates, source_validity, floor_m):
    """Every numeric gate, on the decoded shipped bytes. Returns (report, failures)."""
    deviation = M.certified_deviation(
        source, shipped, gates["deviation_tol_m"], gates["deviation_max_nodes_certify"],
        rel_tol=gates["deviation_rel_tol_certify"],
    )
    validity = shipped.validity(gates, floor_m)
    failures = validity_failures(validity, source_validity, gates)
    if deviation["mm_upper"] > target_mm:
        failures.insert(0, (
            f"deviation upper bound {deviation['mm_upper']:.4f} mm exceeds the rung target "
            f"{target_mm:.4f} mm on the SHIPPED bytes"
        ))
    return {"deviation": deviation, "validity": validity}, failures


# ── the chain ────────────────────────────────────────────────────────────────────────────────────

def build_chain(asset, root, run_render_gate, out_dir):
    """Cut one asset's chain end to end and return its manifest entry."""
    try:
        blend = CONFIG.resolve_source(root, asset["blend"])
    except FileNotFoundError as missing:
        raise GenerationError("source", str(missing)) from missing
    log(f"asset  {asset['name']} <- {blend} :: {asset['object']}")
    bpy.ops.wm.open_mainfile(filepath=blend)

    obj = bpy.data.objects.get(asset["object"])
    material = refuse_unsupported(obj)
    source = evaluated_source(obj)
    source_validity = source.validity(CONFIG.GATES)
    log(f"  source {source.tri_count} tris  {source.vert_count} verts  "
        f"components={source_validity['components']}  "
        f"min_altitude={source_validity['min_altitude_m'] * 1000:.6f} mm  "
        f"tangent_default_faces={source_validity['tangent_default_faces']}  "
        f"radius={source.radius:.4f} m")
    if source_validity["slivers_below_floor"]:
        raise GenerationError(
            "source",
            f"the SOURCE carries {source_validity['slivers_below_floor']} triangle(s) below the "
            f"absolute scale-aware altitude floor "
            f"{source_validity['min_altitude_floor_m'] * 1000:.6f} mm — fix the .blend; a floor "
            f"the source fails is not a floor",
        )
    floor_m = sliver_floor_m(CONFIG.GATES, source, source_validity)
    log(f"  sliver floor for generated levels: {floor_m * 1000:.6f} mm "
        f"(source worst {source_validity['min_altitude_m'] * 1000:.6f} mm / margin "
        f"{CONFIG.GATES['sliver_margin_vs_source']:g}, absolute bound "
        f"{CONFIG.GATES['min_altitude_frac_of_diag'] * source.diagonal * 1000:.6f} mm)")

    probe = _fresh_copy(obj, "FloorProbe")
    modifier = probe.modifiers.new("C", "DECIMATE")
    modifier.decimate_type = "COLLAPSE"
    modifier.use_collapse_triangulate = True
    modifier.ratio = 0.0
    bpy.context.view_layer.update()
    floor_tris = _evaluated_triangles(probe)
    _drop(probe)
    log(f"  topology floor {floor_tris} tris (collapse at ratio 0)")

    # L0 IS THE SOURCE, and it ships inside another glb, so BOTH halves are proven on the decoded
    # bytes: that the surface is the same one, and that those bytes pass every validity gate the
    # generated levels pass. The validity record used to be copied from the Blender source, which
    # is a different mesh by construction — the exporter splits 815 Blender vertices into 3 888
    # glTF ones — so it was a measurement of something that does not ship.
    l0_path = os.path.join(root, asset["l0_glb"])
    if not os.path.isfile(l0_path):
        raise GenerationError("L0", f"{l0_path} does not exist; L0 has nowhere to ship")
    shipped_l0 = M.from_glb(l0_path, asset["l0_node"], "L0")
    identical, identity_reason = M.same_surface(source, shipped_l0)
    l0_dev_mm = M.vertex_deviation(source, shipped_l0)
    l0_validity = shipped_l0.validity(CONFIG.GATES, floor_m)
    l0_failures = validity_failures(l0_validity, source_validity, CONFIG.GATES)
    log(f"  L0     shipped in {asset['l0_glb']} :: {asset['l0_node']} — "
        f"{shipped_l0.tri_count} tris / {shipped_l0.vert_count} decoded verts, "
        f"vertex deviation {l0_dev_mm:.9f} mm, origin radius "
        f"{l0_validity['origin_radius_m']:.6f} m — {identity_reason}")
    if not identical or l0_dev_mm >= 1.0e-3:
        raise GenerationError(
            "L0",
            f"{asset['l0_glb']} does not ship the source for {asset['l0_node']!r}: "
            f"{identity_reason}; vertices sit {l0_dev_mm:.6f} mm off it. L0 IS the source "
            f"(ADR 0033 §1) — re-export the host glb before cutting a chain whose every deviation "
            f"is measured against a surface that does not ship.",
        )
    if l0_failures:
        raise GenerationError(
            "L0", f"the SHIPPED L0 bytes fail validity:\n    - " + "\n    - ".join(l0_failures)
        )
    l0 = {
        "level": 0, "rung": 0, "role": "source", "tris": shipped_l0.tri_count,
        "verts": shipped_l0.vert_count, "glb": asset["l0_glb"], "node": asset["l0_node"],
        "e_target_mm": 0.0, "dev_source_mm": 0.0, "dev_source_mm_upper": 0.0,
        "pairwise_mm": None, "switch_m": 0.0,
        "validity": l0_validity,
        "glb_sha256": sha256_file(l0_path),
        "blender_source_verts": source.vert_count,
        "shipped_dev_from_source_mm": round(l0_dev_mm, 9),
        "shipped_matches_source": True,
        "identity_proof": identity_reason,
    }

    candidates = Candidates(obj, source, CONFIG.GATES)
    # Once per asset, shared by every rung: the complete set of meshes this decimator can produce.
    candidates.enumerate_outputs(floor_tris, source.tri_count)
    levels = [l0]
    skipped = []
    previous = {"tris": l0["tris"], "surface": shipped_l0, "label": "L0",
                "validity": l0_validity}
    termination = "right_wall"

    for rung, target_mm in CONFIG.rungs():
        best = M.pareto_minimal(
            [t for t in candidates.outputs if t <= source.tri_count],
            candidates.deviation, target_mm,
        )
        if best is None:
            skipped.append({"rung": rung, "e_target_mm": round(target_mm, 4),
                            "reason": "no candidate meets the target",
                            "floor_tris": floor_tris})
            log(f"  rung {rung} e={target_mm:.3f} mm: no candidate clears it — skipped")
            continue
        shed = 1.0 - best["tris"] / previous["tris"]
        if shed < CONFIG.SKIP_FRACTION:
            skipped.append({
                "rung": rung, "e_target_mm": round(target_mm, 4), "best_tris": best["tris"],
                "shed_fraction": round(shed, 4),
                "reason": f"sheds {shed:.1%} of {previous['label']}, below SKIP_FRACTION "
                          f"{CONFIG.SKIP_FRACTION:.0%}",
            })
            log(f"  rung {rung} e={target_mm:.3f} mm: best {best['tris']} tris sheds "
                f"{shed:.1%} of {previous['label']} ({previous['tris']}) — SKIPPED (sparse chain)")
            continue

        level_index = len(levels)
        node_name = f"{asset['object']}_LOD{rung}"
        relpath = f"{asset['stem']}.rung{rung}.glb"
        log(f"  rung {rung} e={target_mm:.3f} mm: {best['tris']} tris "
            f"(sheds {shed:.1%} of {previous['label']}) -> {relpath}")

        mesh, _reached = candidate_mesh(obj, best["budget"], source.diagonal)
        if mesh is None:
            raise GenerationError("generate", f"rung {rung}: the chosen budget went below the floor")
        cleanup_report = cleanup(mesh, source.diagonal)
        staged = os.path.join(out_dir, os.path.basename(relpath))
        write_level_glb(mesh, node_name, staged)
        bpy.data.meshes.remove(mesh)

        # DOUBLE GENERATION, on the bytes. The whole level is built and exported a second time and
        # the two files are compared by hash. Recording a Blender version says which build produced
        # a chain; this says the pipeline produces the SAME chain twice, which is the property
        # anyone re-running it actually depends on. It costs a decimation and a 30 KB export —
        # the expensive half (certification) is not repeated.
        repeat_mesh, _ = candidate_mesh(obj, best["budget"], source.diagonal)
        cleanup(repeat_mesh, source.diagonal)
        repeat_path = os.path.join(out_dir, f"repeat_{os.path.basename(relpath)}")
        # The SAME node name: it lands in the JSON chunk, so a different one would guarantee a
        # different hash and prove nothing. `write_level_glb` frees the name on its way out.
        write_level_glb(repeat_mesh, node_name, repeat_path)
        bpy.data.meshes.remove(repeat_mesh)
        first_hash, second_hash = sha256_file(staged), sha256_file(repeat_path)
        os.remove(repeat_path)
        if first_hash != second_hash:
            raise GenerationError(
                "reproducibility",
                f"rung {rung} generated two different files from the same inputs "
                f"({first_hash[:16]} against {second_hash[:16]}) — the pipeline is not "
                f"deterministic, so no manifest it writes can be verified by regeneration",
            )

        shipped = M.from_glb(staged, None, f"rung{rung}")
        report, failures = certify(
            source, shipped, target_mm, CONFIG.GATES, source_validity, floor_m
        )
        pairwise = M.certified_deviation(
            previous["surface"], shipped, CONFIG.GATES["deviation_tol_m"],
            CONFIG.GATES["deviation_max_nodes_certify"],
            rel_tol=CONFIG.GATES["deviation_rel_tol_certify"],
        )
        if failures:
            raise GenerationError(
                "certify",
                f"rung {rung} ({relpath}) failed on the shipped bytes:\n    - "
                + "\n    - ".join(failures),
            )

        # The slack is the farthest vertex from the ORIGIN — the point `VisibilityRange` measures
        # to — taken over BOTH levels at this boundary, since either may be the one on screen there.
        slack = max(
            report["validity"]["origin_radius_m"],
            previous["validity"]["origin_radius_m"],
        )
        switch_from_source = CONFIG.switch_distance_m(report["deviation"]["mm_upper"], slack)
        switch_from_pairwise = CONFIG.switch_distance_m(pairwise["mm_upper"], slack)
        switch = max(switch_from_source, switch_from_pairwise)
        diagnostic = M.normal_angle_diagnostic(
            source, shipped, CONFIG.RENDER_GATE["normal_samples"]
        )
        log(f"         certified dev={report['deviation']['mm']:.3f} mm "
            f"(ub {report['deviation']['mm_upper']:.3f})  "
            f"pairwise vs {previous['label']}={pairwise['mm']:.3f} mm "
            f"(ub {pairwise['mm_upper']:.3f})  switch={switch:.1f} m  "
            f"normal p99={diagnostic['p99_deg']:.1f} deg (diagnostic)")

        levels.append({
            "level": level_index,
            "rung": rung,
            "role": "generated",
            "e_target_mm": round(target_mm, 6),
            "glb": relpath,
            "node": node_name,
            "staged": staged,
            "tris": shipped.tri_count,
            "verts": shipped.vert_count,
            "shed_fraction_vs_parent": round(shed, 6),
            "parent_level": previous["label"],
            "dev_source_mm": round(report["deviation"]["mm"], 6),
            "dev_source_mm_upper": round(report["deviation"]["mm_upper"], 6),
            "dev_source_bracket_mm": round(report["deviation"]["bracket_mm"], 6),
            "dev_source_to_level_mm": round(report["deviation"]["a_to_b_mm"], 6),
            "dev_level_to_source_mm": round(report["deviation"]["b_to_a_mm"], 6),
            "pairwise_mm": round(pairwise["mm"], 6),
            "pairwise_mm_upper": round(pairwise["mm_upper"], 6),
            "switch_m": round(switch, 4),
            "switch_from_source_dev_m": round(switch_from_source, 4),
            "switch_from_pairwise_m": round(switch_from_pairwise, 4),
            "validity": report["validity"],
            "cleanup": cleanup_report,
            "reproducible": True,
            "normal_diagnostic_deg": diagnostic,
        })
        previous = {"tris": shipped.tri_count, "surface": shipped,
                    "label": f"L{level_index}", "validity": report["validity"]}

        if best["tris"] <= floor_tris:
            termination = "topology_floor"
            log(f"  chain stops: rung {rung} reached the topology floor ({floor_tris} tris)")
            break
        if switch >= CONFIG.RIGHT_WALL_M:
            termination = "right_wall"
            log(f"  chain stops: rung {rung} is honest past the right wall "
                f"({switch:.0f} m >= {CONFIG.RIGHT_WALL_M:.0f} m)")
            break
    else:
        termination = "max_rungs"

    if termination == "topology_floor" and levels[-1]["switch_m"] < CONFIG.RIGHT_WALL_M:
        short = CONFIG.RIGHT_WALL_M - levels[-1]["switch_m"]
        log(f"  NOTE: the chain ends at the topology floor {short:.0f} m short of the right wall "
            f"({levels[-1]['switch_m']:.0f} m against {CONFIG.RIGHT_WALL_M:.0f} m). Beyond that "
            f"this asset cannot be made coarser by collapse at all — the last level simply keeps "
            f"rendering, which costs nothing extra and is the honest thing to record. The named "
            f"successor when this starts mattering is meshoptimizer (ADR 0033, Simplifier choice).")

    render_reports = []
    render_material_source = "not run"
    if run_render_gate and len(levels) > 1:
        pairs = []
        # L0's parent is the DECODED SHIPPED L0, not the Blender source surface: the gate renders
        # what ships, at every level of the chain including the top one.
        parents = [shipped_l0] + [
            M.from_glb(level["staged"], None, f"L{level['level']}") for level in levels[1:-1]
        ]
        for level, parent in zip(levels[1:], parents):
            child = M.from_glb(level["staged"], None, f"L{level['level']}")
            pairs.append((f"{asset['name']}_L{level['level']}", parent, child, level["switch_m"]))
        gate_material, render_material_source = shipped_material(l0_path, asset["l0_node"])
        if gate_material is None:
            render_material_source = (
                f"FELL BACK to the .blend material {material.name!r} — {render_material_source}"
            )
            gate_material = material
        log(f"  render gate material: {render_material_source}")
        log(f"  render gate: {len(pairs)} pair(s) x {len(CONFIG.RENDER_GATE['views'])} view(s), "
            f"Cycles {CONFIG.RENDER_GATE['samples']} spp at "
            f"{CONFIG.RENDER_GATE['tile_px']}x{CONFIG.RENDER_GATE['supersample']} px")
        render_reports = render_gate.compare(
            pairs, gate_material, CONFIG.RENDER_GATE, CONFIG.REFERENCE_VIEW,
            os.path.join(out_dir, "renders"),
        )
        for report in render_reports:
            report["material_source"] = render_material_source
        for level, report in zip(levels[1:], render_reports):
            level["render_gate"] = report
            log(f"         render L{level['level']} vs {level['parent_level']} at "
                f"{report['distance_m']:.0f} m: worst mean |dI|={report['worst_mean_abs_diff']:.4f} "
                f"frac>{CONFIG.RENDER_GATE['over_threshold']}={report['worst_frac_over']:.4f}  "
                f"defect score={report['worst_defect_score']:.3f} "
                f"(0=render noise, 1={CONFIG.RENDER_GATE['defect_normal_deg']:g}deg broken "
                f"normals) -> {'PASS' if report['pass'] else 'FAIL'}")
        bad = [r for r in render_reports if not r["pass"]]
        if bad and CONFIG.RENDER_GATE_BLOCKING:
            raise GenerationError(
                "render-gate",
                "the rendered difference exceeded the declared thresholds for: "
                + ", ".join(r["label"] for r in bad),
            )
        if bad:
            log("  ***********************************************************************")
            log(f"  RENDER GATE FAILED for {', '.join(r['label'] for r in bad)} — and did NOT")
            log("  block, because config.RENDER_GATE_BLOCKING is False: the defect_fraction it")
            log("  judges against is NOT RATIFIED. Every number is recorded per level in the")
            log("  manifest and this warning is repeated by `chain.py --verify`. Ratify the")
            log("  threshold and set RENDER_GATE_BLOCKING = True to make this a refusal.")
            log("  ***********************************************************************")

    return {
        "name": asset["name"],
        "source": {
            "blend": asset["blend"],
            "blend_sha256": sha256_file(blend),
            "object": asset["object"],
            "evaluated_digest": source.digest(),
            "tris": source.tri_count,
            "verts": source.vert_count,
            "radius_m": round(source.radius, 6),
            "bbox_mm": source_validity["bbox_mm"],
            "validity": source_validity,
        },
        "topology_floor_tris": floor_tris,
        "sliver_floor_m": floor_m,
        "termination": termination,
        "deviation_evaluations": candidates.evaluations,
        "skipped_rungs": skipped,
        "levels": levels,
        "material": material.name,
        "render_material_source": render_material_source,
    }, levels


# ── entry point ──────────────────────────────────────────────────────────────────────────────────

def main(argv):
    root = CONFIG.repo_root()
    only = None
    run_render_gate = True
    if "--asset" in argv:
        only = argv[argv.index("--asset") + 1]
    if "--no-render-gate" in argv:
        run_render_gate = False

    assert_toolchain()
    work = tempfile.mkdtemp(prefix="lod-chain-")
    manifest = {
        "schema": "overmatch.lod.manifest",
        "schema_version": 1,
        "generator": {
            "script": "scripts/lod/generate.py",
            "version": CONFIG.GENERATOR_VERSION,
            "sources_sha256": CONFIG.generator_digest(),
            "blender": f"{bpy.app.version_string} ({bpy.app.build_hash.decode()})",
            "blender_expected": CONFIG.EXPECTED_BLENDER,
            "gltf_exporter": _exporter_version(),
        },
        "ladder": {
            "e1_mm": CONFIG.E1_MM,
            "octave": CONFIG.OCTAVE,
            "skip_fraction": CONFIG.SKIP_FRACTION,
            "max_rungs": CONFIG.MAX_RUNGS,
            "right_wall_m": round(CONFIG.RIGHT_WALL_M, 6),
            "right_wall_source": CONFIG.RIGHT_WALL_SOURCE,
            "reference_view": CONFIG.REFERENCE_VIEW,
        },
        "gates": {
            "numeric": {k: v for k, v in CONFIG.GATES.items()},
            "render": {k: v for k, v in CONFIG.RENDER_GATE.items() if k != "views"},
            "render_views": [list(v) for v in CONFIG.RENDER_GATE["views"]],
            "render_gate_blocking": CONFIG.RENDER_GATE_BLOCKING,
            "render_gate_unratified_note": (
                "defect_fraction is not ratified; a failing rendered-difference gate is RECORDED "
                "and reported, not enforced. Every other gate blocks unconditionally."
            ) if not CONFIG.RENDER_GATE_BLOCKING else "",
        },
        "assets": [],
    }

    try:
        published = []
        for asset in CONFIG.ASSETS:
            if only and asset["name"] != only:
                continue
            entry, levels = build_chain(asset, root, run_render_gate, work)
            for level in levels[1:]:
                published.append((level["staged"], level["glb"], level))
            manifest["assets"].append(entry)
        if not manifest["assets"]:
            raise GenerationError("assets", f"no asset matched {only!r}")

        # Publish only now: a chain that failed a gate leaves every tracked path at its last good
        # state rather than half-updated.
        for staged, relpath, level in published:
            destination = os.path.join(root, relpath)
            os.makedirs(os.path.dirname(destination), exist_ok=True)
            shutil.move(staged, destination)
            level["glb_sha256"] = sha256_file(destination)
            level["glb_bytes"] = os.path.getsize(destination)
            level.pop("staged", None)
            log(f"publish {relpath}  {level['glb_bytes']} bytes  {level['glb_sha256'][:16]}")

        manifest_path = os.path.join(root, CONFIG.MANIFEST_RELPATH)
        with open(manifest_path, "w", encoding="utf-8") as handle:
            json.dump(manifest, handle, indent=2, sort_keys=False)
            handle.write("\n")
        log(f"manifest {CONFIG.MANIFEST_RELPATH}")
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return 0


if __name__ == "__main__":
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    try:
        code = main(argv)
    except SystemExit as exc:
        print(f"\nLOD GENERATION FAILED {exc}", file=sys.stderr)
        sys.stderr.flush()
        sys.stdout.flush()
        os._exit(1)
    except M.Refusal as exc:
        print(f"\nLOD GENERATION REFUSED [{exc.reason}] {exc.detail}", file=sys.stderr)
        sys.stderr.flush()
        sys.stdout.flush()
        os._exit(2)
    except BaseException:
        traceback.print_exc()
        sys.stdout.flush()
        sys.stderr.flush()
        os._exit(1)
    print("LOD-OK")
    sys.stdout.flush()
    os._exit(code)
