"""The error-ladder's generation half, as a library. `scripts/tank/chains.py` is the only driver.

NOT AN ENTRY POINT. It was one: a global `config.ASSETS` table, a corpus of sidecar glbs and
`assets/lod_manifest.json` beside them. ADR 0035 retired all three — the certificate is PER TANK
and the seam is the glTF PRIMITIVE, so the asset table, the manifest and the `--asset` merge went
with them. What is left is the machinery that cuts one surface's ladder, imported by the per-tank
build inside Blender.

WHAT THIS IS. ADR 0033's generation stage as amended by ADR 0036: one global octave grid of
deviation targets, a SPARSE subset of it per surface, triangle counts as outputs. Every threshold
is a constant in `config.py`.

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

WHAT FAILS THE RUN, LOUDLY (ADR 0033 §10): an unexpected Blender build; a level whose certified
upper bound misses its rung after export; a level that lost a component; an empty surface; a
duplicate face; a non-finite attribute; a flipped winding; and a level that does not reproduce
byte-for-byte when built twice. Nothing degrades silently.
"""

import hashlib
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import bmesh  # noqa: E402
import bpy  # noqa: E402

import config as CONFIG  # noqa: E402
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

