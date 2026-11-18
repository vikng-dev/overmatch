"""Tests for the tick trace profiler (scripts/perf/trace_profile.py).

scripts/perf/tick-trace.sh profiles a trace whose server it stopped, and deletes the trace once
both profiles are written: its per-system tables rely on every record being counted or refused.

    python3 -I -B -m unittest discover -s scripts/perf -p test_trace_profile.py
"""

from __future__ import annotations

import json
import re
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from trace_profile import OutputError, Trace, TraceError, WindowError, profile  # noqa: E402


def record(ph: str, name: str, ts: float, tid: int = 1) -> str:
    """One record as tracing-chrome 0.7 writes it."""
    return json.dumps({"ph": ph, "pid": 1, "ts": ts, "name": name, "cat": "bevy", "tid": tid})


THREAD_NAME = json.dumps({"ph": "M", "pid": 1, "name": "thread_name", "tid": 1,
                          "args": {"name": "main"}})
LOG_LINE = json.dumps({"ph": "i", "pid": 1, "ts": 1.5, "name": "event src/net/server.rs:1",
                       "cat": "overmatch", "tid": 1, "s": "t"})


class TraceProfileTest(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def file(self, name: str, text: str) -> str:
        path = Path(self.dir.name) / name
        path.write_text(text)
        return str(path)

    def run_profile(self, records: list[str], w0: float, w1: float) -> int:
        trace = self.file("t.json", "[\n" + ",\n".join(records) + "\n]")
        window = self.file("w.json", json.dumps({"w0_us": w0, "w1_us": w1}))
        return profile(trace, window, str(Path(self.dir.name) / "out.json"))

    def test_an_interior_malformed_record_is_refused(self) -> None:
        """A record that does not parse, with lines after it, is refused naming its line: skipping
        it would drop a span's begin or end and shift every later span on that thread."""
        path = self.file("t.json", "[\n" + record("B", "system: name=a", 1) + ',\n{"ph":"E",\n'
                         + record("E", "system: name=a", 2) + "\n]")
        with self.assertRaisesRegex(TraceError, r"t\.json:3: malformed record"):
            list(Trace(path))

    def test_a_torn_tail_is_accepted(self) -> None:
        """A record cut off on the file's last line (the server killed mid-write) is accepted, the
        records before it read, and its line reported."""
        trace = Trace(self.file("t.json", "[\n" + record("B", "system: name=a", 1) + ",\n"
                                + record("E", "system: name=a", 2) + ',\n{"ph":"B","pid":1,"ts":3'))
        self.assertEqual([line for line, _ in trace], [2, 3])
        self.assertEqual(trace.torn_tail, 4)

    def test_an_unknown_phase_is_refused(self) -> None:
        """A record whose phase is neither B, E nor one the profiler ignores is refused naming its
        line: an async begin (`b`) skipped unseen would leave its span's time out of every tick."""
        path = self.file("t.json", "[\n" + record("b", "system: name=a", 1) + "\n]")
        with self.assertRaisesRegex(TraceError, r"t\.json:2: phase 'b'"):
            list(Trace(path))

    def test_a_phase_that_is_no_string_is_refused(self) -> None:
        """A phase of another JSON type is refused naming its line, not a TypeError from the phase
        lookup."""
        path = self.file("t.json", '[\n{"ph":[]}\n]')
        with self.assertRaisesRegex(TraceError, r"t\.json:2: a record's phase is list"):
            list(Trace(path))

    def test_timestamps_are_finite_floats_before_accounting(self) -> None:
        """The profiler stores doubles; unrepresentable trace input must refuse at its row."""
        for ts in [10 ** 400, float("inf"), float("-inf"), float("nan")]:
            with self.subTest(ts=ts):
                trace = Trace(self.file("t.json", "[\n" + record("B", "system: name=a", ts) + "\n]"))
                with self.assertRaisesRegex(TraceError, r"t\.json:2: ts .*finite.*recapture"):
                    list(trace)
        trace = Trace(self.file("t.json", "[\n" + record("B", "system: name=a", 1) + "\n]"))
        self.assertIs(type(next(iter(trace))[1].ts), float)

    def test_utf8_torn_at_the_tail_is_reported_but_not_inside_the_trace(self) -> None:
        """A SIGKILL may split a UTF-8 codepoint; an interior encoding fault cannot be skipped."""
        path = Path(self.dir.name) / "t.json"
        path.write_bytes(b'[\n{"name":"\xc3')
        trace = Trace(str(path))
        self.assertEqual(list(trace), [])
        self.assertEqual(trace.torn_tail, 2)
        path.write_bytes(path.read_bytes() + b'\n]')
        with self.assertRaisesRegex(TraceError, r"t\.json:2: malformed record"):
            list(Trace(str(path)))

    def test_the_phases_it_ignores_are_skipped(self) -> None:
        """The thread-name (`M`) and instant (`i`) records Bevy's chrome trace writes are skipped,
        the spans around them read: refusing them would refuse every real trace."""
        trace = Trace(self.file("t.json", "[\n" + ",\n".join([
            THREAD_NAME, record("B", "system: name=a", 1), LOG_LINE,
            record("E", "system: name=a", 2)]) + "\n]"))
        self.assertEqual([(line, span.ph) for line, span in trace], [(3, "B"), (5, "E")])

    def test_a_window_missing_a_field_is_refused(self) -> None:
        """A window without both bounds is refused naming the file, the field and the repair,
        instead of a KeyError."""
        trace = self.file("t.json", "[\n" + record("B", "system: name=a", 1) + "\n]")
        window = self.file("w.json", json.dumps({"w0_us": 0}))
        with self.assertRaisesRegex(WindowError, r"w\.json: field w1_us is missing; .*rerun the wrapper"):
            profile(trace, window, str(Path(self.dir.name) / "out.json"))

    def test_an_end_that_does_not_close_the_innermost_span_is_refused(self) -> None:
        """An end must close its thread's innermost open span; any other pairing is refused,
        naming the line, the thread and the open span, instead of timing against the wrong
        start."""
        with self.assertRaisesRegex(TraceError, r":3: thread 1 ends system 'b' .* is system 'a'"):
            self.run_profile([record("B", "system: name=a", 1), record("E", "system: name=b", 2)],
                             0, 10)

    def test_a_span_begun_inside_the_window_and_never_ended_is_refused(self) -> None:
        """A span open when the trace ends counts nowhere, so one begun inside the window would
        leave its period short: refused, naming its begin line."""
        with self.assertRaisesRegex(TraceError, r":2: thread 1's system 'a', begun at 5\.0 µs"):
            self.run_profile([record("B", "system: name=a", 5)], 0, 10)

    def test_an_unwritable_output_path_is_refused_naming_it(self) -> None:
        """A profile path under a missing directory is refused naming the path and the repair,
        instead of an OSError traceback after the whole trace was read."""
        records = []
        for tick in range(12):
            ts = 10.0 * tick
            records += [record("B", "schedule: name=FixedMain", ts),
                        record("B", "system: name=a", ts + 1),
                        record("E", "system: name=a", ts + 2),
                        record("E", "schedule: name=FixedMain", ts + 3)]
        trace = self.file("t.json", "[\n" + ",\n".join(records) + "\n]")
        window = self.file("w.json", json.dumps({"w0_us": 0, "w1_us": 200}))
        out = Path(self.dir.name) / "missing" / "out.json"
        self.assertEqual(profile(trace, window, str(Path(self.dir.name) / "out.json")), 0)
        refusal = rf"{re.escape(str(out))}: cannot write .* create its directory"
        with self.assertRaisesRegex(OutputError, refusal):
            profile(trace, window, str(out))


if __name__ == "__main__":
    unittest.main()
