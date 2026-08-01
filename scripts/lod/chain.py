"""The manifest is the single seam: derive the runtime chain FROM it, never beside it.

    python3 scripts/lod/chain.py            # print the derived chain
    python3 scripts/lod/chain.py --verify   # fail if the manifest and the tree disagree
    python3 scripts/lod/chain.py --emit-rust

WHY THIS FILE EXISTS. A hand-written ledger already drifted: an exporter comment narrated 223.7 m
for a level the runtime derived at 335.5 m, and both numbers were "measured" — the comment had
frozen an old deviation and nobody re-ran the arithmetic. The fix is not a better comment. It is
that NO ONE writes a switch distance down. The manifest carries measured deviations; this module
carries the projection; every consumer asks it. When the runtime chain constants land they are
generated or checked here, and a hand-edited constant fails `--verify` rather than shipping.

WHAT `--verify` PROVES, with no Blender and no cargo:
  * the manifest has the SHAPE it claims — exactly the configured assets, L0 plus at least one
    generated level, indices in order, and a hash, a validity record and a render-gate record on
    every level. Absence is a failure, because "no measurement" and "measured zero defects" are
    different statements and only the second is evidence,
  * every number is FINITE. NaN fails every `>` and every `!=` quietly, so a poisoned manifest used
    to verify clean,
  * every level's glb exists and still hashes to what the manifest recorded (an asset edited by
    hand, or an LFS pointer that never got smudged, fails here),
  * the manifest was cut by the generator version, the generator SOURCES, and the whole gate and
    ladder configuration now in `config.py` — including the render gate's blocking flag, which is
    read from the tree and never from the manifest, so ratifying the threshold invalidates every
    chain cut before the ruling instead of leaving its failures as warnings for ever,
  * every recorded defect counter is inside its declared limit,
  * the manifest's own arithmetic is self-consistent: switch distances re-derive from the recorded
    deviations, the chain is monotone in triangles and in distance, and every level's deviation
    really is inside its rung,
  * the source .blend still hashes to what generation read, when the (untracked) .blend is present.

FOUR MUTANTS MOTIVATED MOST OF THAT LIST. An adversarial review deleted every asset, every glb
hash, and every validity record, and replaced every deviation with NaN — and this file reported
success each time, because every check was of the form "if the field is here and wrong, complain".
`scripts/lod/test_chain.py`'s `MutantTests` is that review, kept.
"""

import argparse
import hashlib
import json
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import config as CONFIG  # noqa: E402


def switch_distance_m(deviation_mm, radius_m, view):
    """D = dev_m * height_px / (2 tan(vfov/2) * budget_px) + radius. The ONE projection (§9).

    Exact, not small-angle. The shortcut `dev_m * height / (vfov * budget)` agrees to 0.06 % at the
    optic and is 5.5 % wrong at the commander FOV, which is precisely the kind of error that hides
    in a narrow reference view and surfaces when someone quotes a wide one.
    """
    denominator = 2.0 * math.tan(float(view["vfov_rad"]) / 2.0) * float(view["budget_px"])
    return (deviation_mm / 1000.0) * float(view["height_px"]) / denominator + radius_m


def load(root=None, path=None):
    root = root or CONFIG.repo_root()
    path = path or os.path.join(root, CONFIG.MANIFEST_RELPATH)
    with open(path, encoding="utf-8") as handle:
        return json.load(handle), root, path


