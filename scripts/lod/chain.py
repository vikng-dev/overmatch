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
    generated level, indices in order, and a hash and a validity record on every level. Absence is
    a failure, because "no measurement" and "measured zero defects" are different statements and
    only the second is evidence,
  * every number is FINITE. NaN fails every `>` and every `!=` quietly, so a poisoned manifest used
    to verify clean,
  * every level's glb exists and still hashes to what the manifest recorded (an asset edited by
    hand, or an LFS pointer that never got smudged, fails here),
  * the manifest was cut by the generator version, the generator SOURCES, and the whole gate and
    ladder configuration now in `config.py`,
  * every recorded defect counter is inside its declared limit,
  * the manifest's own arithmetic is self-consistent: switch distances re-derive from the recorded
    deviations, every rung's node budget re-derives from the source's own bounding box, the chain is
    monotone in triangles and in distance, and every level's deviation really is inside its rung,
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
import measure as M  # noqa: E402
from manifest import (  # noqa: E402
    Tree, derive, load, merge_asset_entries, sha256_file, switch_distance_m,
)


LEVEL_NUMERIC_FIELDS = (
    "tris", "verts", "e_target_mm", "dev_source_mm", "dev_source_mm_upper",
    "pairwise_mm", "pairwise_mm_upper", "switch_m", "shed_fraction_vs_parent",
    "switch_from_source_dev_m", "switch_from_pairwise_m", "dev_source_bracket_mm",
    "dev_source_to_level_mm", "dev_level_to_source_mm", "glb_bytes",
    "verdict_node_budget", "undecided_verdicts",
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
    "empty_surfaces", "nonmanifold_edges", "boundary_edges",
    "origin_radius_m", "min_tri_area_mm2", "radius_m",
)

#: Validity fields that are LISTS rather than numbers, and are re-derived from the bytes like the
#: rest. `bbox_mm` was optional, so removing it removed the comparison with it — and an abstention
#: could then be bought by shrinking the gate's own copy.
LEVEL_VALIDITY_VECTORS = ("bbox_mm",)

#: A `skipped_rungs` record's required numerics, and the only things `lost_to` may say.
#:
#: THE SKIP RECORDS ARE EVIDENCE, not commentary, and until now nothing looked at them. They are the
#: manifest's whole account of what the ladder DID NOT ship and why, and two of the reasons —
#: `verdict_node_budget` and `certification_bracket` (`M.SKIP_FIDELITY_LOST_TO`) — are where a
#: corpus admits it traded fidelity for wall time. A record nobody validates is a record that can
#: suppress that admission by being malformed: an unknown `lost_to`, a missing counter, or a
#: positive `undecided_verdicts` filed under `skip_fraction` all leave `verify` silently agreeable.
SKIP_NUMERIC_FIELDS = ("rung", "e_target_mm", "undecided_verdicts", "verdict_node_budget")
#: Counters inside a skip record that must be non-negative integers wherever they appear.
SKIP_COUNTER_FIELDS = ("undecided_verdicts", "verdict_node_budget", "floor_tris", "best_tris")
#: The legal values, READ FROM `measure` rather than spelled again — the same declaration the
#: generator picks from — plus `certification_bracket`, which only the per-primitive build
#: (`scripts/tank/chains.py`) can file and which the legacy corpus therefore never carries. Two
#: copies of the shared tuple is two rules, which is how the writer came to file an UNDECIDED rung
#: under `skip_fraction` while the verifier's warning only knew about the third.
SKIP_LOST_TO = M.SKIP_LOST_TO + ("certification_bracket",)

