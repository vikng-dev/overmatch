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
import subprocess
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


class Tree:
    """Where verification reads its files from: the work tree, or a git revision.

    THE HOOK NEEDS THE SECOND ONE. A pre-push hook that verifies the work tree answers a question
    nobody asked — a dirty-but-coherent tree can bless a completely different commit, and pushing a
    branch that is not `HEAD`, or a tag, or several refs at once, was not covered at all. Reading
    the manifest and the assets out of the REVISION BEING PUSHED is the only version of this check
    that means what its name says.

    LFS pointers are the reason it can be done cheaply. A tracked glb in a commit is a pointer file
    whose `oid sha256:` IS the sha256 of the real bytes — the same number the manifest records — so
    the whole hash comparison works without hydrating a single object.
    """

    def __init__(self, root, rev=None):
        self.root = root
        self.rev = rev

    def read(self, relpath):
        if self.rev is None:
            with open(os.path.join(self.root, relpath), "rb") as handle:
                return handle.read()
        result = subprocess.run(
            ["git", "show", f"{self.rev}:{relpath}"],
            cwd=self.root, capture_output=True, check=False,
        )
        if result.returncode != 0:
            raise FileNotFoundError(f"{relpath} is not in {self.rev}")
        return result.stdout

    def exists(self, relpath):
        try:
            self.read(relpath)
        except FileNotFoundError:
            return False
        return True

    def digest(self, relpath):
        """sha256 of the file's real content, resolving an LFS pointer to its recorded oid."""
        blob = self.read(relpath)
        if blob.startswith(b"version https://git-lfs.github.com/spec/v1"):
            for line in blob.splitlines():
                if line.startswith(b"oid sha256:"):
                    return line.split(b":", 1)[1].decode().strip()
            raise ValueError(f"{relpath} is an LFS pointer with no sha256 oid")
        return hashlib.sha256(blob).hexdigest()

    def label(self):
        return f"{self.rev}:{CONFIG.MANIFEST_RELPATH}" if self.rev else CONFIG.MANIFEST_RELPATH


def load(root=None, path=None, rev=None):
    root = root or CONFIG.repo_root()
    tree = Tree(root, rev)
    if rev is None and path is not None:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle), root, tree
    return json.loads(tree.read(CONFIG.MANIFEST_RELPATH).decode()), root, tree


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
    "switch_from_source_dev_m", "switch_from_pairwise_m", "dev_source_bracket_mm",
    "dev_source_to_level_mm", "dev_level_to_source_mm", "glb_bytes",
)

#: The same, for the SOURCE level. It has its own shape, and leaving it out meant L0 could carry a
#: NaN triangle count through verification untouched — the one level every other level's deviation
#: is measured against.
SOURCE_LEVEL_NUMERIC_FIELDS = (
    "tris", "verts", "e_target_mm", "dev_source_mm", "dev_source_mm_upper", "switch_m",
    "shipped_dev_from_source_mm", "blender_source_verts",
)

#: Numerics on the asset's source record.
SOURCE_NUMERIC_FIELDS = ("tris", "verts", "radius_m")

#: Validity counters every level must carry, measured on ITS OWN decoded shipped bytes. Absence is
#: a failure: "no record" and "a record of zero defects" are not the same statement, and only the
#: second one is evidence.
LEVEL_VALIDITY_FIELDS = (
    "tris", "verts", "components", "duplicate_faces", "nonfinite_attrs", "orientation_flips",
    "nonmanifold_edges", "slivers_below_floor", "tangent_default_faces", "tangent_default_verts",
    "min_altitude_m", "origin_radius_m", "min_altitude_floor_m", "min_tri_area_mm2",
    "boundary_edges",
)

#: Render-gate fields required on every generated level, and which of them must be finite numbers.
GATE_FIELDS = (
    "pass", "worst_defect_score", "worst_mean_abs_diff", "worst_frac_over", "views", "thresholds",
    "distance_m", "material_source", "tile_px", "supersample", "samples", "tile_vfov_rad",
)
GATE_NUMERIC_FIELDS = (
    "worst_defect_score", "worst_mean_abs_diff", "worst_frac_over", "distance_m",
    "tile_px", "supersample", "samples", "tile_vfov_rad",
)

#: Per-view statistics inside a gate record, and the numerics inside those.
GATE_VIEW_FIELDS = ("signal", "noise_floor", "defect_floor", "defect_score", "pass")
GATE_VIEW_NUMERIC_FIELDS = (
    "footprint_px", "mean_abs_diff", "p99_abs_diff", "max_abs_diff", "frac_over",
    "silhouette_band_px", "silhouette_band_frac",
)

