"""The prop build's laws, on the tracked tree trio and on mutants of it.

    python3 scripts/prop/test_prop.py
    python3 -m pytest scripts/prop

The tracked `fir_1` trio is the fixture: each law of the three contracts and the certificate is
exercised by MUTATING the thing it gates on — the glb document, the BIN chunk's bytes, the ron,
the species row, the certificate — and asserting the refusal names it. The tracked-trio cases
re-read what the pipeline wrote and add the independent assertions (DBH, ranges, census). The
cases that need Blender say so and skip without the pinned one: generation, export and the
verifier's re-export are what they prove, and nothing else can stand in.

`OVERMATCH_REGISTRY` names the substance registry the one registry case reads; the repository's
`assets/materials/materials.ron` by default.
"""

import copy
import json
import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import types
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import build  # noqa: E402
import plan  # noqa: E402
import ron  # noqa: E402
import toolchain  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
TABLE = os.path.join(ROOT, "assets", "trees", "species.ron")
REGISTRY = os.environ.get("OVERMATCH_REGISTRY") or os.path.join(ROOT, build.REGISTRY)
FIR = os.path.join(ROOT, "assets", "trees", "fir_1", "fir_1")
BLENDER = toolchain.blender()

#: The species the slice contract (§3) names; the table may hold more.
CONTRACT_SPECIES = {"norway_spruce", "scots_pine", "silver_birch"}

#: How far the DBH read off the taper at 1.3 m may sit from the sampled one, metres. DERIVED from
#: two sources of error: each ring radius is written to four decimals (at most 0.1 mm on a
#: diameter) and the chord of the power-law taper across the breast-height segment is not the
#: curve. MEASURED: 0.157 mm at most over 10,000 seeds of each species (plan.py as of this file);
#: the bound leaves that a 60 % margin.
DBH_TOL_M = 2.5e-4


def tracked() -> types.SimpleNamespace:
    """The tracked fir_1 trio, parsed and raw."""
    view_blob = build.read(FIR + ".glb")
    ron_blob = build.read(FIR + ".tree.ron")
    js, binary = build.parse_glb(view_blob)
    with open(FIR + ".lod.json", encoding="utf-8") as handle:
        cert = json.load(handle)
    return types.SimpleNamespace(js=js, binary=binary, view=view_blob, ron=ron_blob,
                                 tree=ron.parse(ron_blob.decode("utf-8")), cert=cert)


def node(js, name):
    return next(n for n in js["nodes"] if n.get("name") == name)


def table_substances():
    """The substances the table names — what the registry holds once the runtime lane's rows
    land; the contract cases that are not about the registry read this."""
    return {row["substance"] for row in ron.load(TABLE).values()}


def taper_dbh(segments) -> float:
    """The loader's reading of DBH: the piecewise-linear taper at 1.3 m, doubled."""
    z = 0.0
    for segment in segments:
        if z <= plan.BREAST_HEIGHT_M < z + segment["height_m"]:
            t = (plan.BREAST_HEIGHT_M - z) / segment["height_m"]
            return 2.0 * (segment["r_bottom_m"] + t * (segment["r_top_m"] - segment["r_bottom_m"]))
        z = plan.round_m(z + segment["height_m"])
    raise AssertionError("no segment spans breast height")


def positions_of(js, binary, name):
    index = js["nodes"].index(node(js, name))
    return build.node_geometry(js, binary, index, name)[0]


def rewritten(js, binary, name, attribute, vertex, values) -> bytes:
    """The BIN chunk with one vertex of one attribute of one node's first primitive rewritten."""
    primitive = js["meshes"][node(js, name)["mesh"]]["primitives"][0]
    acc = js["accessors"][primitive["attributes"][attribute]]
    view = js["bufferViews"][acc["bufferView"]]
    at = view.get("byteOffset", 0) + acc.get("byteOffset", 0) + 12 * vertex
    out = bytearray(binary)
    struct.pack_into("<fff", out, at, *values)
    return bytes(out)


