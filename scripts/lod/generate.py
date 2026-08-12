"""Cut every asset's error-ladder chain and write the manifest. The one entry point.

    /Applications/Blender.app/Contents/MacOS/Blender -b --factory-startup \
        --python scripts/lod/generate.py -- [--asset NAME]

WHAT THIS IS. ADR 0033's generation stage as amended by ADR 0036: one global octave grid of
deviation targets, a per-asset SPARSE subset of it, triangle counts as outputs. Nothing in this
file knows what a track shoe is — the assets are rows in `config.ASSETS`, and every threshold is a
constant in `config.py`.

THE SEARCH IS DIRECTED, AND ITS PREDICATE IS A BUDGETED BOOLEAN (ADR 0036 §1). Per rung, a
bisection over integer triangle BUDGETS, each probe realized by the same Blender collapse as
before and decided by `measure.fits_target` — PROVEN_FAIL at the first sampled witness over the
target, PROVEN_PASS when every live bound closes under it, UNDECIDED when the node budget runs out.
UNDECIDED, an invalid candidate and a budget below the topology floor are all treated as FAIL, so
they can cost triangles and never honesty.

WHAT THAT GIVES UP, ON THE RECORD. The exhaustive staircase this replaced proved the winner was the
FEWEST-triangle valid output meeting the rung. ADR 0036 §2 retires that claim deliberately: 85 % of
a reference run was measurement, the enforcement was the difference between unfinishable and
minutes, and the 5 % search bracket leaked minimality anyway — the directed search found a CHEAPER
answer at two rungs of the reference asset. The contract is now "deterministically found,
certified", and what the player depends on — the certified bound at the switch distance — is
untouched.

THE ORDER IS SACRED (ADR 0033 §6). Per level: decimate -> cleanup -> export -> decode the written
glb -> measure everything on those bytes. The search itself measures pre-export candidates, because
choosing which candidate to ship has to be cheap; but a chosen candidate is re-certified from the
file, and if the file disagrees with the search the level FAILS. The search is an optimiser, the
decode is the certificate. That includes L0, which is not generated but IS decoded: its identity
with the source is proven on welded topology, and its validity is measured on the shipped bytes
rather than inherited from the Blender mesh that produced them.

WHAT FAILS THE RUN, LOUDLY (ADR 0033 §10): an unexpected Blender build; a skinned or morph-target
source; a multi-material or multi-primitive source; a shipped L0 that is not the source; a level
whose certified upper bound misses its rung after export; a level that lost a component; an empty
surface; a duplicate face; a non-finite attribute; a flipped winding; and a level that does not
reproduce byte-for-byte when built twice. Nothing degrades silently.
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
import manifest as MANIFEST  # noqa: E402
import measure as M  # noqa: E402


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
    running = {
        "Blender version": (bpy.app.version_string.split()[0], CONFIG.EXPECTED_BLENDER),
        "Blender build": (bpy.app.build_hash.decode(), CONFIG.EXPECTED_BLENDER_BUILD),
        "glTF exporter": (_exporter_version(), CONFIG.EXPECTED_GLTF_EXPORTER),
    }
    wrong = [
        f"{what} is {actual!r}, pinned to {expected!r}"
        for what, (actual, expected) in running.items()
        if actual != expected
    ]
    if not wrong:
        return
    # ALL THREE, not just the version string. Two builds can report the same version, and the glTF
    # exporter is a separately-versioned add-on that decides the bytes — recording either without
    # comparing it is provenance theatre. Double generation inside one process cannot see a
    # cross-build difference at all, so this assertion is the only thing that can.
    allowed = os.environ.get(CONFIG.BLENDER_OVERRIDE_ENV, "")
    if allowed in ("1", "yes", "any"):
        log(f"  toolchain: {'; '.join(wrong)} — allowed by {CONFIG.BLENDER_OVERRIDE_ENV}")
        return
    raise GenerationError(
        "toolchain",
        "; ".join(wrong) + ". A simplifier is a program: a point release can move every level in "
        f"the game. Set {CONFIG.BLENDER_OVERRIDE_ENV}=1 to regenerate the whole corpus "
        f"deliberately, and update the config.EXPECTED_* constants with what it reports.",
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
    """The GREATEST realizable triangle count <= `budget`, with the modifier left driving it.

    THE CONTRACT IS "GREATEST", AND IT USED TO BE "WITHIN 1 %". The bisection stopped as soon as it
    landed inside a percent of the budget, which is fine if you only want a mesh of roughly that
    size and fatal for the search built on top of it: `measure.directed_rung_search` steps from an
    accepted candidate to `reached - 1`, and that step is only sound when `reached` is the greatest
    realizable count at or below the budget. An early stop meant outputs in `(reached, budget]` were
    never visited, so the search could step over the answer.

    The bisection itself is `measure.bisect_to_budget`, kept as a pure function of an injected
    evaluator so the property can be PROVEN on synthetic staircases whose answer is known
    independently. That separation is the point: a search built on top cannot establish this
    contract by asking about it, so it has to be established here, where it is arithmetic.

    Returns `(reached, ratio)`, or `(None, None)` below the mesh's topology floor.
    """
    modifier = obj.modifiers.new("Collapse", "DECIMATE")
    modifier.decimate_type = "COLLAPSE"
    modifier.use_collapse_triangulate = True

    def evaluate(ratio):
        modifier.ratio = ratio
        bpy.context.view_layer.update()
        return _evaluated_triangles(obj)

    count, ratio = M.bisect_to_budget(evaluate, budget)
    if count is None:
        modifier.ratio = 0.0
        bpy.context.view_layer.update()
        return None, None
    modifier.ratio = ratio
    bpy.context.view_layer.update()
    return count, ratio


def cleanup(mesh, scale_m):
    """Dissolve degenerate faces and zero-length edges, drop loose geometry, keep it triangles.

    RUN ON EVERY GENERATED LEVEL, BEFORE EXPORT. This is the same defect class that was just
    repaired in the source by hand: a collapse can leave an edge of length ~0, and the triangle it
    belongs to has no interior, no meaningful normal and — when its UV area collapses with it — no
    tangent, so bevy hands the shader a defaulted one and the level draws a wrongly lit band while
    passing every positional check.

    THE DISTANCE IS DECLARED IN `config.GATES["cleanup_dissolve_frac_of_diag"]` and it had to grow.
    At 1e-6 of the bounding diagonal (0.77 um here) it removed nothing real, and the decimator kept
    producing a NEEDLE — a 50 mm triangle 4.7 um thick — inherited from the source. That needle
    passes every positional gate honestly and still breaks the loader: after decimation the corner
    at its tip ends up with a fan of one, and mikktspace declines to give it a tangent.

    So the distance is now 1e-5 of the diagonal, 7.7 um on this shoe. What that spans is worth
    stating plainly: it removes the 4.7 um needle and leaves the next-thinnest features (21 um and
    56 um edges) untouched, and it is three orders of magnitude below e1 = 3.89 mm — the finest lie
    the ladder can even express.

    MEASURED CONSEQUENCE on the reference asset: one triangle left L1, L2 and L3 (855->854,
    581->580, 315->314). L4 did NOT change — it is at the topology floor and never contained the
    needle, which is also why it was the one level whose tangents were already clean. Every
    certified deviation is unchanged to six decimals. Everything is re-certified after this pass on
    the shipped bytes, so if it had moved anything that mattered the deviation gates would say so.
    """
    bm = bmesh.new()
    bm.from_mesh(mesh)
    distance = max(scale_m * CONFIG.GATES["cleanup_dissolve_frac_of_diag"], 1.0e-9)
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
    on, before cleanup. The search steps the budget axis by `reached - 1`, and that step is only
    sound against the decimator's own staircase: cleanup can dissolve a degenerate afterwards and
    lower the count, and stepping from the lowered number would jump over realizable outputs.
    """
    work = _fresh_copy(source_obj, f"cand_{budget}")
    try:
        reached, _ratio = _fit_collapse(work, budget)
        if reached is None:
            return None, None
        mesh = _baked(work, f"cand_{budget}_mesh")
    finally:
        _drop(work)
    cleanup(mesh, scale_m)
    return mesh, reached


