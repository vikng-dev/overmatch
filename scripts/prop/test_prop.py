"""The prop build's laws, on the tracked tree trio and on mutants of it.

    python3 scripts/prop/test_prop.py
    python3 -m pytest scripts/prop

Every gate is exercised by MUTATING the thing it gates on, from the tracked `fir_1` trio, so a
check that passes everything is caught. The cases that need Blender say so and skip without the
pinned one: generation and export are what they prove, and nothing else can stand in.
"""

import copy
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import build  # noqa: E402
import plan  # noqa: E402
import ron  # noqa: E402
import toolchain  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
TABLE = os.path.join(ROOT, "assets", "trees", "species.ron")
FIR = os.path.join(ROOT, "assets", "trees", "fir_1", "fir_1")
BLENDER = toolchain.blender()


def tracked():
    """The tracked fir_1 trio: `(glb json, glb bytes, tree ron value, tree ron bytes, cert)`."""
    view_blob = build.read(FIR + ".glb")
    ron_blob = build.read(FIR + ".tree.ron")
    js, _ = build.parse_glb(view_blob)
    with open(FIR + ".lod.json", encoding="utf-8") as handle:
        cert = json.load(handle)
    return js, view_blob, ron.parse(ron_blob.decode("utf-8")), ron_blob, cert


def node(js, name):
    return next(n for n in js["nodes"] if n.get("name") == name)


# ── the species table ────────────────────────────────────────────────────────────────────────────

class SpeciesTable(unittest.TestCase):
    def test_three_species_in_the_contract_shape(self):
        """The runtime keys a tree by species; a key the table lacks is a tree with no substance
        and no thresholds."""
        table = ron.load(TABLE)
        self.assertEqual(sorted(table), ["norway_spruce", "scots_pine", "silver_birch"])
        for key, row in table.items():
            self.assertEqual(row["__struct__"], "Species", key)
            for field in ("height_m", "dbh_m"):
                lo, hi = row[field]
                self.assertLess(0.0, lo, key)
                self.assertLess(lo, hi, key)
            self.assertIsInstance(row["substance"], str)
            for field in ("fell_k", "hp"):
                value, kind = row[field]
                self.assertGreater(value, 0, "{} {}: a placeholder is positive (A6)".format(key, field))
                self.assertEqual(kind.name, "Unknown", "{} {} is a placeholder".format(key, field))
                self.assertTrue(kind.args[0])


# ── the plan ─────────────────────────────────────────────────────────────────────────────────────

class Plan(unittest.TestCase):
    def test_segment_heights_sum_to_the_height_inside_the_band(self):
        """A cut snaps to a segment boundary, so the boundaries must tile the height exactly and
        every segment must sit in the contract's 1.5–2 m band."""
        for height in (15.0, 18.37, 25.0, 27.02, 33.333, 40.0):
            rows = plan.segments(height, 0.4)
            self.assertAlmostEqual(sum(r["height_m"] for r in rows), round(height, 4), places=9)
            for row in rows:
                self.assertGreaterEqual(row["height_m"], plan.SEGMENT_MIN_M - 1e-9)
                self.assertLessEqual(row["height_m"], plan.SEGMENT_MAX_M + 1e-9)
            for below, above in zip(rows, rows[1:]):
                self.assertEqual(below["r_top_m"], above["r_bottom_m"])
                self.assertGreaterEqual(below["r_bottom_m"], below["r_top_m"])

    def test_the_plan_is_a_function_of_species_and_seed(self):
        table = ron.load(TABLE)
        first = plan.sample(table["scots_pine"], 11)
        second = plan.sample(table["scots_pine"], 11)
        self.assertEqual((first["height_m"], first["dbh_m"]), (second["height_m"], second["dbh_m"]))
        self.assertEqual(plan.whorls(first["height_m"], table["scots_pine"], first["rng"]),
                         plan.whorls(second["height_m"], table["scots_pine"], second["rng"]))
        other = plan.sample(table["scots_pine"], 12)
        self.assertNotEqual(first["height_m"], other["height_m"])


# ── the tracked trio ─────────────────────────────────────────────────────────────────────────────

