# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Refusal tests for the cost-trace tables, about the game's damage-model cost harness (game state
only). A trace the tables silently misread skews a measured percentile or a per-round cost, and
the CLI still succeeds; each case below differs from a healthy trace in one way.

    uv run scripts/cost/test_analyze.py
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import analyze  # noqa: E402


def meta(tick_hz):
    return {"k": "meta", "role": "server", "tick_hz": tick_hz, "ver": "fixture", "warmup": 0}


def tick(t, us, mus):
    return {"k": "tick", "t": t, "us": us, "mus": mus, "mc": 1, "np": 1, "nt": 2, "ne": 10}


class Trace:
    """Fixture traces written to a temporary directory: each a list of rows, joined by newlines."""

    def __init__(self):
        self.directory = tempfile.TemporaryDirectory()

    def write(self, name, lines, newline_at_end=True):
        path = Path(self.directory.name) / name
        text = "\n".join(line if isinstance(line, str) else json.dumps(line) for line in lines)
        path.write_text(text + ("\n" if newline_at_end else ""))
        return path


def run(*argv):
    """The CLI's exit status, stdout and stderr."""
    out, err = StringIO(), StringIO()
    saved = sys.argv
    sys.argv = ["analyze.py", *map(str, argv)]
    try:
        with redirect_stdout(out), redirect_stderr(err):
            status = analyze.main()
    finally:
        sys.argv = saved
    return status, out.getvalue(), err.getvalue()


class Refusals(unittest.TestCase):
    def setUp(self):
        self.traces = Trace()
        self.addCleanup(self.traces.directory.cleanup)

    def test_a_malformed_interior_row_is_refused_with_its_line(self):
        # A dropped interior row would leave its tick out of every percentile.
        path = self.traces.write(
            "t.jsonl", [meta(64.0), tick(0, 100.0, 1.0), "{torn", tick(1, 900.0, 9.0)]
        )
        status, _, err = run(path)
        self.assertEqual(status, 2)
        self.assertIn(f"{path}:3", err)

    def test_a_torn_final_line_is_omitted_and_named(self):
        # The recorder may stop mid-write: the last line without a newline is all it can lose.
        path = self.traces.write("t.jsonl", [meta(64.0), tick(0, 100.0, 1.0), '{"k": "ti'],
                                 newline_at_end=False)
        status, out, err = run(path)
        self.assertEqual(status, 0, err)
        self.assertIn("torn final line omitted", err)
        self.assertIn("ticks 1", out)

    def test_a_tick_row_lacking_a_field_is_refused(self):
        row = tick(0, 100.0, 1.0)
        del row["mus"]
        path = self.traces.write("t.jsonl", [meta(64.0), row])
        status, _, err = run(path)
        self.assertEqual(status, 2)
        self.assertIn("mus", err)

    def test_a_trace_without_its_tick_rate_is_refused(self):
        # No rate may stand in for the one the trace declares.
        no_rate = meta(64.0)
        del no_rate["tick_hz"]
        path = self.traces.write("t.jsonl", [no_rate, tick(0, 100.0, 1.0)])
        status, _, err = run(path)
        self.assertEqual(status, 2)
        self.assertIn("tick_hz", err)

    def test_an_integer_beyond_float_range_is_refused_naming_its_field(self):
        # JSON admits integers no float holds; the tables compute in floats.
        huge = 10**400
        path = self.traces.write("t.jsonl", [meta(huge), tick(0, 100.0, 1.0)])
        status, _, err = run(path)
        self.assertEqual(status, 2)
        self.assertIn(f"{path}:1", err)
        self.assertIn("tick_hz", err)
        path = self.traces.write("u.jsonl", [meta(64.0), tick(0, huge, 1.0)])
        status, _, err = run(path)
        self.assertEqual(status, 2)
        self.assertIn(f"{path}:2: tick row lacks a finite us", err)

    def test_the_per_round_cost_uses_the_declared_tick_rate(self):
        # Δmarch µs/tick × ticks/s ÷ rounds/s, at the traces' own rate (DERIVED, fixture numbers).
        hz, rounds_per_s = 128.0, 32.0
        idle = self.traces.write("idle.jsonl", [meta(hz), tick(0, 100.0, 1.0), tick(1, 100.0, 1.0)])
        fire = self.traces.write("fire.jsonl", [meta(hz), tick(0, 200.0, 5.0), tick(1, 200.0, 5.0)])
        status, out, err = run(f"idle={idle}", f"fire={fire}", "--baseline", "idle",
                               "--rounds-per-s", rounds_per_s)
        self.assertEqual(status, 0, err)
        expected = (5.0 - 1.0) * hz / rounds_per_s
        self.assertIn(f"{expected:.3f} µs", out)

    def test_a_delta_across_tick_rates_is_refused(self):
        idle = self.traces.write("idle.jsonl", [meta(64.0), tick(0, 100.0, 1.0)])
        fire = self.traces.write("fire.jsonl", [meta(128.0), tick(0, 200.0, 5.0)])
        status, _, err = run(f"idle={idle}", f"fire={fire}", "--baseline", "idle")
        self.assertEqual(status, 2)
        self.assertIn("Hz", err)

    def test_a_baseline_naming_no_trace_is_refused(self):
        path = self.traces.write("fire.jsonl", [meta(64.0), tick(0, 100.0, 1.0)])
        status, _, err = run(f"fire={path}", "--baseline", "idle")
        self.assertEqual(status, 2)
        self.assertIn("idle", err)


if __name__ == "__main__":
    unittest.main()