def derive(manifest, view=None):
    """The runtime chain, derived from measured deviations. The only place a threshold is computed.

    Per level: the glb it loads, its triangle count, and the distance at or beyond which it is the
    honest choice. That distance is the WORSE of two bounds (ADR 0033 §4):

      * source-relative: the level's own lie must be under budget,
      * pairwise: the level and the one it replaces may lie in OPPOSITE directions, so their
        separation can reach e_{N-1} + e_N = 1.5 e_N. That separation is the pop, and pricing the
        switch on source-relative deviation alone under-states it by up to a half octave.
    """
    view = view or manifest["ladder"]["reference_view"]
    chains = []
    for asset in manifest["assets"]:
        levels = asset["levels"]
        # THE SLACK IS AN ORIGIN RADIUS, OVER BOTH ADJACENT LEVELS.
        #
        # `VisibilityRange` tests the distance to the entity ORIGIN; the guarantee is about the
        # surface, so the slack must be the farthest any shipped vertex sits FROM THAT ORIGIN — not
        # half the AABB diagonal, which bounds distance from the box centre and is a different point
        # entirely. Measured on the shipped Link: 0.400124 m from the origin against a 0.384004 m
        # half-diagonal, so every switch was landing 16 mm early.
        #
        # And over BOTH levels at the boundary, because either one may be the mesh on screen there;
        # taking the child's alone would under-slack whenever the parent is the bigger shape.
        origin_radius = [
            (level.get("validity") or {}).get("origin_radius_m", asset["source"]["radius_m"])
            for level in levels
        ]
        rows = []
        for index, level in enumerate(levels):
            if level["role"] == "source":
                rows.append({
                    "level": level["level"], "rung": 0, "glb": level["glb"],
                    "node": level.get("node"), "tris": level["tris"],
                    "dev_source_mm": 0.0, "pairwise_mm": None,
                    "switch_m": 0.0, "role": "source",
                })
                continue
            radius = max(origin_radius[index - 1], origin_radius[index])
            from_source = switch_distance_m(level["dev_source_mm_upper"], radius, view)
            from_pairwise = switch_distance_m(level["pairwise_mm_upper"], radius, view)
            rows.append({
                "level": level["level"], "rung": level["rung"], "glb": level["glb"],
                "node": level.get("node"), "tris": level["tris"],
                "e_target_mm": level["e_target_mm"],
                "dev_source_mm": level["dev_source_mm_upper"],
                "pairwise_mm": level["pairwise_mm_upper"],
                "switch_from_source_m": from_source,
                "switch_from_pairwise_m": from_pairwise,
                "switch_m": max(from_source, from_pairwise),
                "origin_radius_m": radius,
                "role": "generated",
            })
        chains.append({
            "asset": asset["name"], "radius_m": max(origin_radius),
            "termination": asset["termination"],
            "right_wall_m": manifest["ladder"]["right_wall_m"], "levels": rows,
        })
    return chains


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


#: Numeric fields every generated level must carry, all of which must be finite. A manifest whose
#: numbers are NaN passed every comparison in the first version of this file: NaN fails every `>`
#: and every `!=` test silently, so a corrupted manifest verified clean.
LEVEL_NUMERIC_FIELDS = (
    "tris", "verts", "e_target_mm", "dev_source_mm", "dev_source_mm_upper",
    "pairwise_mm", "pairwise_mm_upper", "switch_m", "shed_fraction_vs_parent",
)

#: Validity counters every level must carry, measured on ITS OWN decoded shipped bytes. Absence is
#: a failure: "no record" and "a record of zero defects" are not the same statement, and only the
#: second one is evidence.
LEVEL_VALIDITY_FIELDS = (
    "tris", "verts", "components", "duplicate_faces", "nonfinite_attrs", "orientation_flips",
    "nonmanifold_edges", "slivers_below_floor", "tangent_default_faces", "tangent_default_verts",
    "min_altitude_m", "origin_radius_m",
)

#: Render-gate fields required on every generated level.
GATE_FIELDS = ("pass", "worst_defect_score", "worst_mean_abs_diff", "views", "thresholds")


