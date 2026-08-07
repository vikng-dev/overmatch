"""The seam's own tests: the projection, the drift catches, and the shipped manifest.

    python3 scripts/lod/test_chain.py

Stdlib `unittest` on purpose — this must run without Blender, without numpy and without cargo, so
`scripts/hooks/pre-push` can afford it on every push.

THE REGRESSION THESE EXIST FOR: an exporter comment narrated 223.7 m for a level whose runtime
derivation said 335.5 m. Both were "measured"; the comment had frozen a stale deviation and used
the small-angle projection besides. `the_drifted_ledger_is_caught` reconstructs exactly that shape
and asserts `verify` refuses it.
"""

import hashlib
import json
import shutil
import struct
import tempfile
import math
import os
import sys
import unittest

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import chain  # noqa: E402
import config as CONFIG  # noqa: E402

VIEW = CONFIG.REFERENCE_VIEW


def _numeric_paths(node, path=()):
    """Every path to a numeric leaf — the sweep is over the document, not over a curated list."""
    if isinstance(node, dict):
        for key, value in node.items():
            yield from _numeric_paths(value, path + (key,))
    elif isinstance(node, list):
        for index, value in enumerate(node):
            yield from _numeric_paths(value, path + (index,))
    elif isinstance(node, (int, float)) and not isinstance(node, bool):
        yield path


def _set_path(node, path, value):
    for key in path[:-1]:
        node = node[key]
    node[path[-1]] = value


def _rebuild_glb(gltf, binary):
    """Re-serialise a decoded glb. Only the JSON chunk changes; the BIN blob rides along."""
    text = json.dumps(gltf).encode()
    text += b" " * ((-len(text)) % 4)
    padded = binary + b"\x00" * ((-len(binary)) % 4)
    total = 12 + 8 + len(text) + 8 + len(padded)
    return b"".join([
        struct.pack("<4sII", b"glTF", 2, total),
        struct.pack("<II", len(text), 0x4E4F534A), text,
        struct.pack("<II", len(padded), 0x004E4942), padded,
    ])


def validity_record(tris, verts, origin_radius=0.1, bbox_mm=(700.0, 180.0, 180.0)):
    """A clean validity record — every field the strict schema requires, all zero defects."""
    return {
        "tris": tris, "verts": verts, "components": 1, "duplicate_faces": 0,
        "nonfinite_attrs": 0, "orientation_flips": 0, "nonmanifold_edges": 0,
        "boundary_edges": 0, "slivers_below_floor": 0, "tangent_default_faces": 0,
        "tangent_default_verts": 0, "min_altitude_m": 0.001, "min_altitude_floor_m": 0.0001,
        "min_tri_area_mm2": 1.0, "origin_radius_m": origin_radius,
        "baked_tangents": verts, "degenerate_tangents": 0, "min_tangent_length": 0.999999,
        "bbox_mm": list(bbox_mm), "radius_m": 0.4,
    }


def view_record(passed):
    """One rendered view's statistics, INTERNALLY CONSISTENT.

    The score is computed from the three means exactly as the gate computes it, because the verifier
    now re-derives it — a fixture carrying a score its own numbers do not support would be testing
    nothing except that the fixture is wrong.
    """
    noise_mean, defect_mean = 0.0005, 0.0105
    signal_mean = 0.001 if passed else 0.5
    signal = {
        "footprint_px": 900,
        "mean_abs_diff": signal_mean,
        "p99_abs_diff": 0.002, "max_abs_diff": 0.003,
        "frac_over": 0.0 if passed else 0.5,
        "silhouette_band_px": 120, "silhouette_band_frac": 0.1,
    }

    def reference(mean):
        return {
            "footprint_px": 900, "mean_abs_diff": mean, "p99_abs_diff": 0.001,
            "max_abs_diff": 0.002, "frac_over": 0.0,
            "silhouette_band_px": 120, "silhouette_band_frac": 0.1,
        }

    score = round(max(0.0, (signal_mean - noise_mean) / (defect_mean - noise_mean)), 6)
    floor_ok = (
        signal_mean <= CONFIG.RENDER_GATE["max_mean_abs_diff"]
        and signal["frac_over"] <= CONFIG.RENDER_GATE["max_footprint_frac_over"]
    )
    return {
        "signal": signal,
        "noise_floor": reference(noise_mean),
        "defect_floor": reference(defect_mean),
        "defect_score": score,
        "under_absolute_floor": floor_ok,
        "pass": score <= CONFIG.RENDER_GATE["defect_fraction"] or floor_ok,
    }


def gate_record(passed=True, material="decoded from probe.glb", distance_m=90.0,
                bbox_mm=(700.0, 180.0, 180.0)):
    """A render-gate record whose recorded verdict and summaries FOLLOW from its per-view numbers."""
    views = {name: view_record(passed) for name, _e, _a in CONFIG.RENDER_GATE["views"]}
    footprint = chain.screen_footprint_px(list(bbox_mm), distance_m, VIEW)
    return {
        "abstained": False,
        "screen_footprint_px": round(footprint, 4),
        "min_footprint_px": CONFIG.RENDER_GATE["min_footprint_px"],
        "bbox_mm": list(bbox_mm),
        "pass": all(view["pass"] for view in views.values()),
        "worst_defect_score": max(view["defect_score"] for view in views.values()),
        "worst_mean_abs_diff": max(view["signal"]["mean_abs_diff"] for view in views.values()),
        "worst_frac_over": max(view["signal"]["frac_over"] for view in views.values()),
        "distance_m": distance_m, "tile_px": CONFIG.RENDER_GATE["tile_px"],
        "supersample": CONFIG.RENDER_GATE["supersample"],
        "samples": CONFIG.RENDER_GATE["samples"],
        "tile_vfov_rad": chain.tile_vfov_rad(CONFIG.RENDER_GATE, VIEW),
        "material_source": material,
        "views": views,
        # The same single declaration the renderer and the verifier both read.
        "thresholds": {
            key: CONFIG.RENDER_GATE[key] for key in CONFIG.RECORDED_GATE_THRESHOLDS
        },
    }


def synthetic_manifest():
    """A two-level chain with hand-computed numbers, independent of any generation run.

    Built FROM the configuration rather than beside it, because the verifier compares the whole
    configuration and a hand-typed copy would drift the moment a threshold moves — which is the same
    disease the manifest exists to cure.
    """
    radius = 0.1
    dev1, pair1 = 4.0, 5.0
    switch1 = chain.switch_distance_m(max(dev1, pair1), radius, VIEW)
    asset_name = CONFIG.ASSETS[0]["name"]
    return {
        "schema": "overmatch.lod.manifest",
        "schema_version": CONFIG.SCHEMA_VERSION,
        "generator": {
            "script": "scripts/lod/generate.py",
            "version": CONFIG.GENERATOR_VERSION,
            "sources_sha256": CONFIG.generator_digest(),
            "blender": CONFIG.EXPECTED_BLENDER,
            "blender_build": CONFIG.EXPECTED_BLENDER_BUILD,
            "gltf_exporter": CONFIG.EXPECTED_GLTF_EXPORTER,
        },
        "ladder": {
            "e1_mm": CONFIG.E1_MM, "octave": CONFIG.OCTAVE,
            "skip_fraction": CONFIG.SKIP_FRACTION, "max_rungs": CONFIG.MAX_RUNGS,
            "right_wall_m": round(CONFIG.RIGHT_WALL_M, 6),
            "right_wall_source": CONFIG.RIGHT_WALL_SOURCE, "reference_view": VIEW,
            "ratification": dict(CONFIG.RATIFICATION_EVIDENCE["ruling"]),
        },
        "gates": {
            "numeric": dict(CONFIG.GATES),
            "render": {k: v for k, v in CONFIG.RENDER_GATE.items() if k != "views"},
            "render_views": [list(v) for v in CONFIG.RENDER_GATE["views"]],
            "search_limits": dict(CONFIG.SEARCH_LIMITS),
            "render_gate_blocking": CONFIG.RENDER_GATE_BLOCKING,
        },
        "assets": [{
            "name": asset_name,
            "source": {"blend": "assets/does/not/exist.blend", "blend_sha256": "0" * 64,
                       "object": "Probe", "evaluated_digest": "1" * 64, "tris": 1000,
                       "verts": 500, "radius_m": radius, "bbox_mm": [1, 1, 1],
                       "validity": validity_record(1000, 500, radius)},
            "topology_floor_tris": 100,
            "termination": "right_wall",
            "skipped_rungs": [],
            "levels": [
                {"level": 0, "rung": 0, "role": "source", "tris": 1000, "verts": 2400,
                 "glb": "a/source.glb", "glb_sha256": "a" * 64, "node": "Probe",
                 "e_target_mm": 0.0, "dev_source_mm": 0.0, "dev_source_mm_upper": 0.0,
                 "pairwise_mm": None, "switch_m": 0.0,
                 "blender_source_verts": 500, "shipped_dev_from_source_mm": 0.0,
                 "shipped_matches_source": True,
                 "identity_proof": "identical welded topology and positions",
                 "tangents_are_baked": True,
                 "validity": validity_record(1000, 2400, radius)},
                {"level": 1, "rung": 2, "role": "generated", "tris": 400, "verts": 1100,
                 "glb": "a/l1.glb", "glb_sha256": "b" * 64, "node": "Probe_LOD2",
                 "e_target_mm": 7.78, "shed_fraction_vs_parent": 0.6, "glb_bytes": 4096,
                 "dev_source_mm": dev1, "dev_source_mm_upper": dev1,
                 "dev_source_bracket_mm": 0.0, "dev_source_to_level_mm": dev1,
                 "dev_level_to_source_mm": dev1 - 0.5,
                 "pairwise_mm": pair1, "pairwise_mm_upper": pair1,
                 "switch_m": round(switch1, 4),
                 "switch_from_source_dev_m": chain.switch_distance_m(dev1, radius, VIEW),
                 "switch_from_pairwise_m": chain.switch_distance_m(pair1, radius, VIEW),
                 "validity": validity_record(400, 1100, radius),
                 "tangents_are_baked": True,
                 "render_gate": gate_record(True, distance_m=round(switch1, 4))},
            ],
        }],
    }