def glb_bytes(js, binary) -> bytes:
    """A glb of these two chunks, framed the way the format requires."""
    jb = json.dumps(js, separators=(",", ":")).encode()
    jb += b" " * (-len(jb) % 4)
    bb = binary + b"\0" * (-len(binary) % 4)
    out = bytearray(struct.pack("<III", 0x46546C67, 2, 12 + 8 + len(jb) + (8 + len(bb) if bb else 0)))
    out += struct.pack("<II", len(jb), 0x4E4F534A) + jb
    if bb:
        out += struct.pack("<II", len(bb), 0x004E4942) + bb
    return bytes(out)


# ── the species table ────────────────────────────────────────────────────────────────────────────

class SpeciesTable(unittest.TestCase):
    def test_the_contract_species_are_in_the_table(self):
        """The runtime keys a tree by species; the three the slice contract names must resolve."""
        self.assertTrue(CONTRACT_SPECIES <= set(ron.load(TABLE)))

    def test_every_row_is_in_the_contract_shape(self):
        """Class invariants over every row the table holds, so a fourth species is held to the
        same law and not to a list: ordered positive ranges a taper can be cut from, a substance
        name, positive provisional thresholds under a stated Unknown, view parameters in range."""
        table = ron.load(TABLE)
        self.assertTrue(table)
        for key, row in table.items():
            self.assertEqual(row["__struct__"], "Species", key)
            for field in ("height_m", "dbh_m"):
                lo, hi = row[field]
                self.assertLess(0.0, lo, key)
                self.assertLess(lo, hi, key)
            self.assertGreater(row["height_m"][0], 2.0 * plan.BREAST_HEIGHT_M,
                               "{}: the taper is cut from breast height".format(key))
            self.assertIsInstance(row["substance"], str)
            for field in ("fell_k", "hp"):
                value, kind = row[field]
                self.assertGreater(value, 0, "{} {}: a placeholder is positive (A6)".format(key, field))
                self.assertEqual(kind.name, "Unknown", "{} {} is a placeholder".format(key, field))
                self.assertTrue(kind.args[0])
            self.assertIsInstance(row["hp"][0], int, key)
            self.assertTrue(0.0 < row["crown_base_frac"] < 1.0, key)
            self.assertGreater(row["crown_width_frac"], 0.0, key)
            self.assertGreater(row["whorl_spacing_m"], 0.0, key)
            self.assertEqual(build.species_failures(key, row, {row["substance"]}), [])

    def test_every_substance_is_a_registry_key(self):
        """A species' substance is read out of the registry at spawn; one the registry lacks is a
        tree with no matter."""
        registry = build.substances(REGISTRY)
        for key, row in ron.load(TABLE).items():
            self.assertIn(row["substance"], registry,
                          "{}: not a key of {}".format(key, os.path.relpath(REGISTRY, ROOT)))


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

    def test_the_taper_returns_the_sampled_dbh(self):
        """The ron carries no dbh (A6): the loader reads it off the taper at 1.3 m, and that
        reading must be the diameter the plan sampled, within `DBH_TOL_M`, for every species and
        seed — a taper law that misplaced breast height would pass range checks and fail this."""
        table = ron.load(TABLE)
        worst = 0.0
        for key, row in table.items():
            for seed in range(1000):
                sampled = plan.sample(row, seed)
                rows = plan.segments(sampled["height_m"], sampled["dbh_m"])
                error = abs(taper_dbh(rows) - sampled["dbh_m"])
                self.assertLessEqual(error, DBH_TOL_M, "{} seed {}".format(key, seed))
                worst = max(worst, error)
        self.assertGreater(worst, 0.0, "the taper is a chord: some seed must miss by a little")


# ── the tracked trio ─────────────────────────────────────────────────────────────────────────────