def _finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def _check_schema(manifest, failures):
    """Structure before semantics: everything below assumes these fields exist and are numbers.

    THIS FUNCTION EXISTS BECAUSE THE VERIFIER PASSED FOUR MUTANTS. Removing every asset, every
    `glb_sha256`, every `validity` record, and replacing every deviation with NaN each produced a
    clean verification, because the checks were all of the form "if the field is here and wrong,
    complain". A gate that only inspects what it is given certifies nothing about what it is not.
    So: the shape is required first, and every requirement below is a positive assertion that a
    field EXISTS, is FINITE, and matches the configuration in this tree.
    """
    for key in ("schema", "schema_version", "generator", "ladder", "gates", "assets"):
        if key not in manifest:
            failures.append(f"manifest has no {key!r} — not a manifest this pipeline wrote")
            return False
    if manifest["schema"] != "overmatch.lod.manifest":
        failures.append(f"unknown schema {manifest['schema']!r}")
        return False

    declared = [asset["name"] for asset in CONFIG.ASSETS]
    present = [asset.get("name") for asset in manifest["assets"]]
    if present != declared:
        failures.append(
            f"manifest covers {present or 'NO assets'}, config declares {declared} — every "
            f"declared asset must have a chain, in order, and nothing else may appear"
        )
        return False

    ok = True
    for asset in manifest["assets"]:
        name = asset.get("name")
        for key in ("source", "levels", "termination", "topology_floor_tris"):
            if key not in asset:
                failures.append(f"{name}: asset record has no {key!r}")
                ok = False
        levels = asset.get("levels") or []
        if len(levels) < 2:
            failures.append(
                f"{name}: {len(levels)} level(s) — a chain is L0 plus at least one generated level"
            )
            ok = False
            continue
        if levels[0].get("role") != "source" or levels[0].get("level") != 0:
            failures.append(f"{name}: level 0 is not the source record")
            ok = False
        for index, level in enumerate(levels):
            label = f"{name} L{level.get('level', '?')}"
            if level.get("level") != index:
                failures.append(f"{label}: level indices are not 0..{len(levels) - 1} in order")
                ok = False
            for key in ("glb", "glb_sha256", "validity", "role"):
                if not level.get(key):
                    failures.append(f"{label}: no {key!r} — nothing to verify against")
                    ok = False
            validity = level.get("validity") or {}
            for key in LEVEL_VALIDITY_FIELDS:
                if key not in validity:
                    failures.append(f"{label}: validity record has no {key!r}")
                    ok = False
                elif not _finite(validity[key]):
                    failures.append(f"{label}: validity {key!r} is {validity[key]!r}, not a number")
                    ok = False
            if index == 0:
                continue
            if level.get("role") != "generated":
                failures.append(f"{label}: role is {level.get('role')!r}, expected 'generated'")
                ok = False
            for key in LEVEL_NUMERIC_FIELDS:
                if key not in level:
                    failures.append(f"{label}: no {key!r}")
                    ok = False
                elif not _finite(level[key]):
                    failures.append(f"{label}: {key!r} is {level[key]!r}, not a finite number")
                    ok = False
            gate = level.get("render_gate")
            if gate is None:
                failures.append(
                    f"{label}: no render-gate record — this manifest was cut with "
                    f"--no-render-gate and is not shippable; re-run generation"
                )
                ok = False
            else:
                for key in GATE_FIELDS:
                    if key not in gate:
                        failures.append(f"{label}: render-gate record has no {key!r}")
                        ok = False
                if not gate.get("views"):
                    failures.append(f"{label}: render gate recorded no views")
                    ok = False
    return ok


def _check_config_match(manifest, failures):
    """The manifest must have been cut by the configuration THIS TREE holds, all of it.

    Not a sample of it. The blocking flag is included deliberately: reading it out of the manifest
    and trusting it means flipping `RENDER_GATE_BLOCKING` to True leaves an old manifest's recorded
    failures as warnings forever, which is the ratification quietly failing to take effect.
    """
    generator = manifest.get("generator") or {}
    if generator.get("version") != CONFIG.GENERATOR_VERSION:
        failures.append(
            f"manifest was cut by generator {generator.get('version')}, the tree holds "
            f"{CONFIG.GENERATOR_VERSION} — regenerate"
        )
    digest = generator.get("sources_sha256")
    if not digest:
        failures.append("manifest records no generator source digest")
    elif digest != CONFIG.generator_digest():
        failures.append(
            "the generator's sources have changed since this manifest was cut — the version "
            "string can stay the same while the algorithm moves, which is what this catches"
        )

    ladder = manifest.get("ladder") or {}
    for key, declared in (("e1_mm", CONFIG.E1_MM), ("octave", CONFIG.OCTAVE),
                          ("skip_fraction", CONFIG.SKIP_FRACTION),
                          ("max_rungs", CONFIG.MAX_RUNGS),
                          ("right_wall_m", CONFIG.RIGHT_WALL_M)):
        value = ladder.get(key)
        if not _finite(value) or abs(float(value) - float(declared)) > 1e-6:
            failures.append(f"ladder {key}: manifest {value!r} != config {declared}")
    view = ladder.get("reference_view") or {}
    for key in ("vfov_rad", "height_px", "budget_px"):
        if not _finite(view.get(key)) or abs(
            float(view[key]) - float(CONFIG.REFERENCE_VIEW[key])
        ) > 1e-12:
            failures.append(
                f"reference view {key}: manifest {view.get(key)!r} != config "
                f"{CONFIG.REFERENCE_VIEW[key]}"
            )

    gates = manifest.get("gates") or {}
    numeric = gates.get("numeric") or {}
    for key, declared in CONFIG.GATES.items():
        if key not in numeric:
            failures.append(f"gate {key!r} is not recorded in the manifest")
        elif numeric[key] != declared:
            failures.append(f"gate {key}: manifest {numeric[key]!r} != config {declared!r}")
    render = gates.get("render") or {}
    for key, declared in CONFIG.RENDER_GATE.items():
        if key == "views":
            continue
        if key not in render:
            failures.append(f"render-gate setting {key!r} is not recorded in the manifest")
        elif render[key] != declared:
            failures.append(f"render {key}: manifest {render[key]!r} != config {declared!r}")
    recorded_views = [tuple(v) for v in (gates.get("render_views") or [])]
    if recorded_views != [tuple(v) for v in CONFIG.RENDER_GATE["views"]]:
        failures.append("render-gate viewpoints differ from config")
    if gates.get("render_gate_blocking") != CONFIG.RENDER_GATE_BLOCKING:
        failures.append(
            f"render_gate_blocking: manifest {gates.get('render_gate_blocking')!r} != config "
            f"{CONFIG.RENDER_GATE_BLOCKING!r} — regenerate so the ruling takes effect"
        )


