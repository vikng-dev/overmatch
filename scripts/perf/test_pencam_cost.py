#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Tests for pencam_cost.py: the marks, the segments they bracket, the inside/outside split and
the command's gates. Run: uv run scripts/perf/test_pencam_cost.py"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import pencam_cost  # noqa: E402
from pencam_cost import Mark, Segment, clip, parse_marks, segments, split  # noqa: E402

PREFIX = "2026-10-18T12:00:00.000000Z DEBUG overmatch::damage_cam::pen_cam::game: "


def shows(kind: str, t: float, surface: str = "Corner") -> str:
    return f"{PREFIX}pen cam: the slot shows {kind} on {surface} at t={t:.3f} s (Time<Real>)"


def clears(t: float) -> str:
    return f"{PREFIX}pen cam: the slot shows nothing at t={t:.3f} s (Time<Real>)"


def run(edge: str, t: float) -> str:
    return f"{PREFIX}kill cam: the run {edge} at t={t:.3f} s (Time<Real>)"


def stream(seconds: float, hz: float = 100.0, ms_at=lambda t: 10.0) -> str:
    """A frame stream at a steady `hz`, `frame_ms` from `ms_at(t)`, with one monitor row."""
    lines = [json.dumps({"t": 0.0, "monitor": "Built-in", "refresh_mhz": 120000, "primary": True})]
    n = int(seconds * hz)
    for i in range(n + 1):
        t = i / hz
        lines.append(json.dumps({"t": t, "frame_ms": ms_at(t)}))
    return "\n".join(lines) + "\n"


class Marks(unittest.TestCase):
    def test_the_four_line_shapes_parse_and_other_lines_do_not(self) -> None:
        lines = [
            "2026-10-18 INFO something else at t=1.000 s",
            shows("Slice", 61.25),
            shows("Victim", 70.5),
            shows("Glass", 80.0, surface="World"),
            clears(84.0),
            run("starts", 50.0),
            run("ends", 66.0),
        ]
        self.assertEqual(
            parse_marks(lines),
            [
                Mark(61.25, "corner"),
                Mark(70.5, "glass"),
                Mark(80.0, "world"),
                Mark(84.0, "none"),
                Mark(50.0, "run-start"),
                Mark(66.0, "run-end"),
            ],
        )

    def test_an_unknown_kind_on_the_corner_is_an_error_not_a_guess(self) -> None:
        with self.assertRaises(ValueError):
            parse_marks([shows("Mystery", 1.0)])

    def test_every_corner_kind_is_classed(self) -> None:
        for kind in ("Glass", "Slice", "Ricochet", "Damaged"):
            self.assertEqual(parse_marks([shows(kind, 1.0)])[0].label, "corner", kind)
        self.assertEqual(parse_marks([shows("Victim", 1.0)])[0].label, "glass")


class Segments(unittest.TestCase):
    def test_a_showing_closes_at_the_next_mark(self) -> None:
        segs = segments([Mark(10.0, "corner"), Mark(13.0, "none")], 100.0)
        self.assertEqual(segs["corner"], [Segment(10.0, 13.0)])
        self.assertEqual(segs["glass"], [])
        self.assertEqual(segs["killcam"], [])

    def test_a_replacement_of_the_same_class_keeps_one_open_segment_per_mark(self) -> None:
        segs = segments([Mark(10.0, "corner"), Mark(12.0, "corner"), Mark(15.0, "none")], 100.0)
        self.assertEqual(segs["corner"], [Segment(10.0, 12.0), Segment(12.0, 15.0)])

    def test_the_glass_taking_the_slot_ends_the_corner_segment(self) -> None:
        segs = segments([Mark(10.0, "corner"), Mark(11.5, "glass"), Mark(16.0, "none")], 100.0)
        self.assertEqual(segs["corner"], [Segment(10.0, 11.5)])
        self.assertEqual(segs["glass"], [Segment(11.5, 16.0)])

    def test_a_world_showing_is_nobodys_segment(self) -> None:
        segs = segments([Mark(10.0, "world"), Mark(14.0, "none")], 100.0)
        self.assertEqual(segs, {"corner": [], "glass": [], "killcam": []})

    def test_a_showing_open_at_the_streams_end_closes_there(self) -> None:
        segs = segments([Mark(90.0, "glass")], 95.5)
        self.assertEqual(segs["glass"], [Segment(90.0, 95.5)])

    def test_the_run_brackets_the_kill_cam_and_an_unended_run_closes_at_the_end(self) -> None:
        self.assertEqual(
            segments([Mark(50.0, "run-start"), Mark(66.0, "run-end")], 100.0)["killcam"],
            [Segment(50.0, 66.0)],
        )
        self.assertEqual(segments([Mark(50.0, "run-start")], 100.0)["killcam"], [Segment(50.0, 100.0)])

    def test_a_stray_run_end_without_a_start_is_ignored(self) -> None:
        self.assertEqual(segments([Mark(66.0, "run-end")], 100.0)["killcam"], [])