#: Generator provenance that must be present AND must match this tree.
GENERATOR_PINNED_FIELDS = (
    ("blender", "EXPECTED_BLENDER", lambda v: v.split()[0]),
    ("blender_build", "EXPECTED_BLENDER_BUILD", lambda v: v),
    ("gltf_exporter", "EXPECTED_GLTF_EXPORTER", lambda v: v),
)

#: Fields recorded for a HUMAN and deliberately not compared against anything.
#:
#: THE RULE THIS ENFORCES: every field the manifest records is either CHECKED or listed here. There
#: is no third state. `test_chain.py` walks the shipped manifest and fails on any key that is in
#: neither set, so a field added later cannot quietly become decoration that looks like evidence —
#: which is what `schema_version`, `defect_fraction` and the whole render record had become.
INFORMATIONAL_FIELDS = frozenset({
    "schema", "script", "right_wall_source", "provenance", "name", "note", "reason",
    "render_gate_unratified_note", "identity_proof", "termination", "role", "node", "glb",
    "object", "blend", "bbox_mm", "material", "render_material_source", "evaluated_digest",
    "skipped_rungs", "rung", "level", "parent_level", "e_target_mm", "best_tris",
    "shed_fraction", "floor_tris", "cleanup", "faces_before", "faces_after", "dissolve_dist_m",
    "normal_diagnostic_deg", "max_deg", "p99_deg", "p95_deg", "backface_corr_frac", "samples",
    "shipped_matches_source", "deviation_evaluations", "decimations", "reproducible",
    "under_absolute_floor", "label", "sliver_floor_m", "topology_floor_tris",
    "blend_sha256", "sources_sha256", "glb_sha256", "right_wall_m", "reference_view",
    "vfov_rad", "height_px", "budget_px", "e1_mm", "octave", "skip_fraction", "max_rungs",
    "numeric", "render", "render_views", "render_gate_blocking", "generator", "ladder", "gates",
    "assets", "levels", "source", "validity", "render_gate", "views", "thresholds", "signal",
    "noise_floor", "defect_floor", "search_limits",
})


def _finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def _require_numbers(record, fields, label, what, failures):
    """Every named field must be present AND finite. Returns False on any violation."""
    ok = True
    for key in fields:
        if key not in record:
            failures.append(f"{label}: {what} has no {key!r}")
            ok = False
        elif not _finite(record[key]):
            failures.append(f"{label}: {what} {key!r} is {record[key]!r}, not a finite number")
            ok = False
    return ok


