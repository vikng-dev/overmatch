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
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "lod"))

import asset_door  # noqa: E402
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
    """The two things the door does to a path it does not hold alone — stage and rename, read and
    compare — driven directly, because what is under test is what a SECOND door doing the same
    thing at the same moment can make of them."""

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

    def test_an_export_renames_the_file_it_wrote(self):
        """The staging name is unique to the invocation, so no second export can be writing through
        the file this one is about to rename. With a name every export shares, the rename is of
        whatever the last writer left there — under the first writer's verdict.

        Deterministic rather than lucky: the first export is held AT its rename until the second has
        finished staging its own bytes, which is exactly the window the defect lives in. The claim
        is then simply that what each export renamed is what that export reported.
        """
        directory, (first, second) = self.candidates("renames-its-own")
        tracked = os.path.join(directory, "tracked.glb")
        shutil.copyfile(first, tracked)

        staged = threading.Event()
        held = []
        renamed = []
        real_replace = os.replace

        def observed(staging, target):
            if not held:                       # the first export through waits here
                held.append(True)
                staged.wait(30)
            renamed.append(digest(staging))
            real_replace(staging, target)

        landed = {}
        os.replace = observed
        try:
            thread = threading.Thread(
                target=lambda: landed.__setitem__("first", asset_door.replace(first, tracked))
            )
            thread.start()
            while not held:
                time.sleep(0.01)
            landed["second"] = asset_door.replace(second, tracked)
            staged.set()
            thread.join(30)
        finally:
            os.replace = real_replace

        self.assertEqual(len(landed), 2, "an export did not complete: {}".format(landed))
        self.assertEqual(sorted(renamed), sorted(landed.values()),
                         "an export renamed a file it did not write")
        self.assertEqual(digest(tracked), landed["first"],
                         "the last export to rename did not land its own bytes")
        self.assertEqual(
            [name for name in sorted(os.listdir(directory)) if ".door" in name], [],
            "a staging file outlived the export that made it",
        )

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

    def blender_half(self, blend, raw):
        """The Blender half, run the way the GUI adapter runs it: the caller's own Blender, the
        canon file from the one generator, the candidate written where the caller says."""
        spec = os.path.splitext(blend)[0] + ".tank.ron"
        canon = os.path.join(os.path.dirname(raw), "canon.json")
        with open(canon, "wb") as handle:
            written = subprocess.run(
                asset_door.contract(asset_door.registry_of(blend), "--canon", spec),
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

    def refuses_the_continuation(self, name, mutate):
        """Export a trio, cut an honest raw candidate beside a tracked model, then hand `--from-raw`
        something `mutate` has made untrustworthy. The tracked glb must be the file it was."""
        blend = trio(name)
        glb = os.path.splitext(blend)[0] + ".glb"
        self.assertEqual(door("export", blend)[0], 0)
        before = digest(glb)
        work = os.path.join(_WORK, name + "-work")
        shutil.rmtree(work, ignore_errors=True)
        os.makedirs(work)
        raw = os.path.join(work, "testbed.raw.glb")
        self.blender_half(blend, raw)
        self.assertTrue(os.path.isfile(toolchain.continuation_path(raw)),
                        "the source pass left no continuation token beside the candidate it cut")
        mutate(raw, blend, glb)

        code, printed = door("export", blend, None, "--from-raw", raw)
        self.assertEqual(code, 1, printed)
        self.assertIn("door.continuation", printed)
        self.assertIn("refused at from-raw", printed)
        self.assertNotIn("images ▸", printed, "an unauthenticated candidate reached the encoder")
        self.assertEqual(digest(glb), before, "the refused continuation wrote the tracked glb")
        return printed

    def token(self, raw, **fields):
        """The token beside `raw`, with `fields` written over it."""
        path = toolchain.continuation_path(raw)
        with open(path, encoding="utf-8") as handle:
            document = json.load(handle)
        document.update(fields)
        with open(path, "w", encoding="utf-8") as handle:
            json.dump(document, handle, sort_keys=True)
        return document

    def test_an_untrusted_raw_refuses(self):
        """The whole of `--from-raw`'s exposure: it enters the chain at the consumer contract, past
        every L1 law. A file of the right shape with no source pass behind it — here the tracked
        model itself, which is L2-clean by construction — must not be a way to the tracked path."""
        def untrusted(raw, _blend, glb):
            os.remove(toolchain.continuation_path(raw))
            shutil.copyfile(glb, raw)
        printed = self.refuses_the_continuation("untrusted-raw", untrusted)
        self.assertIn("no continuation token", printed)

    def test_a_continuation_whose_token_is_for_other_bytes_refuses(self):
        """The token names the bytes it was written for, so it cannot be moved onto another file —
        an honest candidate's token beside a candidate nobody linted is the attack it forecloses."""
        def swap(raw, _blend, _glb):
            with open(raw, "r+b") as handle:
                handle.seek(-1, os.SEEK_END)
                last = handle.read(1)
                handle.seek(-1, os.SEEK_END)
                handle.write(bytes([last[0] ^ 0xFF]))
        printed = self.refuses_the_continuation("stale-token", swap)
        self.assertIn("raw sha256", printed)

    def test_a_continuation_replayed_against_an_edited_blend_refuses(self):
        """The token is spent on ONE state of the source. Keep it, edit the blend, and it certifies
        a candidate cut from a model that no longer exists — which is the whole of a replay: an old
        pass's verdict spent on today's source. The blend's bytes are sealed, so the token dies with
        the edit."""
        def edit(_raw, blend, _glb):
            with open(blend, "ab") as handle:
                handle.write(b"\x00")
        printed = self.refuses_the_continuation("replayed-blend", edit)
        self.assertIn("blend sha256", printed)

    def test_a_continuation_replayed_against_an_edited_spec_sheet_refuses(self):
        """The other half of the same source: the spec sheet the canonical lists were cut from, and
        with them `L1.SPEC_REFERENCES` and `L1.SUBSTANCE_IDENTITY`. Its bytes decide an L1 verdict,
        so they are sealed like the model's."""
        def edit(_raw, blend, _glb):
            with open(os.path.splitext(blend)[0] + ".tank.ron", "a", encoding="utf-8") as handle:
                handle.write("\n")
        printed = self.refuses_the_continuation("replayed-spec", edit)
        self.assertIn("spec sha256", printed)

    def test_a_continuation_carrying_a_doctored_report_refuses(self):
        """The token CARRIES the report rather than a claim about it, and the digest is over the
        bytes it carries. Rewriting the verdict inside the token is caught by the same arithmetic
        that catches rewriting the candidate."""
        def doctor(raw, _blend, _glb):
            self.token(raw, report=report.render_json([]))
        printed = self.refuses_the_continuation("doctored-report", doctor)
        self.assertIn("report sha256", printed)

    def test_a_forged_token_whose_report_is_not_clean_refuses(self):
        """Nothing here is a signature — this is a local pipeline, and a token is only as private as
        the machine it is written on. So the report is READ, not believed: a token forged
        consistently end to end, every digest recomputed to match, still cannot certify a candidate
        whose own carried report says the source failed.
        """
        refused = report.render_json([report.Finding(
            report.Check(id="L1.MODIFIER_STACK", stage=report.Stage.SOURCE,
                         severity=report.Severity.ERROR,
                         law="every export-bound object has zero modifiers"),
            report.Subject(report.SubjectKind.OBJECT, "Hull", "Bevel"),
            "1 modifier: Bevel (BEVEL)",
            "apply it or delete it",
        )])

        def forge(raw, _blend, _glb):
            self.token(
                raw, report=refused,
                report_sha256=hashlib.sha256(refused.encode("utf-8")).hexdigest(),
            )
        printed = self.refuses_the_continuation("forged-report", forge)
        self.assertNotIn("report sha256", printed, "the forgery was caught before its own claim")
        self.assertIn("error row", printed)
        self.assertIn("L1.MODIFIER_STACK", printed)

    def test_a_forged_token_carrying_no_report_at_all_refuses(self):
        """The same reading, on a token that carries something which is not a report. A reader that
        cannot find the verdict has not found a passing one."""
        def forge(raw, _blend, _glb):
            self.token(raw, report="", report_sha256=hashlib.sha256(b"").hexdigest())
        printed = self.refuses_the_continuation("forged-nonreport", forge)
        self.assertIn("is not a report", printed)

    def test_a_continuation_cut_by_another_toolchain_refuses(self):
        """A candidate is only as pinned as the Blender that cut it, and the door launches none
        here. The token carries what that Blender MEASURED, and the pins are the door's."""
        def repin(raw, _blend, _glb):
            self.token(raw, toolchain=dict(
                self.token(raw)["toolchain"], **{"glTF exporter": "4.0.0"}
            ))
        printed = self.refuses_the_continuation("repinned", repin)
        self.assertIn("glTF exporter", printed)
        self.assertIn("4.0.0", printed)

    def test_the_lod_lane_and_the_door_pin_the_same_toolchain(self):
        """`scripts/lod/config.py` still carries its own copy of the pins: its bytes are hashed into
        every shipped manifest, so the re-export lands with a corpus regeneration. Until then the
        two declarations are held equal here, mechanically."""
        import config as lod  # noqa: PLC0415 — the LOD lane's config, only needed for this claim

        self.assertEqual(lod.EXPECTED_BLENDER, toolchain.BLENDER_VERSION)
        self.assertEqual(lod.EXPECTED_BLENDER_BUILD, toolchain.BLENDER_BUILD)
        self.assertEqual(lod.EXPECTED_GLTF_EXPORTER, toolchain.GLTF_EXPORTER_VERSION)


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