#: The subset of [`SKIP_LOST_TO`] that means "a rung was lost to the RUN, not to the mesh", and is
#: therefore WARNED about below. `verdict_node_budget` is the search abstaining inside its node
#: budget (ADR 0036 §1); `certification_bracket` is a winner whose certified UPPER bound missed the
#: rung on a bracket whose lower end cleared it (ADR 0033 §6, `scripts/tank/chains.py`). Both cost a
#: rung a longer run would have shipped, so both have to be loud — a fidelity loss is loud whatever
#: took the rung. Neither fails a build: nothing unproven ships either way.
SKIP_FIDELITY_LOST_TO = ("verdict_node_budget", "certification_bracket")

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
    "schema", "script", "right_wall_source", "provenance", "name", "note",
    "identity_proof", "termination", "role", "node", "glb",
    "object", "blend", "bbox_mm", "material", "evaluated_digest",
    "skipped_rungs", "rung", "level", "parent_level", "e_target_mm",
    "shed_fraction", "cleanup", "faces_before", "faces_after",
    "dissolve_dist_m",
    "normal_diagnostic_deg", "max_deg", "p99_deg", "p95_deg", "backface_corr_frac", "samples",
    "shipped_matches_source", "decimations", "verdicts", "verdict_nodes", "distinct_candidates",
    "reproducible", "label", "topology_floor_tris",
    "blend_sha256", "sources_sha256", "glb_sha256", "welded_digest", "right_wall_m",
    "reference_view",
    "vfov_rad", "height_px", "budget_px", "e1_mm", "octave", "skip_fraction", "max_rungs",
    "numeric", "generator", "ladder", "gates", "assets", "levels", "source", "validity",
})


def _finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def _nonfinite_paths(node, path=()):
    """Every path in the document whose leaf is a non-finite number."""
    if isinstance(node, dict):
        for key, value in node.items():
            yield from _nonfinite_paths(value, path + (str(key),))
    elif isinstance(node, list):
        for index, value in enumerate(node):
            yield from _nonfinite_paths(value, path + (str(index),))
    elif isinstance(node, float) and not math.isfinite(node):
        yield ".".join(path)


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
    # EVERY numeric leaf, everywhere, must be finite — not just the ones a check happens to name.
    #
    # A field-by-field list only defends the fields on it, and a NaN defeats every `abs(x-y) > tol`
    # binding silently, because every comparison with NaN is False. Sweeping the whole document
    # takes microseconds and removes the entire class rather than the instances anyone thought of:
    # a probe found 206 numeric leaves that accepted NaN, all of them in records no named check
    # covered. There is no legitimate NaN or infinity anywhere in a manifest.
    nonfinite = sorted(_nonfinite_paths(manifest))
    if nonfinite:
        failures.append(
            f"{len(nonfinite)} non-finite number(s) in the manifest, starting at "
            f"{', '.join(nonfinite[:4])} — a NaN loses every comparison silently, so any binding "
            f"that reads it passes without looking"
        )
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
            for key in LEVEL_VALIDITY_VECTORS:
                value = validity.get(key)
                if not isinstance(value, list) or not value or not all(
                    _finite(component) for component in value
                ):
                    failures.append(
                        f"{label}: validity {key!r} is {value!r} — required, and every component "
                        f"must be a finite number"
                    )
                    ok = False
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
        ok = _check_skipped_rungs(asset, failures) and ok
    return ok


