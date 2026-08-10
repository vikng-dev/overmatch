"""The three derivation laws, mutated one clause at a time.

    python3 scripts/tank/test_glb_ktx2.py

The corpus is a REAL pair: the fixture tank of `.agents/blender/fixture_tank.py` exported by the
real Blender and baked by the real `basisu`, once, into a raw candidate and the baked document that
came out of it. Every case then performs one operation of byte or JSON surgery on a copy of that
pair and asserts the clause it broke — and, where the neighbours would be ambiguous, only that
clause — comes back as an ERROR naming what was measured.

Surgery rather than defective fixtures, because these laws are about a DERIVATION: a fixture cannot
carry a dropped mip level or a rewritten accessor offset, only a bake that went wrong can, and
inventing a bad bake would prove the invention rather than the law.

The door's own suite (`scripts/tank/test_asset_door.py`) covers the chain these laws sit in; this
one covers what the laws say.
"""

import copy
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import asset_door  # noqa: E402
import glb_ktx2  # noqa: E402
import report  # noqa: E402
import toolchain  # noqa: E402
from report import Severity  # noqa: E402

ROOT = asset_door.repo_root()
FIXTURE = os.path.join(ROOT, ".agents", "blender", "fixture_tank.py")
SOURCE_PASS = os.path.join(ROOT, asset_door.SOURCE_PASS)
ENCODE = os.path.join(ROOT, asset_door.ENCODE)

_WORK = tempfile.mkdtemp(prefix="glb-ktx2-test-")

#: The one built pair, cut once: a Blender launch and three `basisu` runs.
_PAIR = {}

#: Where a KTX2 field the surgery reaches sits inside a payload.
LEVEL_COUNT = 12 + 7 * 4
SUPERCOMPRESSION = 12 + 8 * 4
COLOUR_MODEL = 12
TRANSFER = 14


