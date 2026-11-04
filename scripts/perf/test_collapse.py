"""Contracts of the game's destruction benchmark reporting and failure handling."""

import contextlib
import copy
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import collapse


class CollapseTests(unittest.TestCase):
    def test_only_kernel_allocation_evidence_admits_two_vcpu(self):
        # Fixture: a wide cpuset with a two-core quota is the hosted contract, not pool sizing.
        line = "server: host available_parallelism=2 pools(compute/async_compute/io)=2/1/1 cpu_quota=2.00 cpus (200000/100000 us) cpuset=0-15"
        self.assertEqual(collapse.allocation(line), line)
        for bad in ["", line + "\n" + line, line.replace("200000", "100000"),
                    line.replace("available_parallelism=2", "available_parallelism=10"),
                    line.replace("2.00 cpus (200000/100000 us)", "unknown"),
                    line.replace("cpuset=0-15", "cpuset=0")]:
            with self.subTest(bad=bad), self.assertRaisesRegex(RuntimeError, "allocation"):
                collapse.allocation(bad)
        self.assertIn("UNPRICED LOCAL REHEARSAL", collapse.allocation(line.replace("available_parallelism=2", "available_parallelism=10"), rehearsal=True))

    def match_fixture(self):
        # Fixture: two unequal server periods, with a real link and actual ledger outcomes.
        recipe = dict(tanks=2, events=[dict(at_s=0.5, instances=["a", "b"])])
        row = dict(period_s=1, ticks=64, budget_us=15625, tick_hz=64, tanks=2,
                   clients=1, cpu_pct=80,
                   tick_us=dict(p50=1000, p95=2000, p99=3000, total=100000),
                   physics_us=dict(p50=200, p95=300, p99=400, total=25000),
                   send_us=dict(p50=10, p95=20, p99=30, total=500),
                   loadtest=dict(recipe=recipe, sequence=1, elapsed_s=1.1, window_start_s=0.1, combat=True,
                                 send_bytes=12000, awake_bodies=12, construction_pieces=20,
                                 constructions=10, events_fired=1,
                                 targets=[dict(id=i, falls=1, dust=1, chunks=3) for i in ["a", "b"]]))
        second = copy.deepcopy(row)
        second.update(period_s=2, ticks=128)
        second["loadtest"].update(sequence=2, elapsed_s=3.1, window_start_s=1.1)
        return recipe, [row, second]

    def test_missing_or_malformed_windows_and_mismatched_recipe_refuse(self):
        recipe, rows = self.match_fixture()
        text = "\n".join("server_stats " + json.dumps(r) for r in rows)
        self.assertEqual(collapse.match_rows(text, recipe), rows)
        bad = copy.deepcopy(rows)
        bad[1]["loadtest"]["sequence"] = 3
        for text in ["server_stats {", "\n".join("server_stats " + json.dumps(r) for r in bad)]:
            with self.assertRaises(RuntimeError):
                collapse.match_rows(text, recipe)
        with self.assertRaisesRegex(RuntimeError, "recipe differs"):
            collapse.match_rows("server_stats " + json.dumps(rows[0]), dict(tanks=3, events=recipe["events"]))

    def test_zero_tick_asset_admission_is_pending_but_never_a_complete_window(self):
        recipe, rows = self.match_fixture()
        pending = copy.deepcopy(rows[0])
        pending.update(ticks=0, tick_hz=0, tanks=0, clients=0, tick_us=None, physics_us=None)
        pending["loadtest"].update(elapsed_s=None, window_start_s=None, combat=False, events_fired=0)
        self.assertEqual(collapse.match_rows("server_stats " + json.dumps(pending), recipe), [pending])
        self.assertIsNone(collapse.complete_windows([pending], recipe, 3, 1))
        pending.update(tanks=recipe["tanks"], clients=1)
        pending["loadtest"].update(elapsed_s=1.1, window_start_s=0.1, combat=True)
        with self.assertRaisesRegex(RuntimeError, "no completed ticks"):
            collapse.complete_windows([pending], recipe, 3, 1)

    def test_complete_windows_require_full_span_population_combat_and_native_falls(self):
        recipe, rows = self.match_fixture()
        self.assertEqual(collapse.complete_windows(rows, recipe, 3, 1), rows)
        self.assertIsNone(collapse.complete_windows(rows[:1], recipe, 3, 1), "a healthy prefix is incomplete")
        for change in [lambda r: r.update(tanks=1), lambda r: r.update(clients=0),
                       lambda r: r.update(cpu_pct=None),
                       lambda r: r["loadtest"].update(combat=False),
                       lambda r: r["loadtest"].update(events_fired=0),
                       lambda r: r["loadtest"].update(window_start_s=1.2),
                       lambda r: r["loadtest"]["targets"][0].update(falls=0)]:
            bad = copy.deepcopy(rows)
            change(bad[-1])
            with self.assertRaises(RuntimeError):
                collapse.complete_windows(bad, recipe, 3, 1)

    def test_exact_edges_exclude_a_period_straddling_admission_despite_rounding(self):
        recipe, rows = self.match_fixture()
        partial = copy.deepcopy(rows[0])
        # Fixture: rounded period 1.0 appears wholly admitted at elapsed 1.01, but started before
        # admission. Only exact monotonic edges can refuse it as a full workload window.
        partial["loadtest"].update(elapsed_s=1.01, window_start_s=-0.01)
        rows[0]["loadtest"].update(elapsed_s=2.01, window_start_s=1.01)
        rows[1]["loadtest"].update(elapsed_s=4.01, window_start_s=2.01)
        self.assertEqual(collapse.complete_windows([partial, *rows], recipe, 4, 1), rows)

    def test_report_uses_wall_totals_for_shares_and_reports_period_percentiles(self):
        _, rows = self.match_fixture()
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            collapse.report_match(rows)
        self.assertIn("1.000/2.000/3.000", output.getvalue())
        self.assertIn("25.0/75.0", output.getvalue(), "ratios of percentiles are not a time share")

    def test_schedule_refuses_reused_ids_and_reordered_or_nonfinite_times(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "schedule.json"
            good = dict(events=[dict(at_s=1, instances=["a", "b"])])
            path.write_text(json.dumps(good))
            self.assertEqual(collapse.recipe_for(path, 30), dict(tanks=30, **good))
            for events in [[dict(at_s=1, instances=["a", "a"])],
                           [dict(at_s=float("nan"), instances=["a"])],
                           [dict(at_s=1, instances=["a"]), dict(at_s=0, instances=["b"])],
                           [dict(at_s=1, instances=["a"]), dict(at_s=2, instances=["a"])]]:
                path.write_text(json.dumps(dict(events=events)))
                with self.assertRaises(ValueError):
                    collapse.recipe_for(path, 30)

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
