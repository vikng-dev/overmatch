"""Contracts of the game's destruction benchmark reporting and failure handling."""

import contextlib
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import collapse


class CollapseTests(unittest.TestCase):
    def test_speed_uses_simulated_time_over_wall_time(self):
        # Fixture: unequal wall periods prevent a mean-of-Hz implementation passing.
        rows = [dict(ticks=32, budget_us=15625, period_s=1),
                dict(ticks=64, budget_us=15625, period_s=4)]
        self.assertEqual(collapse.speed(rows), 0.3)

    def test_worst_window_requires_sustained_time_not_one_slow_period(self):
        # Fixture: one stopped period in five is 0.8x sustained, never a 0x window.
        rows = [dict(wall_s=i + 1, bursts=0, debris=dict(live=0, sleeping=0),
                     ticks=0 if i == 2 else 64, budget_us=15625, period_s=1,
                     tick_us=None, physics_us=None, send_us=None, rss_mb=None)
                for i in range(5)]
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            collapse.report(rows)
        self.assertIn("Worst sustained speed: 0.800x at 0.0–5.0 s", output.getvalue())

    def run_clocked_server(self, stall_after):
        # Fixture: completed one-second stats, then either a live stall or normal delivery.
        clock = [0.0]
        class Server:
            killed = False
            last = 0
            def poll(self):
                second = int(clock[0])
                if second > self.last and (stall_after is None or second <= stall_after):
                    row = dict(ticks=64, budget_us=15625, period_s=1,
                               debris=dict(live=0, sleeping=0), tick_us=None,
                               physics_us=None, send_us=None, rss_mb=None)
                    self.log.write("server_stats " + json.dumps(row) + "\n")
                    self.log.flush()
                    self.last = second
                return None
            def kill(self):
                self.killed = True
            def wait(self):
                return 0
        server = Server()
        def spawn(*args, **kwargs):
            server.log = kwargs["stdout"]
            return server
        def sleep(seconds):
            clock[0] = round(clock[0] + seconds, 6)
        args = SimpleNamespace(duration=10, bursts=3, period=1, sample=False)
        with tempfile.TemporaryDirectory() as directory, \
             patch("collapse.subprocess.Popen", side_effect=spawn), \
             patch("collapse.time.monotonic", side_effect=lambda: clock[0]), \
             patch("collapse.time.sleep", side_effect=sleep), patch("socket.socket"):
            try:
                collapse.run(args, Path(directory))
                return json.loads((Path(directory) / "rows.json").read_text())
            finally:
                self.assertTrue(server.killed, "the runner must reap even an alive stalled server")

    def test_trailing_stall_cannot_report_healthy_earlier_stats(self):
        # Operators must see an incomplete run when a live stall outlasts the deadline.
        output, errors = io.StringIO(), io.StringIO()
        for last in [5, 8]:
            with self.subTest(last=last), contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
                with self.assertRaisesRegex(RuntimeError, "UNMEASURED TRAILING INTERVAL"):
                    self.run_clocked_server(stall_after=last)
        self.assertNotIn("Completed", output.getvalue())
        self.assertNotIn("Worst sustained speed", output.getvalue())
        self.assertIn("COLLAPSE RUN FAILED", errors.getvalue())

    def test_normal_periodic_stats_complete_the_run(self):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            rows = self.run_clocked_server(stall_after=None)
        self.assertIn("Completed", output.getvalue())
        self.assertEqual(rows[-1]["wall_s"], 10, "drain stats arriving at the deadline")

    def test_early_exit_is_failure_and_prints_the_servers_message(self):
        class Server:
            returncode = 101
            def poll(self):
                return self.returncode

        def spawn(*args, **kwargs):
            kwargs["stdout"].write("thread main panicked: fixture invariant failed\n")
            kwargs["stdout"].write("backtrace fixture\n" * 100)
            kwargs["stdout"].flush()
            return Server()

        args = SimpleNamespace(duration=10, bursts=3, period=1, sample=False)
        with tempfile.TemporaryDirectory() as directory, patch("collapse.subprocess.Popen", side_effect=spawn):
            errors = io.StringIO()
            with contextlib.redirect_stderr(errors), self.assertRaisesRegex(RuntimeError, "EXITED EARLY"):
                collapse.run(args, Path(directory))
            self.assertIn("COLLAPSE RUN FAILED", errors.getvalue())
            self.assertIn("fixture invariant failed", errors.getvalue())


if __name__ == "__main__":
    unittest.main()
