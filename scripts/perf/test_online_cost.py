# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Tests for the online cost runner's server-side parser (scripts/perf/online_cost.py).

Each names the invariant a mutant would break and who relies on it:

    uv run scripts/perf/test_online_cost.py
"""

from __future__ import annotations

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))

from online_cost import (  # noqa: E402
    MalformedRecord,
    after_marker,
    aggregate,
    bots_spawned,
    jitter_object,
    main,
    net_row,
    parse_stats_line,
    stress_cells,
    stress_rollup,
)

# The shape src/net/server_stats.rs prints (`stats_line`), keys in its order.
STATS = (
    'server_stats {"budget_us":15625,"clients":1,"cpu_pct":11.2,"entities":812,"overruns":0,'
    '"period_s":1.0,"projectiles":3,"rss_mb":null,"tanks":3,"tick_hz":64.0,'
    '"tick_us":{"max":900,"p50":210,"p95":480,"p99":700,"total":20000},"ticks":64,'
    '"inputs":[{"link":"9v1","arrival":{"state":"live","silence_ms":0,"gap_ms":null,'
    '"stale_threshold_ms":100,"stale_declarations":1,"stale_arrivals":2}}],'
    '"debris":{"live":0,"sleeping":0,"frozen":0}}'
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

    def test_arrival_counters_stay_with_their_links_when_the_census_reorders(self):
        # The operator's per-link table must not confuse links when the server's census reorders.
        value = json.loads(STATS.removeprefix("server_stats "))
        first = value["inputs"][0]
        other = {"link": "7v2", "arrival": first["arrival"] | {"stale_declarations": 5, "stale_arrivals": 3}}
        for entries in ([first, other], [other, first]):
            value["inputs"] = entries
            parsed = parse_stats_line("server_stats " + json.dumps(value))
            self.assertEqual(parsed["inputs.9v1.arrival.stale_declarations"], 1)
            self.assertEqual(parsed["inputs.9v1.arrival.stale_arrivals"], 2)
            self.assertEqual(parsed["inputs.7v2.arrival.stale_declarations"], 5)
            self.assertEqual(parsed["inputs.7v2.arrival.stale_arrivals"], 3)

    def test_a_missing_arrival_field_names_the_field_instead_of_reading_the_retired_counts(self):
        # A release with an obsolete census must fail visibly rather than produce an empty column.
        for field in ("stale_declarations", "stale_arrivals"):
            value = json.loads(STATS.removeprefix("server_stats "))
            entry = value["inputs"][0]
            del entry["arrival"][field]
            entry["counts"] = {"held": 1, "tick_exact": 63}
            with self.subTest(field), self.assertRaisesRegex(MalformedRecord, rf"inputs\[0\]\.arrival\.{field}"):
                parse_stats_line("server_stats " + json.dumps(value))

    def test_reads_through_a_container_prefix_and_colour_codes(self):
        prefixed = "\x1b[2m2026-10-30T13:02:03Z\x1b[0m stdout F " + STATS
        self.assertEqual(parse_stats_line(prefixed)["ticks"], 64)

    def test_a_line_that_is_not_a_stats_line_is_skipped_not_fatal(self):
        # A server without the lever, or the lever's own announcement line, must yield an empty
        # table rather than kill the runner mid-night.
        self.assertIsNone(parse_stats_line("INFO server: client connected: 1.2.3.4"))
        self.assertIsNone(parse_stats_line("server: stats line every 1.0 s on stdout (OVERMATCH_SERVER_STATS_S)"))
        self.assertIsNone(parse_stats_line("INFO overmatch::net::server_stats: freeze bound armed"))

    def test_a_record_that_does_not_parse_is_refused_not_skipped(self):
        # A skipped period would leave a server window, and the AWS harness's table, that looks
        # whole with a period missing; a line cut before its object is such a period too.
        for broken in ("server_stats {not json", STATS[:-20], "stdout F " + STATS[:40], "stdout F server_stats ",
                       "server_stats", "server_stats [1]"):
            with self.subTest(broken), self.assertRaises(MalformedRecord):
                parse_stats_line(broken)


class JitterObject(unittest.TestCase):
    def test_the_client_record_is_read_through_a_journal_prefix_and_a_broken_one_is_refused(self):
        # The AWS harness reads each client's starvation and view clock from this record alone; a
        # record dropped instead of refused would leave a window that looks covered.
        # Fixture: the current producer's full shape, including an unready stamp window.
        value = {"period_s": 1.0, "ticks": {"received": 0, "frontier": 0},
                 "arrival_ms": None, "headroom_ticks": None,
                 "holds": {"events": 0, "ms": 0.0, "longest_ms": 0.0},
                 "view_clock": {"usual_work_ms": 0.0, "stamp_interval_ms": None, "delta_star_ms": None,
                                "stamp_lag_ms": None, "applied_delta_ms": None,
                                "applied_above_latest_x_ms": None, "target_minus_applied_ms": None,
                                "target_above_latest_x_ms": None, "cuts": 0,
                                "completion": {key: 0 for key in ("completed_ticks", "sampled_ticks", "late_ticks",
                                 "worst_lag_frames", "worst_lag_ms", "evicted_incomplete",
                                 "completed_without_stamp", "unmapped_stamps", "receipt_overflow")}, "pending_ticks": 0}}
        line = '[  101.000000] host overmatch[9]: INFO overmatch::net::frontier: client_jitter ' + json.dumps(value)
        self.assertEqual(jitter_object(line), value)
        for retired in ("min_delay_ms", "starved", "snaps", "estimate_error_ticks"):
            with self.subTest(retired=retired), self.assertRaisesRegex(MalformedRecord, "unknown fields"):
                jitter_object('client_jitter ' + json.dumps(value | {retired: {}}))
        self.assertIsNone(jitter_object("INFO overmatch::net::frontier: net: view cut"))
        for broken in (line[:-40], line[:line.index("{")]):
            with self.subTest(broken), self.assertRaises(MalformedRecord):
                jitter_object(broken)


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

    def test_a_missing_log_refuses_the_bot_census_but_an_empty_log_reports_zero(self):
        # The night wrapper must not accept a fabricated zero-bot count after a failed log pull.
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "server.log"
            out, err = io.StringIO(), io.StringIO()
            with mock.patch.object(sys, "argv", ["online_cost.py", "bots", str(log)]), \
                    contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                self.assertEqual(main(), 2)
            self.assertEqual(out.getvalue(), "")
            self.assertIn(str(log), err.getvalue())
            log.write_text("")
            with mock.patch.object(sys, "argv", ["online_cost.py", "bots", str(log)]), contextlib.redirect_stdout(out):
                self.assertEqual(main(), 0)
            self.assertEqual(out.getvalue(), "0\n")

    def test_a_count_line_reads_as_its_count(self):
        self.assertEqual(bots_spawned(["INFO server: spawned 10 bots (OVERMATCH_BOT=10)"]), 10)


def debris_line(tick_p50: float, live: int) -> str:
    # The shape src/net/server_stats.rs prints with the debris stress lever armed (`stats_line`).
    return (
        'server_stats {"budget_us":15625,"clients":0,"cpu_pct":40.0,"entities":4000,"overruns":0,'
        '"period_s":1.0,"projectiles":0,"rss_mb":90.0,"tanks":30,"tick_hz":64.0,'
        f'"tick_us":{{"max":3000,"p50":{tick_p50},"p95":2500}},"ticks":64,'
        '"physics_us":{"max":900,"p50":400,"p95":700},"send_us":{"max":300,"p50":100,"p95":200},'
        f'"debris":{{"live":{live},"sleeping":120,"frozen":0,"breaches":4,"released":{40 * live}}},'
        '"inputs":[]'
        "}"
    )


class StressTable(unittest.TestCase):
    def test_the_stress_columns_read_the_keys_the_server_prints(self):
        # The night's debris table is built from these dotted keys; a renamed field on either side
        # empties its column instead of failing, so the coupling is pinned here.
        stats = aggregate([parse_stats_line(debris_line(1000, 10)), parse_stats_line(debris_line(1200, 30))])
        cells = stress_cells(stats)
        self.assertEqual(cells["tick_p50"], 1100)
        self.assertEqual(cells["phys_p95"], 700)
        self.assertEqual(cells["send_p50"], 100)
        self.assertEqual(cells["live"], 20)
        self.assertEqual(cells["live_max"], 30)
        self.assertEqual(cells["sleeping_max"], 120)
        self.assertEqual(cells["released"], 800, "released is cumulative: the window's count is last - first")

    def test_an_off_condition_has_no_stress_release_count_and_anchors_the_deltas(self):
        # The census always prints piece counts; only an armed stress lever prints `released`.
        # Every lever's change is taken from the off median, not from a neighbour's.
        off = stress_cells(aggregate([parse_stats_line(STATS)]))
        self.assertEqual(off["live"], 0)
        self.assertIsNone(off["released"])
        armed = stress_cells(aggregate([parse_stats_line(debris_line(1000, 10))]))
        rows = stress_rollup([("off", off), ("10:5", armed), ("off", off)])
        self.assertEqual([row[0] for row in rows], ["off", "10:5"])
        self.assertEqual(rows[0][1], "2")
        # tick_p50: 210 off, 1000 armed.
        self.assertEqual(rows[1][2:4], ["1000", "+790"])
        self.assertEqual(rows[0][3], "+0")


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

    def test_a_missing_sample_refuses_a_present_window(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "c.window.tsv"
            path.write_text("edge\tepoch_s\tspan_s\tbytes_in\tbytes_out\nstart\t1\t20\t-\t-\nend\t2\t80\t-\t-\n")
            with self.assertRaisesRegex(ValueError, "invalid span_s/bytes_in/bytes_out"):
                net_row(path)


if __name__ == "__main__":
    unittest.main()
