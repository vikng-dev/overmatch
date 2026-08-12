"""The tank build's own laws, on synthetic documents and on the shipped trio.

    python3 scripts/tank/test_build.py

DOCUMENT SURGERY, not a build. Everything here is what `scripts/tank/trio.py` assembles and takes
apart, so the suite runs in seconds and every gate is exercised by MUTATING the thing it gates on
rather than by asserting a happy path twice. One case needs the pinned toolchain and says so: the
claim that a coherent trio REACHES the door is a claim about a stage that then runs.

The fixtures are cut by `document`, which writes real glbs through the derivation's own writer: a
tank-shaped scene with a multi-primitive mesh, a mesh eight nodes share, and two meshes holding one
geometry. A law proven on a document only this suite can make is a law about nothing.
"""

import json
import os
import sys
import tempfile
import unittest

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "lod"))

import build  # noqa: E402
import glb_ktx2  # noqa: E402
import measure  # noqa: E402
import toolchain  # noqa: E402
import trio as TRIO  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
TIGER = os.path.join(ROOT, "assets", "tiger_1", "tiger_1.glb")


# ── fixtures ─────────────────────────────────────────────────────────────────────────────────────

def _box(scale=1.0, shift=0.0):
    """A closed box as (positions, triangles): twelve triangles over eight corners."""
    corners = np.array([
        (x, y, z) for x in (-1.0, 1.0) for y in (-1.0, 1.0) for z in (-1.0, 1.0)
    ], dtype=np.float32) * scale + shift
    faces = np.array([
        (0, 1, 3), (0, 3, 2), (4, 7, 5), (4, 6, 7), (0, 4, 5), (0, 5, 1),
        (2, 3, 7), (2, 7, 6), (0, 2, 6), (0, 6, 4), (1, 5, 7), (1, 7, 3),
    ], dtype=np.uint32)
    return corners, faces


class Writer:
    """Accessors appended to one buffer, the way the exporter writes them: one tightly packed
    bufferView each, no stride, no shared views."""

    def __init__(self):
        self.js = {"asset": {"version": "2.0", "generator": "test"}, "accessors": [],
                   "bufferViews": [], "meshes": [], "nodes": [], "materials": []}
        self.bin = bytearray()

    def accessor(self, array, kind, component, target):
        payload = np.ascontiguousarray(array).tobytes()
        self.bin += b"\0" * (-len(self.bin) % 4)
        self.js["bufferViews"].append({
            "buffer": 0, "byteOffset": len(self.bin), "byteLength": len(payload), "target": target,
        })
        self.bin += payload
        accessor = {
            "bufferView": len(self.js["bufferViews"]) - 1, "componentType": component,
            "count": int(len(array)), "type": kind,
        }
        if kind == "VEC3" and component == 5126:
            accessor["min"] = [float(v) for v in array.min(axis=0)]
            accessor["max"] = [float(v) for v in array.max(axis=0)]
        self.js["accessors"].append(accessor)
        return len(self.js["accessors"]) - 1

    def primitive(self, positions, faces, material=None, extras=True):
        corners = positions[faces.reshape(-1)]
        indices = np.arange(len(corners), dtype=np.uint32)
        attributes = {
            "POSITION": self.accessor(corners, "VEC3", 5126, 34962),
            "NORMAL": self.accessor(
                np.tile(np.array([[0.0, 1.0, 0.0]], dtype=np.float32), (len(corners), 1)),
                "VEC3", 5126, 34962,
            ),
        }
        if extras:
            attributes["TEXCOORD_0"] = self.accessor(
                np.zeros((len(corners), 2), dtype=np.float32), "VEC2", 5126, 34962)
            attributes["TANGENT"] = self.accessor(
                np.tile(np.array([[1.0, 0.0, 0.0, 1.0]], dtype=np.float32), (len(corners), 1)),
                "VEC4", 5126, 34962)
        primitive = {"attributes": attributes,
                     "indices": self.accessor(indices, "SCALAR", 5125, 34963), "mode": 4}
        if material is not None:
            primitive["material"] = material
        return primitive

    def mesh(self, name, primitives):
        self.js["meshes"].append({"name": name, "primitives": primitives})
        return len(self.js["meshes"]) - 1

    def node(self, name, mesh, translation=(0.0, 0.0, 0.0)):
        self.js["nodes"].append({"name": name, "mesh": mesh,
                                 "translation": [float(v) for v in translation]})
        return len(self.js["nodes"]) - 1

    def bytes(self):
        self.js["buffers"] = [{"byteLength": len(self.bin) + (-len(self.bin) % 4)}]
        self.js["scenes"] = [{"nodes": list(range(len(self.js["nodes"])))}]
        self.js["scene"] = 0
        return glb_ktx2.glb_bytes(self.js, bytes(self.bin))