class ProjectionTests(unittest.TestCase):
    def test_the_projection_is_exact_not_small_angle(self):
        """The shortcut the drifted ledger used differs measurably at a wide FOV."""
        dev_mm = 18.641  # the deviation the drifted ledger was quoting
        exact = chain.switch_distance_m(dev_mm, 0.0, VIEW)
        shortcut = (dev_mm / 1000.0) * VIEW["height_px"] / (VIEW["vfov_rad"] * VIEW["budget_px"])
        # 335.1 exact against 335.5 small-angle: the historic "335.5 m derived" was the shortcut,
        # and at the optic the two are close enough that nobody would have noticed.
        self.assertAlmostEqual(exact, 335.135, places=2)
        self.assertAlmostEqual(shortcut, 335.538, places=2)
        self.assertLess(abs(exact - shortcut) / exact, 0.002, "optic hides the error")

        wide = dict(VIEW, vfov_rad=0.785)
        exact_wide = chain.switch_distance_m(dev_mm, 0.0, wide)
        shortcut_wide = (dev_mm / 1000.0) * wide["height_px"] / (wide["vfov_rad"] * 1.0)
        self.assertGreater(
            abs(exact_wide - shortcut_wide) / exact_wide, 0.03,
            "the small-angle shortcut must be visibly wrong at the commander FOV",
        )

    def test_config_and_chain_agree_on_the_projection(self):
        """`config.switch_distance_m` and `chain.switch_distance_m` are one formula, not two."""
        for dev in (0.5, 3.89, 18.641, 56.223):
            self.assertAlmostEqual(
                CONFIG.switch_distance_m(dev, 0.383),
                chain.switch_distance_m(dev, 0.383, VIEW),
                places=9,
            )

    def test_the_octave_grid_doubles_the_switch_distance(self):
        base = chain.switch_distance_m(CONFIG.E1_MM, 0.0, VIEW)
        for rung, target in CONFIG.rungs()[:5]:
            self.assertAlmostEqual(
                chain.switch_distance_m(target, 0.0, VIEW), base * 2 ** (rung - 1), places=6
            )

    def test_the_bounding_radius_slack_is_conservative(self):
        near = chain.switch_distance_m(10.0, 0.0, VIEW)
        with_radius = chain.switch_distance_m(10.0, 0.383, VIEW)
        self.assertAlmostEqual(with_radius - near, 0.383, places=9)


class DerivationTests(unittest.TestCase):
    def test_the_switch_is_the_worse_of_source_and_pairwise(self):
        manifest = synthetic_manifest()
        row = chain.derive(manifest)[0]["levels"][1]
        self.assertGreater(row["pairwise_mm"], row["dev_source_mm"])
        self.assertAlmostEqual(row["switch_m"], row["switch_from_pairwise_m"], places=9)
        self.assertGreater(row["switch_m"], row["switch_from_source_m"])

    def test_a_clean_synthetic_manifest_verifies(self):
        manifest = synthetic_manifest()
        failures = [f for f in chain.verify(manifest, chain.Tree("/nonexistent-root"))[0]
                    if "missing" not in f]
        self.assertEqual(failures, [], failures)

    def test_a_manifest_with_no_render_gate_is_refused(self):
        """`--no-render-gate` is for iterating on the search, never for a committed chain."""
        manifest = synthetic_manifest()
        del manifest["assets"][0]["levels"][1]["render_gate"]
        failures, _ = chain.verify(manifest, chain.Tree("/nonexistent-root"))
        self.assertTrue(any("no render-gate record" in f for f in failures), failures)

    def _failing_gate_manifest(self, blocking):
        """A manifest whose gate FAILED, cut under `blocking`. Patches CONFIG, not the manifest.

        The blocking flag is read from the TREE now, never from the manifest: trusting the recorded
        flag meant ratifying the threshold left every already-recorded failure a warning for ever.
        """
        manifest = synthetic_manifest()
        manifest["gates"]["render_gate_blocking"] = blocking
        level = manifest["assets"][0]["levels"][1]
        level["render_gate"] = gate_record(False, distance_m=level["switch_m"])
        return manifest

    def test_a_failing_render_gate_blocks_once_the_threshold_is_ratified(self):
        original = CONFIG.RENDER_GATE_BLOCKING
        CONFIG.RENDER_GATE_BLOCKING = True
        try:
            failures, warnings = chain.verify(
                self._failing_gate_manifest(True), chain.Tree("/nonexistent-root")
            )
        finally:
            CONFIG.RENDER_GATE_BLOCKING = original
        self.assertTrue(any("render gate recorded a FAIL" in f for f in failures), failures)
        self.assertEqual(warnings, [])

    def test_a_failing_render_gate_only_warns_while_the_gate_is_DISARMED(self):
        """Ratified is not armed: a verdict measured under fallback textures cannot block.

        Recorded and shouted about, never silently dropped — and it arms itself the moment the
        render is honest, with no second decision for anyone to remember.
        """
        manifest = self._failing_gate_manifest(True)
        for level in manifest["assets"][0]["levels"][1:]:
            level["render_gate"]["material_source"] = "FELL BACK to the .blend material 'Mat'"
        failures, warnings = chain.verify(manifest, chain.Tree("/nonexistent-root"))
        self.assertEqual([f for f in failures if "render gate" in f], [])
        self.assertTrue(any("render gate recorded a FAIL" in w for w in warnings), warnings)
        self.assertTrue(any("NOT ARMED" in w for w in warnings), warnings)

    def test_the_gate_arms_itself_once_the_material_is_honest(self):
        """The other half: same failing verdict, shipped material, and it blocks."""
        failures, _ = chain.verify(
            self._failing_gate_manifest(True), chain.Tree("/nonexistent-root")
        )
        self.assertTrue(any("render gate recorded a FAIL" in f for f in failures), failures)

    def test_a_manifest_recording_a_stale_blocking_flag_is_refused(self):
        """Ratifying the threshold must invalidate every manifest cut before the ruling."""
        manifest = synthetic_manifest()
        manifest["gates"]["render_gate_blocking"] = not CONFIG.RENDER_GATE_BLOCKING
        failures, _ = chain.verify(manifest, chain.Tree("/nonexistent-root"))
        self.assertTrue(any("render_gate_blocking" in f for f in failures), failures)

    def test_the_drifted_ledger_is_caught(self):
        """The 223.7-vs-335.5 shape: a recorded distance that no longer re-derives."""
        manifest = synthetic_manifest()
        manifest["assets"][0]["levels"][1]["switch_m"] = 223.7
        failures, _ = chain.verify(manifest, chain.Tree("/nonexistent-root"))
        self.assertTrue(
            any("drifted from the measurement" in f for f in failures),
            f"a stale hand-written switch distance must fail verification; got {failures}",
        )

    def test_a_level_over_its_rung_is_caught(self):
        manifest = synthetic_manifest()
        manifest["assets"][0]["levels"][1]["dev_source_mm_upper"] = 99.0
        failures, _ = chain.verify(manifest, chain.Tree("/nonexistent-root"))
        self.assertTrue(any("exceeds its rung target" in f for f in failures), failures)

    def test_a_moved_right_wall_is_caught(self):
        manifest = synthetic_manifest()
        manifest["ladder"]["right_wall_m"] = 5000.0
        failures, _ = chain.verify(manifest, chain.Tree("/nonexistent-root"))
        self.assertTrue(any("right_wall_m" in f for f in failures), failures)

    def test_a_stale_generator_version_is_caught(self):
        manifest = synthetic_manifest()
        manifest["generator"]["version"] = "0.0.1"
        failures, _ = chain.verify(manifest, chain.Tree("/nonexistent-root"))
        self.assertTrue(any("regenerate" in f for f in failures), failures)

    def test_a_chain_that_grows_triangles_is_caught(self):
        manifest = synthetic_manifest()
        manifest["assets"][0]["levels"][1]["tris"] = 5000
        failures, _ = chain.verify(manifest, chain.Tree("/nonexistent-root"))
        self.assertTrue(any("is not fewer than" in f for f in failures), failures)

    def test_emit_rust_carries_the_derived_distance(self):
        manifest = synthetic_manifest()
        text = chain.emit_rust(chain.derive(manifest), manifest)
        self.assertIn(CONFIG.ASSETS[0]["name"].upper() + "_CHAIN", text)
        self.assertIn("do not hand-edit", text)
        expected = chain.derive(manifest)[0]["levels"][1]["switch_m"]
        self.assertIn(f"{expected:.4f}", text)