class Split(unittest.TestCase):
    def test_windows_are_half_open_and_the_warmup_is_discarded(self) -> None:
        rows = [(float(t), float(t)) for t in range(0, 10)]
        inside, outside = split(rows, [Segment(3.0, 6.0)], 2.0)
        self.assertEqual(inside, [3.0, 4.0, 5.0])
        self.assertEqual(outside, [2.0, 6.0, 7.0, 8.0, 9.0])

    def test_frames_from_the_cut_on_are_discarded_and_windows_clipped(self) -> None:
        rows = [(float(t), float(t)) for t in range(0, 10)]
        inside, outside = split(rows, [Segment(3.0, 6.0)], 2.0, 8.0)
        self.assertEqual(inside, [3.0, 4.0, 5.0])
        self.assertEqual(outside, [2.0, 6.0, 7.0])
        self.assertEqual(
            clip([Segment(3.0, 6.0), Segment(7.0, 9.0), Segment(8.0, 9.0)], 8.0),
            [Segment(3.0, 6.0), Segment(7.0, 8.0)],
        )
        self.assertEqual(clip([Segment(3.0, 6.0)], None), [Segment(3.0, 6.0)])

    def test_frames_under_another_class_are_on_neither_side(self) -> None:
        rows = [(float(t), float(t)) for t in range(0, 10)]
        inside, outside = split(
            rows, [Segment(3.0, 6.0)], 2.0, busy=[Segment(3.0, 6.0), Segment(7.0, 9.0)]
        )
        self.assertEqual(inside, [3.0, 4.0, 5.0])
        self.assertEqual(outside, [2.0, 6.0, 9.0])