class TrackedTrio(unittest.TestCase):
    def test_fir_1_satisfies_the_node_contract(self):
        """Lane B's loader reads extras, chain parenting and origins; this is the glb it tests on."""
        trio = tracked()
        self.assertEqual(build.contract_failures(trio.js, "fir_1"), [])
        by_part = build.tree_nodes(trio.js)
        self.assertEqual(len(by_part["trunk"]), len(trio.tree["segments"]))
        self.assertEqual(len(by_part["cap_bottom"]), len(by_part["trunk"]))
        self.assertEqual(len(by_part["cap_top"]), len(by_part["trunk"]))
        self.assertGreater(len(by_part["canopy"]), 0)
        self.assertTrue(all(len(n.get("children", [])) >= 2
                            for i, n in enumerate(trio.js["nodes"]) if i in by_part["trunk"].values()))

    def test_fir_1_ron_matches_its_glb_and_is_a_realistic_spruce(self):
        """The ron's species row resolves, its segments are the chain's, and both the height and
        the DBH read off the taper sit in the species' ranges."""
        trio = tracked()
        table = ron.load(TABLE)
        self.assertEqual(build.ron_failures(trio.tree, trio.js, table, table_substances()), [])
        self.assertNotIn("dbh_m", trio.tree)
        height = sum(s["height_m"] for s in trio.tree["segments"])
        lo, hi = table[trio.tree["species"]]["height_m"]
        self.assertTrue(lo <= height <= hi, height)
        lo, hi = table[trio.tree["species"]]["dbh_m"]
        self.assertTrue(lo <= taper_dbh(trio.tree["segments"]) <= hi)

    def test_fir_1_taper_encodes_the_seed_it_records(self):
        """The root records the seed; re-sampling the species with it gives the DBH the taper
        was cut from, within `DBH_TOL_M` — the stored geometry and the recorded provenance agree."""
        trio = tracked()
        root = trio.js["nodes"][build.root_node(trio.js)]["extras"]
        self.assertEqual(root["tree_species"], trio.tree["species"])
        sampled = plan.sample(ron.load(TABLE)[trio.tree["species"]], root["tree_seed"])
        self.assertLessEqual(abs(taper_dbh(trio.tree["segments"]) - sampled["dbh_m"]), DBH_TOL_M)
        self.assertAlmostEqual(sum(s["height_m"] for s in trio.tree["segments"]),
                               sampled["height_m"], places=3)

    def test_fir_1_geometry_is_the_ron_s(self):
        """The stored vertices, not the node offsets: rings, caps and shared loops are read out
        of the BIN chunk against the ron's dimensions."""
        trio = tracked()
        self.assertEqual(build.geometry_failures(trio.js, trio.binary, trio.tree["segments"]), [])
        bottom = [p for p in positions_of(trio.js, trio.binary, "trunk_00") if abs(p[1]) < 1e-6]
        self.assertEqual(len(bottom), 12)
        self.assertAlmostEqual(max(math.hypot(x, z) for x, _, z in bottom),
                               trio.tree["segments"][0]["r_bottom_m"], places=4)

    def test_fir_1_certificate_is_coherent_and_lod0_only(self):
        trio = tracked()
        digest = build.blend_digest(FIR + ".blend", FIR + ".tree.ron", TABLE)
        self.assertEqual(build.coherence(trio.cert, trio.view, trio.ron, digest), [])
        self.assertEqual(trio.cert["chains"], {})


# ── mutants: the node contract ───────────────────────────────────────────────────────────────────