def document():
    """A tank-shaped candidate: a two-primitive hull, a wheel eight nodes share, and two ammo boxes
    of one geometry."""
    writer = Writer()
    writer.js["materials"] = [{"name": "RHA"}, {"name": "Rubber"}, {"name": "Ammunition"}]
    hull, hull_faces = _box(2.0)
    decor, decor_faces = _box(0.5, 3.0)
    writer.mesh("Hull", [writer.primitive(hull, hull_faces, 0),
                         writer.primitive(decor, decor_faces, 1)])
    wheel, wheel_faces = _box(0.4, -2.0)
    writer.mesh("Wheel_L", [writer.primitive(wheel, wheel_faces, 1)])
    ammo, ammo_faces = _box(0.25, 7.0)
    writer.mesh("Ammo_0", [writer.primitive(ammo, ammo_faces, 2)])
    writer.mesh("Ammo_1", [writer.primitive(ammo, ammo_faces, 2)])
    writer.node("Hull", 0)
    for index in range(8):
        writer.node("Wheel_L_{}".format(index), 1, (float(index), 0.0, 0.0))
    writer.node("Ammo_0", 2)
    writer.node("Ammo_1", 3)
    return writer.bytes()


def rung(scale=1.0):
    """A single-mesh, single-primitive glb of the shape `write_level_glb` exports."""
    writer = Writer()
    positions, faces = _box(scale)
    writer.mesh("LOD1_deadbeef", [writer.primitive(positions, faces)])
    writer.node("LOD1_deadbeef", 0)
    return writer.bytes()


def parsed(blob):
    return glb_ktx2.parse_glb(blob, "<test>")


def certificate_over(view_blob, rungs, chains=None):
    """A coherent trio built from a candidate and a rung list, for a case that then breaks one."""
    embedded, mesh_count = TRIO.embed_rungs(view_blob, rungs)
    sim = TRIO.sim_bytes(embedded, mesh_count)
    chains = chains if chains is not None else {
        "Hull#0": {"radius_m": 2.0,
                   "rungs": [{"mesh": name, "deviation_mm": 3.9 * (index + 1)}
                             for index, (name, _) in enumerate(rungs)]},
    }
    cert = TRIO.certificate(
        "blend-digest", TRIO.sha256_bytes(embedded), TRIO.sha256_bytes(sim), mesh_count, chains,
    )
    return embedded, sim, cert


# ── the census ───────────────────────────────────────────────────────────────────────────────────

class CensusLaw(unittest.TestCase):
    """The seam is the PRIMITIVE, and chains dedup by source geometry (ADR 0035)."""

    def setUp(self):
        self.js, self.bin = parsed(document())
        self.rows = TRIO.census(self.js, self.bin)

    def test_every_primitive_is_addressed_including_a_multi_primitive_mesh(self):
        self.assertEqual([row["chain"] for row in self.rows],
                         ["Hull#0", "Hull#1", "Wheel_L#0", "Ammo_0#0", "Ammo_1#0"])
        self.assertEqual([row for row in self.rows if "refusal" in row], [])

    def test_a_mesh_eight_nodes_share_is_one_chain(self):
        wheel = [row for row in self.rows if row["chain"] == "Wheel_L#0"]
        self.assertEqual(len(wheel), 1)
        self.assertEqual(wheel[0]["nodes"], 8)

    def test_two_meshes_of_one_geometry_share_one_chain(self):
        groups = TRIO.chains_by_digest(self.rows)
        self.assertIn(["Ammo_0#0", "Ammo_1#0"], list(groups.values()))
        self.assertEqual(len(groups), 4, "five primitives over four unique geometries")

    def test_duplicate_mesh_names_refuse(self):
        self.js["meshes"][3]["name"] = "Ammo_0"
        with self.assertRaises(TRIO.TrioError) as refusal:
            TRIO.census(self.js, self.bin)
        self.assertIn("Ammo_0", str(refusal.exception))

    def test_a_primitive_this_lane_cannot_certify_carries_its_refusal_and_no_digest(self):
        self.js["meshes"][1]["primitives"][0]["attributes"].pop("TEXCOORD_0")
        rows = TRIO.census(self.js, self.bin)
        wheel = next(row for row in rows if row["chain"] == "Wheel_L#0")
        self.assertIn("missing-attribute", wheel["refusal"])
        self.assertNotIn("digest", wheel)