class TrackedTrio(unittest.TestCase):
    def test_fir_1_satisfies_the_node_contract(self):
        """Lane B's loader reads extras, chain parenting and origins; this is the glb it tests on."""
        js, _, tree, _, _ = tracked()
        self.assertEqual(build.contract_failures(js, "fir_1"), [])
        by_part = build.tree_nodes(js)
        self.assertEqual(len(by_part["trunk"]), len(tree["segments"]))
        self.assertEqual(len(by_part["cap_bottom"]), len(by_part["trunk"]))
        self.assertEqual(len(by_part["cap_top"]), len(by_part["trunk"]))
        self.assertGreater(len(by_part["canopy"]), 0)
        self.assertTrue(all(len(n.get("children", [])) >= 2
                            for i, n in enumerate(js["nodes"]) if i in by_part["trunk"].values()))

    def test_fir_1_ron_matches_its_glb_and_is_a_realistic_spruce(self):
        """The ron carries no dbh (A6); the loader derives it from the taper at 1.3 m, which is
        done here the same way, and both it and the height must sit in the species' ranges."""
        js, _, tree, _, _ = tracked()
        table = ron.load(TABLE)
        self.assertEqual(build.ron_failures(tree, js, table), [])
        self.assertNotIn("dbh_m", tree)
        height = sum(s["height_m"] for s in tree["segments"])
        lo, hi = table[tree["species"]]["height_m"]
        self.assertTrue(lo <= height <= hi, height)
        z = 0.0
        for segment in tree["segments"]:
            if z <= plan.BREAST_HEIGHT_M < z + segment["height_m"]:
                t = (plan.BREAST_HEIGHT_M - z) / segment["height_m"]
                dbh = 2.0 * (segment["r_bottom_m"] + t * (segment["r_top_m"] - segment["r_bottom_m"]))
                break
            z += segment["height_m"]
        lo, hi = table[tree["species"]]["dbh_m"]
        self.assertTrue(lo <= dbh <= hi, dbh)

    def test_fir_1_certificate_is_coherent_and_lod0_only(self):
        _, view_blob, _, ron_blob, cert = tracked()
        digest = build.blend_digest(FIR + ".blend", FIR + ".tree.ron", TABLE)
        self.assertEqual(build.coherence(cert, view_blob, ron_blob, digest), [])
        self.assertEqual(cert["chains"], {})


# ── mutants ──────────────────────────────────────────────────────────────────────────────────────

class ContractMutants(unittest.TestCase):
    def mutant(self):
        js, _, _, _, _ = tracked()
        return copy.deepcopy(js)

    def test_a_node_without_extras_is_refused(self):
        js = self.mutant()
        del node(js, "cap_top_03")["extras"]
        self.assertTrue(any("cap_top_03" in f and "extras" in f
                            for f in build.contract_failures(js, "fir_1")))

    def test_a_segment_reparented_to_the_root_breaks_the_chain(self):
        js = self.mutant()
        root, t0, t1 = node(js, "fir_1"), node(js, "trunk_00"), node(js, "trunk_01")
        index = js["nodes"].index(t1)
        t0["children"].remove(index)
        root["children"].append(index)
        self.assertTrue(any("trunk 1" in f and "chain" in f
                            for f in build.contract_failures(js, "fir_1")))

    def test_a_rotated_segment_is_refused(self):
        js = self.mutant()
        node(js, "trunk_02")["rotation"] = [0.0, 0.0, 0.3826834, 0.9238795]
        self.assertTrue(any("trunk_02" in f and "rotation" in f
                            for f in build.contract_failures(js, "fir_1")))

    def test_an_origin_off_the_axis_is_refused(self):
        js = self.mutant()
        node(js, "trunk_02")["translation"] = [0.1, node(js, "trunk_02")["translation"][1], 0.0]
        self.assertTrue(any("trunk_02" in f and "axis" in f
                            for f in build.contract_failures(js, "fir_1")))

    def test_a_missing_cap_is_refused(self):
        js = self.mutant()
        node(js, "cap_bottom_05")["extras"]["tree_part"] = "canopy"
        failures = build.contract_failures(js, "fir_1")
        self.assertTrue(any("trunk 5 has no cap_bottom" in f for f in failures), failures)

    def test_two_nodes_claiming_one_part_are_refused(self):
        js = self.mutant()
        node(js, "cap_top_05")["extras"]["tree_segment"] = 4
        self.assertTrue(any("two nodes" in f for f in build.contract_failures(js, "fir_1")))