class ContractMutants(unittest.TestCase):
    def setUp(self):
        self.js = copy.deepcopy(tracked().js)

    def failures(self):
        return build.contract_failures(self.js, "fir_1")

    def assertNamed(self, *words):
        failures = self.failures()
        self.assertTrue(any(all(w in f for w in words) for f in failures), (words, failures))

    def test_a_node_without_extras_is_refused(self):
        del node(self.js, "cap_top_03")["extras"]
        self.assertNamed("cap_top_03", "extras")

    def test_a_segment_reparented_to_the_root_breaks_the_chain(self):
        root, t0, t1 = node(self.js, "fir_1"), node(self.js, "trunk_00"), node(self.js, "trunk_01")
        index = self.js["nodes"].index(t1)
        t0["children"].remove(index)
        root["children"].append(index)
        self.assertNamed("trunk 1", "chain")

    def test_a_rotated_segment_is_refused(self):
        node(self.js, "trunk_02")["rotation"] = [0.0, 0.0, 0.3826834, 0.9238795]
        self.assertNamed("trunk_02", "rotation")

    def test_a_tilted_root_is_refused(self):
        node(self.js, "fir_1")["rotation"] = [0.0, 0.0, 0.3826834, 0.9238795]
        self.assertNamed("fir_1", "rotation")

    def test_a_scaled_root_is_refused(self):
        node(self.js, "fir_1")["scale"] = [1.0, 2.0, 1.0]
        self.assertNamed("fir_1", "scale")

    def test_a_displaced_root_is_refused(self):
        node(self.js, "fir_1")["translation"] = [0.0, 0.5, 0.0]
        self.assertNamed("root", "origin")

    def test_a_raised_first_segment_is_refused(self):
        node(self.js, "trunk_00")["translation"] = [0.0, 0.5, 0.0]
        self.assertNamed("trunk 0", "root")

    def test_a_non_finite_translation_is_refused(self):
        node(self.js, "trunk_05")["translation"] = [0.0, float("nan"), 0.0]
        self.assertNamed("trunk_05", "finite")

    def test_a_short_rotation_array_is_refused(self):
        node(self.js, "trunk_05")["rotation"] = [0.0, 0.0, 0.0]
        self.assertNamed("trunk_05", "4 finite")

    def test_a_matrix_is_refused(self):
        node(self.js, "trunk_05")["matrix"] = [1.0, 0, 0, 0, 0, 1.0, 0, 0, 0, 0, 1.0, 0, 0, 0, 0, 1.0]
        self.assertNamed("trunk_05", "matrix")

    def test_an_origin_off_the_axis_is_refused(self):
        node(self.js, "trunk_02")["translation"] = [0.1, node(self.js, "trunk_02")["translation"][1], 0.0]
        self.assertNamed("trunk_02", "axis")

    def test_a_missing_cap_is_refused(self):
        node(self.js, "cap_bottom_05")["extras"]["tree_part"] = "canopy"
        self.assertNamed("trunk 5 has no cap_bottom")

    def test_two_nodes_claiming_one_part_are_refused(self):
        node(self.js, "cap_top_05")["extras"]["tree_segment"] = 4
        self.assertNamed("two nodes")

    def test_a_root_of_another_name_is_refused(self):
        node(self.js, "fir_1")["name"] = "spruce_1"
        self.assertNamed("root node(s)", "'fir_1'")

    def test_a_part_outside_the_four_is_refused(self):
        node(self.js, "canopy_03")["extras"]["tree_part"] = "leaves"
        self.assertNamed("canopy_03", "tree_part 'leaves'")

    def test_a_negative_segment_index_is_refused(self):
        node(self.js, "canopy_03")["extras"]["tree_segment"] = -1
        self.assertNamed("canopy_03", "tree_segment -1")

    def test_a_node_without_a_mesh_is_refused(self):
        del node(self.js, "cap_top_03")["mesh"]
        self.assertNamed("cap_top_03", "references no mesh")

    def test_a_document_without_a_trunk_is_refused(self):
        for n in self.js["nodes"]:
            if (n.get("extras") or {}).get("tree_part") == "trunk":
                n["extras"]["tree_part"] = "canopy"
                n["extras"]["tree_segment"] += 100
        self.assertNamed("no trunk node")

    def test_a_gap_in_the_trunk_numbering_is_refused(self):
        node(self.js, "trunk_14")["extras"]["tree_segment"] = 20
        self.assertNamed("without a gap")

    def test_a_segment_not_above_its_parent_is_refused(self):
        node(self.js, "trunk_03")["translation"] = [0.0, 0.0, 0.0]
        self.assertNamed("trunk 3", "up its parent")

    def test_a_canopy_reparented_off_its_segment_is_refused(self):
        root, t3 = node(self.js, "fir_1"), node(self.js, "trunk_03")
        index = self.js["nodes"].index(node(self.js, "canopy_03"))
        t3["children"].remove(index)
        root["children"].append(index)
        self.assertNamed("canopy 3 is not a child of trunk 3")

    def test_a_part_naming_a_segment_the_trunk_lacks_is_refused(self):
        node(self.js, "canopy_03")["extras"]["tree_segment"] = 40
        self.assertNamed("canopy node for segment 40", "does not have")

    def test_a_raised_bottom_cap_is_refused(self):
        node(self.js, "cap_bottom_05")["translation"] = [0.0, 0.1, 0.0]
        self.assertNamed("cap_bottom 5 is not at its segment's origin")


# ── mutants: the ron, the species row ────────────────────────────────────────────────────────────