class TigerCensus(unittest.TestCase):
    """The shipped tank, which is the census the projection was cut against.

    Over the DOOR-OWNED PREFIX of the tracked model: the tiger is a view artifact now, so its rung
    records are meshes too and the source census is what `strip_rungs` returns.
    """

    @unittest.skipUnless(os.path.isfile(TIGER), "the tracked tiger glb is not hydrated")
    def test_every_tiger_primitive_is_eligible_and_the_wheels_are_one_chain_each(self):
        with open(TIGER, "rb") as handle:
            blob = handle.read()
        cert_path = TRIO.paths(TIGER)[2]
        if os.path.isfile(cert_path):
            with open(cert_path, encoding="utf-8") as handle:
                blob = TRIO.strip_rungs(blob, json.load(handle)["mesh_count"])
        js, binary = measure.glb_chunks_from_bytes(blob, TIGER)
        rows = TRIO.census(js, binary)
        self.assertEqual(len(js["meshes"]), 58)
        self.assertEqual(len(rows), 61, "58 meshes, three of them multi-primitive")
        self.assertEqual([row["chain"] for row in rows if "refusal" in row], [])
        for name in ("Wheel_L#0", "Wheel_L#1", "Wheel_R#0", "Wheel_R#1"):
            row = next(r for r in rows if r["chain"] == name)
            self.assertEqual(row["nodes"], 8, "{} is one chain for eight nodes".format(name))


# ── embedding and stripping ──────────────────────────────────────────────────────────────────────

class EmbedLaw(unittest.TestCase):
    """Rung records are APPENDED, so the door-owned prefix survives byte for byte."""

    def setUp(self):
        self.view = document()

    def test_strip_returns_the_candidate_byte_for_byte(self):
        embedded, mesh_count = TRIO.embed_rungs(self.view, [("Hull#0_LOD1", rung(0.9)),
                                                            ("Hull#0_LOD2", rung(0.8))])
        self.assertEqual(mesh_count, 4)
        self.assertEqual(TRIO.strip_rungs(embedded, mesh_count), self.view)

    def test_a_rung_record_wears_no_material(self):
        embedded, mesh_count = TRIO.embed_rungs(self.view, [("Hull#0_LOD1", rung())])
        js, _ = parsed(embedded)
        self.assertNotIn("material", js["meshes"][mesh_count]["primitives"][0])

    def test_the_packing_is_a_function_of_the_rung_list_alone(self):
        rungs = [("Hull#0_LOD1", rung(0.9)), ("Hull#0_LOD2", rung(0.8))]
        first, _ = TRIO.embed_rungs(self.view, rungs)
        second, _ = TRIO.embed_rungs(self.view, list(rungs))
        self.assertEqual(first, second)
        other, _ = TRIO.embed_rungs(self.view, rungs[::-1])
        self.assertNotEqual(first, other, "a different order is different bytes, deliberately")

    def test_a_scene_node_reaching_into_the_rung_range_refuses(self):
        embedded, mesh_count = TRIO.embed_rungs(self.view, [("Hull#0_LOD1", rung())])
        js, binary = parsed(embedded)
        js["nodes"][0]["mesh"] = mesh_count
        with self.assertRaises(TRIO.TrioError) as refusal:
            TRIO.strip_rungs(glb_ktx2.glb_bytes(js, binary), mesh_count)
        self.assertIn("rung record", str(refusal.exception))

    def test_a_rung_that_is_not_one_mesh_of_one_primitive_refuses(self):
        with self.assertRaises(TRIO.TrioError):
            TRIO.embed_rungs(self.view, [("Hull#0_LOD1", document())])


# ── the sim artifact ─────────────────────────────────────────────────────────────────────────────