def _check_skipped_rungs(asset, failures):
    """The record of what the ladder did NOT ship, checked as evidence rather than read as prose."""
    name = asset.get("name")
    skipped = asset.get("skipped_rungs")
    if not isinstance(skipped, list):
        failures.append(f"{name}: skipped_rungs is {skipped!r}, not a list")
        return False

    ok = True
    kept = {level.get("rung") for level in asset.get("levels") or []}
    seen = set()
    for entry in skipped:
        if not isinstance(entry, dict):
            failures.append(f"{name}: a skipped-rung record is {entry!r}, not a record")
            ok = False
            continue
        label = f"{name} skipped rung {entry.get('rung')!r}"
        if not _require_numbers(entry, SKIP_NUMERIC_FIELDS, label, "skip record", failures):
            ok = False
            continue
        rung = entry["rung"]
        if not isinstance(rung, int) or isinstance(rung, bool) or not 1 <= rung <= CONFIG.MAX_RUNGS:
            failures.append(f"{label}: not a rung of the grid (1..{CONFIG.MAX_RUNGS})")
            ok = False
            continue
        if rung in seen:
            failures.append(f"{label}: recorded twice")
            ok = False
        seen.add(rung)
        if rung in kept:
            failures.append(f"{label}: this rung also earned a level — it cannot be both")
            ok = False
        expected_target = CONFIG.E1_MM * CONFIG.OCTAVE ** (rung - 1)
        if abs(entry["e_target_mm"] - expected_target) > 1e-3:
            failures.append(
                f"{label}: records a {entry['e_target_mm']} mm target, but rung {rung} is "
                f"{expected_target:.4f} mm on the global grid"
            )
            ok = False
        for key in SKIP_COUNTER_FIELDS:
            if key not in entry:
                continue
            value = entry[key]
            if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                failures.append(f"{label}: {key} is {value!r}, not a non-negative whole count")
                ok = False
        if not str(entry.get("reason") or "").strip():
            failures.append(f"{label}: skipped without recording why")
            ok = False
        if entry.get("lost_to") not in SKIP_LOST_TO:
            failures.append(
                f"{label}: lost_to is {entry.get('lost_to')!r}, not one of {list(SKIP_LOST_TO)} — "
                f"an unknown reason is a reason nothing can act on, and one of these three is the "
                f"only place a corpus admits it traded fidelity for wall time"
            )
            ok = False
        # THE MISATTRIBUTION THIS EXISTS TO CATCH, enforced rather than trusted to the writer: a
        # rung the search abstained on is lost to the BUDGET, whatever else is also true of it. A
        # record filing an UNDECIDED verdict under `skip_fraction` reads as an ordinary sparse-chain
        # skip and suppresses the fidelity warning on exactly the rung that earned one.
        expected = M.rung_lost_to(entry.get("undecided_verdicts", 0), entry.get("lost_to"))
        if entry.get("lost_to") != expected:
            failures.append(
                f"{label}: {entry['undecided_verdicts']} verdict(s) went UNDECIDED here but the "
                f"rung is filed as {entry.get('lost_to')!r} — a rung the search could not decide "
                f"is lost to {expected!r}, and filing it otherwise hides the trade"
            )
            ok = False
    return ok


def _check_config_match(manifest, failures):
    """The manifest must have been cut by the configuration THIS TREE holds, all of it.

    Not a sample of it, and never read back out of the manifest and trusted: a corpus cut before a
    constant moved is stale, and the only honest answer to that is a regeneration.
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
    # The SEARCH's declared budget, which decides which rungs a chain could even keep — so a corpus
    # cut under a different coefficient or cap is a corpus that answers a different question.
    for key, declared in (
        ("normal_diagnostic_samples", CONFIG.NORMAL_DIAGNOSTIC_SAMPLES),
        ("verdict_nodes_per_square", CONFIG.VERDICT_NODES_PER_SQUARE),
        ("verdict_nodes_cap", CONFIG.VERDICT_NODES_CAP),
    ):
        if key not in gates:
            failures.append(f"search setting {key!r} is not recorded in the manifest")
        elif gates[key] != declared:
            failures.append(f"search {key}: manifest {gates[key]!r} != config {declared!r}")


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

    # THE SEARCH'S BUDGET IS ARITHMETIC ON TWO RECORDED NUMBERS, so it re-derives like every other
    # threshold rather than being taken on trust. A manifest claiming a budget this tree's constants
    # would not have granted describes a search that never ran here.
    diagonal_mm = CONFIG.diagonal_mm_from_bbox(asset["source"]["validity"]["bbox_mm"])
    expected_budget = CONFIG.verdict_node_budget(diagonal_mm, level["e_target_mm"])
    if level["verdict_node_budget"] != expected_budget:
        failures.append(
            f"{label}: records a {level['verdict_node_budget']}-node verdict budget, but a "
            f"{diagonal_mm:.1f} mm diagonal at a {level['e_target_mm']} mm target gives "
            f"{expected_budget}"
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


#: Validity fields that are NOT re-derivable from a level's own bytes, with why.
#:
#: Empty: every field in a level's validity record is recomputed from that level's own decoded
#: bytes. `components` is compared against decoded L0's, which is derived rather than recorded.
VALIDITY_NOT_FROM_OWN_BYTES = ()


def _decode_level(tree, level, label, failures):
    """The level's shipped bytes as a Surface, or None with a named failure."""
    if not tree.exists(level["glb"]):
        return None                 # the missing-file failure is raised by the hash check
    blob = tree.blob(level["glb"])
    if blob is None:
        failures.append(
            f"{label}: {level['glb']} is an LFS pointer whose object is not available locally, so "
            f"nothing about these bytes can be verified. Run `git lfs pull` (CI does) — a level "
            f"that cannot be decoded is not a level that passed."
        )
        return None
    try:
        return M.surface_from_bytes(blob, level.get("node"), label)
    except M.Refusal as refusal:
        failures.append(f"{label}: the shipped bytes do not decode — {refusal}")
        return None