class RonMutants(unittest.TestCase):
    def setUp(self):
        trio = tracked()
        self.js, self.tree = trio.js, copy.deepcopy(trio.tree)
        self.table = ron.load(TABLE)
        self.registry = table_substances()

    def assertNamed(self, *words):
        failures = build.ron_failures(self.tree, self.js, self.table, self.registry)
        self.assertTrue(any(all(w in f for w in words) for f in failures), (words, failures))

    def test_a_height_the_chain_does_not_place_is_refused(self):
        self.tree["segments"][3]["height_m"] += 0.01
        self.assertNamed("trunk 4")

    def test_radii_that_do_not_meet_are_refused(self):
        self.tree["segments"][2]["r_top_m"] += 0.01
        self.assertNamed("one disc")

    def test_dbh_m_is_refused(self):
        self.tree["dbh_m"] = 0.4
        self.assertNamed("dbh_m", "A6")

    def test_a_species_outside_the_table_is_refused(self):
        self.tree["species"] = "baobab"
        self.assertNamed("baobab")

    def test_a_segment_count_the_glb_lacks_is_refused(self):
        self.tree["segments"].append(dict(self.tree["segments"][-1]))
        self.tree["segments"][-1]["r_bottom_m"] = self.tree["segments"][-2]["r_top_m"]
        self.assertNamed("trunk node")

    def test_more_segments_than_the_wire_word_holds_are_refused(self):
        last = self.tree["segments"][-1]
        while len(self.tree["segments"]) <= build.MAX_SEGMENTS:
            self.tree["segments"].append(dict(last, r_bottom_m=last["r_top_m"]))
        self.assertNamed("256 segments", "A3")

    def test_a_zero_hp_is_refused(self):
        self.table[self.tree["species"]]["hp"] = (0, ron.Call("Unknown", ("placeholder",)))
        self.assertNamed("hp is 0")

    def test_a_negative_fell_k_is_refused(self):
        self.table[self.tree["species"]]["fell_k"] = (-2.0e5, ron.Call("Unknown", ("placeholder",)))
        self.assertNamed("fell_k is -200000.0")

    def test_a_substance_the_registry_lacks_is_refused(self):
        self.registry = {"RHA"}
        self.assertNamed("not a key of the substance registry", "RHA")

    def test_a_substance_that_is_no_name_is_refused(self):
        self.table[self.tree["species"]]["substance"] = 7
        self.assertNamed("substance 7")

    def test_a_document_that_is_no_tree_asset_is_refused(self):
        self.tree["__struct__"] = "TankSpec"
        self.assertNamed("not a TreeAsset")

    def test_a_missing_field_is_refused(self):
        del self.tree["segments"]
        self.assertNamed("TreeAsset has no segments")

    def test_an_empty_trunk_is_refused(self):
        self.tree["segments"] = []
        self.assertNamed("segments is empty")

    def test_a_row_that_is_no_segment_is_refused(self):
        self.tree["segments"][2] = (1.8, 0.2, 0.19)
        self.assertNamed("segment 2 is not a Segment")

    def test_a_non_positive_dimension_is_refused(self):
        self.tree["segments"][2]["r_top_m"] = 0.0
        self.assertNamed("segment 2 r_top_m is 0.0")

    def test_a_cap_top_the_ron_does_not_place_is_refused(self):
        self.js = copy.deepcopy(self.js)
        node(self.js, "cap_top_02")["translation"][1] += 0.01
        self.assertNamed("cap_top 2 sits")

    def test_a_recorded_height_the_segments_do_not_sum_to_is_refused(self):
        self.js = copy.deepcopy(self.js)
        node(self.js, "fir_1")["extras"]["tree_height_m"] = 30.0
        self.assertNamed("root records a 30.0 m tree")


# ── mutants: the geometry ────────────────────────────────────────────────────────────────────────