class RonMutants(unittest.TestCase):
    def setUp(self):
        self.js, _, self.tree, _, _ = tracked()
        self.table = ron.load(TABLE)

    def test_a_height_the_chain_does_not_place_is_refused(self):
        tree = copy.deepcopy(self.tree)
        tree["segments"][3]["height_m"] += 0.01
        failures = build.ron_failures(tree, self.js, self.table)
        self.assertTrue(any("trunk 4" in f for f in failures), failures)

    def test_radii_that_do_not_meet_are_refused(self):
        tree = copy.deepcopy(self.tree)
        tree["segments"][2]["r_top_m"] += 0.01
        failures = build.ron_failures(tree, self.js, self.table)
        self.assertTrue(any("one disc" in f for f in failures), failures)

    def test_dbh_m_is_refused(self):
        tree = copy.deepcopy(self.tree)
        tree["dbh_m"] = 0.4
        failures = build.ron_failures(tree, self.js, self.table)
        self.assertTrue(any("dbh_m" in f and "A6" in f for f in failures), failures)

    def test_a_species_outside_the_table_is_refused(self):
        tree = copy.deepcopy(self.tree)
        tree["species"] = "baobab"
        failures = build.ron_failures(tree, self.js, self.table)
        self.assertTrue(any("baobab" in f for f in failures), failures)

    def test_a_segment_count_the_glb_lacks_is_refused(self):
        tree = copy.deepcopy(self.tree)
        tree["segments"].append(dict(tree["segments"][-1]))
        tree["segments"][-1]["r_bottom_m"] = tree["segments"][-2]["r_top_m"]
        failures = build.ron_failures(tree, self.js, self.table)
        self.assertTrue(any("trunk node" in f for f in failures), failures)


class CertificateMutants(unittest.TestCase):
    def setUp(self):
        _, self.view, _, self.ron, self.cert = tracked()

    def test_an_edited_hash_is_incoherent(self):
        cert = dict(self.cert, view_glb_sha="0" * 64)
        self.assertTrue(any("incoherent" in f for f in build.coherence(cert, self.view, self.ron, None)))

    def test_a_stale_digest_is_named(self):
        self.assertTrue(any("stale" in f
                            for f in build.coherence(self.cert, self.view, self.ron, "x")))

    def test_a_sixth_field_or_a_chain_is_refused(self):
        cert = dict(self.cert, note="lod0")
        self.assertTrue(any("five fields" in f for f in build.coherence(cert, self.view, self.ron, None)))
        cert = dict(self.cert, chains={"trunk_00#0": {"radius_m": 1.0, "rungs": []}})
        self.assertTrue(any("nothing measured" in f
                            for f in build.coherence(cert, self.view, self.ron, None)))

    def test_a_changed_ron_is_incoherent(self):
        self.assertTrue(any("tree ron" in f
                            for f in build.coherence(self.cert, self.view, self.ron + b"\n", None)))


# ── generation, in Blender ───────────────────────────────────────────────────────────────────────

@unittest.skipIf(BLENDER.mismatch, "the pinned Blender is not here: {}".format(BLENDER))
class Generation(unittest.TestCase):
    def generate(self, out, species, seed):
        subprocess.run([BLENDER.binary, "--background", "--factory-startup", "--python",
                        os.path.join(ROOT, "scripts", "prop", "gen_tree.py"), "--",
                        "--species", species, "--seed", str(seed), "--out", out],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
        glb = out[:-len(".blend")] + ".glb"
        subprocess.run([BLENDER.binary, "--background", "--factory-startup", out, "--python",
                        os.path.join(ROOT, "scripts", "prop", "export_tree.py"), "--",
                        "--out", glb],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
        return build.read(glb), build.read(out[:-len(".blend")] + ".tree.ron")

    def test_the_same_seed_exports_the_same_bytes(self):
        """Determinism is by seed: two generations of one (species, seed) export one glb and one
        ron. The `.blend` is not compared — Blender writes memory addresses into it, so its bytes
        differ run to run while the geometry does not."""
        with tempfile.TemporaryDirectory() as work:
            first = self.generate(os.path.join(work, "a", "birch_7.blend"), "silver_birch", 7)
            second = self.generate(os.path.join(work, "b", "birch_7.blend"), "silver_birch", 7)
        self.assertEqual(first[0], second[0])
        self.assertEqual(first[1], second[1])

    def test_a_generated_tree_satisfies_both_contracts(self):
        with tempfile.TemporaryDirectory() as work:
            view, ron_blob = self.generate(os.path.join(work, "pine_3.blend"), "scots_pine", 3)
        js, _ = build.parse_glb(view)
        self.assertEqual(build.contract_failures(js, "pine_3"), [])
        self.assertEqual(build.ron_failures(ron.parse(ron_blob.decode()), js, ron.load(TABLE)), [])


if __name__ == "__main__":
    unittest.main()
