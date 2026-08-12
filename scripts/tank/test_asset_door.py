"""The asset door end to end, on a synthetic trio.

    python3 scripts/tank/test_asset_door.py

Every case runs the REAL door — the real Blender, the real consumer contract, the real encoder —
against the tank `.agents/blender/fixture_tank.py` builds in a temporary directory. Nothing under
`assets/` is read or written, and the fixture is a second vehicle rather than a copy of the first:
a door that only works on the Tiger is not a door.

Three classes of claim are proven here. That the chain CERTIFIES: lint is clean, the chain stages a
mip-baked candidate, a second run is byte-identical, and verify accepts what was staged and landed.
That every refusal LEAVES THE TRACKED GLB ALONE and stages nothing — one case per stage, each
proving the model on disk is the byte-for-byte file it was before the door ran. And what verify's
own comparison says about a tracked model, clause by clause (`ComparisonLaw`), including the one
section it does not compare byte for byte.

THE DOOR PUBLISHES NOTHING (ADR 0035): the export chain ends at a staged candidate, and a case that
needs a tracked model lands one itself with `land` — which is the build's own second step, split so
a case can assert between them. `TheSideDoorIsClosed` is that law held as a fact.

The two stages with no defect a fixture can carry — an encoder that fails, and a consumer contract
that refuses the BAKED bytes after the raw ones passed — are reached by putting a stub earlier on
PATH. What that injects is the stage's exit code, which is exactly the door's contract with it.
"""

import hashlib
import json
import os
import shutil
import stat
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

ROOT = asset_door.repo_root()
DOOR = os.path.join(ROOT, "scripts", "tank", "asset_door.py")
FIXTURE = os.path.join(ROOT, ".agents", "blender", "fixture_tank.py")
SOURCE_PASS = os.path.join(ROOT, asset_door.SOURCE_PASS)

_WORK = tempfile.mkdtemp(prefix="asset-door-test-")

#: One built trio per defect, because building one costs a Blender launch and no case mutates the
#: source. A case that mutates the tracked MODEL takes a copy (`trio`).
_BUILT = {}