class GeometryMutants(unittest.TestCase):
    def setUp(self):
        trio = tracked()
        self.js, self.binary = copy.deepcopy(trio.js), trio.binary
        self.segments = copy.deepcopy(trio.tree["segments"])

    def assertNamed(self, *words):
        failures = build.geometry_failures(self.js, self.binary, self.segments)
        self.assertTrue(any(all(w in f for w in words) for f in failures), (words, failures))

    def test_radii_ten_times_the_mesh_are_refused(self):
        """The review's mutant: a ron whose radii all moved together passes continuity and every
        node-offset check; only the stored rings refuse it."""
        for segment in self.segments:
            segment["r_bottom_m"] *= 10.0
            segment["r_top_m"] *= 10.0
        self.assertNamed("trunk 0's bottom ring", "off the ron's r_bottom_m")
        self.assertNamed("cap_top 3 reaches")

    def test_a_vertex_999_m_away_is_refused(self):
        self.binary = rewritten(self.js, self.binary, "trunk_03", "POSITION", 0, (999.0, 0.0, 0.0))
        self.assertNamed("trunk_03", "999.000 m from its origin")
        self.assertNamed("trunk 3's bottom ring")

    def test_a_vertex_off_the_ring_plane_is_refused(self):
        self.binary = rewritten(self.js, self.binary, "trunk_03", "POSITION", 0, (0.2, 0.9, 0.0))
        self.assertNamed("trunk 3", "off its ring planes")

    def test_a_non_finite_vertex_is_refused(self):
        self.binary = rewritten(self.js, self.binary, "canopy_03", "POSITION", 4,
                                (float("nan"), 0.0, 0.0))
        self.assertNamed("canopy_03", "not finite")

    def test_an_empty_mesh_is_refused(self):
        primitive = self.js["meshes"][node(self.js, "cap_top_02")["mesh"]]["primitives"][0]
        self.js["accessors"][primitive["attributes"]["POSITION"]]["count"] = 0
        self.js["accessors"][primitive["indices"]]["count"] = 0
        self.assertNamed("cap_top_02", "empty")

    def test_a_mesh_the_glb_lacks_is_refused(self):
        node(self.js, "cap_top_02")["mesh"] = 999
        self.assertNamed("cap_top_02", "references mesh 999")

    def test_an_index_past_the_vertices_is_refused(self):
        primitive = self.js["meshes"][node(self.js, "trunk_01")["mesh"]]["primitives"][0]
        acc = self.js["accessors"][primitive["indices"]]
        view = self.js["bufferViews"][acc["bufferView"]]
        out = bytearray(self.binary)
        struct.pack_into("<H", out, view["byteOffset"] + acc.get("byteOffset", 0), 60000)
        self.binary = bytes(out)
        self.assertNamed("trunk_01", "past its")

    def test_a_missing_bin_chunk_is_refused(self):
        js, binary = build.parse_glb(glb_bytes(self.js, b""))
        self.assertEqual(binary, b"")
        failures = build.geometry_failures(js, binary, self.segments)
        self.assertTrue(any("BIN chunk holds 0" in f for f in failures), failures)

    def test_a_tilted_cap_is_refused(self):
        self.binary = rewritten(self.js, self.binary, "cap_bottom_04", "POSITION", 3,
                                (0.1, 0.05, 0.0))
        self.assertNamed("cap_bottom 4 is not flat")

    def test_a_cap_facing_the_wrong_way_is_refused(self):
        for vertex in range(13):
            self.binary = rewritten(self.js, self.binary, "cap_top_01", "NORMAL", vertex,
                                    (0.0, -1.0, 0.0))
        self.assertNamed("cap_top 1 does not face +Y")

    def test_a_mesh_without_a_primitive_is_refused(self):
        self.js["meshes"][node(self.js, "cap_top_02")["mesh"]]["primitives"] = []
        self.assertNamed("cap_top_02", "no primitive")

    def test_a_primitive_that_is_not_triangles_is_refused(self):
        self.js["meshes"][node(self.js, "cap_top_02")["mesh"]]["primitives"][0]["mode"] = 1
        self.assertNamed("cap_top_02", "mode 1")

    def test_a_primitive_without_indices_is_refused(self):
        del self.js["meshes"][node(self.js, "cap_top_02")["mesh"]]["primitives"][0]["indices"]
        self.assertNamed("cap_top_02", "no POSITION or no indices")

    def test_a_position_that_is_not_float_vec3_is_refused(self):
        primitive = self.js["meshes"][node(self.js, "cap_top_02")["mesh"]]["primitives"][0]
        self.js["accessors"][primitive["attributes"]["POSITION"]]["type"] = "VEC2"
        self.assertNamed("cap_top_02", "VEC3 of float32")

    def test_signed_indices_are_refused(self):
        primitive = self.js["meshes"][node(self.js, "cap_top_02")["mesh"]]["primitives"][0]
        self.js["accessors"][primitive["indices"]]["componentType"] = 5122
        self.assertNamed("cap_top_02", "not unsigned")

    def test_indices_that_are_no_whole_triangles_are_refused(self):
        primitive = self.js["meshes"][node(self.js, "cap_top_02")["mesh"]]["primitives"][0]
        self.js["accessors"][primitive["indices"]]["count"] -= 1
        self.assertNamed("cap_top_02", "whole number of triangles")

    def test_an_accessor_past_the_bin_chunk_is_refused(self):
        primitive = self.js["meshes"][node(self.js, "cap_top_02")["mesh"]]["primitives"][0]
        self.js["bufferViews"][self.js["accessors"][primitive["indices"]]["bufferView"]]["byteOffset"] += 1 << 20
        self.assertNamed("cap_top_02", "BIN chunk holds")

    def test_a_cap_without_normals_is_refused(self):
        del self.js["meshes"][node(self.js, "cap_top_02")["mesh"]]["primitives"][0]["attributes"]["NORMAL"]
        self.assertNamed("cap_top 2 has no normals")

    def test_a_trunk_with_one_ring_is_refused(self):
        """Trunk 2's top ring folded onto its bottom ring: nothing strays, the bottom ring is
        over-full and still right, the top ring has no vertex at all."""
        points = positions_of(self.js, self.binary, "trunk_02")
        bottom = [p for p in points if abs(p[1]) < 1e-6]
        for vertex, (x, y, z) in enumerate(points):
            if abs(y) >= 1e-6:
                self.binary = rewritten(self.js, self.binary, "trunk_02", "POSITION", vertex,
                                        bottom[vertex % len(bottom)])
        self.assertNamed("trunk 2's top ring has 0 vertex(es)")

    def test_a_boundary_loop_that_does_not_meet_is_refused(self):
        """Trunk 4's bottom ring turned by half a side: same plane, same radius, no vertex twin
        on trunk 3's top ring — the continuity the ron states but only the mesh can break."""
        points = positions_of(self.js, self.binary, "trunk_04")
        turn = math.pi / 12.0
        for vertex, (x, y, z) in enumerate(points):
            if abs(y) < 1e-6:
                self.binary = rewritten(self.js, self.binary, "trunk_04", "POSITION", vertex,
                                        (x * math.cos(turn) - z * math.sin(turn), y,
                                         x * math.sin(turn) + z * math.cos(turn)))
        self.assertNamed("trunk 3's top ring and trunk 4's bottom ring are not one loop")