def _check_decoded_bytes(asset, level, tree, failures, l0_derived=None):
    """Re-derive the level's whole validity record FROM THE SHIPPED BYTES and compare it.

    THIS IS THE ROOT FIX FOR A WHOLE BYPASS CLASS: verification compared a recorded number against
    another recorded number, which proves the manifest agrees with itself and says nothing about the
    asset. Editing a level's attributes and updating its hash left the stale validity record
    describing a mesh that was no longer in the file, and verification passed.

    So the bytes are decoded — the verifier already reads them to hash them — and every counter is
    recomputed. A record can no longer describe a mesh that is not in the file, because the file is
    what the record is checked against.
    """
    label = f"{asset['name']} L{level['level']}"
    surface = _decode_level(tree, level, label, failures)
    if surface is None:
        return

    recorded = level["validity"]
    derived = surface.validity()
    for key in LEVEL_VALIDITY_FIELDS:
        if key in VALIDITY_NOT_FROM_OWN_BYTES:
            continue
        if isinstance(derived[key], float):
            if abs(derived[key] - recorded[key]) > 1e-9:
                failures.append(
                    f"{label}: validity records {key}={recorded[key]}, the shipped bytes give "
                    f"{derived[key]}"
                )
        elif derived[key] != recorded[key]:
            failures.append(
                f"{label}: validity records {key}={recorded[key]}, the shipped bytes give "
                f"{derived[key]}"
            )
    for key in LEVEL_VALIDITY_VECTORS:
        if [round(float(v), 4) for v in recorded[key]] != [
            round(float(v), 4) for v in derived[key]
        ]:
            failures.append(
                f"{label}: validity records {key}={recorded[key]}, the shipped bytes give "
                f"{derived[key]}"
            )

    # THE SAME GATE LIST GENERATION USES, against values re-derived from the bytes — including
    # `source_validity`, which is decoded L0 rather than a number the manifest asserted. This is
    # where `components_must_match` finally applies at verification: it was compared when a level
    # was cut and never again, so a level that had split into two pieces verified clean.
    if l0_derived is not None:
        for failure in M.validity_gate_failures(derived, l0_derived, CONFIG.GATES):
            failures.append(f"{label}: {failure}")


