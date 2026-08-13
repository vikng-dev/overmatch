"""Cut one tank's LOD chains, per SOURCE PRIMITIVE, inside Blender.

    blender -b --factory-startup --python scripts/tank/chains.py -- \
        --glb <candidate.glb> --out <dir> --select <digest> [--select <digest> ...]

NOT AN ENTRY POINT ANYONE RUNS. `scripts/tank/build.py` drives it: it decodes the candidate, groups
its primitives by source-geometry digest, and hands each worker the digests it owns. A worker writes
one directory per chain into `--out` — `rung<N>.glb` beside the `chain.json` that is landed last —
and nothing else.

THE SEAM IS THE PRIMITIVE (ADR 0035). ADR 0033 §10's multi-primitive refusal is retired here: a
mesh's primitives are addressed one at a time, so a multi-material object is several chains rather
than a refusal, and the three multi-primitive tiger meshes carry 48 % of its unique triangles.

RUNG 0 IS THE DECODED SHIPPED PRIMITIVE. The candidate this reads is the one the door just certified
and the one that ships, so the surface every deviation is measured against is the surface that
renders — L0 identity is by construction rather than by proof.

THE SEARCH, THE GATES AND THE EXPORT ARE `scripts/lod/generate.py`'S, imported rather than restated:
`Directed` (the budgeted Boolean bisection of ADR 0036 §1), `candidate_mesh`, `cleanup`,
`write_level_glb` and `certify` are one implementation with two drivers.
"""

import argparse
import json
import os
import sys
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "lod"))

import numpy as np  # noqa: E402

import bpy  # noqa: E402

import config as CONFIG  # noqa: E402
import generate as GEN  # noqa: E402
import measure as M  # noqa: E402
import trio as TRIO  # noqa: E402


def log(line):
    print(line, flush=True)


class ChainError(SystemExit):
    """A named failure. SystemExit so `blender --background` reports it through its exit code — an
    unhandled Exception in a background Blender prints a traceback and still exits 0."""

    def __init__(self, stage, message):
        super().__init__(f"[{stage}] {message}")


# ── the source primitive as a Blender object ─────────────────────────────────────────────────────

def blender_uv(corner_uv):
    """Decoded glTF corner UVs in Blender's convention. V runs top-down in glTF and bottom-up in
    Blender, and the exporter applies `1 - v` on the way out — so the write-in must apply it too,
    or every re-exported rung ships V-flipped against its L0."""
    uv = np.asarray(corner_uv, dtype=np.float64).copy()
    uv[..., 1] = 1.0 - uv[..., 1]
    return uv


def object_from_surface(surface, name):
    """A Blender object holding the decoded primitive, welded back to its authored topology.

    THE WELD IS LOAD-BEARING. glTF splits a corner per distinct (position, normal, uv), so the
    shipped Link is 4 747 vertices out of 815 authored ones — fed to the decimator unwelded, every
    triangle is its own island and collapse has no interior edge to contract. Positions weld at a
    nanometre; the normals and UVs stay PER CORNER, which is where the split lived anyway.
    """
    weld = surface.welded()
    count = int(weld.max()) + 1 if len(weld) else 0
    positions = np.zeros((count, 3), dtype=np.float64)
    positions[weld] = surface.verts
    faces = weld[surface.tri_v]
    degenerate = int(
        ((faces[:, 0] == faces[:, 1]) | (faces[:, 1] == faces[:, 2]) | (faces[:, 2] == faces[:, 0]))
        .sum()
    )
    if degenerate:
        raise ChainError("source", f"{name}: {degenerate} triangle(s) collapse under the weld")

    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(
        [tuple(row) for row in positions], [], [tuple(int(i) for i in f) for f in faces]
    )
    mesh.update()
    if len(mesh.polygons) != len(faces):
        raise ChainError(
            "source",
            f"{name}: {len(faces)} triangle(s) went in and {len(mesh.polygons)} came out — the "
            f"decode and the Blender mesh are not the same surface",
        )
    layer = mesh.uv_layers.new(name="UVMap")
    layer.uv.foreach_set("vector", blender_uv(surface.corner_uv).reshape(-1).astype(np.float32))
    mesh.normals_split_custom_set(
        [tuple(row) for row in surface.corner_n.reshape(-1, 3)]
    )
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.scene.collection.objects.link(obj)
    return obj