class SimLaw(unittest.TestCase):
    """A byte-strip of the certified view artifact, and nothing re-encoded."""

    def setUp(self):
        self.view, self.mesh_count = TRIO.embed_rungs(document(), [("Hull#0_LOD1", rung(0.9))])
        self.sim = TRIO.sim_bytes(self.view, self.mesh_count)

    def test_the_geometry_accessor_bytes_are_the_view_artifacts_own(self):
        view = TRIO.geometry_payloads(self.view, self.mesh_count)
        sim = TRIO.geometry_payloads(self.sim, self.mesh_count)
        self.assertEqual(sorted(view), sorted(sim))
        self.assertTrue(view)
        self.assertEqual(view, sim)

    def test_one_moved_byte_of_view_geometry_separates_the_two_sides(self):
        js, binary = parsed(self.view)
        view = js["bufferViews"][js["accessors"][
            js["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
        ]["bufferView"]]
        binary = bytearray(binary)
        at = view["byteOffset"]
        binary[at] = (binary[at] + 1) % 256
        moved = glb_ktx2.glb_bytes(js, bytes(binary))
        self.assertNotEqual(
            TRIO.geometry_payloads(moved, self.mesh_count),
            TRIO.geometry_payloads(self.sim, self.mesh_count),
        )

    def test_nothing_a_renderer_needs_survives(self):
        js, _ = parsed(self.sim)
        for absent in ("images", "textures", "samplers", "extensionsUsed", "extensionsRequired"):
            self.assertNotIn(absent, js)
        for mesh in js["meshes"]:
            for primitive in mesh["primitives"]:
                self.assertEqual(sorted(primitive["attributes"]), ["NORMAL", "POSITION"])

    def test_a_material_is_a_name_and_membership_is_kept(self):
        js, _ = parsed(self.sim)
        self.assertEqual(js["materials"], [{"name": "RHA"}, {"name": "Rubber"},
                                           {"name": "Ammunition"}])
        self.assertEqual(js["meshes"][0]["primitives"][0]["material"], 0)
        self.assertEqual(js["meshes"][0]["primitives"][1]["material"], 1)

    def test_no_rung_record_survives(self):
        js, _ = parsed(self.sim)
        self.assertEqual(len(js["meshes"]), self.mesh_count)


# ── the certificate ──────────────────────────────────────────────────────────────────────────────

class CertificateLaw(unittest.TestCase):
    """Five fields, nothing derivable, and every claim held against the bytes beside it."""

    def setUp(self):
        self.rungs = [("Hull#0_LOD1", rung(0.9)), ("Hull#0_LOD2", rung(0.8))]
        self.view, self.sim, self.cert = certificate_over(document(), self.rungs)

    def test_the_schema_is_exactly_five_fields(self):
        self.assertEqual(list(self.cert), list(TRIO.CERTIFICATE_FIELDS))

    def test_a_coherent_trio_has_no_failures(self):
        self.assertEqual(
            TRIO.coherence(self.cert, self.view, self.sim, "blend-digest", self.cert["mesh_count"]),
            [],
        )

    def test_deviations_must_strictly_ascend(self):
        with self.assertRaises(TRIO.TrioError):
            TRIO.certificate("d", "v", "s", 1, {"Hull#0": {"radius_m": 1.0, "rungs": [
                {"mesh": "a", "deviation_mm": 4.0}, {"mesh": "b", "deviation_mm": 4.0},
            ]}})

    def test_a_chain_with_no_rungs_is_not_a_row(self):
        cert = TRIO.certificate("d", "v", "s", 1, {"Hull#0": {"radius_m": 1.0, "rungs": []}})
        self.assertEqual(cert["chains"], {})

    def test_a_sixth_field_is_refused(self):
        self.cert["schema_version"] = 2
        self.assertIn("schema_version", " ".join(TRIO.coherence(self.cert, self.view, self.sim)))

    def test_each_of_the_three_digests_is_detected_when_tampered(self):
        for field, expected in (("view_glb_sha", "view_glb_sha"), ("sim_glb_sha", "sim_glb_sha"),
                                ("blend_digest", "blend_digest")):
            with self.subTest(field=field):
                tampered = dict(self.cert, **{field: "0" * 64})
                failures = TRIO.coherence(tampered, self.view, self.sim, "blend-digest")
                self.assertTrue(any(expected in text for text in failures), failures)

    def test_a_moved_binary_is_detected_even_when_the_certificate_is_untouched(self):
        moved = bytearray(self.view)
        moved[-1] = (moved[-1] + 1) % 256
        failures = TRIO.coherence(self.cert, bytes(moved), self.sim, "blend-digest")
        self.assertTrue(any("view_glb_sha" in text for text in failures), failures)

    def test_mesh_count_fires_when_the_source_grows_a_mesh(self):
        failures = TRIO.coherence(
            self.cert, self.view, self.sim, "blend-digest",
            source_mesh_count=self.cert["mesh_count"] + 1,
        )
        self.assertTrue(any("mesh_count" in text for text in failures), failures)

    def test_a_chain_naming_a_rung_the_view_does_not_hold_is_detected(self):
        self.cert["chains"]["Hull#0"]["rungs"][0]["mesh"] = "Hull#0_LOD9"
        failures = TRIO.coherence(self.cert, self.view, self.sim, "blend-digest")
        self.assertTrue(any("Hull#0_LOD9" in text for text in failures), failures)

    def test_a_node_reaching_into_the_rung_range_is_detected(self):
        js, binary = parsed(self.view)
        js["nodes"][0]["mesh"] = self.cert["mesh_count"]
        moved = glb_ktx2.glb_bytes(js, binary)
        cert = dict(self.cert, view_glb_sha=TRIO.sha256_bytes(moved))
        failures = TRIO.coherence(cert, moved, self.sim, "blend-digest")
        self.assertTrue(any("the certificate declares" in text for text in failures), failures)


# ── the staged publish ───────────────────────────────────────────────────────────────────────────

class PublishLaw(unittest.TestCase):
    """Binaries first, the certificate last — so an interruption is loud rather than silent."""

    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="trio-publish-")
        self.glb = os.path.join(self.dir, "testbed.glb")
        self.view, self.sim, self.cert = certificate_over(
            document(), [("Hull#0_LOD1", rung(0.9))]
        )

    def test_the_certificate_lands_after_both_binaries(self):
        order = []

        def watch():
            order.append(sorted(os.listdir(self.dir)))

        TRIO.publish(self.glb, self.view, self.sim, self.cert, after_binaries=watch)
        self.assertEqual(order, [["testbed.glb", "testbed.sim.glb"]])
        self.assertEqual(sorted(os.listdir(self.dir)),
                         ["testbed.glb", "testbed.lod.json", "testbed.sim.glb"])

    def test_an_interrupted_publish_leaves_a_certificate_that_names_bytes_that_are_gone(self):
        TRIO.publish(self.glb, self.view, self.sim, self.cert)
        next_view, next_sim, next_cert = certificate_over(
            document(), [("Hull#0_LOD1", rung(0.7))]
        )
        self.assertNotEqual(next_view, self.view)

        class Interrupted(Exception):
            pass

        def interrupt():
            raise Interrupted

        with self.assertRaises(Interrupted):
            TRIO.publish(self.glb, next_view, next_sim, next_cert, after_binaries=interrupt)
        _, sim_path, cert_path = TRIO.paths(self.glb)
        with open(self.glb, "rb") as handle:
            landed_view = handle.read()
        with open(sim_path, "rb") as handle:
            landed_sim = handle.read()
        with open(cert_path, encoding="utf-8") as handle:
            landed_cert = json.load(handle)
        self.assertEqual(landed_view, next_view, "the binaries landed")
        self.assertEqual(landed_cert, self.cert, "the certificate is the one from before")
        failures = TRIO.coherence(landed_cert, landed_view, landed_sim)
        self.assertTrue(any("view_glb_sha" in text for text in failures), failures)

    def test_the_three_paths_derive_from_the_tracked_model(self):
        self.assertEqual(
            TRIO.paths("/a/tiger_1.glb"),
            ("/a/tiger_1.glb", "/a/tiger_1.sim.glb", "/a/tiger_1.lod.json"),
        )


