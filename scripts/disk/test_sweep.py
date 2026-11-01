"""Disk-sweep status using a fake checkout inventory and cargo; never sweeps a real checkout."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

RUNNER = Path(__file__).with_name("sweep.sh").resolve()


class SweepTests(unittest.TestCase):
    def test_a_failed_reaper_is_visible_and_fails_the_run(self):
        # The disk ceiling's owner must see failure rather than a false 'swept' success.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "fixture" / "target").mkdir(parents=True)
            binary = root / "bin"
            binary.mkdir()
            (binary / "git").write_text('#!/bin/sh\nprintf "worktree %s/fixture\\n" "$TEST_ROOT"\n')
            (binary / "cargo").write_text('#!/bin/sh\necho "fixture cargo sweep refused" >&2\nexit 7\n')
            for executable in binary.iterdir():
                executable.chmod(0o755)
            env = dict(os.environ, FORCE="1", TEST_ROOT=str(root), PATH=f"{binary}:{os.environ['PATH']}")
            run = subprocess.run([str(RUNNER)], env=env, capture_output=True, text=True, timeout=10)
            self.assertNotEqual(run.returncode, 0)
            self.assertIn("fixture cargo sweep refused", run.stderr)
            self.assertIn("FAILED cargo sweep", run.stderr)
            self.assertNotIn("  swept ", run.stdout)


if __name__ == "__main__":
    unittest.main()