class Command(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def run_main(self, *argv: str) -> tuple[int, str, str]:
        out, err = StringIO(), StringIO()
        sys.argv = ["pencam_cost.py", *argv]
        with redirect_stdout(out), redirect_stderr(err):
            try:
                code = pencam_cost.main()
            except SystemExit as exit_:
                code = exit_.code if isinstance(exit_.code, int) else 1
                if isinstance(exit_.code, str):
                    err.write(exit_.code)
        return code, out.getvalue(), err.getvalue()

    def test_on_run_reports_inside_against_outside_on_its_own_marks(self) -> None:
        # 10 ms frames, 16 ms while the corner is up (60-64 s).
        (self.dir / "corner-on.client.jsonl").write_text(
            stream(80.0, ms_at=lambda t: 16.0 if 60.0 <= t < 64.0 else 10.0)
        )
        (self.dir / "corner-on.log").write_text("\n".join([shows("Slice", 60.0), clears(64.0)]) + "\n")
        code, out, err = self.run_main(
            "--stream", str(self.dir / "corner-on.client.jsonl"),
            "--log", str(self.dir / "corner-on.log"),
            "--surface", "corner", "--json", str(self.dir / "corner-on.cost.json"),
        )
        self.assertEqual(code, 0, err)
        result = json.loads((self.dir / "corner-on.cost.json").read_text())
        corner = result["surfaces"]["corner"]
        self.assertEqual(corner["segments"], [[60.0, 64.0]])
        self.assertAlmostEqual(corner["on"]["p50"], 16.0)
        self.assertAlmostEqual(corner["off"]["p50"], 10.0)
        self.assertFalse(result["windows_derived"])
        self.assertIn("MEASURED", out)

    def test_the_cut_drops_the_death_screen_and_clips_the_open_showing(self) -> None:
        # The corner is up 60-64 s and again from 68 s; the knockout at 70 s turns the frames into
        # the 5 ms death screen, which belongs to neither side of the comparison.
        (self.dir / "mp-on.client.jsonl").write_text(
            stream(
                80.0,
                ms_at=lambda t: 16.0 if 60.0 <= t < 64.0 or 68.0 <= t < 70.0 else (5.0 if t >= 70.0 else 10.0),
            )
        )
        (self.dir / "mp-on.log").write_text(
            "\n".join([shows("Slice", 60.0), clears(64.0), shows("Slice", 68.0)]) + "\n"
        )
        code, out, err = self.run_main(
            "--stream", str(self.dir / "mp-on.client.jsonl"),
            "--log", str(self.dir / "mp-on.log"),
            "--surface", "corner", "--until-s", "70",
            "--json", str(self.dir / "mp-on.cost.json"),
        )
        self.assertEqual(code, 0, err)
        result = json.loads((self.dir / "mp-on.cost.json").read_text())
        corner = result["surfaces"]["corner"]
        self.assertEqual(corner["segments"], [[60.0, 64.0], [68.0, 70.0]])
        self.assertAlmostEqual(corner["on"]["p50"], 16.0)
        self.assertAlmostEqual(corner["off"]["p50"], 10.0)
        self.assertAlmostEqual(corner["off"]["worst"], 10.0)
        self.assertEqual(result["until_s"], 70.0)
        self.assertIn("until 70 s", out)

    def test_off_run_takes_the_paired_runs_windows_and_says_derived(self) -> None:
        (self.dir / "corner-on.log").write_text("\n".join([shows("Slice", 60.0), clears(64.0)]) + "\n")
        # The on run's numbers: 16 ms in its window; the off run's same seconds cost 11 ms (the
        # workload there), so the surface's cost over that workload is 5 ms.
        on_stats = {"n": 400, "mean": 16.0, "p50": 16.0, "p95": 16.0, "p99": 16.0, "worst": 16.0}
        (self.dir / "corner-on.cost.json").write_text(
            json.dumps({"surfaces": {"corner": {"on": on_stats, "off": None}}})
        )
        (self.dir / "corner-off.client.jsonl").write_text(
            stream(80.0, ms_at=lambda t: 11.0 if 60.0 <= t < 64.0 else 10.0)
        )
        (self.dir / "corner-off.log").write_text("no marks here\n")
        code, out, err = self.run_main(
            "--stream", str(self.dir / "corner-off.client.jsonl"),
            "--log", str(self.dir / "corner-off.log"),
            "--surface", "corner", "--windows-from", str(self.dir / "corner-on.log"),
            "--paired-json", str(self.dir / "corner-on.cost.json"),
            "--json", str(self.dir / "corner-off.cost.json"),
        )
        self.assertEqual(code, 0, err)
        result = json.loads((self.dir / "corner-off.cost.json").read_text())
        self.assertTrue(result["windows_derived"])
        corner = result["surfaces"]["corner"]
        self.assertEqual(corner["segments"], [[60.0, 64.0]])
        self.assertAlmostEqual(corner["on"]["p50"], 11.0)
        self.assertAlmostEqual(corner["paired"]["delta"]["p50"], 5.0)
        self.assertIn("DERIVED", out)
        self.assertIn("paired", out)

    def test_two_surfaces_back_to_back_are_not_each_others_baseline(self) -> None:
        # Corner 60-64 s at 16 ms, glass 64-70 s at 18 ms, nothing else on screen at 10 ms.
        (self.dir / "mp-on.client.jsonl").write_text(
            stream(80.0, ms_at=lambda t: 16.0 if 60.0 <= t < 64.0 else (18.0 if 64.0 <= t < 70.0 else 10.0))
        )
        (self.dir / "mp-on.log").write_text(
            "\n".join([shows("Slice", 60.0), shows("Victim", 64.0), clears(70.0)]) + "\n"
        )
        code, out, err = self.run_main(
            "--stream", str(self.dir / "mp-on.client.jsonl"),
            "--log", str(self.dir / "mp-on.log"),
            "--surface", "corner", "--surface", "glass",
            "--json", str(self.dir / "mp-on.cost.json"),
        )
        self.assertEqual(code, 0, err)
        result = json.loads((self.dir / "mp-on.cost.json").read_text())
        corner, glass = result["surfaces"]["corner"], result["surfaces"]["glass"]
        self.assertAlmostEqual(corner["on"]["p50"], 16.0)
        self.assertAlmostEqual(glass["on"]["p50"], 18.0)
        self.assertAlmostEqual(corner["off"]["worst"], 10.0)
        self.assertAlmostEqual(glass["off"]["worst"], 10.0)
        self.assertEqual(corner["off"]["n"], glass["off"]["n"])

    def test_an_off_run_that_showed_the_surface_is_refused(self) -> None:
        (self.dir / "on.log").write_text(shows("Slice", 60.0) + "\n")
        (self.dir / "off.client.jsonl").write_text(stream(80.0))
        (self.dir / "off.log").write_text(shows("Slice", 61.0) + "\n")
        code, _, err = self.run_main(
            "--stream", str(self.dir / "off.client.jsonl"), "--log", str(self.dir / "off.log"),
            "--surface", "corner", "--windows-from", str(self.dir / "on.log"),
        )
        self.assertNotEqual(code, 0)
        self.assertIn("not an off run", err)

    def test_a_requested_surface_without_a_segment_fails(self) -> None:
        (self.dir / "on.client.jsonl").write_text(stream(80.0))
        (self.dir / "on.log").write_text(shows("Slice", 60.0) + "\n")
        code, _, err = self.run_main(
            "--stream", str(self.dir / "on.client.jsonl"), "--log", str(self.dir / "on.log"),
            "--surface", "corner", "--surface", "glass",
        )
        self.assertNotEqual(code, 0)
        self.assertIn("no glass segment", err)

    def test_a_half_written_last_row_is_forgiven_but_a_middle_one_is_not(self) -> None:
        good = stream(80.0)
        (self.dir / "tail.client.jsonl").write_text(good + '{"t": 80.01, "fra')
        (self.dir / "on.log").write_text("\n".join([shows("Slice", 60.0), clears(64.0)]) + "\n")
        code, _, err = self.run_main(
            "--stream", str(self.dir / "tail.client.jsonl"), "--log", str(self.dir / "on.log"),
            "--surface", "corner",
        )
        self.assertEqual(code, 0, err)
        lines = good.splitlines()
        lines.insert(100, '{"t": 0.995, "fra')
        (self.dir / "mid.client.jsonl").write_text("\n".join(lines) + "\n")
        code, _, err = self.run_main(
            "--stream", str(self.dir / "mid.client.jsonl"), "--log", str(self.dir / "on.log"),
            "--surface", "corner",
        )
        self.assertNotEqual(code, 0)
        self.assertIn("unparseable", err)


if __name__ == "__main__":
    unittest.main()