def verify(manifest, root):
    """Every drift a manifest can suffer. Returns (failures, warnings); empty failures means clean.

    A WARNING is a recorded verdict that is deliberately not enforced yet — today that is only the
    rendered-difference gate, whose threshold is unratified. It is printed every time, so nothing is
    hidden; it just does not fail the build until someone rules on the number.
    """
    failures = []
    warnings = []
    if not _check_schema(manifest, failures):
        return failures, warnings
    _check_config_match(manifest, failures)

    for chain, asset in zip(derive(manifest), manifest["assets"]):
        try:
            blend = CONFIG.resolve_source(root, asset["source"]["blend"])
        except FileNotFoundError:
            blend = None  # untracked and absent: nothing to check, not a failure
        if blend and sha256_file(blend) != asset["source"].get("blend_sha256"):
            failures.append(
                f"{asset['name']}: {asset['source']['blend']} has changed since generation — the "
                f"chain is cut from a source that no longer exists"
            )
        previous = None
        for row, level in zip(chain["levels"], asset["levels"]):
            label = f"{asset['name']} L{level['level']}"
            path = os.path.join(root, level["glb"])
            if not os.path.isfile(path):
                failures.append(f"{label}: missing {level['glb']}")
            elif sha256_file(path) != level["glb_sha256"]:
                failures.append(
                    f"{label}: {level['glb']} does not hash to the manifest's record — it was "
                    f"edited or rebuilt outside the pipeline"
                )
            validity = level["validity"]
            for key, limit in (
                ("duplicate_faces", CONFIG.GATES["max_duplicate_faces"]),
                ("nonfinite_attrs", CONFIG.GATES["max_nonfinite"]),
                ("orientation_flips", CONFIG.GATES["max_orientation_flips"]),
                ("nonmanifold_edges", CONFIG.GATES["max_nonmanifold_edges"]),
                ("tangent_default_faces", CONFIG.GATES["max_tangent_default_faces"]),
                ("tangent_default_verts", CONFIG.GATES["max_tangent_default_verts"]),
                ("slivers_below_floor", 0),
            ):
                if validity[key] > limit:
                    failures.append(f"{label}: recorded {validity[key]} {key} against a limit of {limit}")
            if level["role"] == "source":
                previous = row
                continue
            if level["dev_source_mm_upper"] > level["e_target_mm"] + 1e-9:
                failures.append(
                    f"{label}: certified {level['dev_source_mm_upper']} mm exceeds its rung target "
                    f"{level['e_target_mm']} mm"
                )
            if abs(row["switch_m"] - level["switch_m"]) > 1e-3:
                failures.append(
                    f"{label}: recorded switch {level['switch_m']} m re-derives to "
                    f"{row['switch_m']:.4f} m — the ledger drifted from the measurement"
                )
            if previous is not None and level["tris"] >= previous["tris"]:
                failures.append(
                    f"{label}: {level['tris']} tris is not fewer than L{previous['level']}'s "
                    f"{previous['tris']}"
                )
            if previous is not None and row["switch_m"] <= previous["switch_m"]:
                failures.append(
                    f"{label}: switch {row['switch_m']:.1f} m is not beyond L{previous['level']}'s "
                    f"{previous['switch_m']:.1f} m"
                )
            gate = level["render_gate"]
            if not gate.get("pass", False):
                verdict = (
                    f"{label}: render gate recorded a FAIL (defect score "
                    f"{gate.get('worst_defect_score')} against a limit of "
                    f"{gate.get('thresholds', {}).get('defect_fraction')})"
                )
                if CONFIG.RENDER_GATE_BLOCKING:
                    failures.append(verdict)
                else:
                    warnings.append(
                        verdict + " — NOT enforced: defect_fraction is unratified "
                        "(config.RENDER_GATE_BLOCKING is False)"
                    )
            previous = row
    return failures, warnings


