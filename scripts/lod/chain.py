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
  * every level's glb exists and still hashes to what the manifest recorded (an asset edited by
    hand, or an LFS pointer that never got smudged, fails here),
  * the manifest was cut by the generator version and the ladder constants now in `config.py`,
  * the manifest's own arithmetic is self-consistent: switch distances re-derive from the recorded
    deviations, the chain is monotone in triangles and in distance, and every level's deviation
    really is inside its rung,
  * the source .blend still hashes to what generation read, when the (untracked) .blend is present.
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
        radius = asset["source"]["radius_m"]
        rows = []
        for level in asset["levels"]:
            if level["role"] == "source":
                rows.append({
                    "level": level["level"], "rung": 0, "glb": level["glb"],
                    "node": level.get("node"), "tris": level["tris"],
                    "dev_source_mm": 0.0, "pairwise_mm": None,
                    "switch_m": 0.0, "role": "source",
                })
                continue
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
                "role": "generated",
            })
        chains.append({
            "asset": asset["name"], "radius_m": radius, "termination": asset["termination"],
            "right_wall_m": manifest["ladder"]["right_wall_m"], "levels": rows,
        })
    return chains


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def verify(manifest, root):
    """Every drift a manifest can suffer. Returns (failures, warnings); empty failures means clean.

    A WARNING is a recorded verdict that is deliberately not enforced yet — today that is only the
    rendered-difference gate, whose threshold is unratified. It is printed every time, so nothing is
    hidden; it just does not fail the build until someone rules on the number.
    """
    failures = []
    warnings = []
    ladder = manifest["ladder"]
    if manifest["generator"]["version"] != CONFIG.GENERATOR_VERSION:
        failures.append(
            f"manifest was cut by generator {manifest['generator']['version']}, the tree holds "
            f"{CONFIG.GENERATOR_VERSION} — regenerate"
        )
    for key, declared in (("e1_mm", CONFIG.E1_MM), ("octave", CONFIG.OCTAVE),
                          ("skip_fraction", CONFIG.SKIP_FRACTION)):
        if abs(float(ladder[key]) - float(declared)) > 1e-12:
            failures.append(f"ladder {key}: manifest {ladder[key]} != config {declared}")
    if abs(float(ladder["right_wall_m"]) - CONFIG.RIGHT_WALL_M) > 1e-6:
        failures.append(
            f"right wall: manifest {ladder['right_wall_m']} m != config {CONFIG.RIGHT_WALL_M} m "
            f"— the map moved, the chain did not"
        )

    for chain, asset in zip(derive(manifest), manifest["assets"]):
        try:
            blend = CONFIG.resolve_source(root, asset["source"]["blend"])
        except FileNotFoundError:
            blend = None  # untracked and absent: nothing to check, not a failure
        if blend and sha256_file(blend) != asset["source"]["blend_sha256"]:
            failures.append(
                f"{asset['name']}: {asset['source']['blend']} has changed since generation — the "
                f"chain is cut from a source that no longer exists"
            )
        previous = None
        for row, level in zip(chain["levels"], asset["levels"]):
            path = os.path.join(root, level["glb"])
            if not os.path.isfile(path):
                failures.append(f"{asset['name']} L{level['level']}: missing {level['glb']}")
            elif "glb_sha256" in level and sha256_file(path) != level["glb_sha256"]:
                failures.append(
                    f"{asset['name']} L{level['level']}: {level['glb']} does not hash to the "
                    f"manifest's record — it was edited or rebuilt outside the pipeline"
                )
            if level["role"] == "source":
                previous = row
                continue
            if level["dev_source_mm_upper"] > level["e_target_mm"] + 1e-9:
                failures.append(
                    f"{asset['name']} L{level['level']}: certified {level['dev_source_mm_upper']} mm "
                    f"exceeds its rung target {level['e_target_mm']} mm"
                )
            if abs(row["switch_m"] - level["switch_m"]) > 1e-3:
                failures.append(
                    f"{asset['name']} L{level['level']}: recorded switch {level['switch_m']} m "
                    f"re-derives to {row['switch_m']:.4f} m — the ledger drifted from the measurement"
                )
            if previous is not None and level["tris"] >= previous["tris"]:
                failures.append(
                    f"{asset['name']} L{level['level']}: {level['tris']} tris is not fewer than "
                    f"L{previous['level']}'s {previous['tris']}"
                )
            if previous is not None and row["switch_m"] <= previous["switch_m"]:
                failures.append(
                    f"{asset['name']} L{level['level']}: switch {row['switch_m']:.1f} m is not "
                    f"beyond L{previous['level']}'s {previous['switch_m']:.1f} m"
                )
            # A MISSING gate is a failure, not a pass. `--no-render-gate` is a development
            # shortcut for iterating on the search; a manifest cut with it names no rendered
            # evidence for any level, and "nothing silently passes" has to include that.
            gate = level.get("render_gate")
            if gate is None:
                failures.append(
                    f"{asset['name']} L{level['level']}: no render-gate record — this manifest was "
                    f"cut with --no-render-gate and is not shippable; re-run generation"
                )
            elif not gate.get("pass", False):
                blocking = manifest.get("gates", {}).get("render_gate_blocking", True)
                verdict = (
                    f"{asset['name']} L{level['level']}: render gate recorded a FAIL "
                    f"(defect score {gate.get('worst_defect_score')} against a limit of "
                    f"{gate.get('thresholds', {}).get('defect_fraction')})"
                )
                if blocking:
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
