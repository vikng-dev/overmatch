"""The seam's own tests: the projection, the drift catches, and the shipped manifest.

    python3 scripts/lod/test_chain.py

Stdlib `unittest` on purpose — this must run without Blender, without numpy and without cargo, so
`scripts/hooks/pre-push` can afford it on every push.

THE REGRESSION THESE EXIST FOR: an exporter comment narrated 223.7 m for a level whose runtime
derivation said 335.5 m. Both were "measured"; the comment had frozen a stale deviation and used
the small-angle projection besides. `the_drifted_ledger_is_caught` reconstructs exactly that shape
and asserts `verify` refuses it.
"""

import json
import math
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import chain  # noqa: E402
import config as CONFIG  # noqa: E402

VIEW = CONFIG.REFERENCE_VIEW


def validity_record(tris, verts, origin_radius=0.1):
    """A clean validity record — every field the strict schema requires, all zero defects."""
    return {
        "tris": tris, "verts": verts, "components": 1, "duplicate_faces": 0,
        "nonfinite_attrs": 0, "orientation_flips": 0, "nonmanifold_edges": 0,
        "slivers_below_floor": 0, "tangent_default_faces": 0, "tangent_default_verts": 0,
        "min_altitude_m": 0.001, "origin_radius_m": origin_radius,
    }


def gate_record(passed=True):
    return {
        "pass": passed, "worst_defect_score": 0.1, "worst_mean_abs_diff": 0.001,
        "views": {"three_quarter": {"pass": passed}},
        "thresholds": {"defect_fraction": CONFIG.RENDER_GATE["defect_fraction"]},
    }


