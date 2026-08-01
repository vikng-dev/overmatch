"""Cut every asset's error-ladder chain and write the manifest. The one entry point.

    /Applications/Blender.app/Contents/MacOS/Blender -b --factory-startup \
        --python scripts/lod/generate.py -- [--asset NAME] [--no-render-gate]

WHAT THIS IS. ADR 0033's generation stage: one global octave grid of deviation targets, a per-asset
SPARSE subset of it, triangle counts as outputs. Nothing in this file knows what a track shoe is —
the assets are rows in `config.ASSETS`, and every threshold is a constant in `config.py`.

THE SEARCH IS OVER UNIQUE INTEGER TRIANGLE TARGETS, NOT OVER RATIOS (ADR 0033 §5). Measured error
is not monotone in collapse ratio and the ratio->mesh map has long plateaus, so a ratio bisection
spends its evaluations re-measuring meshes it has already seen and can converge onto the wrong side
of a step. Candidates are therefore addressed by triangle budget, and cached by the HASH OF THE MESH
THEY PRODUCE: two budgets that alias to one mesh cost one deviation certification between them.

THE ORDER IS SACRED (ADR 0033 §6). Per level: decimate -> cleanup -> export -> decode the written
glb -> measure everything on those bytes. The search itself measures pre-export candidates, because
choosing which candidate to ship has to be cheap; but a chosen candidate is re-certified from the
file, and if the file disagrees with the search the level FAILS. The search is an optimiser, the
decode is the certificate.

WHAT FAILS THE RUN, LOUDLY (ADR 0033 §10): a skinned or morph-target source, a multi-material or
multi-primitive source, a level whose certified upper bound misses its rung after export, a level
that lost a component, a sliver below the scale-aware altitude floor, a defaulted tangent, a
duplicate face, a non-finite attribute, a flipped winding, or a rendered difference over the
declared thresholds. Nothing degrades silently.
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
    """One candidate: collapse to `budget` triangles, then the cleanup pass. Returns a mesh or None."""
    work = _fresh_copy(source_obj, f"cand_{budget}")
    try:
        reached = _fit_collapse(work, budget)
        if reached is None:
            return None
        mesh = _baked(work, f"cand_{budget}_mesh")
    finally:
        _drop(work)
    cleanup(mesh, scale_m)
    return mesh


class Candidates:
    """Deviation for a triangle budget, memoised by the HASH OF THE MESH the budget produced.

    THE PLATEAU IS THE REASON. The map from triangle budget to output mesh is a staircase: on the
    reference asset a run of adjacent budgets collapses to one identical mesh, and every one of them
    would otherwise pay for its own branch-and-bound. Keyed by mesh hash rather than by budget, a
    plateau costs exactly one certification however many budgets land on it.

    A CACHED BRACKET IS ONLY REUSED WHEN IT DECIDES. The search stops its branch-and-bound as soon
    as the current rung's question is answered, so a bracket measured against a 15.56 mm rung can be
    far too loose to answer a 3.89 mm one. Reuse is therefore conditional: an upper bound under the
    new target accepts, a sampled lower bound over it rejects, and anything in between re-runs the
    proof against the tighter target. Never the other way round — a loose bound must not decide a
    question it was not asked.
    """

    def __init__(self, source_obj, source_surface, gates):
        self.source_obj = source_obj
        self.source = source_surface
        self.gates = gates
        self.by_budget = {}
        self.by_hash = {}
        self.evaluations = 0

    def _mesh_for(self, budget):
        if budget in self.by_budget:
            return self.by_budget[budget]
        mesh = candidate_mesh(self.source_obj, budget, self.source.diagonal)
        if mesh is None:
            self.by_budget[budget] = None
            return None
        surface = M.from_bpy_mesh(mesh, None, f"cand{budget}")
        bpy.data.meshes.remove(mesh)
        entry = self.by_hash.setdefault(
            surface.digest(),
            {"tris": surface.tri_count, "surface": surface, "lo_mm": None, "up_mm": None},
        )
        self.by_budget[budget] = entry
        return entry

    def at(self, budget, target_mm):
        """The candidate at `budget`, with a bracket good enough to decide `target_mm`."""
        entry = self._mesh_for(budget)
        if entry is None:
            return None
        if entry["up_mm"] is not None and (
            entry["up_mm"] <= target_mm or entry["lo_mm"] > target_mm
        ):
            log(f"    probe budget={budget:>5} -> {entry['tris']:>5} tris  CACHED "
                f"[{entry['lo_mm']:.3f}, {entry['up_mm']:.3f}] mm")
            return entry
        started = time.time()
        deviation = M.certified_deviation(
            self.source, entry["surface"],
            self.gates["deviation_tol_m"], self.gates["deviation_max_nodes_search"],
            target_mm=target_mm, rel_tol=self.gates["deviation_rel_tol_search"],
        )
        self.evaluations += 1
        entry["lo_mm"] = deviation["mm"]
        entry["up_mm"] = deviation["mm_upper"]
        log(f"    probe budget={budget:>5} -> {entry['tris']:>5} tris  dev in "
            f"[{deviation['mm']:.3f}, {deviation['mm_upper']:.3f}] mm vs target "
            f"{target_mm:.3f}  {time.time() - started:.1f}s")
        return entry


def pareto_minimal(candidates, target_mm, floor_tris, ceiling_tris):
    """Fewest triangles whose certified UPPER bound clears `target_mm`. None if nothing does.

    Bisection over INTEGER TRIANGLE BUDGETS finds a feasible one; a downward refinement then walks
    past the plateau the bisection landed on, because the staircase means the first feasible budget
    a bisection finds is generally not the smallest one on its own step. Acceptance is on the upper
    bound throughout, so an unclosed bracket costs triangles and never honesty.
    """
    def feasible(budget):
        probe = candidates.at(budget, target_mm)
        return probe is not None and probe["up_mm"] <= target_mm

    low, high = floor_tris, ceiling_tris
    if not feasible(high):
        return None
    best = high
    while high - low > 1:
        middle = (low + high) // 2
        if feasible(middle):
            best = high = middle
        else:
            low = middle

    # Walk down past the plateau: 2 % steps until one stops clearing the target. Bounded, and every
    # repeat inside a plateau is a cache hit.
    walk = best
    while walk > floor_tris:
        step = max(1, int(round(candidates.at(walk, target_mm)["tris"] * 0.02)))
        nxt = max(floor_tris, walk - step)
        if nxt == walk or not feasible(nxt):
            break
        best = walk = nxt
    return candidates.at(best, target_mm)


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


def certify(source, shipped, target_mm, gates, source_validity, floor_m):
    """Every numeric gate, on the decoded shipped bytes. Returns (report, failures)."""
    deviation = M.certified_deviation(
        source, shipped, gates["deviation_tol_m"], gates["deviation_max_nodes_certify"],
        rel_tol=gates["deviation_rel_tol_certify"],
    )
    validity = shipped.validity(gates, floor_m)
    failures = []

    if deviation["mm_upper"] > target_mm:
        failures.append(
            f"deviation upper bound {deviation['mm_upper']:.4f} mm exceeds the rung target "
            f"{target_mm:.4f} mm on the SHIPPED bytes"
        )
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
    if validity["duplicate_faces"] > gates["max_duplicate_faces"]:
        failures.append(f"{validity['duplicate_faces']} duplicate face(s)")
    if validity["nonfinite_attrs"] > gates["max_nonfinite"]:
        failures.append(f"{validity['nonfinite_attrs']} non-finite attribute component(s)")
    if validity["orientation_flips"] > gates["max_orientation_flips"]:
        failures.append(
            f"{validity['orientation_flips']} edge(s) traversed the same way by both their faces "
            f"— inconsistent winding"
        )
    if validity["tangent_default_faces"] > gates["max_tangent_default_faces"]:
        failures.append(
            f"{validity['tangent_default_faces']} face(s) with degenerate UV area would take a "
            f"DEFAULTED tangent at bind"
        )
    if validity["tangent_default_verts"] > gates["max_tangent_default_verts"]:
        failures.append(
            f"{validity['tangent_default_verts']} vertex/vertices whose every incident face has "
            f"degenerate UV area"
        )
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

    # L0 is the source, and it ships inside another glb; certify those bytes against it.
    l0 = {"level": 0, "rung": 0, "role": "source", "tris": source.tri_count,
          "verts": source.vert_count, "glb": asset["l0_glb"], "node": asset["l0_node"],
          "e_target_mm": 0.0, "dev_source_mm": 0.0, "dev_source_mm_upper": 0.0,
          "pairwise_mm": None, "switch_m": 0.0, "validity": source_validity}
    l0_path = os.path.join(root, asset["l0_glb"])
    if os.path.isfile(l0_path):
        shipped_l0 = M.from_glb(l0_path, asset["l0_node"], "L0")
        l0_dev_mm = M.vertex_deviation(source, shipped_l0)
        l0["glb_sha256"] = sha256_file(l0_path)
        l0["shipped_tris"] = shipped_l0.tri_count
        l0["shipped_dev_from_source_mm"] = round(l0_dev_mm, 9)
        # Float32 in the buffer, on a 0.77 m part: a micron of quantisation is the noise floor of
        # "identical", and anything the exporter actually changed is thousands of times larger.
        l0["shipped_matches_source"] = bool(
            shipped_l0.tri_count == source.tri_count and l0_dev_mm < 1.0e-3
        )
        log(f"  L0     shipped in {asset['l0_glb']} :: {asset['l0_node']} — "
            f"{shipped_l0.tri_count} tris, vertex deviation from source {l0_dev_mm:.9f} mm  "
            f"{'IS the source' if l0['shipped_matches_source'] else 'IS NOT the source'}")
        if not l0["shipped_matches_source"]:
            raise GenerationError(
                "L0",
                f"{asset['l0_glb']} ships {shipped_l0.tri_count} triangles for "
                f"{asset['l0_node']!r} against the source's {source.tri_count}, and its vertices "
                f"sit {l0_dev_mm:.6f} mm off the evaluated source. L0 IS the source (ADR 0033 §1) "
                f"— re-export the host glb before cutting a chain whose every deviation is "
                f"measured against a surface that does not ship.",
            )
    else:
        raise GenerationError("L0", f"{l0_path} does not exist; L0 has nowhere to ship")

    candidates = Candidates(obj, source, CONFIG.GATES)
    levels = [l0]
    skipped = []
    previous = {"tris": source.tri_count, "surface": source, "label": "L0"}
    termination = "right_wall"

    for rung, target_mm in CONFIG.rungs():
        best = pareto_minimal(candidates, target_mm, floor_tris, source.tri_count)
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

        mesh = candidate_mesh(obj, best["tris"], source.diagonal)
        if mesh is None:
            raise GenerationError("generate", f"rung {rung}: the chosen budget went below the floor")
        cleanup_report = cleanup(mesh, source.diagonal)
        staged = os.path.join(out_dir, os.path.basename(relpath))
        write_level_glb(mesh, node_name, staged)
        bpy.data.meshes.remove(mesh)

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

        switch_from_source = CONFIG.switch_distance_m(report["deviation"]["mm_upper"], source.radius)
        switch_from_pairwise = CONFIG.switch_distance_m(pairwise["mm_upper"], source.radius)
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
            "normal_diagnostic_deg": diagnostic,
        })
        previous = {"tris": shipped.tri_count, "surface": shipped, "label": f"L{level_index}"}

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
    if run_render_gate and len(levels) > 1:
        pairs = []
        parents = [source] + [
            M.from_glb(level["staged"], None, f"L{level['level']}") for level in levels[1:-1]
        ]
        for level, parent in zip(levels[1:], parents):
            child = M.from_glb(level["staged"], None, f"L{level['level']}")
            pairs.append((f"{asset['name']}_L{level['level']}", parent, child, level["switch_m"]))
        log(f"  render gate: {len(pairs)} pair(s) x {len(CONFIG.RENDER_GATE['views'])} view(s), "
            f"Cycles {CONFIG.RENDER_GATE['samples']} spp at {CONFIG.RENDER_GATE['tile_px']} px")
        render_reports = render_gate.compare(
            pairs, material, CONFIG.RENDER_GATE, CONFIG.REFERENCE_VIEW,
            os.path.join(out_dir, "renders"),
        )
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

    work = tempfile.mkdtemp(prefix="lod-chain-")
    manifest = {
        "schema": "overmatch.lod.manifest",
        "schema_version": 1,
        "generator": {
            "script": "scripts/lod/generate.py",
            "version": CONFIG.GENERATOR_VERSION,
            "blender": f"{bpy.app.version_string} ({bpy.app.build_hash.decode()})",
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