class ContainerMutants(unittest.TestCase):
    def setUp(self):
        self.view = tracked().view

    def assertRefused(self, blob, *words):
        with self.assertRaises(ValueError) as caught:
            build.parse_glb(blob)
        self.assertTrue(all(w in str(caught.exception) for w in words), caught.exception)

    def test_trailing_bytes_are_refused(self):
        self.assertRefused(self.view + b"junk", "declares")

    def test_a_declared_length_that_is_not_the_file_s_is_refused(self):
        self.assertRefused(self.view[:8] + struct.pack("<I", len(self.view) + 4) + self.view[12:],
                           "declares", "and the file is")

    def test_a_truncated_file_is_refused(self):
        self.assertRefused(self.view[:-100], "declares")

    def test_a_third_chunk_is_refused(self):
        js, binary = build.parse_glb(self.view)
        blob = glb_bytes(js, binary) + struct.pack("<II", 4, 0x004E4942) + b"\0\0\0\0"
        blob = blob[:8] + struct.pack("<I", len(blob)) + blob[12:]
        self.assertRefused(blob, "BIN chunk at chunk 2")

    def test_a_well_framed_glb_round_trips(self):
        js, binary = build.parse_glb(self.view)
        self.assertEqual(build.parse_glb(glb_bytes(js, binary)), (js, binary))


# ── mutants: the certificate ─────────────────────────────────────────────────────────────────────