def format_chain(chains):
    lines = []
    for chain in chains:
        lines.append(
            f"{chain['asset']}  (radius {chain['radius_m']:.4f} m, right wall "
            f"{chain['right_wall_m']:.1f} m, terminated: {chain['termination']})"
        )
        lines.append(f"  {'level':<7}{'rung':<6}{'tris':>7}{'dev mm':>10}{'pair mm':>10}"
                     f"{'switch m':>11}  glb")
        for row in chain["levels"]:
            dev = "—" if row["role"] == "source" else f"{row['dev_source_mm']:.3f}"
            pair = "—" if row["pairwise_mm"] is None else f"{row['pairwise_mm']:.3f}"
            lines.append(
                f"  L{row['level']:<6}{row['rung']:<6}{row['tris']:>7}{dev:>10}{pair:>10}"
                f"{row['switch_m']:>11.1f}  {row['glb']}"
            )
    return "\n".join(lines)


def emit_rust(chains, manifest):
    """The runtime table as Rust, DERIVED. For the branch that wires selection up.

    Emitted rather than hand-written for one reason: the numbers below are the manifest's, and the
    moment a human retypes one it can be wrong. A runtime branch either includes this output or
    checks its own constants against it.
    """
    out = [
        "// GENERATED by scripts/lod/chain.py from assets/lod_manifest.json — do not hand-edit.",
        f"// generator {manifest['generator']['version']}, blender {manifest['generator']['blender']}",
        f"// reference view: {manifest['ladder']['reference_view']['name']}, "
        f"vfov {manifest['ladder']['reference_view']['vfov_rad']} rad, "
        f"{manifest['ladder']['reference_view']['height_px']:.0f} px, "
        f"{manifest['ladder']['reference_view']['budget_px']:.0f} px budget",
        "",
        "/// One level of a geometry-mipmap chain: the asset it loads and the surface distance at",
        "/// or beyond which it is the honest choice in the reference view.",
        "pub struct LodLevel {",
        "    pub glb: &'static str,",
        "    pub tris: u32,",
        "    pub dev_source_mm: f32,",
        "    pub pairwise_mm: f32,",
        "    pub switch_m: f32,",
        "}",
        "",
    ]
    for chain in chains:
        name = chain["asset"].upper() + "_CHAIN"
        out.append(f"pub const {name}: [LodLevel; {len(chain['levels'])}] = [")
        for row in chain["levels"]:
            pair = 0.0 if row["pairwise_mm"] is None else row["pairwise_mm"]
            out.append(
                f'    LodLevel {{ glb: "{row["glb"]}", tris: {row["tris"]}, '
                f"dev_source_mm: {row['dev_source_mm']:.6f}, pairwise_mm: {pair:.6f}, "
                f"switch_m: {row['switch_m']:.4f} }},"
            )
        out.append("];")
        out.append("")
    return "\n".join(out)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--manifest", default=None)
    parser.add_argument("--verify", action="store_true")
    parser.add_argument("--emit-rust", action="store_true")
    args = parser.parse_args()

    manifest, root, path = load(path=args.manifest)
    if args.emit_rust:
        print(emit_rust(derive(manifest), manifest))
        return 0
    if args.verify:
        failures, warnings = verify(manifest, root)
        for warning in warnings:
            print(f"lod chain \u25b8 WARNING: {warning}", file=sys.stderr)
        if failures:
            print(f"lod chain ▸ {os.path.relpath(path, root)} FAILED:", file=sys.stderr)
            for failure in failures:
                print(f"  - {failure}", file=sys.stderr)
            return 1
        levels = sum(len(a["levels"]) for a in manifest["assets"])
        print(f"lod chain ▸ {os.path.relpath(path, root)} verified: "
              f"{len(manifest['assets'])} asset(s), {levels} level(s)")
        return 0
    print(format_chain(derive(manifest)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
