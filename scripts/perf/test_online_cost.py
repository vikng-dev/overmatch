# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Tests for the online cost runner's server-side parser (scripts/perf/online_cost.py).

Each names the invariant a mutant would break and who relies on it:

    uv run scripts/perf/test_online_cost.py
"""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from online_cost import after_marker, bots_spawned, net_row, parse_stats_line  # noqa: E402

# The shape src/net/server_stats.rs prints (`stats_line`), keys in its order.
STATS = (
    'server_stats {"budget_us":15625,"clients":2,"cpu_pct":11.2,"entities":812,"overruns":0,'
    '"period_s":1.0,"projectiles":3,"rss_mb":null,"tanks":3,"tick_hz":64.0,'
    '"tick_us":{"max":900,"p50":210,"p95":480},"ticks":64}'
)


def stats(ticks: int) -> str:
    return f'server_stats {{"ticks":{ticks}}}'


class ParseStatsLine(unittest.TestCase):
    def test_reads_every_numeric_key_and_flattens_the_tick_percentiles(self):
        # The server table is built from this; a mutant that stops flattening loses the tick
        # percentiles the night run exists to read.
        parsed = parse_stats_line(STATS)
        self.assertEqual(parsed["tick_us.p95"], 480)
        self.assertEqual(parsed["tanks"], 3)
        self.assertEqual(parsed["cpu_pct"], 11.2)
        self.assertNotIn("rss_mb", parsed, "a null is a platform without the reading, not a zero")

    def test_reads_through_a_container_prefix_and_colour_codes(self):
        prefixed = "\x1b[2m2026-10-30T13:02:03Z\x1b[0m stdout F " + STATS
        self.assertEqual(parse_stats_line(prefixed)["ticks"], 64)

    def test_a_line_that_is_not_a_stats_line_is_skipped_not_fatal(self):
        # A server without the lever, or the lever's own announcement line, must yield an empty
        # table rather than kill the runner mid-night.
        self.assertIsNone(parse_stats_line("INFO server: client connected: 1.2.3.4"))
        self.assertIsNone(parse_stats_line("server: stats line every 1.0 s on stdout (OVERMATCH_SERVER_STATS_S)"))
        self.assertIsNone(parse_stats_line("server_stats {not json"))


class AfterMarker(unittest.TestCase):
    def test_the_window_is_the_lines_after_the_markers_last_occurrence(self):
        # The per-condition server row must be the window's periods, not the earlier conditions'
        # or the connect before it; a mutant taking the first occurrence, or the whole log, mixes
        # them in.
        log = [stats(1), stats(2), "INFO server: bot 7v1 knocked out; respawning in 5s", stats(2), stats(3), stats(4)]
        mode, window = after_marker(log, stats(2) + "\n")
        self.assertEqual(mode, "marked")
        self.assertEqual(window, [stats(3), stats(4)])

    def test_a_missing_marker_reads_the_whole_log_and_says_so(self):
        # A truncated pull must not silently pass off the whole log as the window.
        self.assertEqual(after_marker([stats(1)], stats(9))[0], "marker-missing")
        self.assertEqual(after_marker([stats(1)], "")[0], "whole-log")


class BotsSpawned(unittest.TestCase):
    def test_counts_spawns_not_respawns(self):
        # The night wrapper reads this to decide whether the bot lever took; a respawn counted as
        # a spawn would read one bot dying twice as two bots.
        lines = [
            "INFO server: spawned circling bot 0 tank 12v1 (OVERMATCH_BOT) at (0.0, 1.0, 12.0)",
            "INFO server: spawned circling bot 1 tank 13v1 (OVERMATCH_BOT) at (8.0, 1.0, 12.0)",
            "INFO server: bot 12v1 knocked out; respawning in 5s",
            "INFO server: respawned bot as 40v1 (was 12v1)",
            "INFO server: spawned tank 9v1 for client 77 — owner interpolates",
        ]
        self.assertEqual(bots_spawned(lines), 2)

    def test_a_count_line_reads_as_its_count(self):
        self.assertEqual(bots_spawned(["INFO server: spawned 10 bots (OVERMATCH_BOT=10)"]), 10)


class NetRow(unittest.TestCase):
    def test_rates_divide_by_the_stream_span(self):
        # The window is the frame stream's; a mutant dividing by the wall-clock edges would fold
        # the poll's lag into every rate.
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "c.window.tsv"
            path.write_text(
                "edge\tepoch_s\tspan_s\tbytes_in\tbytes_out\n"
                "start\t1000.0\t20.0\t1000\t500\n"
                "end\t1070.0\t80.0\t61000\t120500\n"
            )
            self.assertEqual(net_row(path), ["60.0", "60000", "120000", "1.0", "2.0"])

    def test_a_missing_sample_is_no_row(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "c.window.tsv"
            path.write_text("edge\tepoch_s\tspan_s\tbytes_in\tbytes_out\nstart\t1\t20\t-\t-\nend\t2\t80\t-\t-\n")
            self.assertIsNone(net_row(path))


if __name__ == "__main__":
    unittest.main()
