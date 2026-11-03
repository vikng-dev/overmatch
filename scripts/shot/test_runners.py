"""The game's damage-model test tooling: cohort admission and cleanup, using fake processes."""

import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

RUNNERS = [Path(__file__).with_name(name).resolve() for name in
           ("run-mg-armor.sh", "run-pencam-latency.sh")]


class RunnerTests(unittest.TestCase):
    def run_case(self, runner, failure="none"):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "bin"
            binary.mkdir()
            out = root / "out"
            out.mkdir()
            # A prior run's start file must never release the new cohort.
            (out / "workload.ready").touch()
            server = binary / "overmatch-server"
            server.write_text(f'''#!{sys.executable}
import os
from pathlib import Path
import time
assert os.environ['OVERMATCH_UNATTENDED'] == '1'
assert os.environ['OVERMATCH_SERVER_BIND'] == '127.0.0.1'
out = Path(os.environ['OUT'])
(out / 'server.pid').write_text(str(os.getpid()))
print('listening', flush=True)
while not (out / 'first.pid').exists(): time.sleep(0.01)
print('client connected', flush=True)
while True: time.sleep(1)
''')
            client = binary / "overmatch"
            client.write_text(f'''#!{sys.executable}
import os
from pathlib import Path
import time
assert os.environ['OVERMATCH_UNATTENDED'] == '1'
assert os.environ['OVERMATCH_WINDOW'] == 'hidden'
out = Path(os.environ['OUT'])
first = Path(os.environ['OVERMATCH_HARNESS']).name in ('target.harness.ron', 'a.harness.ron')
role = 'first' if first else 'second'
(out / (role + '.pid')).write_text(str(os.getpid()))
ready = Path(os.environ['SPIKE_WORKLOAD_READY'])
if not first:
    (out / 'second.waiting').touch()
    while not (out / 'admit-second').exists(): time.sleep(0.01)
    assert not ready.exists(), 'the first slot cannot release the cohort'
    if os.environ['FAKE_FAILURE'] == 'admission': raise SystemExit(7)
(out / (role + '.slot')).touch()
print('harness: input slot', flush=True)
while not ready.exists(): time.sleep(0.01)
assert (out / 'first.slot').exists() and (out / 'second.slot').exists()
if not first and os.environ['FAKE_FAILURE'] == 'execution': raise SystemExit(8)
(out / (role + '.done')).touch()
''')
            analyzer = binary / "uv"
            analyzer.write_text('''#!/bin/sh
[ -f "$OUT/first.done" ] && [ -f "$OUT/second.done" ] || exit 9
touch "$OUT/analyzed"
''')
            for executable in (server, client, analyzer):
                executable.chmod(0o755)
            env = dict(os.environ, BIN=str(binary), OUT=str(out), FAKE_FAILURE=failure,
                       PATH=f"{binary}:{os.environ['PATH']}")
            # Private worker entry, as in cost/test_run.py: no real game or build runs here.
            process = subprocess.Popen([str(runner), "--holding-lock"], env=env,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                       text=True, start_new_session=True)
            try:
                deadline = time.monotonic() + 10  # Fixture process-start refusal bound, not a sample.
                while not (out / "second.waiting").exists():
                    if process.poll() is not None or time.monotonic() > deadline:
                        stdout, stderr = process.communicate(timeout=1)
                        self.fail(f"second client never reached admission: {stdout}\n{stderr}")
                    time.sleep(0.01)
                self.assertFalse((out / "workload.ready").exists(), "both slots are required")
                self.assertFalse((out / "first.done").exists(), "the first lifetime is unspent")
                os.kill(int((out / "first.pid").read_text()), 0)
                (out / "admit-second").touch()
                stdout, stderr = process.communicate(timeout=10)
                self.assertEqual((out / "analyzed").exists(), failure == "none", stdout + stderr)
                if failure == "none":
                    self.assertEqual(process.returncode, 0, stdout + stderr)
                else:
                    self.assertNotEqual(process.returncode, 0, stdout + stderr)
                for pid_file in out.glob("*.pid"):
                    with self.assertRaises(ProcessLookupError, msg=f"cleanup owns {pid_file.name}"):
                        os.kill(int(pid_file.read_text()), 0)
                return stderr
            finally:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                process.wait(timeout=5)

    def test_late_admission_preserves_both_workloads(self):
        # A scripted client cannot spend its lifetime while its opponent is still loading.
        for runner in RUNNERS:
            with self.subTest(runner=runner.name):
                self.run_case(runner)

    def test_admission_failure_refuses_analysis_and_reaps_the_first_client(self):
        for runner in RUNNERS:
            with self.subTest(runner=runner.name):
                self.assertIn("second client exited before admission", self.run_case(runner, "admission"))

    def test_execution_failure_refuses_analysis(self):
        for runner in RUNNERS:
            with self.subTest(runner=runner.name):
                self.run_case(runner, "execution")


if __name__ == "__main__":
    unittest.main()
