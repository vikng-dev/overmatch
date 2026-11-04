"""The build queue refuses malformed operator input and serializes its callers."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class BuildLockTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.script = self.root / "build-lock"
        source = Path(__file__).with_name("build-lock").read_text()
        # Exercise the production script without contending with live builds.
        self.script.write_text(source.replace("/tmp/overmatch-cargo.lock", str(self.root / "build.lock")))
        self.script.chmod(0o755)
        self.env = os.environ.copy()
        self.env.pop("OVERMATCH_CARGO_LOCK_WAIT_S", None)
        self.marker = self.root / "ran"

    def invoke(self, setting):
        env = self.env if setting is None else {**self.env, "OVERMATCH_CARGO_LOCK_WAIT_S": setting}
        return subprocess.run(
            [str(self.script), "/usr/bin/touch", str(self.marker)],
            env=env, capture_output=True, text=True, timeout=5,
        )

    def test_unset_or_unsigned_integer_timeout_runs_the_command(self):
        for setting in (None, "0", "2"):
            with self.subTest(setting=setting):
                result = self.invoke(setting)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(self.marker.is_file())
                self.marker.unlink()

    def test_empty_or_malformed_timeout_names_the_setting_and_runs_nothing(self):
        for setting in ("", "typo", "-1", "1.5", " 2", "2\n"):
            with self.subTest(setting=setting):
                result = self.invoke(setting)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("OVERMATCH_CARGO_LOCK_WAIT_S", result.stderr)
                self.assertFalse(self.marker.exists())

    def start(self, *command):
        process = subprocess.Popen(
            [str(self.script), *command],
            env={**self.env, "OVERMATCH_CARGO_LOCK_WAIT_S": "30"},
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        self.addCleanup(process.communicate)
        self.addCleanup(process.kill)
        return process

    def test_a_caller_waits_for_the_holder_then_runs(self):
        holder = self.start(sys.executable, "-u", "-c", "print('held'); input()")
        self.assertEqual(holder.stdout.readline(), "held\n")
        queued = self.start("/usr/bin/touch", str(self.marker))
        with self.assertRaises(subprocess.TimeoutExpired):
            queued.communicate(timeout=0.2)
        self.assertFalse(self.marker.exists(), "a queued command cannot run under another holder")
        holder.communicate("\n", timeout=5)
        self.assertEqual(holder.returncode, 0)
        _, stderr = queued.communicate(timeout=5)
        self.assertEqual(queued.returncode, 0, stderr)
        self.assertTrue(self.marker.is_file())


if __name__ == "__main__":
    unittest.main()