def topology_floor(obj):
    """The triangle count a collapse at ratio 0 reaches — below it no candidate exists."""
    probe = GEN._fresh_copy(obj, "FloorProbe")  # noqa: SLF001 — one copy helper, two drivers
    modifier = probe.modifiers.new("C", "DECIMATE")
    modifier.decimate_type = "COLLAPSE"
    modifier.use_collapse_triangulate = True
    modifier.ratio = 0.0
    bpy.context.view_layer.update()
    count = GEN._evaluated_triangles(probe)  # noqa: SLF001
    GEN._drop(probe)  # noqa: SLF001
    return count


# ── one chain ────────────────────────────────────────────────────────────────────────────────────

#: What a cut chain is called inside its own directory. Its PRESENCE is what the build reads as
#: "this chain is cached", so it is written last and it is written atomically.
RECORD = "chain.json"


def write_record(path, record):
    """The chain's record, landed by a rename inside its own directory.

    A worker killed between `open` and the last byte leaves a truncated file at the name the next
    build treats as a complete chain. A rename cannot be observed half-done, so the name either does
    not exist or names every byte. `allow_nan=False` refuses to write a token no strict reader
    parses — a deviation that is not a number is not a measurement.
    """
    staging = path + ".partial"
    with open(staging, "w", encoding="utf-8") as handle:
        json.dump(record, handle, indent=2, sort_keys=False, allow_nan=False)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(staging, path)


def log_fidelity_loss(digest, rung, target_mm, lost_to, undecided, node_budget):
    """Announce a rung the RUN lost, at the volume the certification bracket already gets.

    THE TWO FIDELITY LOSSES ARE ONE CLASS: a rung lost to the node budget (`M.LOST_TO_BUDGET`) and
    one lost to a certification bracket (`TRIO.LOST_TO_BRACKET`) both cost a level that a longer run
    would have shipped. Only the bracket said so out loud, so a build that traded fidelity for wall
    time on the budget was silent about it at the one moment a human is watching. The record carries
    the trade either way — what a build-time silence changes is whether anyone NOTICES. Neither
    fails a build: nothing unproven ships.

    A rung lost to the GEOMETRY is not this class and is not logged — no run length recovers it.
    """
    if lost_to != M.LOST_TO_BUDGET:
        return
    log(f"  {digest[:12]} rung {rung} e={target_mm:.3f} mm: {undecided} verdict(s) spent the whole "
        f"{node_budget}-node budget without closing a bound — rung LOST to the node budget, not to "
        f"the geometry")


def rung_node_name(digest, rung):
    """What a rung glb calls its one node. A function of the GEOMETRY and the rung, so a cached
    rung is valid however the meshes that share it are named."""
    return f"LOD{rung}_{digest[:16]}"


