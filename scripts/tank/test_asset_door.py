"""The asset door end to end, on a synthetic trio.

    python3 scripts/tank/test_asset_door.py

Every case runs the REAL door — the real Blender, the real consumer contract, the real encoder —
against the tank `.agents/blender/fixture_tank.py` builds in a temporary directory. Nothing under
`assets/` is read or written, and the fixture is a second vehicle rather than a copy of the first:
a door that only works on the Tiger is not a door.

Two classes of claim are proven here. That the chain CERTIFIES: lint is clean, export writes a
mip-baked glb, a second export is byte-identical, and verify accepts what export just wrote. And
that every refusal LEAVES THE TRACKED GLB ALONE — one case per stage, each proving the model on
disk is the byte-for-byte file it was before the door ran.

The two stages with no defect a fixture can carry — an encoder that fails, and a consumer contract
that refuses the BAKED bytes after the raw ones passed — are reached by putting a stub earlier on
PATH. What that injects is the stage's exit code, which is exactly the door's contract with it.
"""

import hashlib
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "lod"))

import asset_door  # noqa: E402
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


class ExportChain(unittest.TestCase):
    """The chain when everything is right."""

    def test_lint_certifies_the_clean_source(self):
        code, printed = door("lint", build())
        self.assertEqual(code, 0, printed)
        self.assertIn("lint certified", printed)
        self.assertNotIn("error:", printed)

    def test_export_bakes_the_textures_and_is_byte_stable(self):
        blend = trio("byte-stable")
        glb = os.path.splitext(blend)[0] + ".glb"

        code, printed = door("export", blend)
        self.assertEqual(code, 0, printed)
        self.assertTrue(os.path.isfile(glb), printed)
        # Every stage ran, in the order that makes the door cheap to fail and the tracked path the
        # last thing written. A certified export is the only place this can be asserted: a chain
        # missing a stage still certifies everything the stages it kept could see.
        marks = ["canon:", "source:", "consumer (raw):", "ktx2:", "derivation:",
                 "consumer (baked):", "export:"]
        found = [printed.find("door  ▸ " + mark) for mark in marks]
        self.assertNotIn(-1, found, "a stage of the chain did not run: {}".format(
            [mark for mark, at in zip(marks, found) if at < 0]
        ))
        self.assertEqual(found, sorted(found), "the stages ran out of order: {}".format(marks))
        # The derivation happened: the tracked model carries mipped KTX2, not the exporter's PNG.
        # Asserted through the verifier the release workflow and the pre-push hook run, on the
        # TRACKED path — `scripts/tank/test_glb_ktx2.py` is where its laws are mutated one by one.
        verified = subprocess.run(
            [sys.executable, os.path.join(ROOT, asset_door.DERIVATION_VERIFIER), "verify", glb],
            cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        self.assertEqual(verified.returncode, 0, verified.stdout)
        self.assertIn("0 errors", verified.stdout)

        first = digest(glb)
        code, printed = door("export", blend)
        self.assertEqual(code, 0, printed)
        self.assertEqual(
            digest(glb), first,
            "a second export of an unchanged source produced different bytes — a re-export would "
            "be a diff for git-lfs to store every time",
        )

    def test_verify_certifies_what_export_wrote(self):
        blend = trio("verify-clean")
        self.assertEqual(door("export", blend)[0], 0)
        code, printed = door("verify", blend)
        self.assertEqual(code, 0, printed)
        self.assertIn("verify certified", printed)


class VerifyComparison(unittest.TestCase):
    """Verify's own verdict: the rebuilt candidate against the tracked bytes."""

    def test_a_flipped_byte_is_a_mismatch_naming_both_digests(self):
        blend = trio("flipped-byte")
        self.assertEqual(door("export", blend)[0], 0)
        glb = os.path.splitext(blend)[0] + ".glb"
        with open(glb, "rb") as handle:
            tracked = bytearray(handle.read())
        tracked[-1] ^= 0xFF
        with open(glb, "wb") as handle:
            handle.write(bytes(tracked))

        code, printed = door("verify", blend)
        self.assertEqual(code, 1, printed)
        self.assertIn("door.candidate-mismatch", printed)
        self.assertIn(digest(glb), printed)
        self.assertIn("rebuilt sha256", printed)

    def test_a_missing_tracked_model_is_a_mismatch_not_a_pass(self):
        blend = trio("no-tracked-model")
        code, printed = door("verify", blend)
        self.assertEqual(code, 1, printed)
        self.assertIn("door.candidate-mismatch", printed)

    def test_verify_writes_nothing(self):
        blend = trio("verify-writes-nothing")
        self.assertEqual(door("export", blend)[0], 0)
        directory = os.path.dirname(blend)
        before = {
            name: digest(os.path.join(directory, name)) for name in sorted(os.listdir(directory))
        }
        self.assertEqual(door("verify", blend)[0], 0)
        after = {
            name: digest(os.path.join(directory, name)) for name in sorted(os.listdir(directory))
        }
        self.assertEqual(before, after)


class RefusalsLeaveTheModelAlone(unittest.TestCase):
    """One case per stage that can refuse. Each asserts the same thing: the tracked glb is the file
    it was."""

    def refuses(self, blend, expect, env=None, mode="export"):
        """Export against a tracked model that already exists, and prove the refusal changed
        nothing. Returns what the door printed."""
        glb = os.path.splitext(blend)[0] + ".glb"
        before = digest(glb)
        self.assertIsNotNone(before, "the case has no tracked model to leave alone")
        code, printed = door(mode, blend, env)
        self.assertEqual(code, 1, printed)
        self.assertIn(expect, printed)
        self.assertEqual(digest(glb), before, "the refused chain wrote the tracked glb")
        return printed

    def exported(self, name, defect="none"):
        """A trio with a certified tracked model already in place, then rebuilt with `defect`."""
        blend = trio(name)
        self.assertEqual(door("export", blend)[0], 0, "the clean export this case builds on failed")
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
        printed = self.refuses(
            self.exported("refuse-encode"), "door.stage-failed", on_path(encoder)
        )
        self.assertIn("refused at ktx2", printed)

    def test_a_consumer_refusal_on_the_baked_candidate_refuses_after_the_encode(self):
        """The last stage before the tracked path is written. Injected through a `cargo` that
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
        self.assertEqual(door("export", blend)[0], 0)
        library = os.path.join(
            os.path.dirname(os.path.dirname(blend)), "materials", "materials.blend"
        )
        os.remove(library)
        for mode in ("lint", "export", "verify"):
            printed = self.refuses(blend, "door.unresolved-library", mode=mode)
            self.assertIn("MildSteel", printed, "the missing datablock is unnamed")

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


class DoorMechanics(unittest.TestCase):
    """The seams the modes are built out of."""

    def test_a_mode_with_a_chain_refuses_without_a_candidate_path(self):
        """The Blender half writes its candidate where the door says. Asked for one with no path,
        it refuses by name rather than exporting nowhere."""
        blend = build()
        canon = os.path.join(_WORK, "canon-no-raw.json")
        with open(canon, "wb") as handle:
            written = subprocess.run(
                ["cargo", "run", "--quiet", "--bin", "asset_verify", "--", "--canon",
                 os.path.splitext(blend)[0] + ".tank.ron"],
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

    def blender_half(self, blend, raw):
        """The Blender half, run the way the GUI adapter runs it: the caller's own Blender, the
        canon file from the one generator, the candidate written where the caller says."""
        spec = os.path.splitext(blend)[0] + ".tank.ron"
        canon = os.path.join(os.path.dirname(raw), "canon.json")
        with open(canon, "wb") as handle:
            written = subprocess.run(
                ["cargo", "run", "--quiet", "--bin", "asset_verify", "--", "--canon", spec],
                cwd=ROOT, stdout=handle,
            )
        self.assertEqual(written.returncode, 0)
        result = subprocess.run(
            [toolchain.blender().binary, "--background", "--factory-startup", blend,
             "--python", SOURCE_PASS, "--", "--mode", "export", "--spec", spec,
             "--glb", os.path.splitext(blend)[0] + ".glb", "--canon", canon, "--raw", raw],
            cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertTrue(os.path.isfile(raw), result.stdout)

    def test_a_continued_chain_bakes_the_bytes_the_full_chain_bakes(self):
        """`--from-raw` is the GUI adapter's whole claim to being an adapter: the stages after the
        candidate are the same calls in the same order, so the model it lands is the model the
        headless door lands, to the byte. Anything else would be a second door."""
        blend = trio("from-raw")
        glb = os.path.splitext(blend)[0] + ".glb"
        self.assertEqual(door("export", blend)[0], 0)
        whole = digest(glb)

        os.remove(glb)
        work = os.path.join(_WORK, "from-raw-work")
        os.makedirs(work, exist_ok=True)
        raw = os.path.join(work, "testbed.raw.glb")
        self.blender_half(blend, raw)

        code, printed = door("export", blend, None, "--from-raw", raw)
        self.assertEqual(code, 0, printed)
        self.assertIn("door  ▸ from-raw", printed)
        self.assertNotIn("door  ▸ source", printed, "the continuation launched a second Blender")
        for mark in ("consumer (raw):", "ktx2:", "derivation:", "consumer (baked):", "export:"):
            self.assertIn("door  ▸ " + mark, printed, "the continuation skipped a stage")
        self.assertEqual(digest(glb), whole, "the continued chain baked different bytes")

    def test_a_continuation_with_no_candidate_refuses(self):
        """The caller's source pass either wrote the candidate or refused. A missing file is that
        refusal arriving as silence, so the door names it instead of encoding nothing."""
        blend = build()
        code, printed = door("export", blend, None, "--from-raw",
                             os.path.join(_WORK, "no-such-candidate.glb"))
        self.assertEqual(code, 1, printed)
        self.assertIn("door.stage-failed", printed)
        self.assertIn("refused at from-raw", printed)

    def test_the_lod_lane_and_the_door_pin_the_same_toolchain(self):
        """`scripts/lod/config.py` still carries its own copy of the pins: its bytes are hashed into
        every shipped manifest, so the re-export lands with a corpus regeneration. Until then the
        two declarations are held equal here, mechanically."""
        import config as lod  # noqa: PLC0415 — the LOD lane's config, only needed for this claim

        self.assertEqual(lod.EXPECTED_BLENDER, toolchain.BLENDER_VERSION)
        self.assertEqual(lod.EXPECTED_BLENDER_BUILD, toolchain.BLENDER_BUILD)
        self.assertEqual(lod.EXPECTED_GLTF_EXPORTER, toolchain.GLTF_EXPORTER_VERSION)


if __name__ == "__main__":
    unittest.main(verbosity=2)