class Directed:
    """Candidates realized ON DEMAND, and the rung predicate decided over integer budgets.

    THE CANDIDATE FAMILY IS UNCHANGED. `candidate_mesh` — the same Blender collapse, the same
    `bisect_to_budget` contract, the same cleanup — realizes every probe, and the same structural
    pre-filter admits it. What is gone is the promise to realize ALL of them: the exhaustive
    staircase cost 27x the decimations for a minimality claim ADR 0036 §2 retires.

    VALIDITY FIRST, THEN THE VERDICT. A candidate is admitted only if it already passes the
    structural gates; an invalid one never costs a verdict. This is a PRUNE, not a new gate — the
    checks are `measure.validity_gate_failures`, the same list the final certification calls — and
    every level that ships is still certified on its DECODED SHIPPED BYTES afterwards.

    THE TWO MEMOS ARE SOUND AND FREE. A candidate PROVEN under a target is proven under every LARGER
    target; a sampled witness over a target is a witness against every SMALLER one. Rung targets
    ascend, so the first is the one that fires — measured on the reference asset, rungs 8-12 cost
    zero verdicts because every candidate they ask about was already proven at a finer target.
    """

    def __init__(self, source_obj, source, gates, source_validity):
        self.obj = source_obj
        self.source = source
        self.gates = gates
        self.source_validity = source_validity
        self.by_budget = {}          # budget -> digest, or None below the topology floor
        self.by_digest = {}          # digest -> entry
        self.decimations = 0
        self.verdicts = 0
        self.verdict_nodes = 0
        self.undecided = 0

    def realize(self, budget):
        """The candidate at this budget, memoized by budget AND by geometry. None below the floor.

        KEYED BY GEOMETRY, NOT BY TRIANGLE COUNT. Two different cleaned meshes can carry the same
        count, and keying by the count discards one of them arbitrarily — possibly the only one that
        meets a rung. The digest is the identity; the count is an attribute.
        """
        if budget in self.by_budget:
            key = self.by_budget[budget]
            return None if key is None else self.by_digest[key]
        mesh, reached = candidate_mesh(self.obj, budget, self.source.diagonal)
        self.decimations += 1
        if mesh is None:
            self.by_budget[budget] = None
            return None
        surface = M.from_bpy_mesh(mesh, None, f"cand{budget}")
        bpy.data.meshes.remove(mesh)
        key = surface.digest()
        if key in self.by_digest:
            self.by_budget[budget] = key
            return self.by_digest[key]
        failures = M.validity_gate_failures(surface.validity(), self.source_validity, self.gates)
        # THE BUDGET IS KEPT, not just the count it produced. Rebuilding a chosen level means
        # re-running the decimator, and its input is a BUDGET; feeding back the post-cleanup
        # triangle count would ask for a different mesh than the one the verdict was about.
        entry = {
            "digest": key, "tris": surface.tri_count, "budget": budget, "reached": reached,
            "surface": surface, "valid": not failures,
            "failure": failures[0] if failures else None,
            "proven_le_mm": None, "witness_mm": 0.0,
        }
        self.by_digest[key] = entry
        self.by_budget[budget] = key
        return entry

    def verdict(self, entry, target_mm, node_budget):
        """The three-valued rung predicate for one candidate, with both memos applied."""
        if not entry["valid"]:
            return M.PROVEN_FAIL, 0, "invalid"
        if entry["proven_le_mm"] is not None and entry["proven_le_mm"] <= target_mm:
            return M.PROVEN_PASS, 0, "memo-pass"
        if entry["witness_mm"] > target_mm:
            return M.PROVEN_FAIL, 0, "memo-witness"
        result = M.fits_target(self.source, entry["surface"], target_mm, node_budget)
        self.verdicts += 1
        self.verdict_nodes += result["nodes"]
        entry["witness_mm"] = max(entry["witness_mm"], result["witness_mm"])
        if result["verdict"] == M.PROVEN_PASS:
            entry["proven_le_mm"] = (
                target_mm if entry["proven_le_mm"] is None
                else min(entry["proven_le_mm"], target_mm)
            )
        if result["verdict"] == M.UNDECIDED:
            self.undecided += 1
        return result["verdict"], result["nodes"], "measured"

    def search(self, target_mm, floor_tris, ceiling_tris, node_budget):
        """The winning entry for one rung and this rung's UNDECIDED count. `entry` may be None.

        THE UNDECIDED COUNT IS RETURNED SEPARATELY BECAUSE A LOST RUNG HAS TWO CAUSES AND THEY ARE
        NOT THE SAME FACT. "No structurally valid collapse output is inside this target" is the
        geometry answering; "the node budget ran out before a bound closed" is FIDELITY TRADED FOR
        TIME, and it must be loud in the manifest rather than indistinguishable from the first.
        """
        undecided = [0]

        def probe(budget):
            entry = self.realize(budget)
            if entry is None:
                log(f"    probe budget {budget:>6} -> below the topology floor")
                return None, M.PROVEN_FAIL
            verdict, nodes, how = self.verdict(entry, target_mm, node_budget)
            if verdict == M.UNDECIDED:
                undecided[0] += 1
            log(f"    probe budget {budget:>6} -> {entry['tris']:>6} tris  {verdict:<12} "
                f"{nodes:>8} nodes  ({how})")
            return entry["reached"], verdict

        budget = M.directed_rung_search(floor_tris, ceiling_tris, probe)
        return (None if budget is None else self.realize(budget)), undecided[0]