def cut_chain(digest, surface, out_dir):
    """One chain: rung 0 is `surface`, and every rung the ladder earns is exported beside it.

    The stop rules are ADR 0033's: a rung that sheds less than `SKIP_FRACTION` of the previous kept
    level is not a level, the chain ends at the topology floor, and it ends when a level is honest
    past the right wall. Every level is certified on its DECODED SHIPPED BYTES, and every winner is
    generated twice and must reproduce byte-identically.
    """
    started = time.monotonic()
    name = f"src_{digest[:16]}"
    obj = object_from_surface(surface, name)
    source_validity = surface.validity()
    floor_tris = topology_floor(obj)
    log(f"  {digest[:12]} {surface.tri_count} tris  floor {floor_tris}  "
        f"diagonal {surface.diagonal * 1000.0:.1f} mm  components {source_validity['components']}")

    directed = GEN.Directed(obj, surface, CONFIG.GATES, source_validity)
    diagonal_mm = surface.diagonal * 1000.0
    rungs = []
    skipped = []
    previous = {"tris": surface.tri_count, "surface": surface, "label": "L0",
                "validity": source_validity}
    termination = "right_wall"

    for rung, target_mm in CONFIG.rungs():
        node_budget = CONFIG.verdict_node_budget(diagonal_mm, target_mm)
        best, undecided = directed.search(target_mm, floor_tris, surface.tri_count, node_budget)
        # WHAT A LOST RUNG IS LOST TO IS `M.rung_lost_to`'S ANSWER, on BOTH paths. Any UNDECIDED
        # verdict outranks every other explanation: the winner the search settled on won partly by
        # default, so a shed fraction measured against it is a comparison that should not have been
        # the last word. Spelling the precedence here — or, as the sparse-chain path did, not
        # spelling it at all — is how the writer came to file a budget-lost rung as an ordinary skip
        # and silence the loss it should have announced.
        if best is None:
            lost_to = M.rung_lost_to(undecided, "geometry")
            skipped.append({
                "rung": rung, "e_target_mm": round(target_mm, 4), "lost_to": lost_to,
                "undecided_verdicts": undecided, "verdict_node_budget": node_budget,
            })
            log_fidelity_loss(digest, rung, target_mm, lost_to, undecided, node_budget)
            continue
        shed = 1.0 - best["tris"] / previous["tris"]
        if shed < CONFIG.SKIP_FRACTION:
            lost_to = M.rung_lost_to(undecided, "skip_fraction")
            skipped.append({
                "rung": rung, "e_target_mm": round(target_mm, 4), "best_tris": best["tris"],
                "shed_fraction": round(shed, 4), "lost_to": lost_to,
                "undecided_verdicts": undecided, "verdict_node_budget": node_budget,
            })
            log_fidelity_loss(digest, rung, target_mm, lost_to, undecided, node_budget)
            continue

        node_name = rung_node_name(digest, rung)
        staged = os.path.join(out_dir, f"rung{rung}.glb")
        mesh, _reached = GEN.candidate_mesh(obj, best["budget"], surface.diagonal)
        if mesh is None:
            raise ChainError("generate", f"rung {rung}: the chosen budget went below the floor")
        GEN.cleanup(mesh, surface.diagonal)
        GEN.write_level_glb(mesh, node_name, staged)
        bpy.data.meshes.remove(mesh)

        repeat_mesh, _ = GEN.candidate_mesh(obj, best["budget"], surface.diagonal)
        GEN.cleanup(repeat_mesh, surface.diagonal)
        repeat_path = os.path.join(out_dir, f"repeat.rung{rung}.glb")
        GEN.write_level_glb(repeat_mesh, node_name, repeat_path)
        bpy.data.meshes.remove(repeat_mesh)
        first, second = GEN.sha256_file(staged), GEN.sha256_file(repeat_path)
        os.remove(repeat_path)
        if first != second:
            raise ChainError(
                "reproducibility",
                f"{digest[:12]} rung {rung} generated two different files from the same inputs "
                f"({first[:16]} against {second[:16]})",
            )

        shipped = M.from_glb(staged, None, f"rung{rung}")
        report, failures = GEN.certify(surface, shipped, target_mm, CONFIG.GATES, source_validity)
        # WHICH OF THE TWO CAUSES A MISS HAS is `trio.rung_certification` — one law, away from
        # `bpy`, so both sides of the boundary are driven by a test rather than by the corpus
        # happening to contain one.
        structural = M.validity_gate_failures(report["validity"], source_validity, CONFIG.GATES)
        verdict = TRIO.rung_certification(report["deviation"], structural, target_mm)
        if verdict == TRIO.FAILS_THE_RUN:
            raise ChainError(
                "certify",
                f"{digest[:12]} rung {rung} failed on the shipped bytes:\n    - "
                + "\n    - ".join(failures or ["the certified deviation is not a finite number"]),
            )
        if verdict == TRIO.LOST_TO_BRACKET:
            os.remove(staged)
            skipped.append({
                "rung": rung, "e_target_mm": round(target_mm, 4), "best_tris": best["tris"],
                "lost_to": TRIO.LOST_TO_BRACKET,
                "dev_mm": round(report["deviation"]["mm"], 6),
                "dev_mm_upper": round(report["deviation"]["mm_upper"], 6),
                "bracket_mm": round(report["deviation"]["bracket_mm"], 6),
                "undecided_verdicts": undecided, "verdict_node_budget": node_budget,
            })
            log(f"  {digest[:12]} rung {rung} e={target_mm:.3f} mm: certified upper bound "
                f"{report['deviation']['mm_upper']:.4f} mm misses the rung on a bracket whose "
                f"lower end is {report['deviation']['mm']:.4f} mm — rung LOST to the bracket")
            continue
        pairwise = M.certified_deviation(
            previous["surface"], shipped, CONFIG.GATES["deviation_tol_m"],
            CONFIG.GATES["deviation_max_nodes_certify"],
            rel_tol=CONFIG.GATES["deviation_rel_tol_certify"],
        )
        # THE CERTIFIED DEVIATION IS THE WORSE OF TWO BOUNDS (ADR 0033 §4). Source-relative is the
        # level's own lie; pairwise is how far it can sit from the level it replaces, which is the
        # pop. One number ships, so it is the one that prices the switch.
        deviation_mm = max(report["deviation"]["mm_upper"], pairwise["mm_upper"])
        slack = max(report["validity"]["origin_radius_m"], previous["validity"]["origin_radius_m"])
        switch = CONFIG.switch_distance_m(deviation_mm, slack)
        diagnostic = M.normal_angle_diagnostic(surface, shipped, CONFIG.NORMAL_DIAGNOSTIC_SAMPLES)
        log(f"  {digest[:12]} rung {rung} e={target_mm:.3f} mm -> {shipped.tri_count} tris  "
            f"dev {deviation_mm:.3f} mm  switch {switch:.0f} m  "
            f"normal p99 {diagnostic['p99_deg']:.1f} deg")

        rungs.append({
            "rung": rung,
            "glb": os.path.basename(staged),
            "node": node_name,
            "sha256": first,
            "tris": shipped.tri_count,
            "verts": shipped.vert_count,
            "e_target_mm": round(target_mm, 6),
            "deviation_mm": round(deviation_mm, 6),
            "dev_source_mm_upper": round(report["deviation"]["mm_upper"], 6),
            "pairwise_mm_upper": round(pairwise["mm_upper"], 6),
            "shed_fraction_vs_parent": round(shed, 6),
            "switch_m": round(switch, 4),
            "origin_radius_m": report["validity"]["origin_radius_m"],
            "undecided_verdicts": undecided,
            "verdict_node_budget": node_budget,
            "validity": report["validity"],
            "normal_diagnostic_deg": diagnostic,
        })
        previous = {"tris": shipped.tri_count, "surface": shipped, "label": f"L{len(rungs)}",
                    "validity": report["validity"]}

        if best["tris"] <= floor_tris:
            termination = "topology_floor"
            break
        if switch >= CONFIG.RIGHT_WALL_M:
            termination = "right_wall"
            break
    else:
        termination = "max_rungs"

    GEN._drop(obj)  # noqa: SLF001
    return {
        "digest": digest,
        "source": {
            "tris": surface.tri_count, "verts": surface.vert_count,
            "diagonal_mm": round(diagonal_mm, 4),
            "origin_radius_m": source_validity["origin_radius_m"],
            "validity": source_validity,
        },
        "topology_floor_tris": floor_tris,
        "termination": termination,
        "decimations": directed.decimations,
        "verdicts": directed.verdicts,
        "verdict_nodes": directed.verdict_nodes,
        "undecided_verdicts": directed.undecided,
        "distinct_candidates": len(directed.by_digest),
        "skipped_rungs": skipped,
        "rungs": rungs,
        "seconds": round(time.monotonic() - started, 3),
    }


