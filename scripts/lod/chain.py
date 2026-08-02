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
from manifest import (  # noqa: E402
    Tree, derive, load, merge_asset_entries, sha256_file, screen_footprint_px,
    switch_distance_m, tile_vfov_rad,
)


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
    "boundary_edges", "baked_tangents", "degenerate_tangents", "min_tangent_length",
)

#: Recorded per level and CHECKED: whether the level's tangents ship in its bytes. False is legal
#: only for L0, which lives inside a host glb this pipeline does not export.
TANGENT_PRESENCE_FIELDS = ("tangents_are_baked",)

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

#: Fields pinned by `config.RATIFICATION_EVIDENCE` and compared against it by the test suite, so
#: the decision evidence quoted beside an unratified threshold cannot go stale a third time.
PINNED_EVIDENCE_FIELDS = ("enumerated_outputs",)

#: Generator provenance that must be present AND must match this tree.
GENERATOR_PINNED_FIELDS = (
    ("blender", "EXPECTED_BLENDER", lambda v: v.split()[0]),
    ("blender_build", "EXPECTED_BLENDER_BUILD", lambda v: v),
    ("gltf_exporter", "EXPECTED_GLTF_EXPORTER", lambda v: v),
)

#: THREE CLASSES, AND THE THIRD IS NAMED RATHER THAN HIDDEN.
#:
#:   1. RE-DERIVED — recomputed here from other recorded values and compared (every switch distance,
#:      every gate summary and verdict, every defect score, the deviation record's internal
#:      consistency). `test_chain.RederivationSweepTests` mutates each and demands a failure.
#:   2. COMPARED — checked against this tree's configuration, the pinned toolchain, or the bytes on
#:      disk (every threshold, every provenance field, every glb hash, every defect counter).
#:   3. MEASURED — a number the pipeline observed that nothing can re-derive from the manifest
#:      alone: the losing side of a `max`, a p99, a footprint size. These are required to be
#:      PRESENT and FINITE, and what actually pins them is the level's glb hash plus regeneration —
#:      the bytes are fixed and re-running the pipeline re-measures them. Saying "checked" of these
#:      would be the overstatement an earlier version of this file made.
#:
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
    "tangent_note",
    "under_absolute_floor", "label", "sliver_floor_m", "topology_floor_tris",
    "blend_sha256", "sources_sha256", "glb_sha256", "right_wall_m", "reference_view",
    "vfov_rad", "height_px", "budget_px", "e1_mm", "octave", "skip_fraction", "max_rungs",
    "numeric", "render", "render_views", "render_gate_blocking", "generator", "ladder", "gates",
    "assets", "levels", "source", "validity", "render_gate", "views", "thresholds", "signal",
    "noise_floor", "defect_floor", "search_limits",
})


def _finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def effective_render_blocking(manifest):
    """Is the rendered-difference gate ARMED? Returns (armed, reason).

    RATIFIED is not ARMED. Yan ruled the threshold, so `config.RENDER_GATE_BLOCKING` is True; but a
    verdict measured under a FALLBACK material was measured with the wrong textures, and blocking on
    it would be a gate certifying the wrong thing. So the flag arms itself the moment the material
    path is honest, with no second decision for anyone to remember — and until then the failures are
    recorded and shouted about rather than enforced.
    """
    if not CONFIG.RENDER_GATE_BLOCKING:
        return False, "config.RENDER_GATE_BLOCKING is False"
    for asset in manifest.get("assets", []):
        for level in asset.get("levels", []):
            gate = level.get("render_gate") or {}
            if str(gate.get("material_source", "")).startswith("FELL BACK"):
                return False, (
                    f"{asset.get('name')} L{level.get('level')} was judged under a fallback "
                    f"material ({gate.get('material_source')})"
                )
    return True, "ratified and rendering the shipped material"


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


