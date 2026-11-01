"""The game's damage-model dev runner: failure statuses using fake processes, never game runs."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

RUNNER = Path(__file__).with_name("run.sh").resolve()


class RunnerTests(unittest.TestCase):
    def run_case(self, *scenarios, **switches):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "bin"
            binary.mkdir()
            server = binary / "overmatch-server"
            server.write_text("#!/bin/sh\n[ \"${FAKE_NO_LISTEN:-0}\" = 0 ] || exit 1\necho listening\nexec sleep 60\n")
            client = binary / "overmatch"
            client.write_text('''#!/bin/sh
[ "$OVERMATCH_UNATTENDED" = 1 ] && [ "$OVERMATCH_WINDOW" = headless ] || exit 90
[ "${FAKE_CLIENT_FAIL:-0}" = 0 ] || exit 7
case "$OVERMATCH_HARNESS" in *fire2b*) [ "${FAKE_SECOND_FAIL:-0}" = 0 ] || exit 8 ;; esac
exit 0
''')
            for executable in (server, client):
                executable.chmod(0o755)
            env = dict(os.environ, BIN=str(binary), OUT=str(root / "out"), **switches)
            # Fake processes exercise the worker directly; they consume no game/build capacity.
            return subprocess.run([str(RUNNER), "--holding-lock", *scenarios], env=env, text=True,
                                  capture_output=True, timeout=10)

    def test_success_requires_a_ready_server_and_successful_clients(self):
        # Automation may consume the traces only after the complete condition succeeds.
        run = self.run_case("fire2")
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertIn("fire2 OK", run.stdout)

    def test_dead_server_cannot_report_success(self):
        run = self.run_case("idle1", FAKE_NO_LISTEN="1")
        self.assertNotEqual(run.returncode, 0)
        self.assertIn("server did not become ready", run.stderr)

    def test_primary_client_failure_propagates(self):
        run = self.run_case("fire1", FAKE_CLIENT_FAIL="1")
        self.assertNotEqual(run.returncode, 0)
        self.assertIn("primary client", run.stderr)

    def test_second_client_failure_propagates(self):
        run = self.run_case("fire2", FAKE_SECOND_FAIL="1")
        self.assertNotEqual(run.returncode, 0)
        self.assertIn("second client", run.stderr)


if __name__ == "__main__":
    unittest.main()