class CertificateMutants(unittest.TestCase):
    def setUp(self):
        trio = tracked()
        self.view, self.ron, self.cert = trio.view, trio.ron, trio.cert

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

    def test_a_missing_field_is_refused(self):
        cert = {k: v for k, v in self.cert.items() if k != "sim_ron_sha"}
        self.assertTrue(any("has no sim_ron_sha" in f
                            for f in build.coherence(cert, self.view, self.ron, None)))

    def test_a_mesh_count_the_glb_does_not_hold_is_refused(self):
        cert = dict(self.cert, mesh_count=self.cert["mesh_count"] + 1)
        self.assertTrue(any("mesh_count is" in f
                            for f in build.coherence(cert, self.view, self.ron, None)))

    def test_an_unreadable_glb_is_named(self):
        view = self.view[:-4]
        cert = dict(self.cert, view_glb_sha=build.sha256_bytes(view))
        self.assertTrue(any("cannot be read" in f
                            for f in build.coherence(cert, view, self.ron, None)))


# ── generation, export and the verifier, in Blender ──────────────────────────────────────────────

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

    def test_a_generated_tree_satisfies_the_three_contracts(self):
        with tempfile.TemporaryDirectory() as work:
            view, ron_blob = self.generate(os.path.join(work, "pine_3.blend"), "scots_pine", 3)
        js, binary = build.parse_glb(view)
        tree = ron.parse(ron_blob.decode())
        self.assertEqual(build.contract_failures(js, "pine_3"), [])
        self.assertEqual(build.ron_failures(tree, js, ron.load(TABLE), table_substances()), [])
        self.assertEqual(build.geometry_failures(js, binary, tree["segments"]), [])


@unittest.skipIf(BLENDER.mismatch, "the pinned Blender is not here: {}".format(BLENDER))
class Verifier(unittest.TestCase):
    """`verify` over a copy of the tracked trio, with a registry that holds the table's
    substances so the cases are about the re-export and nothing else."""

    def setUp(self):
        self.work = tempfile.mkdtemp(prefix="prop-verify-")
        self.addCleanup(shutil.rmtree, self.work)
        self.blend = os.path.join(self.work, "fir_1.blend")
        for suffix in (".blend", ".glb", ".tree.ron", ".lod.json"):
            shutil.copyfile(FIR + suffix, os.path.join(self.work, "fir_1" + suffix))
        self.registry = os.path.join(self.work, "registry.ron")
        with open(self.registry, "w", encoding="utf-8") as handle:
            handle.write("(substances: {" + ", ".join(
                '"{}": Hard()'.format(name) for name in sorted(table_substances())) + "})\n")

    def verify(self):
        with tempfile.TemporaryDirectory() as scratch:
            return build.verify(self.blend, ROOT, scratch, BLENDER.binary, self.registry)

    def test_the_tracked_trio_is_its_blend_s_own_export(self):
        self.assertEqual(self.verify(), 0)

    def test_a_tracked_glb_the_blend_does_not_export_is_refused(self):
        """One texture coordinate moved: every contract still holds and the certificate is
        re-cut to match, so only the byte comparison with the re-export can refuse it."""
        trio = tracked()
        primitive = trio.js["meshes"][node(trio.js, "trunk_02")["mesh"]]["primitives"][0]
        acc = trio.js["accessors"][primitive["attributes"]["TEXCOORD_0"]]
        at = trio.js["bufferViews"][acc["bufferView"]]["byteOffset"]
        binary = bytearray(trio.binary)
        struct.pack_into("<ff", binary, at, 0.25, 0.75)
        view = glb_bytes(trio.js, bytes(binary))
        js, _ = build.parse_glb(view)
        self.assertEqual(build.contract_failures(js, "fir_1"), [])
        build.land(os.path.join(self.work, "fir_1.glb"), view)
        cert = build.certificate(build.blend_digest(self.blend, self.blend[:-6] + ".tree.ron", TABLE),
                                 view, trio.ron, len(js["meshes"]))
        build.land(os.path.join(self.work, "fir_1.lod.json"), build.certificate_bytes(cert))
        with self.assertRaises(build.Refused) as caught:
            self.verify()
        self.assertEqual(caught.exception.stage, "compare")
        self.assertTrue(any("first divergence at byte" in f for f in caught.exception.failures))


if __name__ == "__main__":
    unittest.main()