def _check_gate_record(gate, label, failures):
    """A render-gate record, checked as EVIDENCE rather than as a field list.

    Presence was not enough and the mutants proved it: a record whose every metric was NaN, or whose
    per-level `defect_fraction` had been rewritten to 999, still carried `pass: true` and verified
    clean. So the thresholds must match the tree, every metric must be a real number, and the
    verdict must RE-DERIVE from the metrics — a recorded pass that the recorded numbers do not
    support is a contradiction, and contradictions are exactly what a manifest is for catching.
    """
    ok = True
    for key in GATE_FIELDS:
        if key not in gate:
            failures.append(f"{label}: render-gate record has no {key!r}")
            ok = False
    if not ok:
        return False
    if not isinstance(gate["pass"], bool):
        failures.append(f"{label}: render-gate 'pass' is {gate['pass']!r}, not a boolean")
        ok = False
    ok = _require_numbers(gate, GATE_NUMERIC_FIELDS, label, "render gate", failures) and ok

    thresholds = gate.get("thresholds") or {}
    for key in ("defect_fraction", "max_mean_abs_diff", "max_footprint_frac_over",
                "over_threshold"):
        if key not in thresholds:
            failures.append(f"{label}: render-gate thresholds have no {key!r}")
            ok = False
        elif thresholds[key] != CONFIG.RENDER_GATE[key]:
            failures.append(
                f"{label}: render gate was judged against {key}={thresholds[key]!r}, the tree "
                f"declares {CONFIG.RENDER_GATE[key]!r}"
            )
            ok = False

    views = gate.get("views") or {}
    if not views:
        failures.append(f"{label}: render gate recorded no views")
        return False
    expected_views = {name for name, _e, _a in CONFIG.RENDER_GATE["views"]}
    if set(views) != expected_views:
        failures.append(
            f"{label}: render gate covered {sorted(views)}, config declares "
            f"{sorted(expected_views)}"
        )
        ok = False
    for view_name, view in views.items():
        view_label = f"{label} [{view_name}]"
        for key in GATE_VIEW_FIELDS:
            if key not in view:
                failures.append(f"{view_label}: view record has no {key!r}")
                ok = False
        if not ok:
            continue
        if not _finite(view.get("defect_score")):
            failures.append(f"{view_label}: defect_score is not a finite number")
            ok = False
        for part in ("signal", "noise_floor", "defect_floor"):
            ok = _require_numbers(
                view.get(part) or {}, GATE_VIEW_NUMERIC_FIELDS, view_label, part, failures
            ) and ok
        if ok and view["signal"]["footprint_px"] <= 0:
            failures.append(
                f"{view_label}: an empty footprint — the gate never saw the asset, which is a "
                f"failure and not a pass"
            )
            ok = False

    if not ok:
        return False

    # THE VERDICT MUST RE-DERIVE FROM THE METRICS.
    limit = CONFIG.RENDER_GATE["defect_fraction"]
    derived = True
    for view_name, view in views.items():
        signal = view["signal"]
        floor_ok = (
            signal["mean_abs_diff"] <= CONFIG.RENDER_GATE["max_mean_abs_diff"]
            and signal["frac_over"] <= CONFIG.RENDER_GATE["max_footprint_frac_over"]
        )
        if not (signal["footprint_px"] > 0 and (view["defect_score"] <= limit or floor_ok)):
            derived = False
    if derived != gate["pass"]:
        failures.append(
            f"{label}: render gate records pass={gate['pass']} but its own numbers re-derive to "
            f"{derived} — the verdict does not follow from the evidence beside it"
        )
        return False

    # THE PRECONDITION ON BLOCKING, made mechanical. A gate judged under fallback materials must
    # not be allowed to block: the number would be measured with the wrong textures.
    if CONFIG.RENDER_GATE_BLOCKING and str(gate["material_source"]).startswith("FELL BACK"):
        failures.append(
            f"{label}: RENDER_GATE_BLOCKING is on, but this level was judged under a FALLBACK "
            f"material ({gate['material_source']}). Blocking on a number measured with the wrong "
            f"textures certifies the wrong thing — fix the material path first (see "
            f"config.RENDER_GATE_BLOCKING)"
        )
        return False
    return True