class MutantTests(unittest.TestCase):
    """The four mutants an adversarial review drove through the old verifier untouched.

    Every one of them verified CLEAN, because every check was of the form "if the field is present
    and wrong, complain" — so deleting the field, or filling it with NaN (which fails every
    comparison silently), was indistinguishable from being correct. These are the regression: a
    verifier that only inspects what it is handed certifies nothing about what it is not.
    """

    def failures_for(self, mutate):
        manifest = synthetic_manifest()
        mutate(manifest)
        return chain.verify(manifest, chain.Tree("/nonexistent-root"))[0]

    def test_a_manifest_with_no_assets_is_refused(self):
        failures = self.failures_for(lambda m: m.__setitem__("assets", []))
        self.assertTrue(any("NO assets" in f or "config declares" in f for f in failures), failures)

    def test_a_manifest_missing_every_glb_hash_is_refused(self):
        def strip(manifest):
            for level in manifest["assets"][0]["levels"]:
                del level["glb_sha256"]

        failures = self.failures_for(strip)
        self.assertTrue(any("glb_sha256" in f for f in failures), failures)

    def test_a_manifest_missing_every_validity_record_is_refused(self):
        def strip(manifest):
            for level in manifest["assets"][0]["levels"]:
                del level["validity"]

        failures = self.failures_for(strip)
        self.assertTrue(any("validity" in f for f in failures), failures)

    def test_a_manifest_full_of_nans_is_refused(self):
        """NaN fails every `>` and every `!=` quietly, so it used to sail through untouched."""
        def poison(manifest):
            for level in manifest["assets"][0]["levels"][1:]:
                for key in ("dev_source_mm", "dev_source_mm_upper", "pairwise_mm",
                            "pairwise_mm_upper", "switch_m"):
                    level[key] = float("nan")

        failures = self.failures_for(poison)
        self.assertTrue(any("non-finite" in f for f in failures), failures)

    def test_a_manifest_naming_an_unknown_asset_is_refused(self):
        failures = self.failures_for(
            lambda m: m["assets"][0].__setitem__("name", "some_other_asset")
        )
        self.assertTrue(any("config declares" in f for f in failures), failures)

    def test_a_chain_with_only_a_source_level_is_refused(self):
        def truncate(manifest):
            manifest["assets"][0]["levels"] = manifest["assets"][0]["levels"][:1]

        failures = self.failures_for(truncate)
        self.assertTrue(any("at least one generated level" in f for f in failures), failures)

    def test_a_manifest_with_a_loosened_gate_threshold_is_refused(self):
        """The manifest must record the configuration THIS tree holds — all of it, not a sample."""
        def loosen(manifest):
            manifest["gates"]["numeric"]["max_duplicate_faces"] = 99

        failures = self.failures_for(loosen)
        self.assertTrue(any("max_duplicate_faces" in f for f in failures), failures)

    def test_a_manifest_whose_bytes_are_absent_cannot_pass(self):
        """Verification without the bytes is not verification, and does not quietly succeed.

        This used to assert that a RECORDED defect count was refused, which the verifier caught with
        a second hand-maintained loop over the recorded numbers. That loop is gone: defect counters
        are now re-derived from the decoded bytes, so a recorded lie is caught by disagreeing with
        the file (`RederivationSweepTests.test_every_recorded_defect_counter_is_compared_against_
        its_limit`, against the real corpus). What is left to assert here — and it matters — is that
        a manifest whose assets are not present cannot pass by default.
        """
        def defect(manifest):
            manifest["assets"][0]["levels"][1]["validity"]["nonmanifold_edges"] = 3

        failures = self.failures_for(defect)
        self.assertTrue(failures, "a manifest with no readable assets must never verify clean")
        self.assertTrue(any("missing" in f for f in failures), failures)

    def test_a_manifest_cut_by_different_generator_sources_is_refused(self):
        failures = self.failures_for(
            lambda m: m["generator"].__setitem__("sources_sha256", "0" * 64)
        )
        self.assertTrue(any("sources have changed" in f for f in failures), failures)

    def test_a_manifest_with_no_generator_source_digest_is_refused(self):
        failures = self.failures_for(lambda m: m["generator"].pop("sources_sha256"))
        self.assertTrue(any("no generator source digest" in f for f in failures), failures)

    def test_a_manifest_with_a_gutted_render_gate_record_is_refused(self):
        def gut(manifest):
            manifest["assets"][0]["levels"][1]["render_gate"] = {"pass": True}

        failures = self.failures_for(gut)
        self.assertTrue(any("render-gate record has no" in f for f in failures), failures)

    def test_a_manifest_with_reordered_level_indices_is_refused(self):
        def scramble(manifest):
            manifest["assets"][0]["levels"][1]["level"] = 7

        failures = self.failures_for(scramble)
        self.assertTrue(any("level indices" in f for f in failures), failures)


class SecondRoundMutantTests(unittest.TestCase):
    """The mutants that survived the FIRST attempt at strict schema validation.

    Round one deleted assets, hashes and validity records and poisoned deviations with NaN. Round
    two went after everything the new code required-but-never-compared: a schema version it checked
    for presence only, the source level's own numerics (omitted from the field list entirely), a
    render record whose metrics were NaN under `pass: true`, a per-level threshold rewritten to 999,
    and the toolchain provenance deleted wholesale. Every one verified clean.

    The rule that came out of it, and that `test_no_recorded_field_is_unclassified` enforces: every
    field the manifest records is either CHECKED or named informational. There is no third state,
    because the third state is a field that reads as evidence and is not.
    """

    def failures_for(self, mutate):
        manifest = synthetic_manifest()
        mutate(manifest)
        return chain.verify(manifest, chain.Tree("/nonexistent-root"))[0]

    def test_an_unreadable_schema_version_is_refused(self):
        failures = self.failures_for(lambda m: m.__setitem__("schema_version", 999))
        self.assertTrue(any("schema_version" in f for f in failures), failures)

    def test_a_nan_triangle_count_on_the_source_level_is_refused(self):
        """L0 is what every deviation in the chain is measured against."""
        def poison(manifest):
            manifest["assets"][0]["levels"][0]["tris"] = float("nan")

        failures = self.failures_for(poison)
        self.assertTrue(any("non-finite" in f for f in failures), failures)

    def test_nan_render_metrics_under_a_recorded_pass_are_refused(self):
        def poison(manifest):
            gate = manifest["assets"][0]["levels"][1]["render_gate"]
            gate["worst_defect_score"] = float("nan")
            for view in gate["views"].values():
                view["signal"]["mean_abs_diff"] = float("nan")

        failures = self.failures_for(poison)
        self.assertTrue(any("non-finite" in f for f in failures), failures)

    def test_a_rewritten_per_level_threshold_is_refused(self):
        """A gate judged against a threshold the tree does not declare judged nothing."""
        def loosen(manifest):
            manifest["assets"][0]["levels"][1]["render_gate"]["thresholds"]["defect_fraction"] = 999

        failures = self.failures_for(loosen)
        self.assertTrue(any("defect_fraction" in f for f in failures), failures)

    def test_a_verdict_that_does_not_follow_from_its_numbers_is_refused(self):
        """`pass: true` beside metrics that say otherwise is a contradiction, not a pass."""
        def contradict(manifest):
            level = manifest["assets"][0]["levels"][1]
            level["render_gate"] = gate_record(          # every number says FAIL, consistently
                False, distance_m=level["switch_m"]
            )
            level["render_gate"]["pass"] = True            # ...and the verdict says otherwise

        failures = self.failures_for(contradict)
        self.assertTrue(any("does not follow from the evidence" in f for f in failures), failures)

    def test_removed_toolchain_provenance_is_refused(self):
        for field in ("blender", "blender_build", "gltf_exporter"):
            with self.subTest(field=field):
                failures = self.failures_for(lambda m, f=field: m["generator"].pop(f))
                self.assertTrue(any(f"no {field!r}" in x for x in failures), failures)

    def test_a_different_blender_build_is_refused(self):
        failures = self.failures_for(
            lambda m: m["generator"].__setitem__("blender_build", "deadbeef1234")
        )
        self.assertTrue(any("blender_build" in f for f in failures), failures)

    def test_a_different_gltf_exporter_is_refused(self):
        failures = self.failures_for(
            lambda m: m["generator"].__setitem__("gltf_exporter", "4.0.1")
        )
        self.assertTrue(any("gltf_exporter" in f for f in failures), failures)

    def test_removed_material_provenance_is_refused(self):
        failures = self.failures_for(
            lambda m: m["assets"][0]["levels"][1]["render_gate"].pop("material_source")
        )
        self.assertTrue(any("material_source" in f for f in failures), failures)

    def test_a_gate_with_missing_views_is_refused(self):
        def drop(manifest):
            gate = manifest["assets"][0]["levels"][1]["render_gate"]
            gate["views"] = {next(iter(gate["views"])): next(iter(gate["views"].values()))}

        failures = self.failures_for(drop)
        self.assertTrue(any("config declares" in f for f in failures), failures)

    def test_an_empty_footprint_is_refused(self):
        """Two frames that never saw the asset differ by nothing, which is not a pass."""
        def blank(manifest):
            for view in manifest["assets"][0]["levels"][1]["render_gate"]["views"].values():
                view["signal"]["footprint_px"] = 0

        failures = self.failures_for(blank)
        self.assertTrue(any("empty footprint" in f for f in failures), failures)

    def test_removed_search_limits_are_refused(self):
        failures = self.failures_for(lambda m: m["gates"].pop("search_limits"))
        self.assertTrue(any("search limit" in f for f in failures), failures)

    def test_a_missing_l0_identity_proof_is_refused(self):
        failures = self.failures_for(
            lambda m: m["assets"][0]["levels"][0].pop("identity_proof")
        )
        self.assertTrue(any("identity proof" in f for f in failures), failures)

    def test_a_nonfinite_source_record_is_refused(self):
        failures = self.failures_for(
            lambda m: m["assets"][0]["source"].__setitem__("tris", float("nan"))
        )
        self.assertTrue(any("non-finite" in f for f in failures), failures)

    def test_a_fallback_material_disarms_the_gate_rather_than_condemning_the_manifest(self):
        """The precondition beside RENDER_GATE_BLOCKING, enforced rather than written down.

        A verdict measured with the wrong textures must not block — but the manifest is not thereby
        invalid, it is unenforced, and the difference is the whole of Yan's ruling: the constant
        flips today and enforcement arms itself when the render is honest.
        """
        manifest = synthetic_manifest()
        manifest["assets"][0]["levels"][1]["render_gate"]["material_source"] = (
            "FELL BACK to the .blend material 'Mat' — importer cannot read KTX2"
        )
        armed, reason = chain.effective_render_blocking(manifest)
        self.assertFalse(armed)
        self.assertIn("fallback material", reason)
        failures, warnings = chain.verify(manifest, chain.Tree("/nonexistent-root"))
        self.assertEqual([f for f in failures if "render gate" in f], [])
        self.assertTrue(any("NOT ARMED" in w for w in warnings), warnings)