def build(defect="none"):
    """The fixture trio for one defect, built once."""
    if defect in _BUILT:
        return _BUILT[defect]
    directory = os.path.join(_WORK, "built-" + defect)
    os.makedirs(directory, exist_ok=True)
    result = subprocess.run(
        [toolchain.blender().binary, "--background", "--factory-startup",
         "--python", FIXTURE, "--", "--dir", directory, "--defect", defect],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    assert result.returncode == 0, "the fixture builder failed:\n{}".format(result.stdout)
    _BUILT[defect] = os.path.join(directory, "assets", "testbed", "testbed.blend")
    return _BUILT[defect]


def trio(name, defect="none"):
    """A private copy of a built trio, for a case that writes to it."""
    directory = os.path.join(_WORK, name)
    shutil.rmtree(directory, ignore_errors=True)
    shutil.copytree(os.path.dirname(os.path.dirname(os.path.dirname(build(defect)))), directory)
    return os.path.join(directory, "assets", "testbed", "testbed.blend")


def door(mode, blend, env=None, *extra):
    """One door invocation, as anyone would run it. Returns `(exit code, everything printed)`."""
    result = subprocess.run(
        [sys.executable, DOOR, mode, blend] + list(extra), cwd=ROOT, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=dict(os.environ, **(env or {})),
    )
    return (result.returncode, result.stdout)


def stage(blend, env=None):
    """The export chain, ending where `scripts/tank/build.py` drives it to: a staged candidate.

    THE DOOR HAS NO EXPORT COMMAND. It publishes nothing (ADR 0035 gives all three artifacts to the
    build), so a case that needs a tracked model asks for the same `chain(..., stage_to=)` the build
    asks for, and lands the result itself with `land`. That is not a test seam: it is the build's
    own two steps, split so a case can assert between them.

    Returns `(exit code, everything printed, the staged path)`.
    """
    # ONE staging path, cleared first: every fixture blend is `testbed.blend`, the cases run in one
    # thread, and each reads its candidate before the next stages one.
    staged = os.path.join(_WORK, "staged", os.path.basename(os.path.splitext(blend)[0]) + ".glb")
    os.makedirs(os.path.dirname(staged), exist_ok=True)
    if os.path.exists(staged):
        os.remove(staged)
    driver = (
        "import sys; sys.path.insert(0, {!r}); import asset_door; "
        "sys.exit(asset_door.door('export', {!r}, stage_to={!r}))"
    ).format(os.path.dirname(DOOR), blend, staged)
    result = subprocess.run(
        [sys.executable, "-c", driver], cwd=ROOT, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=dict(os.environ, **(env or {})),
    )
    return (result.returncode, result.stdout, staged)


def land(blend, env=None):
    """A certified candidate on the tracked path, as the build's publish puts it there.

    Returns what the chain printed. Asserts nothing: a case that wants the refusal calls `stage`.
    """
    code, printed, staged = stage(blend, env)
    assert code == 0, "the chain this case builds on refused:\n{}".format(printed)
    shutil.move(staged, os.path.splitext(blend)[0] + ".glb")
    return printed


def digest(path):
    if not os.path.isfile(path):
        return None
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def shim(name, script):
    """An executable standing in for a program, in its own directory so PATH can be pointed at it."""
    directory = os.path.join(_WORK, "shim-" + name)
    os.makedirs(directory, exist_ok=True)
    path = os.path.join(directory, name)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(script)
    os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


def on_path(*paths):
    """An environment whose PATH finds these shims first."""
    return {"PATH": os.pathsep.join(
        [os.path.dirname(path) for path in paths] + [os.environ.get("PATH", "")]
    )}


class Model:
    """A tracked model open for surgery: its document, its BIN chunk, and the KTX2 payload of any
    image. Written back through the derivation's own writer, so what a case produces is a document
    of the shape the pipeline produces and not a file only this suite can make."""

    def __init__(self, path):
        self.path = path
        self.js, chunk = glb_ktx2.read_glb(path)
        self.bin = bytearray(chunk)

    def write(self):
        glb_ktx2.write_glb(self.path, self.js, bytes(self.bin))
        return self.path

    def start(self, index):
        """Where image `index`'s payload begins in the BIN chunk."""
        view = self.js["bufferViews"][self.js["images"][index]["bufferView"]]
        return view.get("byteOffset", 0)

    def payload(self, index):
        view = self.js["bufferViews"][self.js["images"][index]["bufferView"]]
        at = view.get("byteOffset", 0)
        return bytes(self.bin[at : at + view["byteLength"]])

    def ktx2(self, index):
        return glb_ktx2.parse_ktx2(
            glb_ktx2.payloads_in_memory(self.js, bytes(self.bin))(index)
        )

    def patch(self, at, data):
        self.bin[at : at + len(data)] = data

    def header(self, index, field, value):
        """One of the nine u32 header fields of image `index`'s KTX2 payload, by its position."""
        self.patch(self.start(index) + 12 + 4 * field, struct.pack("<I", value))

    def descriptor(self, index, at, value):
        """One byte of the payload's data-format descriptor."""
        self.patch(self.start(index) + self.ktx2(index).dfd_offset + at, bytes([value]))

    def record(self, index, level, field, value):
        """One field of one level index record: 0 offset, 1 length, 2 uncompressed."""
        self.patch(
            self.start(index) + glb_ktx2.KTX2_HEADER
            + glb_ktx2.KTX2_LEVEL_RECORD * level + 8 * field,
            struct.pack("<Q", value),
        )

    def non_image_view(self):
        """The first bufferView holding no image."""
        images = {image["bufferView"] for image in self.js["images"]}
        return next(index for index in range(len(self.js["bufferViews"])) if index not in images)

    def repack(self, payloads):
        """The document rebuilt with `payloads` (image index -> bytes) in place of its own, every
        offset, length and buffer size recomputed the way `glb_ktx2.cmd_repack` computes them —
        which is what a machine whose encoder emitted other bytes would have written."""
        replaced = {self.js["images"][index]["bufferView"]: data
                    for index, data in payloads.items()}
        out = bytearray()
        for index, view in enumerate(self.js["bufferViews"]):
            data = replaced.get(index, glb_ktx2.view_bytes(self.js, bytes(self.bin), index))
            out += b"\0" * (-len(out) % 4)
            view["byteOffset"] = len(out)
            view["byteLength"] = len(data)
            out += data
        self.js["buffers"][0]["byteLength"] = len(out)
        self.bin = out
        return self


def re_encoded(payload):
    """The payload another machine's encoder would have written for the same image: different
    bytes, a different length, and every fact its header states untouched. `basisu` selects UASTC
    blocks with SIMD, so this is the shape of the only difference a cross-platform re-cut has."""
    data = bytearray(payload)
    data[-1] ^= 0xFF
    return bytes(data) + b"\x5a" * 7


def re_encode(glb, index=0):
    """One image of a tracked model re-encoded in place."""
    model = Model(glb)
    return model.repack({index: re_encoded(model.payload(index))}).write()


def flip_in_a_mesh_view(glb):
    """One byte of the first bufferView that holds no image — geometry, which every machine's
    exporter writes identically."""
    model = Model(glb)
    index = model.non_image_view()
    model.bin[model.js["bufferViews"][index].get("byteOffset", 0)] ^= 0xFF
    model.write()
    return index


class ExportChain(unittest.TestCase):
    """The chain when everything is right."""

    def test_lint_certifies_the_clean_source(self):
        code, printed = door("lint", build())
        self.assertEqual(code, 0, printed)
        self.assertIn("lint certified", printed)
        self.assertNotIn("error:", printed)

    def test_the_chain_bakes_the_textures_and_is_byte_stable(self):
        blend = trio("byte-stable")
        glb = os.path.splitext(blend)[0] + ".glb"

        code, printed, staged = stage(blend)
        self.assertEqual(code, 0, printed)
        self.assertTrue(os.path.isfile(staged), printed)
        self.assertFalse(os.path.isfile(glb), "the chain wrote the tracked path")
        # Every stage ran, in the order that makes the door cheap to fail and the staged candidate
        # the last thing written. A certified run is the only place this can be asserted: a chain
        # missing a stage still certifies everything the stages it kept could see.
        marks = ["canon:", "source:", "consumer (raw):", "ktx2:", "derivation:",
                 "consumer (baked):", "stage:"]
        found = [printed.find("door  ▸ " + mark) for mark in marks]
        self.assertNotIn(-1, found, "a stage of the chain did not run: {}".format(
            [mark for mark, at in zip(marks, found) if at < 0]
        ))
        self.assertEqual(found, sorted(found), "the stages ran out of order: {}".format(marks))
        # The derivation happened: the candidate carries mipped KTX2, not the exporter's PNG.
        # Asserted through the verifier the release workflow and the pre-push hook run —
        # `scripts/tank/test_glb_ktx2.py` is where its laws are mutated one by one.
        verified = subprocess.run(
            [sys.executable, os.path.join(ROOT, asset_door.DERIVATION_VERIFIER), "verify", staged],
            cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        self.assertEqual(verified.returncode, 0, verified.stdout)
        self.assertIn("0 errors", verified.stdout)

        first = digest(staged)
        shutil.move(staged, glb)
        code, printed, staged = stage(blend)
        self.assertEqual(code, 0, printed)
        self.assertEqual(
            digest(staged), first,
            "a second run over an unchanged source produced different bytes — a re-export would "
            "be a diff for git-lfs to store every time",
        )

    def test_verify_certifies_what_the_chain_staged(self):
        blend = trio("verify-clean")
        land(blend)
        code, printed = door("verify", blend)
        self.assertEqual(code, 0, printed)
        self.assertIn("verify certified", printed)


class VerifyComparison(unittest.TestCase):
    """Verify's own verdict, through the door: the rebuilt candidate against the tracked bytes.

    `ComparisonLaw` below is where the law's clauses are mutated one at a time; these two cases are
    the claim that the door as a program answers with it — one model it must accept and one it must
    refuse, each costing a full re-cut.
    """

    def test_a_model_whose_images_were_encoded_elsewhere_certifies(self):
        """THE CROSS-PLATFORM CASE, which is what this law exists for: a tracked model cut on
        another architecture carries the same images in different bytes, because `basisu` selects
        UASTC blocks with SIMD. Simulated by re-encoding one payload in place — different bytes, a
        different length, every header fact untouched — and the whole document repacked around it,
        which is exactly what the other machine's repack wrote.
        """
        blend = trio("re-encoded-images")
        land(blend)
        glb = os.path.splitext(blend)[0] + ".glb"
        before = digest(glb)
        re_encode(glb)
        self.assertNotEqual(digest(glb), before, "the case did not re-encode anything")

        code, printed = door("verify", blend)
        self.assertEqual(code, 0, printed)
        self.assertIn("verify certified", printed)

    def test_a_mesh_bufferview_byte_flip_is_a_mismatch_naming_the_section(self):
        blend = trio("flipped-byte")
        land(blend)
        glb = os.path.splitext(blend)[0] + ".glb"
        index = flip_in_a_mesh_view(glb)

        code, printed = door("verify", blend)
        self.assertEqual(code, 1, printed)
        self.assertIn("door.candidate-mismatch", printed)
        self.assertIn("bufferView {}".format(index), printed)
        self.assertIn(digest(glb), printed)
        self.assertIn("rebuilt sha256", printed)

    def test_a_mismatch_keeps_the_candidate_where_asked(self):
        """OVERMATCH_DOOR_KEEP receives the refused candidate — the temporary directory it was cut
        in is gone with the refusal, and on a CI runner so is the machine that could diff it."""
        blend = trio("kept-candidate")
        land(blend)
        glb = os.path.splitext(blend)[0] + ".glb"
        flip_in_a_mesh_view(glb)

        kept = os.path.join(_WORK, "kept-candidates")
        code, printed = door("verify", blend, env={"OVERMATCH_DOOR_KEEP": kept})
        self.assertEqual(code, 1, printed)
        copy = os.path.join(kept, os.path.basename(glb) + ".rebuilt")
        self.assertTrue(os.path.isfile(copy), printed)
        self.assertIn(copy, printed)
        self.assertNotEqual(digest(copy), digest(glb),
                            "the kept candidate is the REBUILT bytes, not the tracked ones")

    def test_a_missing_tracked_model_is_a_mismatch_not_a_pass(self):
        blend = trio("no-tracked-model")
        code, printed = door("verify", blend)
        self.assertEqual(code, 1, printed)
        self.assertIn("door.candidate-mismatch", printed)

    def test_verify_writes_nothing(self):
        blend = trio("verify-writes-nothing")
        land(blend)
        directory = os.path.dirname(blend)
        before = {
            name: digest(os.path.join(directory, name)) for name in sorted(os.listdir(directory))
        }
        self.assertEqual(door("verify", blend)[0], 0)
        after = {
            name: digest(os.path.join(directory, name)) for name in sorted(os.listdir(directory))
        }
        self.assertEqual(before, after)


#: Every fact the compare law reads off a KTX2 header, and one operation that makes a tracked
#: payload state it differently. The nine u32 header fields are addressed by position, the colour
#: model and transfer function by their byte in the data-format descriptor, and a level's
#: uncompressed size by its record — so a case locates each byte itself rather than asking the code
#: under test where it put it.
KTX2_FACTS = {
    "vkFormat": lambda model: model.header(0, 0, model.ktx2(0).vk_format + 1),
    "typeSize": lambda model: model.header(0, 1, model.ktx2(0).type_size + 1),
    "pixelWidth": lambda model: model.header(0, 2, model.ktx2(0).width // 2),
    "pixelHeight": lambda model: model.header(0, 3, model.ktx2(0).height // 2),
    "pixelDepth": lambda model: model.header(0, 4, model.ktx2(0).depth + 1),
    "layerCount": lambda model: model.header(0, 5, model.ktx2(0).layers + 1),
    "faceCount": lambda model: model.header(0, 6, model.ktx2(0).faces + 1),
    "levelCount": lambda model: model.header(0, 7, model.ktx2(0).levels - 1),
    "supercompressionScheme": lambda model: model.header(0, 8, 0),
    "colourModel": lambda model: model.descriptor(0, 12, 163),
    "transferFunction": lambda model: model.descriptor(
        0, 14, glb_ktx2.DFD_SRGB if model.ktx2(0).transfer == glb_ktx2.DFD_LINEAR
        else glb_ktx2.DFD_LINEAR),
    "uncompressedLevelBytes": lambda model: model.record(0, 0, 2, model.ktx2(0).records[0][2] * 2),
}


class ComparisonLaw(unittest.TestCase):
    """`door.candidate-mismatch`, clause by clause, driven at `asset_door.compare` over a model the
    real chain baked.

    Surgery on a copy of a certified model rather than a defective fixture, because this law is
    about a COMPARISON: no blend can carry a texture payload encoded on another architecture, and
    only an operation on the bytes can say what one machine's model may and may not differ from
    another's in. The candidate side is the certified model itself — the one every case is held
    against.
    """

    @classmethod
    def setUpClass(cls):
        blend = trio("comparison-law")
        land(blend)
        cls.candidate = os.path.splitext(blend)[0] + ".glb"

    def tracked(self, name):
        """A private copy of the certified model, standing in for a tracked one."""
        path = os.path.join(_WORK, "comparison-law-{}.glb".format(name))
        shutil.copyfile(self.candidate, path)
        return path

    def row(self, tracked, element):
        """How a finding about one section of the tracked document reads, whole — a row named `x`
        and a row whose evidence mentions `x` are not the same claim."""
        return "file `{}` {}".format(tracked, element)

    def certifies(self, tracked):
        asset_door.compare(self.candidate, tracked)

    def refuses(self, tracked, *phrases):
        """The law refused, as errors of its own check, measuring each of `phrases`."""
        with self.assertRaises(asset_door.Refused) as refusal:
            asset_door.compare(self.candidate, tracked)
        findings = refusal.exception.findings
        self.assertTrue(findings, "the refusal carries no finding")
        for finding in findings:
            self.assertEqual(finding.check.id, "door.candidate-mismatch")
            self.assertEqual(finding.check.severity, report.Severity.ERROR)
            self.assertTrue(finding.evidence and finding.repair)
        measured = "\n".join(
            "{} {}".format(finding.subject, finding.evidence) for finding in findings
        )
        for phrase in phrases:
            self.assertIn(phrase, measured, "the refusal does not name it: {}".format(measured))
        return measured

    def test_the_certified_model_is_its_own_match(self):
        """Every case below is this model minus one clause, so a law that cannot certify it proves
        nothing about them."""
        self.certifies(self.tracked("identical"))

    def test_non_canonical_json_encoding_refuses_as_container(self):
        """The same document re-framed with spaces after separators parses identically — and the
        parser is exactly what a byte hidden in the encoding evades, so the file must BE the
        canonical serialization of what it parses as."""
        tracked = self.tracked("json-spacing")
        js, bin_ = glb_ktx2.read_glb(tracked)
        jb = json.dumps(js, separators=(", ", ": ")).encode()
        jb += b" " * (-len(jb) % 4)
        bb = bytes(bin_) + b"\0" * (-len(bin_) % 4)
        with open(tracked, "wb") as handle:
            handle.write(struct.pack("<III", 0x46546C67, 2, 12 + 8 + len(jb) + 8 + len(bb)))
            handle.write(struct.pack("<II", len(jb), 0x4E4F534A))
            handle.write(jb)
            handle.write(struct.pack("<II", len(bb), 0x004E4942))
            handle.write(bb)
        self.refuses(tracked, "container", "canonical serialization")

    def test_an_extra_chunk_refuses_as_container(self):
        """A chunk the parser ignores, with the declared file length keeping its story straight."""
        tracked = self.tracked("extra-chunk")
        with open(tracked, "rb") as handle:
            raw = bytearray(handle.read())
        raw += struct.pack("<II", 4, 0x12345678) + b"EVIL"
        struct.pack_into("<I", raw, 8, len(raw))
        with open(tracked, "wb") as handle:
            handle.write(raw)
        self.refuses(tracked, "container", "canonical serialization")

    def test_a_symlink_at_the_keep_name_is_not_followed(self):
        """A planted `<name>.glb.rebuilt` symlink must not receive the kept copy wherever it
        points — the door creates the destination fresh and never follows a link."""
        tracked = self.tracked("keep-symlink")
        model = Model(tracked)
        view = model.js["bufferViews"][model.non_image_view()]
        at = view.get("byteOffset", 0)
        model.patch(at, bytes([model.bin[at] ^ 0xFF]))
        model.write()

        kept = os.path.join(_WORK, "kept-symlink")
        os.makedirs(kept, exist_ok=True)
        victim = os.path.join(_WORK, "kept-symlink-victim")
        with open(victim, "wb") as handle:
            handle.write(b"the bytes a followed symlink would have destroyed")
        link = os.path.join(kept, os.path.basename(tracked) + ".rebuilt")
        if os.path.lexists(link):
            os.unlink(link)
        os.symlink(victim, link)

        os.environ["OVERMATCH_DOOR_KEEP"] = kept
        try:
            with self.assertRaises(asset_door.Refused):
                asset_door.compare(self.candidate, tracked)
        finally:
            del os.environ["OVERMATCH_DOOR_KEEP"]
        with open(victim, "rb") as handle:
            self.assertEqual(handle.read(),
                             b"the bytes a followed symlink would have destroyed")
        self.assertFalse(os.path.islink(link), "the kept copy is still the planted symlink")
        self.assertEqual(digest(link), digest(self.candidate),
                         "the kept copy is not the rebuilt candidate's bytes")

    def test_images_encoded_on_another_machine_certify(self):
        """THE ACCEPTED BOUNDARY, stated as what it is: bytes inside a texture payload are not
        compared at all. A payload of a different length whose header facts are the tracked one's
        passes — including one whose pixels would decode differently, which is the price of a law
        that must accept `basisu`'s SIMD-dependent output. The pixels are certified where the
        encoder ran: the chain stages payloads it cut and verified itself, the build publishes
        them, and a second run on that machine is byte-identical.
        """
        tracked = self.tracked("re-encoded")
        model = Model(tracked)
        before = model.payload(0)
        model.repack({0: re_encoded(before)}).write()
        after = Model(tracked)
        self.assertNotEqual(after.payload(0), before, "the case re-encoded nothing")
        self.assertNotEqual(len(after.payload(0)), len(before),
                            "the case did not change the payload's length, which is what moves "
                            "every offset behind it")
        self.assertNotEqual(digest(tracked), digest(self.candidate))
        self.certifies(tracked)

    def test_a_mesh_bufferview_byte_flip_refuses_naming_the_view(self):
        """The other side of the same line: geometry is the exporter's output, identical on every
        machine, and it is compared byte for byte."""
        tracked = self.tracked("mesh-byte")
        index = flip_in_a_mesh_view(tracked)
        self.refuses(tracked, "bufferView {}".format(index), "holds no image")

    def test_every_ktx2_fact_the_law_reads_refuses_by_name(self):
        """One case per fact, because a fact dropped from the table is a payload difference nothing
        would see — and the door would then certify a tracked model whose textures are the wrong
        size, the wrong codec, the wrong colour space or a shorter mip chain."""
        for fact, surgery in KTX2_FACTS.items():
            with self.subTest(fact=fact):
                tracked = self.tracked("fact-" + fact)
                model = Model(tracked)
                surgery(model)
                model.write()
                self.refuses(tracked, "image 0", fact + ": tracked ")

    def test_a_payload_that_is_not_a_ktx2_file_refuses(self):
        tracked = self.tracked("not-ktx2")
        model = Model(tracked)
        model.patch(model.start(0), b"\x89PNG\r\n\x1a\n")
        model.write()
        self.refuses(tracked, "image 0", "not a KTX2 file")

    def test_an_image_that_is_not_embedded_refuses(self):
        """A payload this law cannot read at all is not a payload it may pass over."""
        tracked = self.tracked("unembedded-image")
        model = Model(tracked)
        model.js["images"][0].pop("bufferView")
        model.js["images"][0]["uri"] = "elsewhere.ktx2"
        model.write()
        self.refuses(tracked, "image 0", "carries no embedded payload")

    def test_a_dropped_image_refuses(self):
        tracked = self.tracked("dropped-image")
        model = Model(tracked)
        model.js["images"].pop()
        model.write()
        self.refuses(tracked, self.row(tracked, "images"), "entries tracked")

    def test_an_added_image_refuses(self):
        """A duplicate entry is the smallest addition: same payload, same mimeType, one image more
        than the source produces."""
        tracked = self.tracked("added-image")
        model = Model(tracked)
        model.js["images"].append(dict(model.js["images"][0]))
        model.write()
        self.refuses(tracked, self.row(tracked, "images"), "entries tracked")

    def test_json_outside_the_sanctioned_spans_refuses(self):
        """Everything the encoder's output size does not move is compared whole, and the row names
        the collection it moved in."""
        tracked = self.tracked("json-drift")
        model = Model(tracked)
        model.js["nodes"][0]["name"] = "Renamed"
        model.write()
        self.refuses(tracked, self.row(tracked, "nodes"), "entry 0 differs")

    def test_a_collection_the_source_does_not_produce_refuses(self):
        """A key on one side only, which is neither two values to compare nor a difference the
        encoder's output size can produce."""
        tracked = self.tracked("extra-collection")
        model = Model(tracked)
        model.js["extensionsRequired"] = [glb_ktx2.BASISU]
        model.write()
        self.refuses(tracked, self.row(tracked, "extensionsRequired"),
                     "present in the tracked document only")

    def test_a_dropped_bufferview_refuses(self):
        """The count, before any span is read: a document holding fewer views than the source
        produces is answered by the row that counts them."""
        tracked = self.tracked("dropped-view")
        model = Model(tracked)
        model.js["bufferViews"].pop()
        model.write()
        self.refuses(tracked, self.row(tracked, "bufferViews"), "entries tracked")

    def test_a_resized_mesh_bufferview_refuses(self):
        """byteLength is sanctioned for an IMAGE bufferView and for no other: a mesh view that
        claims a different span is a document the exporter did not write."""
        tracked = self.tracked("resized-view")
        model = Model(tracked)
        index = model.non_image_view()
        model.js["bufferViews"][index]["byteLength"] -= 4
        model.write()
        self.refuses(tracked, self.row(tracked, "bufferViews"),
                     "entry {} differs".format(index))

    def test_a_moved_image_bufferview_is_the_encoder_doing_its_job(self):
        """The sanctioned change, stated so the rows above cannot be tightened into refusing every
        honest re-cut: a payload of another length moves its own span, every span behind it, and the
        buffer they all live in."""
        tracked = self.tracked("sanctioned-spans")
        model = Model(tracked)
        moved = model.repack({0: re_encoded(model.payload(0))})
        self.assertNotEqual(
            moved.js["buffers"][0]["byteLength"],
            glb_ktx2.read_glb(self.candidate)[0]["buffers"][0]["byteLength"],
            "the case did not move the buffer it claims to",
        )
        moved.write()
        self.certifies(tracked)

    def test_a_stray_byte_in_the_alignment_padding_refuses(self):
        """The bytes no other clause names. The derivation pads bufferViews to four bytes with
        zeros, so anything else living in a gap is content nothing in this document accounts for."""
        tracked = self.tracked("stray-padding")
        model = Model(tracked)
        index = next(number for number, image in enumerate(model.js["images"])
                     if image["bufferView"] < len(model.js["bufferViews"]) - 1)
        payload = model.payload(index)
        # A payload one byte past an alignment boundary, so the view behind it is padded — the gap
        # is built rather than looked for, and the case then writes in one that is certainly there.
        model.repack({index: payload + b"\x5a" * ((1 - len(payload)) % 4)})
        view = model.js["bufferViews"][model.js["images"][index]["bufferView"]]
        model.bin[view["byteOffset"] + view["byteLength"]] = 0x7F
        model.write()
        self.refuses(tracked, "container", "outside every bufferView")

    def test_a_container_that_lies_about_its_own_length_refuses(self):
        tracked = self.tracked("short-container")
        with open(tracked, "r+b") as handle:
            handle.seek(8)
            handle.write(struct.pack("<I", os.path.getsize(tracked) - 4))
        self.refuses(tracked, "container", "canonical serialization")

    def test_a_tracked_model_that_is_not_a_glb_is_a_finding_not_a_traceback(self):
        """Fail-closed at the parse: `verify` is run by a hook and a CI lane, and a traceback there
        is a refusal nobody can act on."""
        tracked = self.tracked("not-a-glb")
        with open(tracked, "wb") as handle:
            handle.write(b"not a glb at all")
        self.refuses(tracked, "document", "cannot be read")


class RefusalsLeaveTheModelAlone(unittest.TestCase):
    """One case per stage that can refuse. Each asserts the same thing: the tracked glb is the file
    it was, and no candidate was staged."""

    def refuses(self, blend, expect, env=None, mode=None):
        """Run the chain against a tracked model that already exists, and prove the refusal changed
        nothing. `mode` names a CLI mode; the default is the staging chain the build drives.
        Returns what the door printed."""
        glb = os.path.splitext(blend)[0] + ".glb"
        before = digest(glb)
        self.assertIsNotNone(before, "the case has no tracked model to leave alone")
        if mode is None:
            code, printed, staged = stage(blend, env)
            self.assertFalse(os.path.exists(staged), "the refused chain staged a candidate")
        else:
            code, printed = door(mode, blend, env)
        self.assertEqual(code, 1, printed)
        self.assertIn(expect, printed)
        self.assertEqual(digest(glb), before, "the refused chain wrote the tracked glb")
        return printed

    def exported(self, name, defect="none"):
        """A trio with a certified tracked model already in place, then rebuilt with `defect`."""
        blend = trio(name)
        land(blend)
        if defect != "none":
            shutil.copyfile(build(defect), blend)
        return blend

    def test_an_l1_error_stops_before_the_raw_export(self):
        printed = self.refuses(self.exported("refuse-l1", "modifier"), "L1.MODIFIER_STACK")
        # The exporter announces the candidate it wrote. A refused source produces none: a file cut
        # from a model the pass refused is one nobody may consume.
        self.assertNotIn("raw   ▸", printed, "the exporter ran on a source the pass refused")
        self.assertNotIn("consumer (raw)", printed, "a refused source reached the contract")

    def test_a_consumer_refusal_on_the_raw_candidate_stops_before_the_encode(self):
        printed = self.refuses(self.exported("refuse-raw-l2", "open-wheel"), "L2.")
        self.assertIn("consumer (raw)", printed)
        self.assertNotIn("images ▸", printed, "the minute-long encode ran on a refused candidate")

    def test_an_encoder_failure_refuses(self):
        """A `basisu` that answers the preflight and then fails: the pins pass, the encode does
        not."""
        encoder = shim("basisu", (
            "#!/bin/sh\n"
            "[ \"$1\" = \"-version\" ] && {{ echo 'Basis Universal LDR/HDR GPU Texture "
            "Supercompression System v{}'; exit 0; }}\n"
            "echo 'injected encoder failure' >&2\nexit 1\n"
        ).format(toolchain.BASISU_VERSION))
        # Injected through OVERMATCH_BASISU — the FIRST channel of the resolution rule, so the
        # injection holds on a machine whose own environment already names a pinned encoder (CI).
        printed = self.refuses(
            self.exported("refuse-encode"), "door.stage-failed",
            {toolchain.BASISU_ENV: encoder},
        )
        self.assertIn("refused at ktx2", printed)

    def test_a_consumer_refusal_on_the_baked_candidate_refuses_after_the_encode(self):
        """The last stage before the candidate is staged. Injected through a `cargo` that
        refuses its third call — canon, raw candidate, baked candidate."""
        counter = os.path.join(_WORK, "cargo-calls")
        if os.path.exists(counter):
            os.remove(counter)
        cargo = shim("cargo", (
            "#!/bin/sh\n"
            "count=$(cat '{counter}' 2>/dev/null || echo 0)\n"
            "count=$((count + 1))\n"
            "echo \"$count\" > '{counter}'\n"
            "if [ \"$count\" -ge 3 ]; then\n"
            "  echo 'L2.MANIFOLD_WINDING error: node `Wheel_L` — injected'\n"
            "  exit 1\n"
            "fi\n"
            "exec '{cargo}' \"$@\"\n"
        ).format(counter=counter, cargo=shutil.which("cargo")))
        printed = self.refuses(self.exported("refuse-baked-l2"), "refused at consumer (baked)",
                               on_path(cargo))
        self.assertIn("images ▸", printed, "the case did not reach the stage it claims to test")

    def test_an_unresolved_library_refuses_every_mode(self):
        """The door's precondition. Blender substitutes a placeholder for a datablock whose library
        it cannot read, so there is nothing here worth measuring — in any mode."""
        blend = trio("unresolved-library")
        land(blend)
        library = os.path.join(
            os.path.dirname(os.path.dirname(blend)), "materials", "materials.blend"
        )
        os.remove(library)
        for mode in ("lint", "verify", None):
            printed = self.refuses(blend, "door.unresolved-library", mode=mode)
            self.assertIn("MildSteel", printed, "the missing datablock is unnamed")

    def test_the_registry_beside_the_asset_is_the_one_the_contract_reads(self):
        """The substance registry is DATA and travels with the trio: the door names the
        `assets/materials/materials.ron` beside the model, so a lane verifying a pushed revision
        reads that revision's numbers rather than whatever this work tree holds.

        Driven by breaking that file. The binary carries a perfectly good registry of its own, so a
        door that did not name this one would sail past — which is exactly the silent wrong verdict
        the row exists to make impossible.
        """
        blend = self.exported("registry-beside-the-asset")
        registry = os.path.join(
            os.path.dirname(os.path.dirname(blend)), "materials", "materials.ron"
        )
        self.assertTrue(os.path.isfile(registry), "the fixture ships no substance registry")
        with open(registry, "w", encoding="utf-8") as handle:
            handle.write("not a registry\n")
        printed = self.refuses(blend, "door.registry", mode="verify")
        self.assertIn(registry, printed, "the refusal does not name the file it could not read")

    def test_a_toolchain_mismatch_refuses_before_the_chain(self):
        """The pins are asserted first, so a wrong program costs nothing to discover."""
        encoder = shim("basisu-old", (
            "#!/bin/sh\necho 'Basis Universal LDR/HDR GPU Texture Supercompression System "
            "v1.16.4'\n"
        ))
        printed = self.refuses(
            self.exported("refuse-toolchain"), "door.toolchain",
            {toolchain.BASISU_ENV: encoder},
        )
        self.assertIn("1.16.4", printed)
        self.assertNotIn("door  ▸ canon", printed, "a stage ran on an unpinned toolchain")

    def test_a_blender_that_is_not_the_pinned_one_refuses_lint_too(self):
        stub = shim("blender-old", "#!/bin/sh\necho 'Blender 5.0.1'\necho '\tbuild hash: c0ffee'\n")
        code, printed = door("lint", build(), {toolchain.BLENDER_ENV: stub})
        self.assertEqual(code, 1, printed)
        self.assertIn("door.toolchain", printed)
        self.assertIn("5.0.1", printed)


class TrackedPath(unittest.TestCase):
    """What the door does to a path it does not hold alone: READ AND COMPARE, driven directly,
    because what is under test is what a second writer doing its own thing at the same moment can
    make of it.

    The other half of this class was stage-and-rename, and it went with `asset_door.replace`: the
    door lands no bytes any more, and the trio's one writer is `scripts/tank/trio.py`.
    """

    def candidates(self, name, count=2):
        """Distinct candidates, big enough that copying one is not instantaneous."""
        directory = os.path.join(_WORK, name)
        os.makedirs(directory, exist_ok=True)
        paths = []
        for index in range(count):
            path = os.path.join(directory, "candidate-{}.bin".format(index))
            with open(path, "wb") as handle:
                handle.write(bytes([index + 1]) * (8 << 20))
            paths.append(path)
        return (directory, paths)

    def interrupted(self, name, writer):
        """A comparison that would otherwise CERTIFY — the tracked file starts as a byte-for-byte
        copy of the candidate — interrupted at its one window by `writer(other, tracked)`.

        Deterministic rather than lucky: the interruption happens INSIDE the comparison, at the
        moment the defect needs, by standing in for the rebuilt candidate's digest — the step
        between reading the tracked bytes and answering about them. Returns `(tracked, findings)`.
        """
        directory, (candidate, other) = self.candidates(name)
        tracked = os.path.join(directory, "tracked.glb")
        shutil.copyfile(candidate, tracked)
        real_digest = asset_door.digest

        def interrupt(path):
            writer(other, tracked)
            return real_digest(path)

        asset_door.digest = interrupt
        try:
            with self.assertRaises(asset_door.Refused) as refusal:
                asset_door.compare(candidate, tracked)
        finally:
            asset_door.digest = real_digest
        findings = refusal.exception.findings
        self.assertEqual([finding.check.id for finding in findings], ["door.candidate-mismatch"])
        self.assertIn("changed while it was being compared", findings[0].evidence)
        return (tracked, findings)

    def test_verify_refuses_a_model_replaced_while_it_was_being_compared(self):
        """An open handle outlives the pathname it was opened by, so the digest and the answer are
        about two different moments unless something says otherwise. Another writer landing its own
        model at the tracked path mid-comparison would leave verify certifying an inode nothing can
        reach — and that verdict is what pre-push and CI act on."""
        tracked, _ = self.interrupted("replaced-mid-compare", os.replace)
        self.assertTrue(os.path.isfile(tracked))
        self.assertNotEqual(
            digest(tracked), digest(os.path.join(os.path.dirname(tracked), "candidate-0.bin")),
            "the case did not replace the tracked model it claims to",
        )

    def test_the_identity_compared_is_the_handle_the_digest_came_from(self):
        """The identity the answer is checked against comes off the OPEN FILE, not off the pathname
        a second time. A replacement landing between the open and that measurement would otherwise
        have both measurements describing the newcomer while the digest — read through the handle —
        describes the file that is gone: a certified path whose bytes were never read.

        Driven at that exact window, by standing in for the `fstat` that closes it.
        """
        directory, (candidate, other) = self.candidates("replaced-before-the-fstat")
        tracked = os.path.join(directory, "tracked.glb")
        shutil.copyfile(candidate, tracked)
        real_fstat = os.fstat

        def racing(handle):
            os.fstat = real_fstat        # the window is open once, and this is inside it
            os.replace(other, tracked)
            return real_fstat(handle)

        os.fstat = racing
        try:
            with self.assertRaises(asset_door.Refused) as refusal:
                asset_door.compare(candidate, tracked)
        finally:
            os.fstat = real_fstat
        self.assertIn("changed while it was being compared",
                      refusal.exception.findings[0].evidence)

    def test_verify_refuses_a_model_rewritten_in_place_while_it_was_being_compared(self):
        """Not every second writer renames. One that writes THROUGH the tracked path leaves the
        device and inode alone, and only the generation of the content — its size and its
        modification time — says the bytes are no longer the ones this verdict read."""
        def rewrite(other, path):
            with open(other, "rb") as source, open(path, "r+b") as target:
                target.write(source.read())
        self.interrupted("rewritten-mid-compare", rewrite)

    def test_the_identity_is_the_file_and_the_generation_of_its_content(self):
        """Four fields, each load-bearing, so the tuple is stated whole: device and inode name WHICH
        file — a pathname can be made to mean another one — and size and modification time name
        which generation of its content, for the writer that goes through the path instead of
        around it. A field dropped from it is a class of replacement nothing would see."""
        class Stat:
            st_dev, st_ino, st_size, st_mtime_ns = 11, 22, 33, 44
            st_ctime_ns, st_mode, st_uid = 55, 66, 77

        self.assertEqual(asset_door.identity(Stat()), (11, 22, 33, 44))

    def test_verify_refuses_a_model_taken_away_while_it_was_being_compared(self):
        """The same precondition, fail-closed: a path that cannot be stated at all is not one this
        verdict can describe either, and an unlinked tracked model is exactly that."""
        tracked, findings = self.interrupted(
            "removed-mid-compare", lambda _other, path: os.remove(path)
        )
        self.assertFalse(os.path.exists(tracked))
        self.assertIn("No such file", findings[0].evidence)

    def test_a_tracked_model_that_cannot_be_read_is_a_finding_not_a_traceback(self):
        """Existence and readability are one question here, asked once, by opening the file. Asked
        as two — does it exist, then open it — the answers are about two different moments, and the
        second one raising is a traceback where the report should be."""
        directory, (candidate,) = self.candidates("unreadable", count=1)
        tracked = os.path.join(directory, "tracked.glb")
        shutil.copyfile(candidate, tracked)
        os.chmod(tracked, 0)
        try:
            with self.assertRaises(asset_door.Refused) as refusal:
                asset_door.compare(candidate, tracked)
        finally:
            os.chmod(tracked, 0o644)
        findings = refusal.exception.findings
        self.assertEqual([finding.check.id for finding in findings], ["door.candidate-mismatch"])
        self.assertIn("cannot be read", findings[0].evidence)


class TheSideDoorIsClosed(unittest.TestCase):
    """`asset_door.py export` was a second public writer of a tracked model.

    It replaced `<id>.glb` and only that, leaving `<id>.sim.glb` and `<id>.lod.json` describing
    bytes that were gone — an incoherent trio that `build.py verify` would catch afterwards, which
    is one refusal too late. ADR 0035 gives all three artifacts to one writer, so the entrance is
    gone; the chain behind it is not, and `build.py` still drives it through `stage_to`.
    """

    def test_the_retired_entrance_writes_nothing_and_names_the_successor(self):
        blend = trio("retired-entrance")
        directory = os.path.dirname(blend)
        before = {
            name: digest(os.path.join(directory, name)) for name in sorted(os.listdir(directory))
        }
        code, printed = door("export", blend)
        self.assertEqual(code, 1, printed)
        self.assertIn("scripts/tank/build.py build", printed)
        after = {
            name: digest(os.path.join(directory, name)) for name in sorted(os.listdir(directory))
        }
        self.assertEqual(before, after, "the retired entrance wrote something")

    def test_the_chain_refuses_to_publish_when_asked_for_no_staging_path(self):
        """The API half of the same law: `derive` has no landing branch left to reach."""
        blend = trio("no-staging-path")
        driver = (
            "import sys; sys.path.insert(0, {!r}); import asset_door; "
            "asset_door.chain('export', {!r}, {!r}, {!r}, asset_door.repo_root(), {!r}, 'blender')"
        ).format(
            os.path.dirname(DOOR), blend, os.path.splitext(blend)[0] + ".tank.ron",
            os.path.splitext(blend)[0] + ".glb", _WORK,
        )
        result = subprocess.run(
            [sys.executable, "-c", driver], cwd=ROOT, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_the_door_holds_no_writer_of_a_tracked_path(self):
        """A TEXT TRIPWIRE over the door's own source: no `os.replace` and no `os.rename`.

        The atomic landing this file used to own now lives in `scripts/tank/trio.py`, which is what
        `build.py` publishes through. Two implementations of "put bytes at a tracked path" is how
        one of them comes to be reachable again.
        """
        source = open(DOOR, encoding="utf-8").read()
        for spelling in ("os.replace(", "os.rename(", "def replace("):
            self.assertNotIn(
                spelling, source,
                "{} is back in the door — the trio's one writer is scripts/tank/trio.py".format(
                    spelling
                ),
            )


class DoorMechanics(unittest.TestCase):
    """The seams the modes are built out of."""

    def test_a_mode_with_a_chain_refuses_without_a_candidate_path(self):
        """The Blender half writes its candidate where the door says. Asked for one with no path,
        it refuses by name rather than exporting nowhere."""
        blend = build()
        canon = os.path.join(_WORK, "canon-no-raw.json")
        with open(canon, "wb") as handle:
            written = subprocess.run(
                asset_door.contract(asset_door.registry_of(blend), "--canon",
                                    os.path.splitext(blend)[0] + ".tank.ron"),
                cwd=ROOT, stdout=handle,
            )
        self.assertEqual(written.returncode, 0)
        for mode in ("export", "verify"):
            result = subprocess.run(
                [toolchain.blender().binary, "--background", "--factory-startup", blend,
                 "--python", SOURCE_PASS, "--", "--mode", mode, "--canon", canon],
                cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            )
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertIn("door.mode-unimplemented", result.stdout)


class ToolchainProgram(unittest.TestCase):
    """`scripts/toolchain.py` as the lane that installs these programs runs it.

    The pins are declared in that file and asserted BY RUNNING IT. A few lines of Python quoted into
    a workflow step are executable by the runner and by nothing else: no suite can call them, so the
    step that imported this module with only `scripts/` on its path was red on every run of the lane
    and green in every suite — until the lane was required, which is the worst possible moment to
    find out. The step now runs a file, and this is that file being run.
    """

    def toolchain(self, *arguments, env=None):
        result = subprocess.run(
            [sys.executable, os.path.join(ROOT, "scripts", "toolchain.py")] + list(arguments),
            cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            env=dict(os.environ, **(env or {})),
        )
        return (result.returncode, result.stdout)

    def test_running_it_asserts_the_installed_programs(self):
        code, printed = self.toolchain()
        self.assertEqual(code, 0, printed)
        for program in ("blender", "basisu"):
            self.assertIn("toolchain ▸ {}".format(program), printed)

    def test_a_program_that_is_not_the_pinned_one_exits_non_zero(self):
        encoder = shim("basisu-unpinned", (
            "#!/bin/sh\necho 'Basis Universal LDR/HDR GPU Texture Supercompression System "
            "v1.16.4'\n"
        ))
        code, printed = self.toolchain(env={toolchain.BASISU_ENV: encoder})
        self.assertEqual(code, 1, printed)
        self.assertIn("door.toolchain", printed)
        self.assertIn("1.16.4", printed)

    def test_the_pins_it_prints_are_the_pins_it_declares(self):
        """The lane installs by version before it can assert a version, and cuts its caches on the
        same numbers. They come out of the declaration rather than a second copy in YAML."""
        code, printed = self.toolchain("--pins")
        self.assertEqual(code, 0, printed)
        self.assertEqual(
            dict(line.split("=", 1) for line in printed.splitlines()),
            {
                "OVERMATCH_BLENDER_VERSION": toolchain.BLENDER_VERSION,
                "OVERMATCH_BLENDER_BUILD": toolchain.BLENDER_BUILD,
                "OVERMATCH_BASISU_VERSION": toolchain.BASISU_VERSION,
                "OVERMATCH_BASISU_TAG": toolchain.BASISU_TAG,
            },
        )

    def test_the_workflow_runs_this_file_and_holds_no_python_of_its_own(self):
        """The class the regression came from, closed: a workflow that holds no Python holds none
        that no suite runs."""
        with open(os.path.join(ROOT, ".github", "workflows", "ci.yml"), encoding="utf-8") as handle:
            workflow = handle.read()
        self.assertEqual(
            [line.strip() for line in workflow.splitlines() if "python3 - " in line], [],
            "a workflow step holds inline python — the runner is the only thing that can execute "
            "it, so it is the only thing that can discover it is broken",
        )
        for command in ("python3 scripts/toolchain.py --pins", "python3 scripts/toolchain.py\n"):
            self.assertIn(command, workflow, "the lane does not run the file this case drives")


class HookEnvironment(unittest.TestCase):
    """A pre-push hook exports `GIT_DIR` (without `GIT_WORK_TREE`), under which git answers
    location questions about the hook's repo and reports the asker's CWD as toplevel. The door and
    every stage it launches must keep their own bearings under that environment."""

    def test_repo_root_ignores_the_hooks_git_exports(self):
        """`repo_root()` names the door's own work tree even when GIT_DIR points elsewhere and the
        process sits in a foreign repo — the exact shape of a pre-push hook run."""
        elsewhere = os.path.join(_WORK, "foreign-repo")
        os.makedirs(elsewhere, exist_ok=True)
        subprocess.run(["git", "init", "--quiet", elsewhere], check=True)
        hooked = dict(os.environ, GIT_DIR=os.path.join(elsewhere, ".git"))
        reported = subprocess.run(
            [sys.executable, "-c",
             "import sys; sys.path.insert(0, {!r}); import asset_door; "
             "print(asset_door.repo_root())".format(os.path.dirname(DOOR))],
            cwd=elsewhere, env=hooked, stdout=subprocess.PIPE, text=True, check=True,
        ).stdout.strip()
        self.assertEqual(reported, ROOT)

    def test_stages_run_without_the_hooks_git_exports(self):
        """Children launched by `run_stage` see no `GIT_*` at all: Blender's source pass and the
        encoder each ask git their own location questions."""
        probe = [sys.executable, "-c",
                 "import os, sys; sys.exit(1 if [k for k in os.environ if k.startswith('GIT_')] "
                 "else 0)"]
        os.environ["GIT_DIR"] = os.path.join(_WORK, "foreign-repo", ".git")
        try:
            asset_door.run_stage("probe", probe, ROOT)
        finally:
            del os.environ["GIT_DIR"]


if __name__ == "__main__":
    unittest.main(verbosity=2)