def verify(manifest, tree):
    """Every drift a manifest can suffer. Returns (failures, warnings); empty failures means clean.

    A WARNING is a recorded fact that is deliberately not enforced — a rung lost to the RUN rather
    than to the geometry: to the search's node budget (ADR 0036 §1), or to a certification bracket
    whose upper end missed the rung while its lower end cleared it (ADR 0033 §6). Both are printed
    every time, so a corpus that traded fidelity for time says so out loud; neither fails the build,
    because nothing unproven ships either way.
    """
    failures = []
    warnings = []
    if not _check_schema(manifest, failures):
        return failures, warnings
    _check_config_match(manifest, failures)
    if failures:
        # STOP HERE. Everything below derives numbers from the ladder and the reference view, and
        # deriving from a configuration this tree has already rejected is meaningless — worse, it
        # is where a hostile value gets to raise instead of being reported: `math.tan(inf)` throws,
        # so a poisoned `vfov_rad` turned a verifier that should say "this manifest is wrong" into
        # a traceback. A refusal has to look like a refusal.
        return failures, warnings

    for asset in manifest["assets"]:
        for skip in asset.get("skipped_rungs", []):
            if skip.get("lost_to") not in SKIP_FIDELITY_LOST_TO:
                continue
            if skip.get("lost_to") == "verdict_node_budget":
                lost = (
                    f"LOST TO THE NODE BUDGET, not to the geometry — "
                    f"{skip.get('undecided_verdicts')} verdict(s) spent "
                    f"{skip.get('verdict_node_budget')} nodes without closing a bound"
                )
            else:
                lost = (
                    f"LOST TO THE CERTIFICATION BRACKET, not to the geometry — the winner's "
                    f"certified upper bound {skip.get('dev_mm_upper')} mm missed the rung on a "
                    f"bracket whose lower end is {skip.get('dev_mm')} mm"
                )
            warnings.append(
                f"{asset['name']} rung {skip.get('rung')}: {lost}. The chain is coarser here than "
                f"a longer run would have cut it. ({skip.get('reason')})"
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
        # DECODE L0 FIRST: it is the source of the component count every level is compared
        # against, and that may not come from the manifest.
        #
        # ── WHAT BINDS L0, AND WHAT DOES NOT ─────────────────────────────────────────────────
        #
        # L0 is the BASELINE: the component count every level is compared to is derived from it.
        # So it is worth being exact about what stops it being chosen freely.
        #
        # BOUND: the decoded bytes must reproduce `welded_digest`, a split-invariant fingerprint of
        # L0's geometry recorded at generation, at the moment L0 had just been proven identical to
        # the evaluated .blend source. Moving any vertex, or joining the surface differently,
        # changes it. That closes the bytes-only attack the probe used.
        #
        # NOT BOUND, and this is the honest limit: the fingerprint lives in the same manifest as
        # everything else. Someone who rewrites the assets AND the manifest together can produce a
        # self-consistent corpus that verifies — the fingerprint makes that a VISIBLE diff on a
        # named field rather than a silent byte swap, but it is not a signature. What actually
        # defends this is the .blend digest (checked whenever the untracked source is present), code
        # review, and git history. This verifier proves a corpus is internally consistent and
        # consistent WITH ITS BYTES; it does not prove the bytes are the ones an artist authored.
        l0_surface = _decode_level(tree, asset["levels"][0], f"{asset['name']} L0", failures)
        l0_derived = None
        if l0_surface is not None:
            recorded_digest = asset["levels"][0].get("welded_digest")
            if not recorded_digest:
                failures.append(
                    f"{asset['name']} L0: no welded_digest — the baseline every other level is "
                    f"judged against is unbound to the source generation certified it from"
                )
            elif l0_surface.welded_digest() != recorded_digest:
                failures.append(
                    f"{asset['name']} L0: the shipped bytes do not reproduce the recorded geometry "
                    f"fingerprint. L0 sets the component count for the whole corpus, so a "
                    f"changed baseline silently re-judges every level."
                )
            l0_derived = l0_surface.validity()

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
            # THE ONLY GATE PATH IS THE SHARED LIST, inside `_check_decoded_bytes`, against values
            # re-derived from the decoded bytes. A second hand-maintained copy of the counters used
            # to sit here, comparing the RECORDED numbers — which is both weaker (a record can lie)
            # and exactly the two-lists-that-must-agree arrangement whose drift produced two of this
            # week's findings. The claim "one list" is only true if this is not here.
            if l0_derived is not None:
                _check_decoded_bytes(
                    asset, level, tree, failures, l0_derived
                )
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
            if level["undecided_verdicts"]:
                warnings.append(
                    f"{label}: {level['undecided_verdicts']} verdict(s) at this rung spent the "
                    f"whole {level['verdict_node_budget']}-node budget and were counted as "
                    f"failures — the level that shipped is certified, and it may carry more "
                    f"triangles than an unbounded search would have found"
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