class TargetedRegenerationTests(unittest.TestCase):
    """`--asset` must not replace a full manifest with a subset, nor re-attest what it carried."""

    PROVENANCE = {
        "version": "2.3.0", "sources_sha256": "a" * 64, "blender": "5.1.2",
        "blender_build": "ec6e62d40fa9", "gltf_exporter": "5.1.20",
    }

    def generator(self, **overrides):
        return dict(self.PROVENANCE, **overrides)

    def test_a_regenerated_asset_replaces_only_itself(self):
        existing = [{"name": "alpha", "levels": ["old"]}, {"name": "beta", "levels": ["old"]}]
        merged = chain.merge_asset_entries(
            [{"name": "beta", "levels": ["new"]}], existing, ["alpha", "beta"],
            self.PROVENANCE, self.generator(),
        )
        self.assertEqual([e["name"] for e in merged], ["alpha", "beta"])
        self.assertEqual(merged[0]["levels"], ["old"])
        self.assertEqual(merged[1]["levels"], ["new"])

    def test_the_configured_order_is_restored(self):
        existing = [{"name": "beta"}, {"name": "alpha"}]
        merged = chain.merge_asset_entries(
            [], existing, ["alpha", "beta"], self.PROVENANCE, self.generator()
        )
        self.assertEqual([e["name"] for e in merged], ["alpha", "beta"])

    def test_an_asset_with_nothing_to_carry_over_is_refused(self):
        with self.assertRaises(ValueError) as caught:
            chain.merge_asset_entries(
                [{"name": "beta"}], [], ["alpha", "beta"], self.PROVENANCE, self.generator()
            )
        self.assertIn("run a full generation", str(caught.exception).lower())

    def test_carrying_an_entry_under_different_provenance_is_refused(self):
        """The forged certificate: an old chain re-attested to today's toolchain by accident.

        The manifest has ONE generator block, so anything carried into a manifest written today
        wears today's version, source digest, Blender build and exporter — whatever it was actually
        cut with. A carry-over is only honest when those already agree.
        """
        for field, value in (
            ("version", "9.9.9"), ("sources_sha256", "b" * 64),
            ("blender", "5.2.0"), ("blender_build", "cafebabe"), ("gltf_exporter", "6.0.0"),
        ):
            with self.subTest(field=field):
                with self.assertRaises(ValueError) as caught:
                    chain.merge_asset_entries(
                        [{"name": "beta"}], [{"name": "alpha"}], ["alpha", "beta"],
                        self.PROVENANCE, self.generator(**{field: value}),
                    )
                message = str(caught.exception)
                self.assertIn("different toolchain", message)
                self.assertIn(field, message)

    def test_a_matching_provenance_carries_cleanly(self):
        merged = chain.merge_asset_entries(
            [{"name": "beta"}], [{"name": "alpha", "levels": ["old"]}], ["alpha", "beta"],
            self.PROVENANCE, self.generator(),
        )
        self.assertEqual(merged[0]["levels"], ["old"])