# ── verify's own rows ────────────────────────────────────────────────────────────────────────────

class VerifyRefusals(unittest.TestCase):
    """`build.py verify`'s two findings, driven through `build.verify` over real bytes.

    The certificate and the sim artifact are answered BEFORE the door's chain is launched, which is
    what makes both drivable without a Blender, an encoder or a cargo build.
    """

    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="trio-verify-")
        self.blend = os.path.join(self.dir, "testbed.blend")
        self.spec = os.path.join(self.dir, "testbed.tank.ron")
        for path, payload in ((self.blend, b"BLENDER-FI"), (self.spec, b"TankSpec()")):
            with open(path, "wb") as handle:
                handle.write(payload)
        self.glb = os.path.join(self.dir, "testbed.glb")
        self.view, self.sim, cert = certificate_over(document(), [("Hull#0_LOD1", rung(0.9))])
        self.cert = dict(cert, blend_digest=build.blend_digest(self.blend, self.spec))
        TRIO.publish(self.glb, self.view, self.sim, self.cert)
        self.cert_path = TRIO.paths(self.glb)[2]

    def land(self, cert):
        with open(self.cert_path, "wb") as handle:
            handle.write(TRIO.certificate_bytes(cert))

    def refusal(self):
        work = tempfile.mkdtemp(prefix="trio-verify-work-")
        with self.assertRaises(build.Refused) as raised:
            build.verify(self.blend, self.spec, self.glb, self.dir, work,
                         toolchain.blender().binary, build.Timeline())
        return raised.exception

    def test_a_coherent_trio_reaches_the_door(self):
        # The stage AFTER the two answered here is the door's chain, which this fixture has no
        # Blender source for — reaching it is the whole claim. The one case in this file that needs
        # the pinned toolchain, because reaching a stage means running it.
        if toolchain.finding(toolchain.blender()) is not None:
            self.skipTest("the pinned Blender is not on this machine")
        work = tempfile.mkdtemp(prefix="trio-verify-work-")
        timeline = build.Timeline()
        try:
            build.verify(self.blend, self.spec, self.glb, self.dir, work,
                         toolchain.blender().binary, timeline)
        except build.Refused as refused:
            self.assertNotIn(refused.stage, ("trio", "certificate", "sim artifact"),
                             "the trio's own stages passed, so the door is where this stops")
        else:
            self.fail("a fixture with no Blender source cannot certify")
        self.assertEqual([name for name, _ in timeline.rows],
                         ["certificate", "sim artifact", "door (export chain)"])

    def test_a_tampered_certificate_digest_refuses_at_the_certificate(self):
        self.land(dict(self.cert, view_glb_sha="0" * 64))
        refused = self.refusal()
        self.assertEqual(refused.stage, "certificate")
        self.assertEqual({row.check.id for row in refused.findings}, {"build.trio-incoherent"})

    def test_a_sim_artifact_that_hashes_right_and_is_not_the_strip_refuses(self):
        forged = TRIO.sim_bytes(self.view, self.cert["mesh_count"] - 1)
        with open(TRIO.paths(self.glb)[1], "wb") as handle:
            handle.write(forged)
        self.land(dict(self.cert, sim_glb_sha=TRIO.sha256_bytes(forged)))
        refused = self.refusal()
        self.assertEqual(refused.stage, "sim artifact")
        self.assertEqual({row.check.id for row in refused.findings}, {"build.sim-not-derived"})

    def test_a_missing_artifact_refuses_before_anything_is_read(self):
        os.remove(TRIO.paths(self.glb)[1])
        refused = self.refusal()
        self.assertEqual(refused.stage, "trio")
        self.assertEqual({row.check.id for row in refused.findings}, {"build.trio-incoherent"})

    def test_a_stale_source_is_caught_by_the_blend_digest(self):
        with open(self.blend, "wb") as handle:
            handle.write(b"BLENDER-FI-EDITED")
        refused = self.refusal()
        self.assertEqual(refused.stage, "certificate")
        self.assertTrue(any("blend_digest" in row.evidence for row in refused.findings))