def _check_gate_record(gate, label, failures, level_bbox_mm=None):
    """A render-gate record, checked as EVIDENCE rather than as a field list.

    Presence was not enough and the mutants proved it: a record whose every metric was NaN, or whose
    per-level `defect_fraction` had been rewritten to 999, still carried `pass: true` and verified
    clean. So the thresholds must match the tree, every metric must be a real number, and the
    verdict must RE-DERIVE from the metrics — a recorded pass that the recorded numbers do not
    support is a contradiction, and contradictions are exactly what a manifest is for catching.
    """
    ok = True
    for key in ("abstained", "screen_footprint_px", "min_footprint_px", "bbox_mm", "distance_m",
                "material_source"):
        if key not in gate:
            failures.append(f"{label}: render-gate record has no {key!r}")
            ok = False
    if not ok:
        return False

    # ABSTENTION IS RE-DERIVED FROM GEOMETRY, never trusted. It is the one verdict claimable
    # without rendering anything, so it is the one most worth recomputing: the asset's projected
    # diameter at the evaluation distance against the ratified minimum.
    #
    # AND THE GEOMETRY IS BOUND TO THE LEVEL'S OWN, which is what made the re-derivation worth
    # anything. Recomputing from the gate record's own bounding box only proves the record is
    # self-consistent: shrinking one level's gate bbox tenfold and setting `abstained` accordingly
    # verified clean, while the bytes that ship derive 27.6 px and should have been scored.
    if level_bbox_mm is not None and [round(float(v), 4) for v in gate["bbox_mm"]] != [
        round(float(v), 4) for v in level_bbox_mm
    ]:
        failures.append(
            f"{label}: the render gate measured a {gate['bbox_mm']} mm box, but this level's "
            f"decoded bytes are {level_bbox_mm} mm — it judged something else"
        )
        return False
    expected_footprint = screen_footprint_px(
        gate["bbox_mm"], gate["distance_m"], CONFIG.REFERENCE_VIEW
    )
    # 1e-2 px: the bounding box is recorded rounded to 0.1 um, so the re-derivation carries a
    # little rounding of its own before anything is actually wrong.
    if abs(gate["screen_footprint_px"] - expected_footprint) > 1e-2:
        failures.append(
            f"{label}: records a {gate['screen_footprint_px']} px footprint but its bounding box "
            f"at {gate['distance_m']} m gives {expected_footprint:.4f} px"
        )
        return False
    if gate["min_footprint_px"] != CONFIG.RENDER_GATE["min_footprint_px"]:
        failures.append(
            f"{label}: abstention was judged against {gate['min_footprint_px']} px, the tree "
            f"declares {CONFIG.RENDER_GATE['min_footprint_px']}"
        )
        return False
    should_abstain = expected_footprint < CONFIG.RENDER_GATE["min_footprint_px"]
    if bool(gate["abstained"]) != should_abstain:
        failures.append(
            f"{label}: records abstained={gate['abstained']} but a {expected_footprint:.1f} px "
            f"footprint against a {CONFIG.RENDER_GATE['min_footprint_px']:.0f} px minimum means "
            f"{should_abstain}"
        )
        return False
    if gate["abstained"]:
        # Nothing was scored, so nothing may be recorded as if it had been.
        if gate.get("pass") is not None:
            failures.append(
                f"{label}: abstained, but records a verdict {gate['pass']!r} — an abstention is "
                f"not a pass"
            )
            return False
        if not gate.get("reason"):
            failures.append(f"{label}: abstained without recording why")
            return False
        return True

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

    # EVERY recorded threshold, not a chosen four: a record naming `defect_normal_deg = 999` was
    # describing a gate run against a defect nobody declared, and nothing looked.
    thresholds = gate.get("thresholds") or {}
    for key in CONFIG.RECORDED_GATE_THRESHOLDS:
        if key not in thresholds:
            failures.append(f"{label}: render-gate thresholds have no {key!r}")
            ok = False
    for key, recorded in thresholds.items():
        if key not in CONFIG.RENDER_GATE:
            failures.append(f"{label}: render gate records an unknown threshold {key!r}")
            ok = False
        elif recorded != CONFIG.RENDER_GATE[key]:
            failures.append(
                f"{label}: render gate was judged against {key}={recorded!r}, the tree "
                f"declares {CONFIG.RENDER_GATE[key]!r}"
            )
            ok = False

    # The RENDER PARAMETERS are config, and a record claiming otherwise describes a run this tree
    # cannot reproduce.
    for key in ("tile_px", "supersample", "samples"):
        if gate[key] != CONFIG.RENDER_GATE[key]:
            failures.append(
                f"{label}: render gate ran at {key}={gate[key]!r}, the tree declares "
                f"{CONFIG.RENDER_GATE[key]!r}"
            )
            ok = False
    expected_fov = tile_vfov_rad(CONFIG.RENDER_GATE, CONFIG.REFERENCE_VIEW)
    if abs(gate["tile_vfov_rad"] - expected_fov) > 1e-7:
        failures.append(
            f"{label}: the gate tile's FOV is recorded as {gate['tile_vfov_rad']} but the tile size "
            f"and reference view give {expected_fov:.8f} — it did not preserve the reference "
            f"view's pixels-per-radian, so its pixels are not the player's"
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

    # EVERYTHING BELOW IS RE-DERIVED FROM THE PER-VIEW METRICS AND COMPARED.
    #
    # Presence and finiteness are not checking. An adversarial review set all three `worst_*`
    # summaries to -999 and inverted a per-view verdict, and verification returned zero failures:
    # every field was there, every number was finite, and not one of them had to agree with
    # anything. A summary nobody recomputes is a claim, and a claim recorded next to its own
    # evidence is exactly what a manifest exists to stop.
    limit = CONFIG.RENDER_GATE["defect_fraction"]
    tolerance = 1.5e-6  # the records are rounded to six decimal places
    derived_pass = True
    worst = {"mean": 0.0, "frac": 0.0, "score": 0.0}
    for view_name, view in sorted(views.items()):
        view_label = f"{label} [{view_name}]"
        signal, noise, defect = view["signal"], view["noise_floor"], view["defect_floor"]

        span = defect["mean_abs_diff"] - noise["mean_abs_diff"]
        score = (signal["mean_abs_diff"] - noise["mean_abs_diff"]) / span if span > 1e-9 else 1.0
        score = max(0.0, score)
        if abs(score - view["defect_score"]) > max(tolerance, abs(score) * 1e-5):
            failures.append(
                f"{view_label}: defect_score is recorded as {view['defect_score']} but its own "
                f"signal/noise/defect means re-derive to {score:.6f}"
            )
            return False

        floor_ok = (
            signal["mean_abs_diff"] <= CONFIG.RENDER_GATE["max_mean_abs_diff"]
            and signal["frac_over"] <= CONFIG.RENDER_GATE["max_footprint_frac_over"]
        )
        view_pass = signal["footprint_px"] > 0 and (view["defect_score"] <= limit or floor_ok)
        if view_pass != view["pass"]:
            failures.append(
                f"{view_label}: records pass={view['pass']} but its own metrics re-derive to "
                f"{view_pass} against the declared threshold {limit}"
            )
            return False
        if view["under_absolute_floor"] != floor_ok:
            failures.append(
                f"{view_label}: records under_absolute_floor={view['under_absolute_floor']} but "
                f"its metrics re-derive to {floor_ok}"
            )
            return False
        derived_pass = derived_pass and view_pass
        worst["mean"] = max(worst["mean"], signal["mean_abs_diff"])
        worst["frac"] = max(worst["frac"], signal["frac_over"])
        worst["score"] = max(worst["score"], view["defect_score"])

    for key, recorded, recomputed in (
        ("worst_mean_abs_diff", gate["worst_mean_abs_diff"], worst["mean"]),
        ("worst_frac_over", gate["worst_frac_over"], worst["frac"]),
        ("worst_defect_score", gate["worst_defect_score"], worst["score"]),
    ):
        if abs(recorded - recomputed) > tolerance:
            failures.append(
                f"{label}: {key} is recorded as {recorded} but the per-view records it summarises "
                f"give {recomputed:.6f}"
            )
            return False

    if derived_pass != gate["pass"]:
        failures.append(
            f"{label}: render gate records pass={gate['pass']} but its own numbers re-derive to "
            f"{derived_pass} — the verdict does not follow from the evidence beside it"
        )
        return False

    # The fallback-material precondition is NOT a failure here any more: it disarms the gate rather
    # than condemning the manifest. `effective_render_blocking` owns that decision once, for the
    # whole corpus, and says so loudly — see `RENDER_GATE_BLOCKING`.
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
                ok = _check_gate_record(
                    gate, label, failures, (level.get("validity") or {}).get("bbox_mm")
                ) and ok
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
    ruling = ladder.get("ratification") or {}
    if ruling != dict(CONFIG.RATIFICATION_EVIDENCE["ruling"]):
        failures.append(
            "the manifest's ratification provenance differs from config.RATIFICATION_EVIDENCE — "
            "the threshold this corpus was judged against is not the one the tree records a ruling "
            "for"
        )
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


def _check_level_cross_fields(asset, levels, index, row, failures):
    """Constraints BETWEEN records, which is where the second round of mutants walked straight in.

    Re-deriving a record from its own contents is not enough when the record sits next to the thing
    it should agree with. Each of these was a passing mutation: a gate run distance of -999 beside
    the switch it was supposed to measure, a shed fraction of -999 beside the two triangle counts
    that define it, a validity record claiming 999 triangles beside a level claiming 315, both
    component switch distances at -999 beside the maximum they are supposed to bracket.
    """
    level = levels[index]
    label = f"{asset['name']} L{level['level']}"
    validity = level["validity"]

    # The validity record is a measurement OF THIS LEVEL's bytes, so its own counts must match.
    for key in ("tris", "verts"):
        if validity[key] != level[key]:
            failures.append(
                f"{label}: the validity record describes {validity[key]} {key} but the level "
                f"records {level[key]} — one of them is not about these bytes"
            )

    if index == 0:
        return

    # The rung fixes the target: e_N = e1 * octave^(N-1), and nothing else is a rung of the grid.
    expected_target = CONFIG.E1_MM * CONFIG.OCTAVE ** (level["rung"] - 1)
    if abs(level["e_target_mm"] - expected_target) > 1e-6:
        failures.append(
            f"{label}: rung {level['rung']} is {expected_target:.6f} mm on the global grid, but "
            f"this level records a target of {level['e_target_mm']} mm"
        )

    # The shed fraction is arithmetic on two triangle counts that are both right here.
    parent_tris = levels[index - 1]["tris"]
    expected_shed = 1.0 - level["tris"] / parent_tris
    if abs(level["shed_fraction_vs_parent"] - expected_shed) > 1e-4:
        failures.append(
            f"{label}: records shedding {level['shed_fraction_vs_parent']} of its parent, but "
            f"{level['tris']} against {parent_tris} triangles is {expected_shed:.6f}"
        )
    if expected_shed < CONFIG.SKIP_FRACTION - 1e-9:
        failures.append(
            f"{label}: sheds {expected_shed:.4f} of L{levels[index - 1]['level']}, under the "
            f"declared SKIP_FRACTION {CONFIG.SKIP_FRACTION} — it should not have earned a level"
        )

    # A lower bound above its own upper bound is not a bracket.
    if level["pairwise_mm"] > level["pairwise_mm_upper"] + 2e-6:
        failures.append(
            f"{label}: pairwise deviation {level['pairwise_mm']} exceeds its own certified upper "
            f"bound {level['pairwise_mm_upper']}"
        )

    # Both COMPONENT distances re-derive, and the switch is their maximum. Checking only the
    # maximum let both components be set to -999 without a word.
    slack = row["origin_radius_m"]
    view = CONFIG.REFERENCE_VIEW
    for key, deviation in (
        ("switch_from_source_dev_m", level["dev_source_mm_upper"]),
        ("switch_from_pairwise_m", level["pairwise_mm_upper"]),
    ):
        expected = switch_distance_m(deviation, slack, view)
        if abs(level[key] - expected) > 1e-3:
            failures.append(
                f"{label}: {key} is recorded as {level[key]} m but its deviation and slack give "
                f"{expected:.4f} m"
            )
    components = max(level["switch_from_source_dev_m"], level["switch_from_pairwise_m"])
    if abs(level["switch_m"] - components) > 1e-3:
        failures.append(
            f"{label}: switch_m is {level['switch_m']} m but the two component distances it takes "
            f"the maximum of give {components:.4f} m"
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
    armed, arming_reason = effective_render_blocking(manifest)
    if not armed and CONFIG.RENDER_GATE_BLOCKING:
        warnings.append(
            f"the rendered-difference gate is RATIFIED but NOT ARMED: {arming_reason}. It arms "
            f"itself once that is fixed — no second decision to remember."
        )

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
        for index, (row, level) in enumerate(zip(chain["levels"], asset["levels"])):
            label = f"{asset['name']} L{level['level']}"
            if not tree.exists(level["glb"]):
                failures.append(f"{label}: missing {level['glb']}")
            elif tree.digest(level["glb"]) != level["glb_sha256"]:
                failures.append(
                    f"{label}: {level['glb']} does not hash to the manifest's record — it was "
                    f"edited or rebuilt outside the pipeline"
                )
            validity = level["validity"]
            # A GENERATED level must carry a tangent per vertex. `degenerate_tangents` counts bad
            # ones among those present, so zero baked tangents scored a clean zero and passed —
            # which is exactly the state an exporter omission would leave behind, with bevy
            # generating uncertified tangents at load.
            if level["role"] == "generated":
                if not level.get("tangents_are_baked"):
                    failures.append(f"{label}: does not record baked tangents")
                elif validity["baked_tangents"] != validity["verts"]:
                    failures.append(
                        f"{label}: {validity['baked_tangents']} baked tangents for "
                        f"{validity['verts']} vertices — a generated level bakes one per vertex, or "
                        f"the loader generates them and nothing certified what renders"
                    )
            for key, limit in (
                ("duplicate_faces", CONFIG.GATES["max_duplicate_faces"]),
                ("nonfinite_attrs", CONFIG.GATES["max_nonfinite"]),
                ("orientation_flips", CONFIG.GATES["max_orientation_flips"]),
                ("nonmanifold_edges", CONFIG.GATES["max_nonmanifold_edges"]),
                ("tangent_default_faces", CONFIG.GATES["max_tangent_default_faces"]),
                ("tangent_default_verts", CONFIG.GATES["max_tangent_default_verts"]),
                ("degenerate_tangents", CONFIG.GATES["max_degenerate_tangents"]),
                ("slivers_below_floor", 0),
            ):
                if validity[key] > limit:
                    failures.append(f"{label}: recorded {validity[key]} {key} against a limit of {limit}")
            _check_level_cross_fields(asset, asset["levels"], index, row, failures)
            if level["role"] == "source":
                previous = row
                continue
            # THE DEVIATION RECORD MUST BE INTERNALLY CONSISTENT. Without this, understating a
            # level's certified deviation is invisible whenever the pairwise figure dominates the
            # switch derivation: the rung-target check only bounds from above, and the derived
            # distance does not move. Tying the four recorded numbers to each other makes any single
            # one of them impossible to edit alone.
            # 2e-6: each field is rounded to six places independently, so a comparison between two
            # of them carries up to 1e-6 of rounding on its own before anything is wrong.
            rounding = 2e-6
            two_way = max(level["dev_source_to_level_mm"], level["dev_level_to_source_mm"])
            if abs(level["dev_source_mm"] - two_way) > rounding:
                failures.append(
                    f"{label}: dev_source_mm is {level['dev_source_mm']} but the two directions it "
                    f"summarises give {two_way}"
                )
            bracket = level["dev_source_mm_upper"] - level["dev_source_mm"]
            if abs(bracket - level["dev_source_bracket_mm"]) > rounding:
                failures.append(
                    f"{label}: the certified bracket is recorded as "
                    f"{level['dev_source_bracket_mm']} but upper - lower is {bracket:.6f}"
                )
            if bracket < -1e-9:
                failures.append(f"{label}: certified upper bound is below its own lower bound")
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
            if gate.get("abstained"):
                warnings.append(
                    f"{label}: render gate ABSTAINED — {gate['screen_footprint_px']:.1f} px across "
                    f"at {gate['distance_m']:.0f} m, under the ratified "
                    f"{gate['min_footprint_px']:.0f} px (Yan, 2026-08-02)"
                )
                previous = row
                continue
            if abs(gate["distance_m"] - level["switch_m"]) > 1e-3:
                failures.append(
                    f"{label}: the render gate was run at {gate['distance_m']} m but this level "
                    f"switches at {level['switch_m']} m — it judged the pop at the wrong distance"
                )
            if not gate.get("pass", False):
                verdict = (
                    f"{label}: render gate recorded a FAIL (defect score "
                    f"{gate.get('worst_defect_score')} against a limit of "
                    f"{gate.get('thresholds', {}).get('defect_fraction')})"
                )
                if armed:
                    failures.append(verdict)
                else:
                    warnings.append(f"{verdict} — NOT enforced: {arming_reason}")
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
    parser.add_argument(
        "--repo", default=None,
        help="the work tree to run git against; needed when this script has been extracted out of "
             "a revision into a temp directory that is not itself inside a repository",
    )
    args = parser.parse_args()

    try:
        manifest, root, tree = load(root=args.repo, path=args.manifest, rev=args.rev)
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