def _check_schema(manifest, failures):
    """Structure before semantics: everything below assumes these fields exist and are numbers.

    THIS FUNCTION EXISTS BECAUSE THE VERIFIER PASSED MUTANTS — twice. First round: every asset
    removed, every hash removed, every validity record removed, every number NaN. Second round,
    after "strict schema validation" was claimed: `schema_version = 999`, L0's triangle count NaN,
    render metrics NaN under `pass: true`, a per-level `defect_fraction` of 999, and provenance
    fields deleted wholesale. Each verified clean.

    The lesson both rounds teach is the same one: a check that only inspects what it is handed
    certifies nothing about what it is not, and a field that is recorded but never compared is
    decoration that reads as evidence. So the rule this file now enforces is that EVERY recorded
    field is either checked here or named in `INFORMATIONAL_FIELDS`, with no third state, and
    `test_chain.py` walks the shipped manifest to prove there is no field in neither set.
    """
    for key in ("schema", "schema_version", "generator", "ladder", "gates", "assets"):
        if key not in manifest:
            failures.append(f"manifest has no {key!r} — not a manifest this pipeline wrote")
            return False
    if manifest["schema"] != "overmatch.lod.manifest":
        failures.append(f"unknown schema {manifest['schema']!r}")
        return False
    if manifest["schema_version"] != CONFIG.SCHEMA_VERSION:
        failures.append(
            f"manifest schema_version is {manifest['schema_version']!r}, this tree reads "
            f"{CONFIG.SCHEMA_VERSION} — a manifest in a shape this code does not understand"
        )
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
        if not ok:
            continue
        ok = _require_numbers(
            asset["source"], SOURCE_NUMERIC_FIELDS, name, "source record", failures
        ) and ok
        if not _finite(asset["topology_floor_tris"]):
            failures.append(f"{name}: topology_floor_tris is not a finite number")
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
            ok = _require_numbers(
                validity, LEVEL_VALIDITY_FIELDS, label, "validity record", failures
            ) and ok
            if index == 0:
                # L0's own numerics were omitted entirely, so the ONE level every deviation in the
                # chain is measured against could carry a NaN triangle count through untouched.
                ok = _require_numbers(
                    level, SOURCE_LEVEL_NUMERIC_FIELDS, label, "source level", failures
                ) and ok
                if not level.get("identity_proof"):
                    failures.append(f"{label}: no identity proof against the source")
                    ok = False
                continue
            if level.get("role") != "generated":
                failures.append(f"{label}: role is {level.get('role')!r}, expected 'generated'")
                ok = False
            ok = _require_numbers(level, LEVEL_NUMERIC_FIELDS, label, "level", failures) and ok
            gate = level.get("render_gate")
            if gate is None:
                failures.append(
                    f"{label}: no render-gate record — this manifest was cut with "
                    f"--no-render-gate and is not shippable; re-run generation"
                )
                ok = False
            else:
                ok = _check_gate_record(gate, label, failures) and ok
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
    # THE BUILD, NOT JUST THE VERSION. Two Blenders can both say 5.1.2 and be different builds, and
    # the glTF exporter is an add-on that moves on its own schedule and decides the bytes. Both were
    # recorded and neither was ever compared, which made them decoration. Double generation inside
    # one process cannot see a cross-build difference either, so this is the only place it is caught.
    for field, constant, normalise in GENERATOR_PINNED_FIELDS:
        recorded = generator.get(field)
        expected = getattr(CONFIG, constant)
        if not recorded:
            failures.append(f"manifest records no {field!r} — toolchain provenance is missing")
        elif normalise(str(recorded)) != expected:
            failures.append(
                f"{field}: manifest {recorded!r} != pinned {expected!r} (config.{constant}) — this "
                f"corpus was cut by a different toolchain than the tree declares"
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
    limits = gates.get("search_limits") or {}
    for key, declared in CONFIG.SEARCH_LIMITS.items():
        if key not in limits:
            failures.append(f"search limit {key!r} is not recorded in the manifest")
        elif limits[key] != declared:
            failures.append(f"search limit {key}: manifest {limits[key]!r} != config {declared!r}")
    if gates.get("render_gate_blocking") != CONFIG.RENDER_GATE_BLOCKING:
        failures.append(
            f"render_gate_blocking: manifest {gates.get('render_gate_blocking')!r} != config "
            f"{CONFIG.RENDER_GATE_BLOCKING!r} — regenerate so the ruling takes effect"
        )


def verify(manifest, tree):
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
            blend = CONFIG.resolve_source(tree.root, asset["source"]["blend"])
        except FileNotFoundError:
            blend = None  # untracked and absent: nothing to check, not a failure
        if tree.rev is None and blend and sha256_file(blend) != asset["source"].get("blend_sha256"):
            failures.append(
                f"{asset['name']}: {asset['source']['blend']} has changed since generation — the "
                f"chain is cut from a source that no longer exists"
            )
        previous = None
        for row, level in zip(chain["levels"], asset["levels"]):
            label = f"{asset['name']} L{level['level']}"
            if not tree.exists(level["glb"]):
                failures.append(f"{label}: missing {level['glb']}")
            elif tree.digest(level["glb"]) != level["glb_sha256"]:
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
    parser.add_argument(
        "--rev", default=None,
        help="verify the manifest and assets AS OF this git revision rather than the work tree; "
             "what scripts/hooks/pre-push runs on each SHA it is about to push",
    )
    args = parser.parse_args()

    try:
        manifest, root, tree = load(path=args.manifest, rev=args.rev)
    except (FileNotFoundError, ValueError) as exc:
        print(f"lod chain ▸ cannot read the manifest: {exc}", file=sys.stderr)
        return 1
    if args.emit_rust:
        print(emit_rust(derive(manifest), manifest))
        return 0
    if args.verify:
        failures, warnings = verify(manifest, tree)
        for warning in warnings:
            print(f"lod chain \u25b8 WARNING: {warning}", file=sys.stderr)
        if failures:
            print(f"lod chain ▸ {tree.label()} FAILED:", file=sys.stderr)
            for failure in failures:
                print(f"  - {failure}", file=sys.stderr)
            return 1
        levels = sum(len(a["levels"]) for a in manifest["assets"])
        print(f"lod chain ▸ {tree.label()} verified: "
              f"{len(manifest['assets'])} asset(s), {levels} level(s)")
        return 0
    print(format_chain(derive(manifest)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