# ── the work split ───────────────────────────────────────────────────────────────────────────────

class PartitionLaw(unittest.TestCase):
    """A worker pool may not move a byte of the trio."""

    def setUp(self):
        self.digests = ["d{:02d}".format(index) for index in range(17)]
        self.sizes = {digest: (index * 37) % 101 for index, digest in enumerate(self.digests)}

    def test_every_digest_lands_in_exactly_one_bucket_at_every_job_count(self):
        for jobs in (1, 2, 3, 8, 32):
            with self.subTest(jobs=jobs):
                buckets = build.partition(self.digests, self.sizes, jobs)
                flat = [digest for bucket in buckets for digest in bucket]
                self.assertEqual(sorted(flat), sorted(self.digests))
                self.assertEqual(len(flat), len(set(flat)))
                self.assertLessEqual(len(buckets), jobs)

    def test_the_split_is_a_function_of_the_inputs_and_not_of_their_order(self):
        self.assertEqual(build.partition(self.digests, self.sizes, 4),
                         build.partition(self.digests[::-1], self.sizes, 4))

    def test_assembly_reads_the_representative_name_and_never_the_split(self):
        rows = TRIO.census(*parsed(document()))
        groups = TRIO.chains_by_digest(rows)
        cache = {}
        records = {}
        for index, digest in enumerate(sorted(groups)):
            records[digest] = {
                "source": {"origin_radius_m": 1.0 + index},
                "rungs": [{"rung": 1, "glb": digest + ".rung1.glb", "deviation_mm": 3.9,
                           "origin_radius_m": 1.0 + index}],
            }
            cache[digest + ".rung1.glb"] = rung(0.9 - 0.01 * index)

        directory = tempfile.mkdtemp(prefix="trio-assemble-")
        for name, blob in cache.items():
            with open(os.path.join(directory, name), "wb") as handle:
                handle.write(blob)
        first = TRIO.embed_rungs(document(), [])[0]
        view_a, count_a, chains_a = build.assemble(first, rows, records, directory)
        view_b, count_b, chains_b = build.assemble(first, rows[::-1], records, directory)
        self.assertEqual(view_a, view_b)
        self.assertEqual(count_a, count_b)
        self.assertEqual(chains_a, chains_b)
        self.assertEqual(chains_a["Ammo_0#0"]["rungs"], chains_a["Ammo_1#0"]["rungs"],
                         "shared geometry names the same rung records")