# ── export ───────────────────────────────────────────────────────────────────────────────────────

def write_level_glb(mesh, node_name, path):
    """Export one mesh alone as the glb a chain level ships as.

    No material from either direction: the slots are cleared AND `export_materials='NONE'` is
    passed, because a level wears the source's material at bind time and a second copy in these
    bytes would be a second answer to how the asset looks.

    TANGENTS ARE BAKED, and that is a correctness fix rather than an optimisation. They used to be
    left out and generated at bind, so the shipped bytes did not contain the thing that renders and
    the pipeline's tangent gate measured a PROXY for it — zero UV-degenerate faces, a necessary
    condition and not the loader's. Measured on this corpus: every level had zero UV-degenerate
    faces, and three of them still contained one corner that mikktspace gives up on and hands the
    shader a defaulted tangent. Baking closes the gap by construction — `bevy_gltf` generates
    tangents only when the attribute is ABSENT (`loader/mod.rs:838`), so what is certified here is
    what renders, and the gate can measure the values themselves.
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
            export_tangents=True,
        )
        if "FINISHED" not in result:
            raise GenerationError("export", f"export_scene.gltf returned {result} for {path}")
    finally:
        bpy.data.objects.remove(obj, do_unlink=True)
    return path


# ── certification ────────────────────────────────────────────────────────────────────────────────

def certify(source, shipped, target_mm, gates, source_validity):
    """Every numeric gate, on the decoded shipped bytes. Returns (report, failures)."""
    deviation = M.certified_deviation(
        source, shipped, gates["deviation_tol_m"], gates["deviation_max_nodes_certify"],
        rel_tol=gates["deviation_rel_tol_certify"],
    )
    validity = shipped.validity()
    failures = M.validity_gate_failures(validity, source_validity, gates)
    if deviation["mm_upper"] > target_mm:
        failures.insert(0, (
            f"deviation upper bound {deviation['mm_upper']:.4f} mm exceeds the rung target "
            f"{target_mm:.4f} mm on the SHIPPED bytes"
        ))
    return {"deviation": deviation, "validity": validity}, failures


# ── the chain ────────────────────────────────────────────────────────────────────────────────────

def build_chain(asset, root, out_dir):
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
    source_validity = source.validity()
    log(f"  source {source.tri_count} tris  {source.vert_count} verts  "
        f"components={source_validity['components']}  "
        f"diagonal={source.diagonal * 1000.0:.1f} mm  radius={source.radius:.4f} m")

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
    l0_validity = shipped_l0.validity()
    l0_failures = M.validity_gate_failures(l0_validity, source_validity, CONFIG.GATES)
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
        # The geometry fingerprint the VERIFIER re-derives from these bytes. Recorded here, where
        # L0 has just been proven identical to the evaluated .blend source, so the number carries
        # that proof forward to a verifier that cannot run Blender.
        "welded_digest": shipped_l0.welded_digest(),
    }

    directed = Directed(obj, source, CONFIG.GATES, source_validity)
    diagonal_mm = source.diagonal * 1000.0
    levels = [l0]
    skipped = []
    previous = {"tris": l0["tris"], "surface": shipped_l0, "label": "L0",
                "validity": l0_validity}
    termination = "right_wall"

    for rung, target_mm in CONFIG.rungs():
        node_budget = CONFIG.verdict_node_budget(diagonal_mm, target_mm)
        log(f"  rung {rung} e={target_mm:.3f} mm  node budget {node_budget}/direction")
        best, undecided = directed.search(target_mm, floor_tris, source.tri_count, node_budget)
        if best is None:
            # BUDGET-EXHAUSTED IS NOT STRUCTURALLY INFEASIBLE. A rung lost to a spent node budget is
            # a rung an unbounded search might have kept: nothing unproven ships either way, but the
            # chain is coarser for a reason that is about cost rather than about the geometry, and
            # the manifest has to say which.
            budget_bound = undecided > 0
            skipped.append({
                "rung": rung, "e_target_mm": round(target_mm, 4),
                "reason": (
                    f"{undecided} verdict(s) spent the whole {node_budget}-node budget without "
                    f"closing a bound; UNDECIDED counts as FAIL, so this rung is lost to the "
                    f"budget rather than to the geometry"
                ) if budget_bound else "no structurally valid candidate meets the target",
                "lost_to": "verdict_node_budget" if budget_bound else "geometry",
                "undecided_verdicts": undecided,
                "verdict_node_budget": node_budget,
                "floor_tris": floor_tris,
            })
            log(f"  rung {rung} e={target_mm:.3f} mm: no candidate clears it — skipped "
                f"({'BUDGET-EXHAUSTED' if budget_bound else 'structurally infeasible'})")
            continue
        shed = 1.0 - best["tris"] / previous["tris"]
        if shed < CONFIG.SKIP_FRACTION:
            skipped.append({
                "rung": rung, "e_target_mm": round(target_mm, 4), "best_tris": best["tris"],
                "shed_fraction": round(shed, 4),
                "reason": f"sheds {shed:.1%} of {previous['label']}, below SKIP_FRACTION "
                          f"{CONFIG.SKIP_FRACTION:.0%}",
                "lost_to": "skip_fraction",
                "undecided_verdicts": undecided,
                "verdict_node_budget": node_budget,
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
            source, shipped, target_mm, CONFIG.GATES, source_validity
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
            source, shipped, CONFIG.NORMAL_DIAGNOSTIC_SAMPLES
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
            # The budget the rung's verdicts ran under. Re-derived by the verifier from the source's
            # bounding box and the rung target, so a manifest cannot claim a search that this tree's
            # constants would not have run.
            "verdict_node_budget": node_budget,
            "undecided_verdicts": undecided,
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
        "termination": termination,
        # The directed search's own run counters. `undecided` is the one to watch: it is the number
        # of verdicts that spent their whole budget and were treated as failures, so a non-zero
        # value means the chain is coarser than an unbounded search would have found it.
        "decimations": directed.decimations,
        "verdicts": directed.verdicts,
        "verdict_nodes": directed.verdict_nodes,
        "undecided_verdicts": directed.undecided,
        "distinct_candidates": len(directed.by_digest),
        "skipped_rungs": skipped,
        "levels": levels,
        "material": material.name,
    }, levels


def merge_targeted(regenerated, root, only, generator):
    """Fold a `--asset` run back into the committed manifest, or refuse. Returns the full list.

    The shape logic is `chain.merge_asset_entries` (testable without Blender); this half is only
    about finding the manifest to merge into and refusing loudly when there is not one.
    """
    if len(CONFIG.ASSETS) == 1:
        return regenerated
    manifest_path = os.path.join(root, CONFIG.MANIFEST_RELPATH)
    if not os.path.isfile(manifest_path):
        raise GenerationError(
            "assets",
            f"--asset {only!r} regenerates one chain, but {CONFIG.MANIFEST_RELPATH} does not exist "
            f"and the other {len(CONFIG.ASSETS) - 1} configured asset(s) have nowhere to come "
            f"from. Run a full generation first.",
        )
    with open(manifest_path, encoding="utf-8") as handle:
        previous = json.load(handle)
    try:
        merged = MANIFEST.merge_asset_entries(
            regenerated, previous.get("assets", []),
            [asset["name"] for asset in CONFIG.ASSETS],
            MANIFEST.asset_provenance(generator), previous.get("generator"),
        )
    except ValueError as exc:
        raise GenerationError("assets", str(exc)) from exc
    log(f"merged  {only} into the existing manifest, carrying "
        f"{len(merged) - 1} other asset chain(s) forward")
    return merged


# ── entry point ──────────────────────────────────────────────────────────────────────────────────

def main(argv):
    root = CONFIG.repo_root()
    only = None
    if "--asset" in argv:
        only = argv[argv.index("--asset") + 1]

    assert_toolchain()
    work = tempfile.mkdtemp(prefix="lod-chain-")
    manifest = {
        "schema": "overmatch.lod.manifest",
        "schema_version": CONFIG.SCHEMA_VERSION,
        "generator": {
            "script": "scripts/lod/generate.py",
            "version": CONFIG.GENERATOR_VERSION,
            "sources_sha256": CONFIG.generator_digest(),
            "blender": bpy.app.version_string.split()[0],
            "blender_build": bpy.app.build_hash.decode(),
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
            "normal_diagnostic_samples": CONFIG.NORMAL_DIAGNOSTIC_SAMPLES,
            "verdict_nodes_per_square": CONFIG.VERDICT_NODES_PER_SQUARE,
            "verdict_nodes_cap": CONFIG.VERDICT_NODES_CAP,
        },
        "assets": [],
    }

    try:
        published = []
        for asset in CONFIG.ASSETS:
            if only and asset["name"] != only:
                continue
            entry, levels = build_chain(asset, root, work)
            for level in levels[1:]:
                published.append((level["staged"], level["glb"], level))
            manifest["assets"].append(entry)
        if not manifest["assets"]:
            raise GenerationError("assets", f"no asset matched {only!r}")
        if only:
            manifest["assets"] = merge_targeted(
                manifest["assets"], root, only, manifest["generator"]
            )

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
