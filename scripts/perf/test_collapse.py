"""Contracts of the game's destruction benchmark reporting and failure handling."""

import contextlib
import io
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
