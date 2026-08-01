# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Validity-gate tests for the frame-budget analyzer.

The gates are the product here — the tables are arithmetic, but a gate that fails open lets an
invalid capture masquerade as a measurement, which is the exact failure class this harness exists
to fight. So these tests assert on the FAILING side as hard as the passing one: each writes a tiny
stream that differs from a healthy one in exactly one way, and asserts the analyzer rejects it and
names the class.

    uv run scripts/perf/test_analyze.py
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from analyze import load_stream  # noqa: E402

BUILT_IN = {"monitor": "Built-in Retina Display", "refresh_mhz": 120000, "primary": True}
ARZOPA = {"monitor": "ARZOPA", "refresh_mhz": 60000, "primary": False}


def frames(seconds: float, hz: float = 120.0, start: float = 0.0) -> list[dict]:
    """A healthy frame stream: `seconds` of rows at a steady `hz`."""
    step = 1.0 / hz
    count = int(seconds * hz)
    return [
        {"t": start + index * step, "frame_ms": step * 1000.0} for index in range(count + 1)
    ]


def write_stream(directory: Path, name: str, rows: list[dict]) -> Path:
    path = directory / f"{name}.client.jsonl"
    path.write_text("".join(json.dumps(row) + "\n" for row in sorted(rows, key=lambda r: r["t"])))
    return path


class DisplayGateTests(unittest.TestCase):
    """The 60 Hz-external failure class: frames quantized by the panel, not by the workload."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def check(self, rows: list[dict], name: str = "cond", **kwargs) -> list[float]:
        path = write_stream(self.dir, name, rows)
        return load_stream(
            path,
            kwargs.get("warmup_s", 2.0),
            kwargs.get("min_rows", 10),
            kwargs.get("expected_duration_s"),
            kwargs.get("occluded_ok", False),
            kwargs.get("display_ok", False),
        )

    def reject(self, rows: list[dict], **kwargs) -> str:
        """Run the gates expecting rejection; return what the analyzer told the operator."""
        captured = StringIO()
        with self.assertRaises(SystemExit) as exit_info, redirect_stderr(captured):
            self.check(rows, **kwargs)
        self.assertEqual(exit_info.exception.code, 1)
        return captured.getvalue()

    def test_a_120hz_stream_passes(self) -> None:
        kept = self.check(frames(6.0) + [{"t": 0.5, **BUILT_IN}])
        # 2 s of the 6 s warmup-discarded, at 120 Hz.
        self.assertGreater(len(kept), 400)

    def test_a_60hz_monitor_row_fails_and_names_the_pacing_class(self) -> None:
        message = self.reject(frames(6.0) + [{"t": 0.5, **ARZOPA}])
        self.assertIn("INVALID", message)
        self.assertIn("ARZOPA", message)
        self.assertIn("60000", message)
        self.assertIn("16.67 ms", message)

    def test_a_stream_with_no_monitor_row_fails(self) -> None:
        message = self.reject(frames(6.0))
        self.assertIn("no monitor row", message)

    def test_an_unknown_refresh_rate_fails_rather_than_passing_unproven(self) -> None:
        # bevy's Monitor::refresh_rate_millihertz is an Option; a null is recorded, never omitted,
        # and "cannot say" is treated as "cannot prove", not as "probably fine".
        row = {"t": 0.5, "monitor": "Mystery", "refresh_mhz": None, "primary": True}
        message = self.reject(frames(6.0) + [row])
        self.assertIn("unknown", message)

    def test_the_startup_park_from_the_external_panel_is_provenance_not_failure(self) -> None:
        # The window is created before the client's Startup park runs, so a window born on the
        # 60 Hz panel records it and then records the primary one a moment later. Both rows are
        # inside the warmup every analysis discards, so the capture is valid.
        rows = frames(6.0) + [{"t": 0.4, **ARZOPA}, {"t": 1.4, **BUILT_IN}]
        self.assertGreater(len(self.check(rows)), 400)

    def test_a_monitor_change_inside_the_measurement_window_fails(self) -> None:
        # Both panels are fast enough, so only the CHANGE can reject this — half the percentiles
        # came off one display and half off another, which is two machines averaged together.
        other = {"monitor": "Studio Display", "refresh_mhz": 120000, "primary": False}
        message = self.reject(frames(6.0) + [{"t": 0.5, **BUILT_IN}, {"t": 4.0, **other}])
        self.assertIn("CHANGED", message)
        self.assertIn("4.0", message)

    def test_a_mid_window_drop_to_the_60hz_panel_reports_the_pacing_class_first(self) -> None:
        # It is also a change, but the operator needs the actionable half: the frames after t=4 s
        # were paced by a 60 Hz panel.
        message = self.reject(frames(6.0) + [{"t": 0.5, **BUILT_IN}, {"t": 4.0, **ARZOPA}])
        self.assertIn("below the 100000 mHz floor", message)

    def test_a_monitor_resolved_only_after_the_window_opened_fails(self) -> None:
        # Warmup ends at t=2 s; a first row at t=4 s leaves 2 s of measured frames with no
        # display provenance at all.
        message = self.reject(frames(6.0) + [{"t": 4.0, **BUILT_IN}])
        self.assertIn("not resolved until", message)

    def test_display_ok_relaxes_the_gate_for_the_runner_s_hidden_smoke_window(self) -> None:
        self.assertGreater(len(self.check(frames(6.0), display_ok=True)), 400)

    def test_the_display_gate_does_not_disturb_the_occlusion_gate(self) -> None:
        rows = frames(6.0) + [{"t": 0.5, **BUILT_IN}, {"t": 3.0, "occluded": True}]
        message = self.reject(rows)
        self.assertIn("occluded", message)

    def test_monitor_rows_are_not_counted_as_frames(self) -> None:
        # A monitor row carries `t` like every other row; counting it as a frame would inflate the
        # row count and, worse, feed a missing `frame_ms` into the percentiles.
        with_rows = self.check(frames(6.0) + [{"t": 0.5, **BUILT_IN}, {"t": 3.5, **BUILT_IN}])
        without = self.check(frames(6.0) + [{"t": 0.5, **BUILT_IN}], name="bare")
        self.assertEqual(len(with_rows), len(without))


if __name__ == "__main__":
    unittest.main()