# ── entry point ──────────────────────────────────────────────────────────────────────────────────

def main(argv):
    parser = argparse.ArgumentParser(prog="chains.py", allow_abbrev=False)
    parser.add_argument("--glb", required=True, help="the certified candidate rung 0 is read from")
    parser.add_argument("--out", required=True, help="where the rung glbs and their records land")
    parser.add_argument("--select", action="append", default=[],
                        help="a source-geometry digest this worker owns; repeatable")
    arguments = parser.parse_args(argv)

    GEN.assert_toolchain()
    with open(arguments.glb, "rb") as handle:
        blob = handle.read()
    gltf, binary = M.glb_chunks_from_bytes(blob, arguments.glb)
    rows = TRIO.census(gltf, binary)
    wanted = set(arguments.select)
    by_digest = {}
    for row in rows:
        if row.get("digest") in wanted and row["digest"] not in by_digest:
            by_digest[row["digest"]] = TRIO.primitive_surface(
                gltf, binary, row["mesh_index"], row["primitive"], row["chain"]
            )
    missing = wanted - set(by_digest)
    if missing:
        raise ChainError("select", f"{arguments.glb} holds no primitive digesting to "
                                   f"{', '.join(sorted(missing))}")
    os.makedirs(arguments.out, exist_ok=True)
    for digest in sorted(by_digest):
        directory = os.path.join(arguments.out, digest)
        os.makedirs(directory, exist_ok=True)
        record = cut_chain(digest, by_digest[digest], directory)
        write_record(os.path.join(directory, RECORD), record)
    return 0


if __name__ == "__main__":
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    try:
        code = main(argv)
    except SystemExit as exc:
        print(f"\nCHAIN CUT FAILED {exc}", file=sys.stderr)
        sys.stderr.flush()
        sys.stdout.flush()
        os._exit(1)
    except M.Refusal as exc:
        print(f"\nCHAIN CUT REFUSED [{exc.reason}] {exc.detail}", file=sys.stderr)
        sys.stderr.flush()
        sys.stdout.flush()
        os._exit(2)
    except BaseException:
        traceback.print_exc()
        sys.stdout.flush()
        sys.stderr.flush()
        os._exit(1)
    print("CHAINS-OK")
    sys.stdout.flush()
    os._exit(code)