# ── the shipped trio ─────────────────────────────────────────────────────────────────────────────

class ShippedTrio(unittest.TestCase):
    """What is in the tree, if a trio has been published into it."""

    def setUp(self):
        self.view_path, self.sim_path, self.cert_path = TRIO.paths(TIGER)
        if not os.path.isfile(self.cert_path):
            self.skipTest("no trio is published yet")
        with open(self.view_path, "rb") as handle:
            self.view = handle.read()
        with open(self.sim_path, "rb") as handle:
            self.sim = handle.read()
        with open(self.cert_path, encoding="utf-8") as handle:
            self.cert = json.load(handle)

    def test_the_trio_is_coherent(self):
        self.assertEqual(TRIO.coherence(self.cert, self.view, self.sim), [])

    def test_the_sim_artifact_re_derives_from_the_view_artifact(self):
        self.assertEqual(TRIO.sim_bytes(self.view, self.cert["mesh_count"]), self.sim)

    def test_the_geometry_bytes_are_one_surface(self):
        self.assertEqual(TRIO.geometry_payloads(self.view, self.cert["mesh_count"]),
                         TRIO.geometry_payloads(self.sim, self.cert["mesh_count"]))

    def test_the_wheel_nodes_resolve_to_one_chain(self):
        js, _ = measure.glb_chunks_from_bytes(self.view, self.view_path)
        nodes = [node for node in js["nodes"]
                 if "mesh" in node and js["meshes"][node["mesh"]].get("name") == "Wheel_L"]
        self.assertEqual(len(nodes), 8)
        self.assertEqual(len({node["mesh"] for node in nodes}), 1, "eight nodes, one mesh")
        self.assertIn("Wheel_L#0", self.cert["chains"])
        # Geometry that ships twice names ONE set of rung records.
        self.assertEqual(self.cert["chains"]["Ammo_L_0#0"]["rungs"],
                         self.cert["chains"]["Ammo_L_1#0"]["rungs"])

    def test_every_rung_record_is_referenced_by_no_scene_node(self):
        js, _ = measure.glb_chunks_from_bytes(self.view, self.view_path)
        self.assertGreater(len(js["meshes"]), self.cert["mesh_count"])
        for node in js["nodes"]:
            self.assertLess(node.get("mesh", 0), self.cert["mesh_count"])
        named = {rung["mesh"] for chain in self.cert["chains"].values() for rung in chain["rungs"]}
        held = {mesh["name"] for mesh in js["meshes"][self.cert["mesh_count"]:]}
        self.assertEqual(named, held, "every embedded rung is named, and every named rung is held")

    def test_the_certificate_carries_no_metre_distance(self):
        text = json.dumps(self.cert)
        for banned in ("switch", "_m\"", "distance"):
            self.assertNotIn(banned, text.replace("radius_m\"", ""))


if __name__ == "__main__":
    unittest.main(verbosity=2)