class RederivationSweepTests(unittest.TestCase):
    """Every field this pipeline calls "checked" must be RE-DERIVED, and here is the proof.

    The meta-test's earlier definition of "checked" was "named in a field list", which turned out to
    mean "present and finite" — an adversarial review set all three `worst_*` summaries to -999 and
    inverted a per-view verdict, and verification reported nothing. Presence is not checking.

    So this sweep perturbs each re-derived field IN THE SHIPPED MANIFEST, one at a time, and demands
    that verification notices. A field that survives its own mutation is not being checked, whatever
    a list says about it.
    """

    def setUp(self):
        self.root = CONFIG.repo_root()
        path = os.path.join(self.root, CONFIG.MANIFEST_RELPATH)
        if not os.path.isfile(path):
            self.skipTest(f"{CONFIG.MANIFEST_RELPATH} has not been generated yet")
        with open(path, encoding="utf-8") as handle:
            self.text = handle.read()
        # The ladder's LENGTH is an OUTPUT of generation, never a constant: `skip_fraction` and the
        # topology floor decide it, and this corpus has already gone from five levels to four when
        # the shoe was re-cut from a welded 764-triangle source. A hardcoded `range(1, 5)` here
        # turned every level the manifest no longer has into an IndexError — a sweep that errors is
        # a sweep that proves nothing, which is exactly the failure mode this class exists to catch
        # one layer down. So the count comes from the shipped manifest.
        levels = json.loads(self.text)["assets"][0]["levels"]
        self.level_count = len(levels)
        # WHICH level abstains is a property of the corpus, not an index to remember: the gate
        # abstains below `min_footprint_px`, so it is whichever level got small enough far enough
        # away. Naming `levels[4]` hardcoded both the ladder's length AND which rung crossed that
        # line; the re-cut moved it to L3 and three mutation tests turned into IndexErrors, which
        # assert nothing at all.
        abstaining = [
            level["level"] for level in levels
            if level.get("render_gate", {}).get("abstained")
        ]
        self.abstaining = abstaining[0] if abstaining else None

    def mutated(self, mutate):
        manifest = json.loads(self.text)
        mutate(manifest)
        return chain.verify(manifest, chain.Tree(self.root))[0]

    def assert_caught(self, description, mutate):
        failures = self.mutated(mutate)
        self.assertTrue(failures, f"{description} was not caught by verification")

    def first_gate(self, manifest):
        return manifest["assets"][0]["levels"][1]["render_gate"]

    def test_each_worst_summary_is_rederived(self):
        for key in ("worst_mean_abs_diff", "worst_frac_over", "worst_defect_score"):
            with self.subTest(key=key):
                self.assert_caught(
                    f"{key} = -999",
                    lambda m, k=key: self.first_gate(m).__setitem__(k, -999),
                )

    def test_an_inverted_per_view_verdict_is_rederived(self):
        for name, _e, _a in CONFIG.RENDER_GATE["views"]:
            with self.subTest(view=name):
                def invert(manifest, view_name=name):
                    view = self.first_gate(manifest)["views"][view_name]
                    view["pass"] = not view["pass"]

                self.assert_caught(f"inverted pass on {name}", invert)

    def test_a_tampered_defect_score_is_rederived_from_its_own_means(self):
        def tamper(manifest):
            self.first_gate(manifest)["views"]["three_quarter"]["defect_score"] = 0.0

        self.assert_caught("defect_score forced to 0", tamper)

    def test_a_tampered_signal_mean_breaks_its_own_score(self):
        def tamper(manifest):
            view = self.first_gate(manifest)["views"]["three_quarter"]
            view["signal"]["mean_abs_diff"] = view["signal"]["mean_abs_diff"] * 3.0 + 0.01

        self.assert_caught("signal mean moved without its score", tamper)

    def test_a_tampered_under_absolute_floor_is_rederived(self):
        def tamper(manifest):
            view = self.first_gate(manifest)["views"]["three_quarter"]
            view["under_absolute_floor"] = not view["under_absolute_floor"]

        self.assert_caught("inverted under_absolute_floor", tamper)

    def test_every_recorded_switch_distance_is_rederived(self):
        for index in range(1, self.level_count):
            with self.subTest(level=index):
                self.assert_caught(
                    f"L{index} switch_m + 10",
                    lambda m, i=index: m["assets"][0]["levels"][i].__setitem__(
                        "switch_m", m["assets"][0]["levels"][i]["switch_m"] + 10.0
                    ),
                )

    def test_every_origin_radius_feeds_a_rederived_switch(self):
        """The slack is an input to the derivation, so moving it must break the recorded distance."""
        self.assert_caught(
            "L1 origin radius + 1 m",
            lambda m: m["assets"][0]["levels"][1]["validity"].__setitem__("origin_radius_m", 1.4),
        )

    def test_inflating_any_deviation_number_is_caught(self):
        """Every recorded deviation is pinned AGAINST INFLATION by the others.

        Raise any one of them and something has to give: the two directions stop summing to their
        maximum, the bracket stops being upper-minus-lower, the rung target is exceeded, or the
        derived switch distance moves. This is the direction that matters for a certificate — a
        level claiming a bigger lie than it tells is the shape of a forged bound.
        """
        for index in range(1, self.level_count):
            for key in ("dev_source_mm", "dev_source_mm_upper", "dev_source_bracket_mm",
                        "dev_source_to_level_mm", "dev_level_to_source_mm", "pairwise_mm_upper"):
                with self.subTest(level=index, key=key):
                    self.assert_caught(
                        f"L{index} {key} doubled",
                        lambda m, i=index, k=key: m["assets"][0]["levels"][i].__setitem__(
                            k, m["assets"][0]["levels"][i][k] * 2.0 + 1.0
                        ),
                    )

    def test_lowering_the_number_that_drives_each_switch_is_caught(self):
        """The switch is `max(source-relative, pairwise)`; whichever WINS is pinned downward too.

        THE HONEST LIMIT, stated rather than glossed: the LOSING one is not. Halving a deviation
        that is not the maximum changes no derivation, because nothing downstream reads it — it is a
        recorded MEASUREMENT, and a manifest cannot re-derive a measurement from itself. What pins
        those is the level's glb hash plus regeneration: the bytes are fixed, and re-running the
        pipeline re-measures them. `test_inflating_any_deviation_number_is_caught` covers the
        direction a forged certificate would actually move in.
        """
        for index in range(1, self.level_count):
            level = json.loads(self.text)["assets"][0]["levels"][index]
            driver = (
                "dev_source_mm_upper"
                if level["dev_source_mm_upper"] >= level["pairwise_mm_upper"]
                else "pairwise_mm_upper"
            )
            with self.subTest(level=index, driver=driver):
                self.assert_caught(
                    f"L{index} {driver} halved",
                    lambda m, i=index, k=driver: m["assets"][0]["levels"][i].__setitem__(
                        k, m["assets"][0]["levels"][i][k] * 0.5
                    ),
                )

    def test_every_level_hash_is_compared_against_the_bytes(self):
        for index in range(0, self.level_count):
            with self.subTest(level=index):
                self.assert_caught(
                    f"L{index} glb_sha256 rewritten",
                    lambda m, i=index: m["assets"][0]["levels"][i].__setitem__(
                        "glb_sha256", "f" * 64
                    ),
                )

    def test_every_recorded_defect_counter_is_compared_against_its_limit(self):
        for key in ("duplicate_faces", "nonfinite_attrs", "orientation_flips",
                    "nonmanifold_edges", "tangent_default_faces", "tangent_default_verts",
                    "slivers_below_floor"):
            with self.subTest(key=key):
                self.assert_caught(
                    f"{key} = 3 on L1",
                    lambda m, k=key: m["assets"][0]["levels"][1]["validity"].__setitem__(k, 3),
                )

    def test_the_gate_distance_must_be_the_switch_distance(self):
        """A gate run at the wrong distance measured the wrong pop."""
        self.assert_caught(
            "distance_m = -999",
            lambda m: self.first_gate(m).__setitem__("distance_m", -999),
        )

    def test_the_render_parameters_must_match_config(self):
        for key in ("tile_px", "supersample", "samples"):
            with self.subTest(key=key):
                self.assert_caught(
                    f"{key} = 999",
                    lambda m, k=key: self.first_gate(m).__setitem__(k, 999),
                )

    def test_the_tile_fov_must_preserve_the_reference_resolution(self):
        self.assert_caught(
            "tile_vfov_rad doubled",
            lambda m: self.first_gate(m).__setitem__(
                "tile_vfov_rad", self.first_gate(m)["tile_vfov_rad"] * 2
            ),
        )

    def test_every_recorded_threshold_must_match_config(self):
        """Not a chosen four — `defect_normal_deg = 999` described a defect nobody declared."""
        manifest = json.loads(self.text)
        for key in self.first_gate(manifest)["thresholds"]:
            with self.subTest(key=key):
                self.assert_caught(
                    f"thresholds.{key} = 999",
                    lambda m, k=key: self.first_gate(m)["thresholds"].__setitem__(k, 999),
                )

    def test_an_unknown_recorded_threshold_is_refused(self):
        self.assert_caught(
            "an invented threshold",
            lambda m: self.first_gate(m)["thresholds"].__setitem__("invented_limit", 1),
        )

    def test_the_pairwise_lower_bound_cannot_exceed_its_upper(self):
        for index in range(1, self.level_count):
            with self.subTest(level=index):
                self.assert_caught(
                    f"L{index} pairwise_mm = 999",
                    lambda m, i=index: m["assets"][0]["levels"][i].__setitem__(
                        "pairwise_mm", 999
                    ),
                )

    def test_both_component_switch_distances_are_rederived(self):
        """Checking only their maximum let BOTH be set to -999 without a word."""
        for index in range(1, self.level_count):
            for key in ("switch_from_source_dev_m", "switch_from_pairwise_m"):
                with self.subTest(level=index, key=key):
                    self.assert_caught(
                        f"L{index} {key} = -999",
                        lambda m, i=index, k=key: m["assets"][0]["levels"][i].__setitem__(k, -999),
                    )

    def test_both_components_set_together_is_still_caught(self):
        def wreck(manifest):
            level = manifest["assets"][0]["levels"][1]
            level["switch_from_source_dev_m"] = -999
            level["switch_from_pairwise_m"] = -999

        self.assert_caught("both components = -999", wreck)

    def test_the_shed_fraction_is_rederived_from_the_triangle_counts(self):
        for index in range(1, self.level_count):
            with self.subTest(level=index):
                self.assert_caught(
                    f"L{index} shed_fraction_vs_parent = -999",
                    lambda m, i=index: m["assets"][0]["levels"][i].__setitem__(
                        "shed_fraction_vs_parent", -999
                    ),
                )

    def test_a_validity_record_that_describes_another_mesh_is_caught(self):
        for index in range(0, self.level_count):
            for key in ("tris", "verts"):
                with self.subTest(level=index, key=key):
                    self.assert_caught(
                        f"L{index} validity.{key} = 999",
                        lambda m, i=index, k=key: m["assets"][0]["levels"][i][
                            "validity"
                        ].__setitem__(k, 999),
                    )

    def test_the_rung_target_is_rederived_from_the_global_grid(self):
        for index in range(1, self.level_count):
            with self.subTest(level=index):
                self.assert_caught(
                    f"L{index} e_target_mm halved",
                    lambda m, i=index: m["assets"][0]["levels"][i].__setitem__(
                        "e_target_mm", m["assets"][0]["levels"][i]["e_target_mm"] / 2
                    ),
                )

    def test_abstention_is_rederived_from_the_geometry(self):
        """The one verdict claimable without rendering anything, so the one most worth recomputing."""
        if self.abstaining is None:
            self.skipTest("this corpus has no abstaining level")
        self.assert_caught(
            f"L{self.abstaining} claims it was scored",
            lambda m: m["assets"][0]["levels"][self.abstaining]["render_gate"].__setitem__(
                "abstained", False
            ),
        )
        self.assert_caught(
            "L1 claims it abstained",
            lambda m: m["assets"][0]["levels"][1]["render_gate"].__setitem__("abstained", True),
        )

    def test_an_abstention_may_not_carry_a_verdict(self):
        if self.abstaining is None:
            self.skipTest("this corpus has no abstaining level")
        self.assert_caught(
            f"L{self.abstaining} abstained but records a pass",
            lambda m: m["assets"][0]["levels"][self.abstaining]["render_gate"].__setitem__(
                "pass", True
            ),
        )

    def test_the_recorded_footprint_is_rederived_from_the_bounding_box(self):
        for index in range(1, self.level_count):
            with self.subTest(level=index):
                self.assert_caught(
                    f"L{index} screen_footprint_px doubled",
                    lambda m, i=index: m["assets"][0]["levels"][i]["render_gate"].__setitem__(
                        "screen_footprint_px",
                        m["assets"][0]["levels"][i]["render_gate"]["screen_footprint_px"] * 2,
                    ),
                )

    def test_a_rewritten_abstention_threshold_is_refused(self):
        if self.abstaining is None:
            self.skipTest("this corpus has no abstaining level")
        self.assert_caught(
            "min_footprint_px = 1",
            lambda m: m["assets"][0]["levels"][self.abstaining]["render_gate"].__setitem__(
                "min_footprint_px", 1.0
            ),
        )

    def test_every_baked_tangent_count_is_gated(self):
        """The tangents that SHIP — the gate the UV proxy could not stand in for."""
        for index in range(1, self.level_count):
            with self.subTest(level=index):
                self.assert_caught(
                    f"L{index} degenerate_tangents = 1",
                    lambda m, i=index: m["assets"][0]["levels"][i]["validity"].__setitem__(
                        "degenerate_tangents", 1
                    ),
                )

    def test_the_ratification_provenance_must_match_the_tree(self):
        self.assert_caught(
            "a different ruler",
            lambda m: m["ladder"]["ratification"].__setitem__("by", "somebody else"),
        )

    def test_a_level_that_baked_no_tangents_is_refused(self):
        """Zero baked tangents scored a clean zero on the DEGENERATE counter and passed."""
        for index in range(1, self.level_count):
            with self.subTest(level=index):
                self.assert_caught(
                    f"L{index} baked_tangents = 0",
                    lambda m, i=index: m["assets"][0]["levels"][i]["validity"].__setitem__(
                        "baked_tangents", 0
                    ),
                )

    def test_a_level_that_stops_claiming_baked_tangents_is_refused(self):
        self.assert_caught(
            "L1 tangents_are_baked = False",
            lambda m: m["assets"][0]["levels"][1].__setitem__("tangents_are_baked", False),
        )

    def test_a_partial_tangent_buffer_is_refused(self):
        self.assert_caught(
            "L1 baked_tangents one short",
            lambda m: m["assets"][0]["levels"][1]["validity"].__setitem__(
                "baked_tangents", m["assets"][0]["levels"][1]["validity"]["verts"] - 1
            ),
        )

    def test_a_shrunken_gate_bbox_cannot_buy_an_abstention(self):
        """The bypass: shrink one level's gate bbox tenfold, declare it too small to judge.

        Re-deriving the footprint from the gate record's OWN box only proves the record agrees with
        itself. Binding it to the level's decoded bytes is what makes the re-derivation mean
        anything.
        """
        def shrink(manifest):
            gate = manifest["assets"][0]["levels"][3]["render_gate"]
            gate["bbox_mm"] = [value / 10.0 for value in gate["bbox_mm"]]
            gate["screen_footprint_px"] = round(
                chain.screen_footprint_px(gate["bbox_mm"], gate["distance_m"], VIEW), 4
            )
            gate["abstained"] = True
            gate["pass"] = None
            gate["reason"] = "too small to judge"

        self.assert_caught("L3 gate bbox shrunk tenfold", shrink)

    def test_a_gate_bbox_that_is_not_the_levels_bbox_is_refused(self):
        self.assert_caught(
            "L1 gate bbox inflated",
            lambda m: m["assets"][0]["levels"][1]["render_gate"].__setitem__(
                "bbox_mm", [v * 2 for v in m["assets"][0]["levels"][1]["render_gate"]["bbox_mm"]]
            ),
        )

    def test_a_stripped_tangent_attribute_is_refused_even_with_a_matching_hash(self):
        """The bypass that record-vs-record checking could never catch.

        Strip TANGENT from a level's bytes, update the manifest to the new (correct) hash, and leave
        the validity record describing tangents that are no longer there: every recorded number
        agreed with every other recorded number, and verification passed. Only decoding the bytes
        finds it.
        """
        import measure as measure_module

        level = json.loads(self.text)["assets"][0]["levels"][1]
        original = open(os.path.join(self.root, level["glb"]), "rb").read()
        gltf, binary = measure_module.glb_chunks_from_bytes(original, level["glb"])
        for mesh in gltf["meshes"]:
            for primitive in mesh["primitives"]:
                primitive["attributes"].pop("TANGENT", None)
        stripped = _rebuild_glb(gltf, binary)

        directory = tempfile.mkdtemp(prefix="lod-stripped-")
        os.makedirs(os.path.join(directory, os.path.dirname(level["glb"])), exist_ok=True)
        for other in json.loads(self.text)["assets"][0]["levels"]:
            source = os.path.join(self.root, other["glb"])
            target = os.path.join(directory, other["glb"])
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(source, "rb") as handle:
                data = handle.read()
            if other["glb"] == level["glb"]:
                data = stripped
            with open(target, "wb") as handle:
                handle.write(data)

        manifest = json.loads(self.text)
        # the honest new hash, and the STALE validity record left in place
        manifest["assets"][0]["levels"][1]["glb_sha256"] = hashlib.sha256(stripped).hexdigest()
        failures, _ = chain.verify(manifest, chain.Tree(directory))
        self.assertTrue(
            any("NO TANGENT" in f or "baked_tangents" in f for f in failures),
            f"a stripped attribute with a correct hash must be caught by decoding: {failures}",
        )

    def test_shrinking_both_bboxes_cannot_buy_an_abstention(self):
        """The gate bbox AND the validity record's copy, moved together — still refused."""
        def shrink(manifest):
            level = manifest["assets"][0]["levels"][3]
            gate = level["render_gate"]
            gate["bbox_mm"] = [value / 10.0 for value in gate["bbox_mm"]]
            level["validity"]["bbox_mm"] = list(gate["bbox_mm"])
            gate["screen_footprint_px"] = round(
                chain.screen_footprint_px(gate["bbox_mm"], gate["distance_m"], VIEW), 4
            )
            gate["abstained"] = True
            gate["pass"] = None
            gate["reason"] = "too small to judge"

        self.assert_caught("both bboxes shrunk together", shrink)

    def test_removing_the_validity_bbox_cannot_buy_an_abstention(self):
        """`bbox_mm` was optional, so deleting it deleted the comparison that used it."""
        def remove(manifest):
            level = manifest["assets"][0]["levels"][3]
            gate = level["render_gate"]
            level["validity"].pop("bbox_mm")
            gate["bbox_mm"] = [value / 10.0 for value in gate["bbox_mm"]]
            gate["screen_footprint_px"] = round(
                chain.screen_footprint_px(gate["bbox_mm"], gate["distance_m"], VIEW), 4
            )
            gate["abstained"] = True
            gate["pass"] = None
            gate["reason"] = "too small to judge"

        self.assert_caught("validity bbox removed", remove)

    def test_every_byte_derived_counter_is_bound_to_the_bytes(self):
        """A sweep: each recorded validity counter must lose to what the file actually contains."""
        for key in ("tris", "verts", "components", "duplicate_faces", "orientation_flips",
                    "nonmanifold_edges", "boundary_edges", "baked_tangents"):
            with self.subTest(key=key):
                self.assert_caught(
                    f"L1 validity.{key} + 7",
                    lambda m, k=key: m["assets"][0]["levels"][1]["validity"].__setitem__(
                        k, m["assets"][0]["levels"][1]["validity"][k] + 7
                    ),
                )

    def test_a_forged_sliver_floor_is_refused(self):
        """The floor was read from the record and used as its own threshold.

        Lowering the recorded number lowered the bar it was checked against, so a level could
        legalise a sliver by editing the manifest. The verifier now DERIVES the corpus floor from
        decoded L0 and this tree's config, and the recorded copy has to match it.
        """
        for index in range(0, self.level_count):
            with self.subTest(level=index):
                self.assert_caught(
                    f"L{index} sliver floor lowered 10x",
                    lambda m, i=index: m["assets"][0]["levels"][i]["validity"].__setitem__(
                        "min_altitude_floor_m",
                        m["assets"][0]["levels"][i]["validity"]["min_altitude_floor_m"] / 10.0,
                    ),
                )

    def test_the_derived_floor_matches_what_generation_recorded(self):
        """The derivation and the corpus agree — otherwise the check above is vacuous."""
        import measure as measure_module

        l0 = json.loads(self.text)["assets"][0]["levels"][0]
        surface = measure_module.surface_from_bytes(
            open(os.path.join(self.root, l0["glb"]), "rb").read(), l0["node"], "L0"
        )
        plain = surface.validity(CONFIG.GATES)
        derived = chain._derived_corpus_floor(plain, surface.diagonal, CONFIG.GATES)
        for level in json.loads(self.text)["assets"][0]["levels"]:
            self.assertAlmostEqual(
                level["validity"]["min_altitude_floor_m"], derived, places=12,
                msg=f"L{level['level']}",
            )

    def test_a_level_that_split_into_two_pieces_is_refused(self):
        """`components_must_match` was compared at generation and never again.

        A detached triangle bolted onto L1 — with every byte-derived field and the hash honestly
        updated to describe the broken bytes — verified clean, because the verifier's gate list
        simply did not contain the check. Now both sides run the same list.
        """
        import measure as measure_module
        from test_refusals import build_glb

        level = json.loads(self.text)["assets"][0]["levels"][1]
        surface = measure_module.surface_from_bytes(
            open(os.path.join(self.root, level["glb"]), "rb").read(), None, "L1"
        )
        # glTF space is Y-up; the decode flipped it, so flip back on the way out.
        def to_gltf(points):
            out = np.empty_like(points)
            out[:, 0], out[:, 1], out[:, 2] = points[:, 0], points[:, 2], -points[:, 1]
            return out

        verts = to_gltf(surface.verts)
        corner_uv = surface.corner_uv.reshape(-1, 2)
        corner_n = to_gltf(surface.corner_n.reshape(-1, 3))
        # rebuild as one vertex per corner, then append a detached triangle far away
        # DETACH AN EXISTING TRIANGLE rather than adding one: the triangle count, the vertex
        # count, the shed fraction and the bounding box all stay exactly as recorded, so the ONLY
        # thing that differs is that the level is now in two pieces. A mutant that also moves those
        # other numbers trips an earlier check and never reaches the gate this test is about.
        positions = verts[surface.tri_v.reshape(-1)]
        centre = (positions.min(axis=0) + positions.max(axis=0)) / 2.0
        positions[-3:] = [centre, centre + [0.002, 0, 0], centre + [0, 0.002, 0]]
        normals = corner_n
        uvs = corner_uv
        tangents = np.tile([1.0, 0.0, 0.0, 1.0], (len(positions), 1))
        indices = list(range(len(positions)))

        directory = tempfile.mkdtemp(prefix="lod-split-")
        for other in json.loads(self.text)["assets"][0]["levels"]:
            target = os.path.join(directory, other["glb"])
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with open(os.path.join(self.root, other["glb"]), "rb") as handle:
                data = handle.read()
            with open(target, "wb") as handle:
                handle.write(data)
        broken = os.path.join(directory, level["glb"])
        build_glb(broken, positions, indices, normals=normals, uvs=uvs, tangents=tangents,
                  node_name=level["node"])
        blob = open(broken, "rb").read()

        # An HONEST manifest for the broken bytes: real hash, real recomputed validity.
        manifest = json.loads(self.text)
        entry = manifest["assets"][0]["levels"][1]
        entry["glb_sha256"] = hashlib.sha256(blob).hexdigest()
        rebuilt = measure_module.surface_from_bytes(blob, level["node"], "L1")
        floor = entry["validity"]["min_altitude_floor_m"]
        entry["validity"] = rebuilt.validity(CONFIG.GATES, floor)
        entry["tris"], entry["verts"] = rebuilt.tri_count, rebuilt.vert_count

        failures, _ = chain.verify(manifest, chain.Tree(directory))
        self.assertEqual(rebuilt.components(), 2, "the mutant must really have two pieces")
        self.assertTrue(
            any("component count" in f for f in failures),
            f"a level that split in two must be refused at verification: {failures}",
        )

    @staticmethod
    def _thinnest_triangle_apex(surface):
        """(vertex index, unit direction toward the opposite edge) for the thinnest triangle.

        THE VERTEX THAT SETS THE CORPUS BAR. The sliver floor is `min_altitude / margin`, and a
        triangle's minimum altitude is the distance from the vertex opposite its longest edge to
        that edge — so pushing THIS vertex toward THAT edge lowers the minimum altitude by (almost
        exactly) the distance moved, and lowers the floor every level is judged against with it.
        Any other vertex would move the geometry fingerprint without moving the bar, which is the
        half of the story a fingerprint-only mutation cannot tell.
        """
        altitudes = surface.altitudes()
        triangle = surface.tri_v[int(np.argmin(altitudes))]
        points = surface.verts[triangle]
        edges = [float(np.linalg.norm(points[(k + 1) % 3] - points[k])) for k in range(3)]
        apex = (int(np.argmax(edges)) + 2) % 3          # the corner the longest edge does not touch
        stand_off = points[apex]
        base, other = [points[k] for k in range(3) if k != apex]
        along = (other - base) / np.linalg.norm(other - base)
        foot = base + along * float(np.dot(stand_off - base, along))
        return int(triangle[apex]), (foot - stand_off) / np.linalg.norm(foot - stand_off)

    def test_a_poisoned_l0_cannot_lower_the_corpus_floor(self):
        """L0 is the BASELINE, so choosing it freely re-judges every level. BYTES, not a digest.

        The probe that found this moved one interior vertex of L0 by 0.9 um, updated the hash and
        recomputed every level's validity against the new bar — and the whole manifest verified
        clean, because the corpus sliver floor is DERIVED FROM L0: thin L0's thinnest triangle and
        every other level is judged against a lower bar for free.

        SO THIS TEST PERFORMS THAT REGRESSION RATHER THAN DESCRIBING IT. It decodes the shipped
        `tiger_1.glb`, moves the apex of L0's thinnest triangle 0.9 um toward the edge it stands off
        (every split corner of that one POSITION, so the weld stays intact and the level is still
        one component — a torn weld is a different refusal), patches those float32s back into the
        BIN chunk, re-encodes the glb, and writes an HONEST manifest for the result: the real
        sha256 of the poisoned bytes and every level's validity recomputed against the floor the
        poisoned L0 derives. Measured here, that floor falls from 1.1808 um to 0.9558 um.

        Both halves are asserted, and the first is what makes the second worth having:

          - with the fingerprint re-derived from the poisoned bytes, the corpus verifies CLEAN. Not
            one other check in the verifier notices a re-judged corpus.
          - with the fingerprint AS GENERATION RECORDED IT — at the moment L0 had just been proven
            identical to the evaluated .blend source — verification refuses, and that is the only
            failure it reports.

        See `verify`'s trust-boundary note for what this does and does not establish: the
        fingerprint binds the bytes to a recorded number, not to the artist.
        """
        import measure as measure_module

        levels = json.loads(self.text)["assets"][0]["levels"]
        source = levels[0]
        with open(os.path.join(self.root, source["glb"]), "rb") as handle:
            original = handle.read()
        gltf, binary = measure_module.glb_chunks_from_bytes(original, source["glb"])
        surface = measure_module.surface_from_bytes(original, source["node"], "L0")
        recorded_floor = source["validity"]["min_altitude_floor_m"]

        target, direction = self._thinnest_triangle_apex(surface)
        coincident = np.where(
            np.all(np.abs(surface.verts - surface.verts[target]) < 1e-12, axis=1)
        )[0]
        # glTF is Y-up and the decode rotated it into Blender's frame, so rotate the step back on
        # the way to the bytes: a Blender step (x, y, z) is the glTF step (x, z, -y).
        step = direction * 0.9e-6
        gltf_step = np.array([step[0], step[2], -step[1]], dtype=np.float64)

        primitive = measure_module.primitive_of(gltf, source["node"])
        accessor = gltf["accessors"][primitive["attributes"]["POSITION"]]
        view = gltf["bufferViews"][accessor["bufferView"]]
        self.assertEqual(accessor["componentType"], 5126, "POSITION is float32 in this glb")
        self.assertNotIn("byteStride", view, "POSITION is tightly packed in this glb")
        base = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
        blob = bytearray(binary)
        for index in coincident:
            at = base + int(index) * 12
            before = np.frombuffer(bytes(blob[at:at + 12]), dtype="<f4").astype(np.float64)
            blob[at:at + 12] = (before + gltf_step).astype("<f4").tobytes()
        poisoned = _rebuild_glb(gltf, bytes(blob))

        # The mutant is real, and it is the mutation this test says it is.
        #
        # The tolerance is a FLOAT32 ULP budget, not a decimal place. The step is added in f64 and
        # then stored back as f32, so what survives into the bytes is the step ROUNDED to the
        # spacing of f32 at the vertex's own magnitude — roughly 24 nm out at the 0.4 m coordinates
        # this shoe lives at, against a 900 nm step. A fixed `places=9` was really asserting that
        # the rounding happened to land inside 0.5 nm, which it did for one set of vertex
        # coordinates and stopped doing the moment the shoe was re-cut. Four ULP is the honest
        # statement: the step is the one intended, to the precision the format can hold it.
        mutant = measure_module.surface_from_bytes(poisoned, source["node"], "L0")
        moved = float(np.linalg.norm(mutant.verts[target] - surface.verts[target]))
        ulp = float(np.spacing(np.float32(np.abs(surface.verts[target]).max())))
        self.assertAlmostEqual(
            moved, 0.9e-6, delta=4 * ulp,
            msg=f"the poison must be a 0.9 um move (f32 ulp here is {ulp:.3g} m)",
        )
        self.assertEqual(mutant.components(), 1, "the weld must survive; a tear is another refusal")
        self.assertNotEqual(mutant.welded_digest(), source["welded_digest"])

        directory = tempfile.mkdtemp(prefix="lod-poisoned-l0-")
        self.addCleanup(shutil.rmtree, directory, ignore_errors=True)
        for level in levels:
            target_path = os.path.join(directory, level["glb"])
            os.makedirs(os.path.dirname(target_path), exist_ok=True)
            if level["level"] == 0:
                data = poisoned
            else:
                with open(os.path.join(self.root, level["glb"]), "rb") as handle:
                    data = handle.read()
            with open(target_path, "wb") as handle:
                handle.write(data)

        # AN HONEST MANIFEST FOR THE POISONED CORPUS: the real hash, and every level's validity
        # re-measured against the floor the poisoned baseline derives — exactly what someone who
        # wanted a lower bar would write, and exactly what the earlier version of this test skipped.
        manifest = json.loads(self.text)
        poisoned_levels = manifest["assets"][0]["levels"]
        poisoned_levels[0]["glb_sha256"] = hashlib.sha256(poisoned).hexdigest()
        floor = chain._derived_corpus_floor(
            mutant.validity(CONFIG.GATES), mutant.diagonal, CONFIG.GATES
        )
        self.assertLess(
            floor, recorded_floor - 1e-7,
            f"the poison must actually lower the corpus bar: {recorded_floor} m -> {floor} m",
        )
        for level in poisoned_levels:
            with open(os.path.join(directory, level["glb"]), "rb") as handle:
                decoded = measure_module.surface_from_bytes(
                    handle.read(), level.get("node"), f"L{level['level']}"
                )
            level["validity"] = decoded.validity(CONFIG.GATES, floor)

        # WITHOUT THE FINGERPRINT THERE IS NOTHING: re-derive it from the poisoned bytes, as an
        # attacker rewriting the manifest alongside the assets would, and the corpus verifies clean.
        rewritten = json.loads(json.dumps(manifest))
        rewritten["assets"][0]["levels"][0]["welded_digest"] = mutant.welded_digest()
        failures, _ = chain.verify(rewritten, chain.Tree(directory))
        self.assertEqual(
            failures, [],
            f"nothing but the fingerprint sees a 0.9 um L0 poison, so this half has to hold for "
            f"the next half to mean anything: {failures}",
        )

        # WITH IT: the recorded fingerprint no longer describes the bytes, and that is the refusal.
        failures, _ = chain.verify(manifest, chain.Tree(directory))
        self.assertTrue(
            any("geometry fingerprint" in f for f in failures),
            f"a poisoned L0 must be refused on the recorded geometry fingerprint: {failures}",
        )
        self.assertEqual(
            len(failures), 1,
            f"and refused SPECIFICALLY on it — every other number in this corpus was recomputed "
            f"honestly against the poisoned baseline: {failures}",
        )

        self.assert_caught(
            "L0 welded_digest no longer matches its bytes",
            lambda m: m["assets"][0]["levels"][0].__setitem__("welded_digest", "0" * 64),
        )
        self.assert_caught(
            "L0 welded_digest removed",
            lambda m: m["assets"][0]["levels"][0].pop("welded_digest"),
        )

    def test_no_manifest_number_may_be_non_finite(self):
        """A NaN loses every comparison silently, so any binding that reads it passes blind.

        Parameterised over every numeric leaf in the shipped manifest rather than a list somebody
        curated: a probe found 206 leaves that accepted NaN, all in records no named check covered.
        """
        manifest = json.loads(self.text)
        paths = list(_numeric_paths(manifest))
        self.assertGreater(len(paths), 100, "the sweep must actually be sweeping something")
        for poison in (float("nan"), float("inf"), float("-inf")):
            for path in paths:
                with self.subTest(poison=poison, path=".".join(str(p) for p in path)):
                    self.assert_caught(
                        f"{path} = {poison}",
                        lambda m, p=path, v=poison: _set_path(m, p, v),
                    )

    def test_the_unmutated_manifest_still_verifies(self):
        """The control: without it, a sweep that fails everything would look like a pass."""
        self.assertEqual(chain.verify(json.loads(self.text), chain.Tree(self.root))[0], [])