def pair():
    """`(raw document, baked document)`, each as `(json, bin)`, built once by the real chain."""
    if _PAIR:
        return _PAIR["raw"], _PAIR["baked"]
    built = subprocess.run(
        [toolchain.blender().binary, "--background", "--factory-startup",
         "--python", FIXTURE, "--", "--dir", _WORK],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    assert built.returncode == 0, "the fixture builder failed:\n{}".format(built.stdout)

    blend = os.path.join(_WORK, "assets", "testbed", "testbed.blend")
    spec = os.path.splitext(blend)[0] + ".tank.ron"
    canon = os.path.join(_WORK, "canon.json")
    with open(canon, "wb") as handle:
        written = subprocess.run(
            ["cargo", "run", "--quiet", "--bin", "asset_verify", "--", "--canon", spec],
            cwd=ROOT, stdout=handle,
        )
    assert written.returncode == 0, "the canon file could not be written"

    raw = os.path.join(_WORK, "raw.glb")
    exported = subprocess.run(
        [toolchain.blender().binary, "--background", "--factory-startup", blend,
         "--python", SOURCE_PASS, "--", "--mode", "export", "--spec", spec,
         "--glb", os.path.splitext(blend)[0] + ".glb", "--canon", canon, "--raw", raw],
        cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    assert exported.returncode == 0, "the raw export failed:\n{}".format(exported.stdout)

    baked = os.path.join(_WORK, "baked.glb")
    encoded = subprocess.run(
        [ENCODE, raw, baked], cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    assert encoded.returncode == 0, "the bake failed:\n{}".format(encoded.stdout)

    _PAIR["raw"] = glb_ktx2.read_glb(raw)
    _PAIR["baked"] = glb_ktx2.read_glb(baked)
    _PAIR["paths"] = (raw, baked)
    return _PAIR["raw"], _PAIR["baked"]


class Document:
    """One document open for surgery: its JSON, its BIN chunk, and the payload of any image."""

    def __init__(self, source):
        js, bin_ = source
        self.js = copy.deepcopy(js)
        self.bin = bytearray(bin_)

    @property
    def parts(self):
        return (self.js, bytes(self.bin))

    def view(self, index):
        """(first byte, length) of image `index`'s payload inside the BIN chunk."""
        view = self.js["bufferViews"][self.js["images"][index]["bufferView"]]
        return (view.get("byteOffset", 0), view["byteLength"])

    def ktx2(self, index):
        return glb_ktx2.parse_ktx2(glb_ktx2.payloads_in_memory(self.js, bytes(self.bin))(index))

    def patch(self, index, offset, data):
        """Overwrite bytes inside image `index`'s payload."""
        start, _length = self.view(index)
        self.bin[start + offset : start + offset + len(data)] = data

    def patch_u32(self, index, offset, value):
        self.patch(index, offset, struct.pack("<I", value))

    def patch_u64(self, index, offset, value):
        self.patch(index, offset, struct.pack("<Q", value))

    def level(self, index, level, field, value):
        """Rewrite one field of one level index record: 0 offset, 1 length, 2 uncompressed."""
        self.patch_u64(index, glb_ktx2.KTX2_HEADER + 24 * level + 8 * field, value)

    def dfd(self, index, offset, value):
        self.patch(index, self.ktx2(index).dfd_offset + offset, bytes([value]))

    def findings(self, raw_sizes=None):
        """Every law this document answers on its own."""
        js, bin_ = self.parts
        return glb_ktx2.document_findings(
            js, glb_ktx2.payloads_in_memory(js, bin_), "baked.glb", raw_sizes
        )


def baked():
    return Document(pair()[1])


def raw():
    return Document(pair()[0])


def derivation(before, after):
    """`D.STRUCTURAL_DERIVATION` over a raw document and the baked one it is claimed to produce."""
    a, abin = before.parts
    b, bbin = after.parts
    return glb_ktx2.derivation_findings(a, abin, b, bbin, "baked.glb")


#: glTF component and element sizes, restated here so a case can locate a byte without asking the
#: code under test where that byte is.
_COMPONENT = {5120: 1, 5121: 1, 5122: 2, 5123: 2, 5125: 4, 5126: 4}
_ELEMENTS = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}


def last_byte(js, index):
    """The final byte accessor `index` owns, for a tightly packed accessor."""
    accessor = js["accessors"][index]
    element = _COMPONENT[accessor["componentType"]] * _ELEMENTS[accessor["type"]]
    view = js["bufferViews"][accessor["bufferView"]]
    start = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
    return start + accessor["count"] * element - 1


class Synthetic(Document):
    """A document built rather than exported, for a shape this pipeline cannot produce."""

    def __init__(self, js, bin_):  # noqa: PLW0231 — built, not copied from a parsed pair
        self.js = js
        self.bin = bytearray(bin_)


def strided(payload):
    """One glb holding one accessor of three VEC2 floats interleaved into a 12-byte stride, so its
    last element ends past where the same accessor packed tightly would."""
    return Synthetic({
        "asset": {"version": "2.0"},
        "extensionsUsed": [glb_ktx2.BASISU],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC2"}],
        "bufferViews": [{"buffer": 0, "byteOffset": 4, "byteLength": len(payload),
                         "byteStride": 12}],
        "buffers": [{"byteLength": 4 + len(payload)}],
    }, b"\0" * 4 + payload)


def normal_mapped(js):
    """(mesh index, primitive index) of the one primitive the fixture normal-maps."""
    for index, mesh in enumerate(js["meshes"]):
        for number, primitive in enumerate(mesh["primitives"]):
            material = primitive.get("material")
            if material is None:
                continue
            if any(role == "normal" for _, role, _ in glb_ktx2.texture_slots(js["materials"][material])):
                return index, number
    raise AssertionError("the fixture carries no normal-mapped primitive")


class Fires(unittest.TestCase):
    """One assertion for every case: the law fired, as an error, saying what it measured."""

    def fires(self, findings, check_id, *phrases):
        hits = [finding for finding in findings if finding.check.id == check_id]
        self.assertTrue(hits, "{} did not fire; the report held {}".format(
            check_id, sorted({finding.check.id for finding in findings}) or "nothing"
        ))
        for finding in hits:
            self.assertEqual(finding.check.severity, Severity.ERROR)
            self.assertTrue(finding.evidence and finding.repair)
        self.assertEqual(report.exit_code(findings), 1)
        measured = "\n".join("{} {}".format(finding.subject, finding.evidence) for finding in hits)
        for phrase in phrases:
            self.assertIn(phrase, measured, "{} did not measure it: {}".format(check_id, measured))
        return hits

    def only(self, findings, check_id):
        """The same, and nothing else fired — for a clause whose neighbours must stay quiet."""
        self.assertEqual(sorted({finding.check.id for finding in findings}), [check_id])


class CleanPair(Fires):
    """What the real chain produced, unmodified. Every case below is this pair minus one clause, so
    a suite that cannot certify it proves nothing about the mutations."""

    def test_the_baked_document_answers_its_own_laws(self):
        self.assertEqual(baked().findings(), [])

    def test_the_pair_answers_the_derivation_law(self):
        self.assertEqual(derivation(raw(), baked()), [])

    def test_the_baked_images_are_the_raw_ones_encoded(self):
        before, after = raw(), baked()
        sizes = glb_ktx2.raw_sizes(*before.parts)
        self.assertEqual(len(sizes), len(after.js["images"]), "a raw image had no readable header")
        self.assertEqual(after.findings(sizes), [])

    def test_the_command_line_certifies_the_pair(self):
        pair()
        for command in (["verify", _PAIR["paths"][1]], ["diff", *_PAIR["paths"]]):
            result = subprocess.run(
                [sys.executable, os.path.join(ROOT, asset_door.DERIVATION_VERIFIER)] + command,
                cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stdout)
            self.assertIn("0 errors", result.stdout)


class Ktx2Payloads(Fires):
    """`D.KTX2_MIPS` — what a baked image IS."""

    def test_a_dropped_mip_level_is_an_incomplete_chain(self):
        document = baked()
        document.patch_u32(0, LEVEL_COUNT, document.ktx2(0).levels - 1)
        self.fires(document.findings(), "D.KTX2_MIPS", "mip level(s)", "down to 1x1")

    def test_a_truncated_level_index_is_not_a_shorter_chain(self):
        """The payload is cut so the records the header promises are not all there. Reading a
        record off the end of the file is what makes this a parse rather than a claim."""
        document = baked()
        view = document.js["bufferViews"][document.js["images"][0]["bufferView"]]
        view["byteLength"] = glb_ktx2.KTX2_HEADER + 24 * 2
        self.fires(document.findings(), "D.KTX2_MIPS", "level record(s)")

    def test_an_unsupercompressed_payload_refuses(self):
        document = baked()
        document.patch_u32(0, SUPERCOMPRESSION, 0)
        self.fires(document.findings(), "D.KTX2_MIPS", "supercompression scheme is 0")
        self.only(document.findings(), "D.KTX2_MIPS")

    def test_an_etc1s_payload_is_not_uastc(self):
        document = baked()
        document.dfd(0, COLOUR_MODEL, 163)
        self.fires(document.findings(), "D.KTX2_MIPS", "colour model is 163")

    def test_a_colour_map_declared_linear_refuses(self):
        document = baked()
        srgb = next(index for index, role in glb_ktx2.image_roles(document.js, "x")[0].items()
                    if role == "srgb")
        document.dfd(srgb, TRANSFER, glb_ktx2.DFD_LINEAR)
        self.fires(document.findings(), "D.KTX2_MIPS", "role srgb wants transfer function 2")

    def test_a_normal_map_declared_srgb_refuses(self):
        document = baked()
        normal = next(index for index, role in glb_ktx2.image_roles(document.js, "x")[0].items()
                      if role == "normal")
        document.dfd(normal, TRANSFER, glb_ktx2.DFD_SRGB)
        self.fires(document.findings(), "D.KTX2_MIPS", "role normal wants transfer function 1")

    def test_an_empty_level_record_refuses(self):
        document = baked()
        document.level(0, 1, 1, 0)
        self.fires(document.findings(), "D.KTX2_MIPS", "level 1 record is 0 byte(s)")

    def test_a_level_record_that_runs_past_the_payload_refuses(self):
        document = baked()
        document.level(0, 0, 1, 1 << 20)
        self.fires(document.findings(), "D.KTX2_MIPS", "level 0 record spans")

    def test_a_level_record_inside_the_index_refuses(self):
        """Level data starts after the level index, the descriptor and the key/value block; an
        offset before that overlaps the metadata the record itself lives in."""
        document = baked()
        document.level(0, 0, 0, 8)
        self.fires(document.findings(), "D.KTX2_MIPS", "level data starts at")

    def test_a_payload_that_is_not_ktx2_refuses(self):
        document = baked()
        document.patch(0, 0, b"\x89PNG\r\n\x1a\n")
        self.fires(document.findings(), "D.KTX2_MIPS", "do not begin with the KTX2 identifier")

    def test_an_unbaked_image_refuses(self):
        document = baked()
        document.js["images"][0]["mimeType"] = "image/png"
        self.fires(document.findings(), "D.KTX2_MIPS", "mimeType is 'image/png'")

    def test_an_image_of_the_wrong_size_refuses(self):
        """The dimensions come from the document the encoder READ, so only a pair can state this
        clause — a baked image is internally consistent at any size."""
        before, after = raw(), baked()
        sizes = glb_ktx2.raw_sizes(*before.parts)
        sizes[0] = (sizes[0][0] * 2, sizes[0][1])
        self.fires(after.findings(sizes), "D.KTX2_MIPS", "the raw image it was encoded from")
        self.assertEqual(after.findings(), [], "the clause fired without the raw document")


class Roles(Fires):
    """`D.KTX2_MIPS` — one known role per image, and the references that select it."""

    def test_an_image_sampled_as_two_roles_refuses(self):
        document = baked()
        material = document.js["materials"][0]
        material["normalTexture"]["index"] = \
            material["pbrMetallicRoughness"]["baseColorTexture"]["index"]
        self.fires(document.findings(), "D.KTX2_MIPS", "sampled as", "and as")

    def test_an_image_no_material_samples_refuses(self):
        document = baked()
        document.js["materials"][0].pop("normalTexture")
        self.fires(document.findings(), "D.KTX2_MIPS", "no known material slot samples it")

    def test_a_texture_whose_declaration_selects_another_image_refuses(self):
        document = baked()
        declared = document.js["textures"][0]["extensions"][glb_ktx2.BASISU]
        declared["source"] = (declared["source"] + 1) % len(document.js["images"])
        self.fires(document.findings(), "D.KTX2_MIPS", "declaration selects image")

    def test_a_texture_with_no_declaration_refuses(self):
        document = baked()
        document.js["textures"][0]["extensions"] = {}
        self.fires(document.findings(), "D.KTX2_MIPS", "carries no KHR_texture_basisu declaration")

    def test_an_undeclared_extension_refuses(self):
        document = baked()
        document.js["extensionsUsed"] = []
        self.fires(document.findings(), "D.KTX2_MIPS", "extensionsUsed is []")

    def test_a_texture_selecting_no_image_refuses(self):
        """A reference is read twice — once walking the materials for the role, once as the
        declaration — and a texture no material samples is reached only by the second."""
        document = baked()
        document.js["textures"][0]["source"] = len(document.js["images"])
        self.fires(document.findings(), "D.KTX2_MIPS", "texture 0 source is")


class Tangents(Fires):
    """`D.TANGENTS` — the basis a normal map is read in."""

    def test_a_normal_mapped_primitive_without_tangents_refuses(self):
        document = baked()
        mesh, primitive = normal_mapped(document.js)
        document.js["meshes"][mesh]["primitives"][primitive]["attributes"].pop("TANGENT")
        self.fires(document.findings(), "D.TANGENTS", "no TANGENT accessor")
        self.only(document.findings(), "D.TANGENTS")

    def test_a_tangent_stream_shorter_than_the_positions_refuses(self):
        """Present is not enough: a stream that runs out before the vertices do is a normal map
        read against nothing on the vertices past its end."""
        document = baked()
        mesh, primitive = normal_mapped(document.js)
        tangent = document.js["meshes"][mesh]["primitives"][primitive]["attributes"]["TANGENT"]
        document.js["accessors"][tangent]["count"] -= 1
        self.fires(document.findings(), "D.TANGENTS", "against POSITION's")

    def test_a_primitive_with_no_normal_texture_needs_no_tangents(self):
        """The law is about normal maps, not about tangents: a primitive nobody samples a
        direction map on is silent whether it carries them or not."""
        document = baked()
        for mesh in document.js["meshes"]:
            for primitive in mesh["primitives"]:
                primitive["attributes"].pop("TANGENT", None)
        for material in document.js["materials"]:
            material.pop("normalTexture", None)
        self.assertEqual(
            [finding for finding in document.findings() if finding.check.id == "D.TANGENTS"], []
        )


class Structure(Fires):
    """`D.STRUCTURAL_DERIVATION` — everything the bake was not allowed to touch."""

    def test_mutated_non_texture_json_refuses(self):
        after = baked()
        after.js["nodes"][0]["name"] = "Renamed"
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "entry 0 differs")

    def test_a_dropped_collection_entry_refuses(self):
        after = baked()
        after.js["nodes"].pop()
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "entries raw")

    def test_reordered_accessors_refuse(self):
        """Counts alone would pass this: the same accessors, in a different order, mean different
        geometry under every primitive that indexes them."""
        after = baked()
        after.js["accessors"][0], after.js["accessors"][1] = \
            after.js["accessors"][1], after.js["accessors"][0]
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "entry 0 differs")

    def test_a_moved_accessor_payload_refuses(self):
        """The JSON is untouched; the bytes it points at are not. An offset rewrite is exactly the
        operation able to get this wrong, which is why the law is stated over the payload."""
        after = baked()
        view = after.js["accessors"][0]["bufferView"]
        start = after.js["bufferViews"][view].get("byteOffset", 0)
        after.bin[start] ^= 0xFF
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "accessor 0")

    def test_the_end_of_an_accessor_is_compared_too(self):
        """The span is every element, not the first: a stream that diverges only in its last vertex
        is the same law, and the row still has to name the accessor it is about.

        The byte is located from the accessor's own JSON here rather than by asking the code under
        test where its span ends — a case that reads its subject's answer proves nothing.
        """
        after = baked()
        index, end = max(
            ((index, last_byte(after.js, index)) for index in range(len(after.js["accessors"]))),
            key=lambda pair: pair[1],
        )
        after.bin[end] ^= 0xFF
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION",
                   "accessor {}".format(index))

    def test_an_interleaved_accessor_is_measured_by_its_stride(self):
        """A document glTF allows and this pipeline does not write. With elements spread across a
        stride, the byte the accessor's LAST element occupies is past where a packed reading of the
        same accessor stops, so only the stride says which bytes it owns."""
        before, after = strided(b"\x11" * 32), strided(b"\x11" * 31 + b"\x22")
        self.fires(derivation(before, after), "D.STRUCTURAL_DERIVATION", "accessor 0")

    def test_a_baked_image_left_unbaked_refuses(self):
        """The pair's own statement of it: the raw document said PNG, and the baked one still
        does. `D.KTX2_MIPS` says the same thing of the baked document alone."""
        after = baked()
        after.js["images"][0]["mimeType"] = "image/png"
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "and the bake writes")

    def test_a_dropped_texture_refuses(self):
        after = baked()
        after.js["textures"].pop()
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "textures")

    def test_a_rewritten_non_image_bufferview_refuses(self):
        after = baked()
        images = {image["bufferView"] for image in after.js["images"]}
        index = next(i for i in range(len(after.js["bufferViews"])) if i not in images)
        start = after.js["bufferViews"][index].get("byteOffset", 0)
        after.bin[start] ^= 0xFF
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "holds no image")

    def test_a_resized_non_image_bufferview_refuses(self):
        after = baked()
        images = {image["bufferView"] for image in after.js["images"]}
        index = next(i for i in range(len(after.js["bufferViews"])) if i not in images)
        after.js["bufferViews"][index]["byteLength"] -= 4
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "only byteOffset may move")

    def test_a_bufferview_past_the_end_of_the_buffer_refuses(self):
        after = baked()
        after.js["bufferViews"][0]["byteOffset"] = after.js["buffers"][0]["byteLength"]
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "of a buffer declared")

    def test_a_dropped_bufferview_refuses(self):
        after = baked()
        after.js["bufferViews"].pop()
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "raw,")

    def test_an_image_pointed_at_another_bufferview_refuses(self):
        after = baked()
        after.js["images"][0]["bufferView"] = after.js["images"][1]["bufferView"]
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "only the mimeType may move")

    def test_a_dropped_image_refuses(self):
        after = baked()
        after.js["images"].pop()
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "raw,")

    def test_a_swapped_texture_reference_refuses(self):
        after = baked()
        after.js["textures"][0]["source"] = after.js["textures"][1]["source"]
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "the derivation writes")

    def test_a_texture_that_gained_more_than_its_declaration_refuses(self):
        after = baked()
        after.js["textures"][0]["sampler"] = 7
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "the derivation writes")

    def test_an_unbaked_image_payload_refuses(self):
        """The mimeType says KTX2 and the bytes are still the exporter's PNG."""
        after = baked()
        before = raw()
        start, _length = after.view(0)
        after.bin[start : start + 8] = b"\x89PNG\r\n\x1a\n"
        self.fires(derivation(before, after), "D.STRUCTURAL_DERIVATION",
                   "does not begin with the KTX2 identifier")

    def test_an_extra_extension_refuses(self):
        after = baked()
        after.js["extensionsUsed"].append("KHR_materials_unlit")
        self.fires(derivation(raw(), after), "D.STRUCTURAL_DERIVATION", "appends only")

    def test_a_raw_document_that_is_already_baked_refuses(self):
        """The bake reads PNG or JPEG. Pointed at its own output it would re-encode a lossy payload
        a second time, and the mimeType is where that is visible."""
        self.fires(derivation(baked(), baked()), "D.STRUCTURAL_DERIVATION", "and the bake reads")


class Verdict(Fires):
    """The rendering and the exit status, through the command line the door and the hook run."""

    def test_a_broken_baked_document_exits_one_and_names_the_law(self):
        pair()
        document = baked()
        document.patch_u32(0, SUPERCOMPRESSION, 0)
        path = os.path.join(_WORK, "unsupercompressed.glb")
        glb_ktx2.write_glb(path, document.js, bytes(document.bin))
        result = subprocess.run(
            [sys.executable, os.path.join(ROOT, asset_door.DERIVATION_VERIFIER), "verify", path],
            cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("D.KTX2_MIPS error:", result.stdout)
        self.assertIn("law:", result.stdout)
        self.assertIn("repair:", result.stdout)
        self.assertIn("1 error", result.stdout)


def tearDownModule():
    shutil.rmtree(_WORK, ignore_errors=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