def synthetic_manifest():
    """A two-level chain with hand-computed numbers, independent of any generation run.

    Built FROM the configuration rather than beside it, because the verifier now compares the whole
    configuration and a hand-typed copy would drift the moment a threshold moves — which is the same
    disease the manifest exists to cure.
    """
    radius = 0.1
    dev1, pair1 = 4.0, 5.0
    switch1 = chain.switch_distance_m(max(dev1, pair1), radius, VIEW)
    asset_name = CONFIG.ASSETS[0]["name"]
    return {
        "schema": "overmatch.lod.manifest",
        "schema_version": 1,
        "generator": {
            "script": "scripts/lod/generate.py",
            "version": CONFIG.GENERATOR_VERSION,
            "sources_sha256": CONFIG.generator_digest(),
            "blender": "test",
        },
        "ladder": {
            "e1_mm": CONFIG.E1_MM, "octave": CONFIG.OCTAVE,
            "skip_fraction": CONFIG.SKIP_FRACTION, "max_rungs": CONFIG.MAX_RUNGS,
            "right_wall_m": round(CONFIG.RIGHT_WALL_M, 6),
            "right_wall_source": CONFIG.RIGHT_WALL_SOURCE, "reference_view": VIEW,
        },
        "gates": {
            "numeric": dict(CONFIG.GATES),
            "render": {k: v for k, v in CONFIG.RENDER_GATE.items() if k != "views"},
            "render_views": [list(v) for v in CONFIG.RENDER_GATE["views"]],
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
                {"level": 0, "rung": 0, "role": "source", "tris": 1000, "verts": 500,
                 "glb": "a/source.glb", "glb_sha256": "a" * 64, "node": "Probe",
                 "e_target_mm": 0.0, "dev_source_mm": 0.0, "dev_source_mm_upper": 0.0,
                 "pairwise_mm": None, "switch_m": 0.0,
                 "validity": validity_record(1000, 500, radius)},
                {"level": 1, "rung": 2, "role": "generated", "tris": 400, "verts": 220,
                 "glb": "a/l1.glb", "glb_sha256": "b" * 64, "node": "Probe_LOD2",
                 "e_target_mm": 7.78, "shed_fraction_vs_parent": 0.6,
                 "dev_source_mm": dev1, "dev_source_mm_upper": dev1,
                 "pairwise_mm": pair1, "pairwise_mm_upper": pair1,
                 "switch_m": round(switch1, 4),
                 "validity": validity_record(400, 220, radius),
                 "render_gate": gate_record(True)},
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
        failures = [f for f in chain.verify(manifest, "/nonexistent-root")[0]
                    if "missing" not in f]
        self.assertEqual(failures, [], failures)

    def test_a_manifest_with_no_render_gate_is_refused(self):
        """`--no-render-gate` is for iterating on the search, never for a committed chain."""
        manifest = synthetic_manifest()
        del manifest["assets"][0]["levels"][1]["render_gate"]
        failures, _ = chain.verify(manifest, "/nonexistent-root")
        self.assertTrue(any("no render-gate record" in f for f in failures), failures)

    def _failing_gate_manifest(self, blocking):
        """A manifest whose gate FAILED, cut under `blocking`. Patches CONFIG, not the manifest.

        The blocking flag is read from the TREE now, never from the manifest: trusting the recorded
        flag meant ratifying the threshold left every already-recorded failure a warning for ever.
        """
        manifest = synthetic_manifest()
        manifest["gates"]["render_gate_blocking"] = blocking
        manifest["assets"][0]["levels"][1]["render_gate"] = gate_record(False)
        return manifest

    def test_a_failing_render_gate_blocks_once_the_threshold_is_ratified(self):
        original = CONFIG.RENDER_GATE_BLOCKING
        CONFIG.RENDER_GATE_BLOCKING = True
        try:
            failures, warnings = chain.verify(
                self._failing_gate_manifest(True), "/nonexistent-root"
            )
        finally:
            CONFIG.RENDER_GATE_BLOCKING = original
        self.assertTrue(any("render gate recorded a FAIL" in f for f in failures), failures)
        self.assertEqual(warnings, [])

    def test_a_failing_render_gate_warns_while_the_threshold_is_unratified(self):
        """Recorded and shouted about, but not enforced — and never silently dropped."""
        original = CONFIG.RENDER_GATE_BLOCKING
        CONFIG.RENDER_GATE_BLOCKING = False
        try:
            failures, warnings = chain.verify(
                self._failing_gate_manifest(False), "/nonexistent-root"
            )
        finally:
            CONFIG.RENDER_GATE_BLOCKING = original
        self.assertEqual([f for f in failures if "render gate" in f], [])
        self.assertTrue(any("render gate recorded a FAIL" in w for w in warnings), warnings)
        self.assertTrue(any("unratified" in w for w in warnings), warnings)

    def test_a_manifest_recording_a_stale_blocking_flag_is_refused(self):
        """Ratifying the threshold must invalidate every manifest cut before the ruling."""
        manifest = synthetic_manifest()
        manifest["gates"]["render_gate_blocking"] = not CONFIG.RENDER_GATE_BLOCKING
        failures, _ = chain.verify(manifest, "/nonexistent-root")
        self.assertTrue(any("render_gate_blocking" in f for f in failures), failures)

    def test_the_drifted_ledger_is_caught(self):
        """The 223.7-vs-335.5 shape: a recorded distance that no longer re-derives."""
        manifest = synthetic_manifest()
        manifest["assets"][0]["levels"][1]["switch_m"] = 223.7
        failures, _ = chain.verify(manifest, "/nonexistent-root")
        self.assertTrue(
            any("drifted from the measurement" in f for f in failures),
            f"a stale hand-written switch distance must fail verification; got {failures}",
        )

    def test_a_level_over_its_rung_is_caught(self):
        manifest = synthetic_manifest()
        manifest["assets"][0]["levels"][1]["dev_source_mm_upper"] = 99.0
        failures, _ = chain.verify(manifest, "/nonexistent-root")
        self.assertTrue(any("exceeds its rung target" in f for f in failures), failures)

    def test_a_moved_right_wall_is_caught(self):
        manifest = synthetic_manifest()
        manifest["ladder"]["right_wall_m"] = 5000.0
        failures, _ = chain.verify(manifest, "/nonexistent-root")
        self.assertTrue(any("right_wall_m" in f for f in failures), failures)

    def test_a_stale_generator_version_is_caught(self):
        manifest = synthetic_manifest()
        manifest["generator"]["version"] = "0.0.1"
        failures, _ = chain.verify(manifest, "/nonexistent-root")
        self.assertTrue(any("regenerate" in f for f in failures), failures)

    def test_a_chain_that_grows_triangles_is_caught(self):
        manifest = synthetic_manifest()
        manifest["assets"][0]["levels"][1]["tris"] = 5000
        failures, _ = chain.verify(manifest, "/nonexistent-root")
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
        return chain.verify(manifest, "/nonexistent-root")[0]

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
        self.assertTrue(any("not a finite number" in f for f in failures), failures)

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

    def test_a_manifest_with_recorded_defects_is_refused(self):
        """A recorded non-zero defect count must fail verification, not just generation."""
        def defect(manifest):
            manifest["assets"][0]["levels"][1]["validity"]["nonmanifold_edges"] = 3

        failures = self.failures_for(defect)
        self.assertTrue(any("nonmanifold_edges" in f for f in failures), failures)

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
        failures, _ = chain.verify(self.manifest, self.root)
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
