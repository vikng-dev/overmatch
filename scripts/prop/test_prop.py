"""Common prop source, geometry and certificate invariants, including refusal mutants.

The shipped prop is fixture input; no expected species dimensions, chunk counts or
emergent physical outcomes are pinned here. Blender integration runs when pinned.
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
from unittest import mock

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

def tracked() -> types.SimpleNamespace:
    """The tracked fir_1 trio and its construction, parsed and raw."""
    view_blob = build.read(FIR + ".glb")
    ron_blob = build.read(FIR + ".tree.ron")
    spec_blob = build.read(build.construction_path(ROOT, "fir_1"))
    js, binary = build.parse_glb(view_blob)
    with open(FIR + ".lod.json", encoding="utf-8") as handle:
        cert = json.load(handle)
    return types.SimpleNamespace(js=js, binary=binary, view=view_blob, ron=ron_blob,
                                 spec=spec_blob, tree=ron.parse(ron_blob.decode("utf-8")),
                                 cert=cert)


def node(js, name):
    return next(n for n in js["nodes"] if n.get("name") == name)



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


class SourceGeometry(unittest.TestCase):
    def test_every_species_has_complete_construction_and_convex_shape_data(self):
        for key, row in ron.load(TABLE).items():
            self.assertEqual(build.species_failures(key, row), [], key)
            self.assertGreater(row["height_m"][0], row["trunk"]["shape"]["breast_height_m"][0])
            for field in ("height_m", "dbh_m"):
                self.assertLess(0, row[field][0])
                self.assertLess(row[field][0], row[field][1])

    def test_source_rows_refuse_unconsumed_fields(self):
        key,row = next(iter(ron.load(TABLE).items()))
        for target in ((), ("trunk",), ("trunk","shape"), ("trunk","cells")):
            mutant = copy.deepcopy(row)
            owner = mutant
            for part in target:
                owner = owner[part]
            owner["unconsumed_fixture_field"] = 1
            self.assertTrue(build.species_failures(key,mutant),target)

    def test_taper_uses_dbh_and_has_concave_radius(self):
        """The construction relies on a convex solid; reversed curvature breaks admission."""
        for row in ron.load(TABLE).values():
            shape = row["trunk"]["shape"]
            height, dbh = row["height_m"][0], row["dbh_m"][1]
            self.assertAlmostEqual(2*plan.radius_at(shape["breast_height_m"][0], height, dbh, shape), dbh)
            radii = [plan.radius_at(height*i/100, height, dbh, shape) for i in range(101)]
            self.assertEqual(radii[-1], 0)
            self.assertTrue(all(b >= (a+c)/2-1e-12 for a,b,c in zip(radii,radii[1:],radii[2:])))

    def test_exterior_approximation_is_bounded_and_has_distinct_side_faces(self):
        """Contact geometry may coarsen the surface only within its declared resolution."""
        for row in ron.load(TABLE).values():
            shape = row["trunk"]["shape"]
            for height in row["height_m"]:
                for dbh in row["dbh_m"]:
                    rings = plan.rings(height, dbh, shape)
                    for low, high in zip(rings,rings[1:]):
                        for step in range(101):
                            t = step/100
                            z = low[0]*(1-t)+high[0]*t
                            r = low[1]*(1-t)+high[1]*t
                            error = plan.radius_at(z,height,dbh,shape)-r
                            self.assertGreaterEqual(error,-1e-12)
                            self.assertLessEqual(error,shape["radial_error_m"][0]+1e-12)
                    corners, faces = plan.solid(rings)
                    planes = build._planes(corners,faces)
                    for n,d in planes:
                        self.assertTrue(all(sum(n[k]*p[k] for k in range(3)) <= d+1e-9 for p in corners))
                    for i, face in enumerate(faces):
                        for j, other in enumerate(faces[:i]):
                            if len(set(face)&set(other)) == 2:
                                dot = sum(a*b for a,b in zip(planes[i][0],planes[j][0]))
                                self.assertLessEqual(dot,1-math.sqrt(2**-23))

    def test_unrepresentable_surface_resolution_is_refused(self):
        shape = copy.deepcopy(next(iter(ron.load(TABLE).values()))["trunk"]["shape"])
        shape["exponent"] = (0.8, shape["exponent"][1])  # Fixture curved source.
        shape["radial_error_m"] = (1e-4, shape["radial_error_m"][1])
        with self.assertRaisesRegex(ValueError,"merge angle"):
            plan.rings(20,0.4,shape)  # Fixture dimensions, not a species target.

    def test_tracked_source_view_construction_and_dressing_are_coherent(self):
        t = tracked()
        table = ron.load(TABLE)
        self.assertEqual(build.contract_failures(t.js,"fir_1"),[])
        self.assertEqual(build.ron_failures(t.tree,t.js,table),[])
        self.assertEqual(build.geometry_failures(t.js,t.binary,t.tree,table),[])
        self.assertEqual(build.identity_failures(t.spec.decode(),t.js,t.binary),[])
        self.assertEqual(len(ron.parse(t.spec.decode())["pieces"]),1,
                         "one source element; only the pattern cuts the interior")
        self.assertEqual(build.coherence(t.cert,t.view,t.ron,t.spec),[])
        dressing = build.dressing(t.view,TABLE,"fir_1")
        self.assertEqual(build.read(build.dressing_path(ROOT,"fir_1")),dressing.encode())

    def test_missing_or_displaced_source_nodes_are_refused(self):
        t = tracked()
        for key,value in (("translation",[0,1,0]),("rotation",[0,0,1,0]),("scale",[2,2,2])):
            js = copy.deepcopy(t.js)
            node(js,"trunk")[key] = value
            self.assertTrue(build.contract_failures(js,"fir_1"), key)
        for change in ("extras","mesh"):
            js = copy.deepcopy(t.js)
            del node(js,"trunk")[change]
            self.assertTrue(build.contract_failures(js,"fir_1"),change)
        js = copy.deepcopy(t.js)
        js["nodes"].append(copy.deepcopy(node(js,"trunk")))
        self.assertTrue(build.contract_failures(js,"fir_1"))

    def test_source_identity_and_dimensions_are_checked_at_the_edge(self):
        t = tracked()
        for field in ("species","seed","height_m","dbh_m"):
            source = copy.deepcopy(t.tree)
            source[field] = "absent" if field == "species" else 0
            self.assertTrue(build.ron_failures(source,t.js,ron.load(TABLE)),field)
            source = copy.deepcopy(t.tree)
            del source[field]
            self.assertTrue(build.ron_failures(source,t.js,ron.load(TABLE)),field)

    def test_missing_shape_data_and_nonconvex_exponents_are_refused(self):
        key,row = next(iter(ron.load(TABLE).items()))
        for field in row["trunk"]["shape"]:
            mutant = copy.deepcopy(row)
            del mutant["trunk"]["shape"][field]
            self.assertTrue(build.species_failures(key,mutant),field)
        mutant = copy.deepcopy(row)
        mutant["trunk"]["shape"]["exponent"] = (2, row["trunk"]["shape"]["exponent"][1])
        self.assertTrue(build.species_failures(key,mutant))

    def test_moved_missing_and_reversed_trunk_geometry_is_refused(self):
        t = tracked()
        for vertex in ((999,0,0),(float("nan"),0,0)):
            binary = rewritten(t.js,t.binary,"trunk","POSITION",0,vertex)
            self.assertTrue(build.geometry_failures(t.js,binary,t.tree,ron.load(TABLE)))
        js = copy.deepcopy(t.js)
        primitive = js["meshes"][node(js,"trunk")["mesh"]]["primitives"][0]
        js["accessors"][primitive["indices"]]["count"] -= 3
        self.assertTrue(build.geometry_failures(js,t.binary,t.tree,ron.load(TABLE)))
        acc = t.js["accessors"][primitive["indices"]]
        fmt,size = build._COMPONENT[acc["componentType"]]
        at = t.js["bufferViews"][acc["bufferView"]].get("byteOffset",0)+acc.get("byteOffset",0)
        binary = bytearray(t.binary)
        indices = struct.unpack_from("<"+fmt*3,binary,at)
        struct.pack_into("<"+fmt*3,binary,at,*reversed(indices))
        self.assertTrue(build.geometry_failures(t.js,bytes(binary),t.tree,ron.load(TABLE)))

    def test_missing_or_moved_foliage_is_refused(self):
        t = tracked()
        text = build.dressing(t.view,TABLE,"fir_1")
        # Removing a card must expose uncovered inspection vertices.
        first = text.index("        (base:")
        end = text.index("\n",first)
        self.assertTrue(build.dressing_failures(text[:first]+text[end:],t.js,t.binary))
        binary = rewritten(t.js,t.binary,"canopy","POSITION",0,(999,0,0))
        self.assertTrue(build.dressing_failures(text,t.js,binary))

    def test_multiple_source_pieces_or_a_moved_corner_are_refused(self):
        t = tracked()
        spec = t.spec.decode()
        piece = spec[spec.index("        ("):spec.rindex("    ],")]
        duplicated = spec[:spec.rindex("    ],")]+piece+spec[spec.rindex("    ],"):]
        self.assertTrue(build.identity_failures(duplicated,t.js,t.binary))
        binary = rewritten(t.js,t.binary,"trunk","POSITION",0,(999,0,0))
        self.assertTrue(build.identity_failures(spec,t.js,binary))


class GltfGeometry(unittest.TestCase):
    def test_invalid_triangle_buffers_are_refused_by_name(self):
        """The source gate must refuse malformed stored geometry before comparing corners."""
        cases = (
            ("empty", lambda js,p: js["accessors"][p["indices"]].update(count=0)),
            ("no primitive", lambda js,p: js["meshes"][node(js,"trunk")["mesh"]].update(primitives=[])),
            ("mode 1", lambda js,p: p.update(mode=1)),
            ("no POSITION or no indices", lambda js,p: p.pop("indices")),
            ("VEC3 of float32", lambda js,p: js["accessors"][p["attributes"]["POSITION"]].update(type="VEC2")),
            ("not unsigned", lambda js,p: js["accessors"][p["indices"]].update(componentType=5122)),
            ("whole number of triangles", lambda js,p: js["accessors"][p["indices"]].update(count=js["accessors"][p["indices"]]["count"]-1)),
            ("BIN chunk holds", lambda js,p: js["bufferViews"][js["accessors"][p["indices"]]["bufferView"]].update(byteOffset=1<<20)),
            ("references mesh", lambda js,p: node(js,"trunk").update(mesh=len(js["meshes"]))),
        )
        t = tracked()
        for expected,mutate in cases:
            js = copy.deepcopy(t.js)
            primitive = js["meshes"][node(js,"trunk")["mesh"]]["primitives"][0]
            mutate(js,primitive)
            failures = build.geometry_failures(js,t.binary,t.tree,ron.load(TABLE))
            self.assertTrue(any(expected in f for f in failures),(expected,failures))
        failures = build.geometry_failures(t.js,b"",t.tree,ron.load(TABLE))
        self.assertTrue(any("BIN chunk holds 0" in f for f in failures),failures)

    def test_out_of_range_indices_and_duplicate_triangles_are_refused(self):
        t = tracked()
        primitive = t.js["meshes"][node(t.js,"trunk")["mesh"]]["primitives"][0]
        acc = t.js["accessors"][primitive["indices"]]
        at = t.js["bufferViews"][acc["bufferView"]].get("byteOffset",0)+acc.get("byteOffset",0)
        fmt,size = build._COMPONENT[acc["componentType"]]
        binary = bytearray(t.binary)
        vertices = t.js["accessors"][primitive["attributes"]["POSITION"]]["count"]
        struct.pack_into("<"+fmt,binary,at,vertices)
        failures = build.geometry_failures(t.js,bytes(binary),t.tree,ron.load(TABLE))
        self.assertTrue(any("past its" in f for f in failures),failures)
        binary = bytearray(t.binary)
        binary[at:at+3*size] = binary[at+3*size:at+6*size]
        failures = build.geometry_failures(t.js,bytes(binary),t.tree,ron.load(TABLE))
        self.assertTrue(any("without duplicates" in f for f in failures),failures)


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
        self.view, self.ron, self.spec, self.cert = trio.view, trio.ron, trio.spec, trio.cert

    def coherence(self, cert, view=None, ron_blob=None, spec=None):
        return build.coherence(cert, view or self.view, ron_blob or self.ron, spec or self.spec)

    def test_an_edited_hash_is_incoherent(self):
        cert = dict(self.cert, view_glb_sha="0" * 64)
        self.assertTrue(any("incoherent" in f for f in self.coherence(cert)))

    def test_an_extra_field_or_a_chain_is_refused(self):
        cert = dict(self.cert, note="lod0")
        self.assertTrue(any("not one of its" in f for f in self.coherence(cert)))
        cert = dict(self.cert, chains={"trunk#0": {"radius_m": 1.0, "rungs": []}})
        self.assertTrue(any("nothing measured" in f for f in self.coherence(cert)))

    def test_a_changed_ron_is_incoherent(self):
        self.assertTrue(any("tree ron" in f for f in self.coherence(self.cert,
                                                                    ron_blob=self.ron + b"\n")))

    def test_a_stale_construction_certificate_is_incoherent(self):
        """A construction rewritten after the certificate was cut: the certificate no longer
        names the bytes the bake reads."""
        failures = self.coherence(self.cert, spec=self.spec + b"\n")
        self.assertTrue(any("construction_sha" in f and "incoherent" in f for f in failures),
                        failures)

    def test_a_missing_field_is_refused(self):
        for field in ("source_ron_sha", "construction_sha"):
            cert = {k: v for k, v in self.cert.items() if k != field}
            self.assertTrue(any("has no " + field in f for f in self.coherence(cert)), field)

    def test_a_mesh_count_the_glb_does_not_hold_is_refused(self):
        cert = dict(self.cert, mesh_count=self.cert["mesh_count"] + 1)
        self.assertTrue(any("mesh_count is" in f for f in self.coherence(cert)))

    def test_an_unreadable_glb_is_named(self):
        view = self.view[:-4]
        cert = dict(self.cert, view_glb_sha=build.sha256_bytes(view))
        self.assertTrue(any("cannot be read" in f for f in self.coherence(cert, view=view)))


# ── mutants: the construction against its view ──────────────────────────────────────────────────

class Publication(unittest.TestCase):
    """Only admitted artifacts publish, including the first build of a construction kind."""

    def setUp(self):
        self.work = tempfile.TemporaryDirectory(prefix="prop-publish-")
        self.addCleanup(self.work.cleanup)
        self.root = self.work.name
        source = os.path.join(self.root, "assets", "trees", "fir_1")
        os.makedirs(source)
        shutil.copyfile(TABLE, os.path.join(self.root, build.SPECIES_TABLE))
        self.blend = os.path.join(source, "fir_1.blend")
        shutil.copyfile(FIR + ".tree.ron", build.paths(self.blend)[1])
        self.view = build.read(FIR + ".glb")

    def test_first_build_creates_the_construction_directory_after_admission(self):
        spec = build.construction_path(self.root, "fir_1")
        self.assertFalse(os.path.exists(os.path.dirname(spec)))
        with mock.patch.object(build, "export", return_value=self.view):
            self.assertEqual(build.build(self.blend, self.root, self.root, "unused", True), 0)
        glb, source, cert = build.paths(self.blend)
        self.assertEqual(build.read(glb), self.view)
        self.assertEqual(build.coherence(json.loads(build.read(cert)), build.read(glb),
                                         build.read(source), build.read(spec)), [])
        self.assertEqual(build.read(build.dressing_path(self.root, "fir_1")),
                         build.dressing(self.view, TABLE, "fir_1").encode())

    def test_invalid_dressing_refuses_lint_and_build_before_publication(self):
        glb = build.paths(self.blend)[0]
        build.land(glb, b"previous admitted artifact")
        for publish in (False, True):
            with mock.patch.object(build, "export", return_value=self.view), \
                 mock.patch.object(build, "dressing", side_effect=build.Refused("dressing", ["bad card"])):
                with self.assertRaises(build.Refused) as caught:
                    build.build(self.blend, self.root, self.root, "unused", publish)
            self.assertEqual(caught.exception.stage, "dressing")
            self.assertEqual(build.read(glb), b"previous admitted artifact")
            self.assertFalse(os.path.exists(build.construction_path(self.root, "fir_1")))

    def test_filesystem_failure_reaches_the_command_as_a_visible_refusal(self):
        with mock.patch.object(build.toolchain, "finding", return_value=None), \
             mock.patch.object(build, "build", side_effect=PermissionError(13, "fixture denied", self.blend)), \
             mock.patch("builtins.print") as printed:
            self.assertEqual(build.run("build", self.blend), 1)
        diagnostic = " ".join(str(call) for call in printed.call_args_list)
        self.assertIn("filesystem", diagnostic)
        self.assertIn("fixture denied", diagnostic)
        self.assertIn("publication may be incomplete", diagnostic)


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
        self.assertEqual(build.ron_failures(tree, js, ron.load(TABLE)), [])
        self.assertEqual(build.geometry_failures(js, binary, tree, ron.load(TABLE)), [])


@unittest.skipIf(BLENDER.mismatch, "the pinned Blender is not here: {}".format(BLENDER))
class Verifier(unittest.TestCase):
    """`verify` over a copy of the tracked trio, so the cases are about the re-export and nothing
    else."""

    def setUp(self):
        self.work = tempfile.mkdtemp(prefix="prop-verify-")
        self.addCleanup(shutil.rmtree, self.work)
        self.blend = os.path.join(self.work, "fir_1.blend")
        for suffix in (".blend", ".glb", ".tree.ron", ".lod.json"):
            shutil.copyfile(FIR + suffix, os.path.join(self.work, "fir_1" + suffix))

    def verify(self):
        with tempfile.TemporaryDirectory() as scratch:
            return build.verify(self.blend, ROOT, scratch, BLENDER.binary)

    def test_the_tracked_trio_is_its_blend_s_own_export(self):
        self.assertEqual(self.verify(), 0)

    def test_a_tracked_glb_the_blend_does_not_export_is_refused(self):
        """One texture coordinate moved: every contract still holds and the certificate is
        re-cut to match, so only the byte comparison with the re-export can refuse it."""
        trio = tracked()
        primitive = trio.js["meshes"][node(trio.js, "trunk")["mesh"]]["primitives"][0]
        acc = trio.js["accessors"][primitive["attributes"]["TEXCOORD_0"]]
        at = trio.js["bufferViews"][acc["bufferView"]]["byteOffset"]
        binary = bytearray(trio.binary)
        struct.pack_into("<ff", binary, at, 0.25, 0.75)
        view = glb_bytes(trio.js, bytes(binary))
        js, _ = build.parse_glb(view)
        self.assertEqual(build.contract_failures(js, "fir_1"), [])
        build.land(os.path.join(self.work, "fir_1.glb"), view)
        cert = build.certificate(view, trio.ron, trio.spec, len(js["meshes"]))
        build.land(os.path.join(self.work, "fir_1.lod.json"), build.certificate_bytes(cert))
        with self.assertRaises(build.Refused) as caught:
            self.verify()
        self.assertEqual(caught.exception.stage, "compare")
        self.assertTrue(any("first divergence at byte" in f for f in caught.exception.failures))


if __name__ == "__main__":
    unittest.main()