class ShippedManifestTests(unittest.TestCase):
    """The manifest actually committed to this tree, if it is here."""

    def setUp(self):
        self.root = CONFIG.repo_root()
        self.path = os.path.join(self.root, CONFIG.MANIFEST_RELPATH)
        if not os.path.isfile(self.path):
            self.skipTest(f"{CONFIG.MANIFEST_RELPATH} has not been generated yet")
        with open(self.path, encoding="utf-8") as handle:
            self.manifest = json.load(handle)

    def test_the_shipped_manifest_verifies(self):
        failures, _ = chain.verify(self.manifest, chain.Tree(self.root))
        self.assertEqual(failures, [], "\n".join(failures))

    def test_every_level_is_a_rung_of_the_global_grid(self):
        grid = {rung: target for rung, target in CONFIG.rungs()}
        for asset in self.manifest["assets"]:
            for level in asset["levels"]:
                if level["role"] == "source":
                    continue
                self.assertIn(level["rung"], grid)
                self.assertAlmostEqual(level["e_target_mm"], grid[level["rung"]], places=6)

    def test_every_kept_level_sheds_the_declared_fraction(self):
        for asset in self.manifest["assets"]:
            previous = asset["levels"][0]["tris"]
            for level in asset["levels"][1:]:
                shed = 1.0 - level["tris"] / previous
                self.assertGreaterEqual(
                    shed, CONFIG.SKIP_FRACTION - 1e-9,
                    f"{asset['name']} L{level['level']} sheds only {shed:.1%}",
                )
                previous = level["tris"]

    def test_the_chain_terminates_for_a_declared_reason(self):
        for asset in self.manifest["assets"]:
            self.assertIn(asset["termination"], {"right_wall", "topology_floor", "max_rungs"})
            last = asset["levels"][-1]
            if asset["termination"] == "right_wall":
                self.assertGreaterEqual(
                    last["switch_m"], self.manifest["ladder"]["right_wall_m"] - 1e-6
                )
            elif asset["termination"] == "topology_floor":
                self.assertEqual(last["tris"], asset["topology_floor_tris"])

    def test_no_shipped_level_can_default_a_tangent(self):
        """REQUIRES the record; it used to default a missing one to zero and pass on absence.

        "No measurement" and "measured zero defects" are not the same statement, and only the second
        is evidence. Defaulting turned the first into the second silently.
        """
        for asset in self.manifest["assets"]:
            for level in asset["levels"]:
                validity = level["validity"]
                for key in ("tangent_default_faces", "tangent_default_verts", "nonmanifold_edges",
                            "duplicate_faces", "orientation_flips", "nonfinite_attrs"):
                    self.assertIn(key, validity, f"{level['glb']} has no {key} record")
                    self.assertEqual(validity[key], 0, f"{level['glb']}: {key}")

    def test_every_level_records_validity_measured_on_its_own_shipped_bytes(self):
        """L0's record used to be copied from the Blender source — a different mesh by construction.

        The exporter splits corners by (position, normal, uv), so an 815-vertex Blender mesh decodes
        to 3 888 glTF vertices. A validity record carrying the Blender count is a measurement of
        something that does not ship.
        """
        for asset in self.manifest["assets"]:
            for level in asset["levels"]:
                self.assertEqual(level["validity"]["tris"], level["tris"], level["glb"])
                self.assertEqual(level["validity"]["verts"], level["verts"], level["glb"])
            l0 = asset["levels"][0]
            self.assertNotEqual(
                l0["verts"], l0["blender_source_verts"],
                "the shipped decode should carry the exporter's split vertices, not Blender's",
            )
            self.assertIn("identity_proof", l0)

    def test_no_recorded_field_is_unclassified(self):
        """EVERY field the manifest records is either CHECKED or named informational.

        There is no third state, because the third state is a field that reads as evidence and is
        not — which is what `schema_version`, the source level's numerics, the whole render record
        and the toolchain provenance each were, one review round after being added. A field added
        later fails this test until someone decides which of the two it is.
        """
        checked = set(
            chain.LEVEL_NUMERIC_FIELDS
            + chain.SOURCE_LEVEL_NUMERIC_FIELDS
            + chain.SOURCE_NUMERIC_FIELDS
            + chain.LEVEL_VALIDITY_FIELDS
            + chain.GATE_FIELDS
            + chain.GATE_NUMERIC_FIELDS
            + chain.GATE_VIEW_FIELDS
            + chain.GATE_VIEW_NUMERIC_FIELDS
        )
        checked |= {field for field, _c, _n in chain.GENERATOR_PINNED_FIELDS}
        checked |= {"schema_version", "version", "topology_floor_tris"}
        checked |= set(chain.PINNED_EVIDENCE_FIELDS)
        checked |= set(CONFIG.GATES) | set(CONFIG.RENDER_GATE) | set(CONFIG.SEARCH_LIMITS)
        # The view NAMES are dict keys inside every gate record, and they are checked: the set of
        # them is compared against the configured viewpoints, so a missing or invented view fails.
        checked |= {name for name, _e, _a in CONFIG.RENDER_GATE["views"]}
        # Re-derived: abstention from the recorded box and distance; the ruling against config.
        checked |= {"abstained", "screen_footprint_px", "bbox_mm", "ratification"}
        checked |= set(chain.TANGENT_PRESENCE_FIELDS)
        checked |= set(CONFIG.RATIFICATION_EVIDENCE["ruling"])
        known = checked | chain.INFORMATIONAL_FIELDS

        def walk(node, seen):
            if isinstance(node, dict):
                for key, value in node.items():
                    seen.add(key)
                    walk(value, seen)
            elif isinstance(node, list):
                for item in node:
                    walk(item, seen)

        seen = set()
        walk(self.manifest, seen)
        unclassified = sorted(seen - known)
        self.assertEqual(
            unclassified, [],
            f"these manifest fields are neither checked nor declared informational: "
            f"{unclassified}. Add each to a field list in chain.py or to INFORMATIONAL_FIELDS.",
        )

    def test_the_ratification_evidence_matches_the_shipped_manifest(self):
        """The pin on `config.RATIFICATION_EVIDENCE`, which went stale twice as prose.

        Numbers quoted beside a pending decision have to BE the numbers, and a comment cannot be
        held to that. This is what stops a regeneration from silently leaving the evidence Yan would
        rule on describing a corpus that no longer exists.
        """
        asset = self.manifest["assets"][0]
        levels = {level["level"]: level for level in asset["levels"]}
        for index, tris, switch_m, score, verdict in CONFIG.RATIFICATION_EVIDENCE["levels"]:
            level = levels[index]
            self.assertEqual(level["tris"], tris, f"L{index} triangle count")
            self.assertAlmostEqual(level["switch_m"], switch_m, places=1, msg=f"L{index} switch")
            gate = level["render_gate"]
            if verdict == "ABSTAIN":
                self.assertTrue(gate["abstained"], f"L{index} should abstain")
                self.assertIsNone(score, "an abstaining level has no score to record")
                self.assertIsNone(gate["pass"], "an abstention is not a verdict")
                continue
            self.assertFalse(gate["abstained"], f"L{index} should be scored")
            self.assertAlmostEqual(
                gate["worst_defect_score"], score, places=6, msg=f"L{index} defect score"
            )
            self.assertEqual("PASS" if gate["pass"] else "FAIL", verdict, f"L{index} verdict")
        self.assertEqual(
            asset["enumerated_outputs"], CONFIG.RATIFICATION_EVIDENCE["enumerated_outputs"]
        )
        skipped = {entry["rung"]: entry for entry in asset["skipped_rungs"]}
        evidence = CONFIG.RATIFICATION_EVIDENCE["skipped_rungs"]
        self.assertEqual(
            sorted(skipped), sorted(rung for rung, _, _ in evidence),
            "the evidence must enumerate EVERY skipped rung — one that vanishes from the list is a "
            "rung the ladder silently stopped considering",
        )
        for rung, best_tris, shed in evidence:
            self.assertEqual(skipped[rung]["best_tris"], best_tris, f"rung {rung} best tris")
            self.assertAlmostEqual(
                skipped[rung]["shed_fraction"], shed, places=4, msg=f"rung {rung} shed fraction"
            )

    def test_the_right_wall_records_where_it_came_from(self):
        source = self.manifest["ladder"]["right_wall_source"]
        self.assertTrue(source and len(source) > 20, "the right wall must say what bounded it")
        self.assertAlmostEqual(
            self.manifest["ladder"]["right_wall_m"], CONFIG.RIGHT_WALL_M, places=6
        )

    def test_the_source_level_is_the_source(self):
        """L0 is the artist's mesh — the shipped host glb must carry exactly it (ADR 0033 §1)."""
        for asset in self.manifest["assets"]:
            l0 = asset["levels"][0]
            self.assertEqual(l0["role"], "source")
            self.assertEqual(l0["tris"], asset["source"]["tris"])
            if "shipped_tris" in l0:
                self.assertEqual(l0["shipped_tris"], asset["source"]["tris"])
                self.assertTrue(l0["shipped_matches_source"])
                self.assertLess(l0["shipped_dev_from_source_mm"], 1.0e-3)


class ProjectionSanityTests(unittest.TestCase):
    def test_one_millimetre_goes_subpixel_at_eighteen_metres(self):
        """The optic reference the doctrine quotes, re-derived rather than remembered."""
        self.assertAlmostEqual(chain.switch_distance_m(1.0, 0.0, VIEW), 17.98, places=1)

    def test_the_right_wall_is_the_map_diagonal(self):
        self.assertAlmostEqual(CONFIG.RIGHT_WALL_M, 1000.0 * math.sqrt(2.0), places=6)
        self.assertIn("diagonal", CONFIG.RIGHT_WALL_SOURCE)


if __name__ == "__main__":
    unittest.main(verbosity=2)
