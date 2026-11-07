"""Contracts of the game's destruction benchmark reporting and failure handling."""

import contextlib
import copy
import io
import json
import subprocess
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import collapse


def tick_fixture(i, tanks):
    # Fixture: unequal ticks expose percentile-of-percentile and nested-cost bugs.
    return dict(tick=i, tick_us=100*i, physics_us=30, sleep_us=10,
                intake_us=5, structure_us=15, refit_us=10, publish_us=5, contacts_us=2,
                sweep_us=3, crumble_us=7, collector_us=1, dt_s=0.1, awake=4-i, tanks=tanks,
                phases_us=dict(fixed_update_us=10, physics_step_us=28, construction_ecs_us=10, fixed_last_us=2))


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
                                 targets=[dict(id=i, falls=1, dust=1, chunks=3) for i in ["a", "b"]],
                                 ticks=[tick_fixture(1, 2)], sends=[dict(tick=1, us=10)]))
        second = copy.deepcopy(row)
        second.update(period_s=2, ticks=128)
        second["loadtest"].update(sequence=2, elapsed_s=3.1, window_start_s=1.1,
                                  ticks=[tick_fixture(2, 2)], sends=[])
        return recipe, [row, second]

    def run_hosted_batches(self, statuses, *, complete=True, complete_after=2, exit_client=False):
        # Fixture: server timestamps cover three seconds, fetched tens of seconds later.
        recipe, rows = self.match_fixture()
        host = "server: host available_parallelism=2 cpu_quota=2.00 cpus (200000/100000 us) cpuset=0-15"
        clock, polls, children, timeouts = [0.0], [], [], []
        args = SimpleNamespace(target="192.0.2.1:5888", deployment="fixture", tanks=2,
                               clients=1, duration=3, startup_grace=7 * collapse.HOSTED_LOG_POLL_S, rehearsal=False,
                               bin=Path("/fixture/bin"), replication_trace=False)
        class Client:
            killed = False
            def poll(self):
                return 1 if exit_client and clock[0] >= 1 else None
            def kill(self):
                self.killed = True
            def wait(self):
                return 0
        def spawn(*args, **kwargs):
            kwargs["stdout"].write("client: connected (entity fixture)\nharness: released\n")
            kwargs["stdout"].flush()
            child = Client()
            children.append(child)
            return child
        def fetch(command, **kwargs):
            polls.append(clock[0])
            timeouts.append(kwargs["timeout"])
            code = statuses[min(len(polls) - 1, len(statuses) - 1)]
            if code:
                raise subprocess.CalledProcessError(code, command)
            text = host
            if len(polls) > 1:
                batch = rows if complete and len(polls) >= complete_after else rows[:1]
                text += "\n" + "\n".join("server_stats " + json.dumps(r) for r in batch)
            Path(command[-1]).write_text(text)
            return subprocess.CompletedProcess(command, 0)
        def sleep(seconds):
            clock[0] = round(clock[0] + seconds, 6)
        with tempfile.TemporaryDirectory() as directory, \
             patch("collapse.subprocess.Popen", side_effect=spawn), \
             patch("collapse.subprocess.run", side_effect=fetch), \
             patch("collapse.time.monotonic", side_effect=lambda: clock[0]), \
             patch("collapse.time.sleep", side_effect=sleep), \
             contextlib.redirect_stdout(io.StringIO()) as output, \
             contextlib.redirect_stderr(io.StringIO()) as errors:
            try:
                collapse.run_match(args, Path(directory), recipe)
                self.assertEqual(json.loads((Path(directory) / "rows.json").read_text()), rows)
                self.assertEqual(json.loads((Path(directory) / "tick-attribution.json").read_text())["ticks"],
                                 [t for r in rows for t in r["loadtest"]["ticks"]])
                self.assertIn("COMPLETE:", output.getvalue())
            finally:
                self.assertTrue(all(c.killed or c.poll() is not None for c in children))
                self.hosted_polls, self.hosted_clock = polls, clock[0]
                self.hosted_errors = errors.getvalue()
                self.hosted_timeouts = timeouts
                self.hosted_deadline = (2 * args.startup_grace + args.duration
                                        if children else args.startup_grace)
        return rows

    def test_hosted_batched_delivery_closes_on_server_time(self):
        rows = self.run_hosted_batches([0], complete_after=3)
        self.assertEqual(len(self.hosted_polls), 3, "a delayed prefix cannot close the window")
        for previous, current in zip(self.hosted_polls, self.hosted_polls[1:]):
            self.assertGreaterEqual(current - previous, collapse.HOSTED_LOG_POLL_S)
        self.assertEqual(rows[-1]["loadtest"]["elapsed_s"], 3.1)
        self.assertGreater(self.hosted_clock, rows[-1]["loadtest"]["elapsed_s"])

    def test_hosted_rate_limit_backs_off_then_completes(self):
        for statuses in ([0, 75, 0], [75, 0], [0, 75, 0, 75, 0]):
            with self.subTest(statuses=statuses):
                self.run_hosted_batches(statuses, complete_after=len(statuses))
                for limited, status in enumerate(statuses):
                    if status == 75:
                        self.assertGreaterEqual(self.hosted_polls[limited + 1] - self.hosted_polls[limited], 2 * collapse.HOSTED_LOG_POLL_S)
                self.assertIn("rate limited", self.hosted_errors)
                self.assertNotIn("MATCH RUN INVALID", self.hosted_errors)

    def test_hosted_incomplete_batch_and_repeated_rate_limits_keep_deadline(self):
        for statuses in ([0], [0, 75], [75]):
            with self.subTest(statuses=statuses), self.assertRaisesRegex(RuntimeError, "deadline"):
                self.run_hosted_batches(statuses, complete=False)
            self.assertEqual(self.hosted_clock, self.hosted_deadline)
            self.assertTrue(all(0 < timeout <= self.hosted_deadline - polled
                                for polled, timeout in zip(self.hosted_polls, self.hosted_timeouts)))
        cadence = collapse.HOSTED_LOG_POLL_S
        intervals = [current - previous for previous, current in zip(self.hosted_polls, self.hosted_polls[1:])]
        self.assertEqual(intervals, [2 * cadence, 4 * cadence])
        self.assertTrue(all(current > previous for previous, current in zip(intervals, intervals[1:])),
                        "repeated 429s must increase the retry interval")

    def test_hosted_other_log_errors_fail_fast(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.run_hosted_batches([0, 1, 0])
        self.assertEqual(len(self.hosted_polls), 2)

    def test_hosted_client_exit_is_seen_between_log_polls(self):
        with self.assertRaisesRegex(RuntimeError, "client exited"):
            self.run_hosted_batches([0], exit_client=True)
        self.assertLess(self.hosted_clock, 2)
        self.assertEqual(len(self.hosted_polls), 1)

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


class RamTests(unittest.TestCase):
    def fixture(self):
        # Fixture: unequal sends expose percentile-of-percentile bugs.
        recipe = collapse.ram_recipe(collapse.ROOT / "scripts/perf/ram.json")
        ticks = [tick_fixture(i, 1) for i in range(1, 5)]
        ram = dict(recipe=recipe, plan=dict(minimum_m_s=9, governed_m_s=10, runup_m=20,
                                           spawn=[0, 0, 0], rotation=[0, 0, 0, 1]),
                   impact=dict(tick=2, speed_m_s=10, normal_speed_m_s=10), done="deadline", ticks=ticks,
                   sends=[dict(tick=1, us=1000), dict(tick=3, us=40)])
        return recipe, ram

    def text(self, ram):
        return "server_stats " + json.dumps(dict(ram=ram))

    def test_replication_histograms_keep_oversized_mutations_separate_from_transport(self):
        # Fixture: identical envelopes to different clients are both traffic; bins drain once.
        text = "\n".join([
            "TRACE send_server_packets: sending 28 bytes on channel_idx=1 to a",
            "TRACE send_server_packets: sending 28 bytes on channel_idx=1 to b",
            "TRACE send_server_packets: sending 200000 bytes on channel_idx=1 to a",
            "TRACE send_server_packets: sending 52 bytes on channel_idx=0 to a",
            'server_stats {"loadtest":{"sequence":1}}',
            'server_stats {"loadtest":{"sequence":2}}',
        ])
        rows = collapse.replication_windows(text)
        self.assertEqual(rows[0]["channels"]["1"],
                         dict(messages=3, bytes=200056, sizes={28: 2, 200000: 1}))
        self.assertEqual(rows[0]["channels"]["0"]["bytes"], 52)
        self.assertEqual(rows[1]["channels"], {})
        with self.assertRaisesRegex(RuntimeError, "byte trace missing"):
            collapse.replication_windows('server_stats {"loadtest":{"sequence":1}}')
        with self.assertRaisesRegex(RuntimeError, "line 1.*malformed replication"):
            collapse.replication_windows(text.replace("sending 28 bytes", "sending malformed bytes", 1))
        with self.assertRaisesRegex(RuntimeError, "replication window"):
            collapse.replication_windows(text.replace('"sequence":2', '"sequence":3'))

    def test_match_trace_uses_shared_validation_and_attributes_each_overrun(self):
        # Fixture: one wave straddles stats lines; the reader must retain exact tick identity.
        _, ram = self.fixture()
        ticks = ram["ticks"]
        for tick in ticks:
            tick.update(dt_s=0.00025, elapsed_s=tick["tick"] / 10, events_fired=1, tanks=2)
        recipe = dict(tanks=2, events=[dict(at_s=0.1, instances=["a", "b"])])
        rows = [dict(loadtest=dict(ticks=ticks[:2], sends=ram["sends"][:1])),
                dict(loadtest=dict(ticks=ticks[2:], sends=ram["sends"][1:]))]
        result = collapse.match_tick_summary(rows, recipe)
        self.assertEqual([t["tick"] for t in result["over_budget_ticks"]], [3, 4])
        self.assertEqual(result["ticks"], ticks)
        self.assertEqual(result["sends"], ram["sends"])
        for row in result["over_budget_ticks"]:
            self.assertAlmostEqual(sum(row["parts_us"].values()), row["tick_us"])
        broken = copy.deepcopy(rows)
        del broken[1]["loadtest"]["ticks"][0]["phases_us"]
        with self.assertRaisesRegex(ValueError, "missing tick timing fields.*phases_us"):
            collapse.match_tick_summary(broken, recipe)
        broken = copy.deepcopy(rows)
        broken[1]["loadtest"]["ticks"][0]["tick"] += 1
        with self.assertRaisesRegex(RuntimeError, "instrument tick"):
            collapse.match_tick_summary(broken, recipe)
        with self.assertRaisesRegex(RuntimeError, "tick trace empty"):
            collapse.match_tick_summary([dict(loadtest=dict(ticks=[], sends=[]))], recipe)
        with self.assertRaisesRegex(RuntimeError, "rebuild the instrumented server"):
            collapse.match_tick_summary([dict(loadtest=dict(sequence=1))], recipe)

    def test_ram_percentile_and_additive_split_use_only_contact_ticks(self):
        recipe, ram = self.fixture()
        result = collapse.ram_summary(self.text(ram), recipe)
        self.assertEqual(result["first_tick"], 2)
        self.assertAlmostEqual(result["window_s"], 0.3)
        self.assertEqual(result["p99_us"], 400)
        self.assertEqual(result["worst_tick"]["tick"], 4)
        self.assertEqual(result["peak_awake"], 2)
        self.assertEqual(result["send_us"], 40)
        self.assertAlmostEqual(sum(result["share_pct"].values()), 100)
        self.assertAlmostEqual(result["share_pct"]["physics_step"], 100*54/900)
        self.assertAlmostEqual(result["share_pct"]["sleep"], 100*30/900)
        self.assertAlmostEqual(result["share_pct"]["sweep"], 100*9/900)
        self.assertAlmostEqual(result["share_pct"]["crumble"], 100*21/900)
        self.assertEqual(result["stage_us"]["unattributed"], 609)

    def test_each_over_budget_tick_reports_its_own_gap_including_the_approach(self):
        # Fixture: all ticks exceed the budget, including the tick before measured contact.
        recipe, ram = self.fixture()
        for tick in ram["ticks"]:
            tick["dt_s"] = 0.00005
        result = collapse.ram_summary(self.text(ram), recipe)
        self.assertEqual([t["tick"] for t in result["over_budget_ticks"]], [1, 2, 3, 4])
        for tick, measured in zip(ram["ticks"], result["over_budget_ticks"]):
            self.assertEqual(measured["parts_us"]["unattributed"], tick["tick_us"] - 97)
            self.assertAlmostEqual(measured["attributed_pct"], 100*97/tick["tick_us"])
        self.assertEqual(result["worst_tick_attribution"], result["over_budget_ticks"][-1])

    def test_capture_peak_is_visible_when_it_precedes_the_contact_window(self):
        recipe, ram = self.fixture()
        ram["ticks"][0]["tick_us"] = 1000  # Fixture: an approach scheduling stall.
        result = collapse.ram_summary(self.text(ram), recipe)
        self.assertEqual(result["worst_tick_attribution"]["tick"], 4)
        self.assertEqual(result["worst_capture_tick_attribution"]["tick"], 1)

    def test_new_stage_and_phase_fields_are_additive_without_another_reader_allowlist(self):
        recipe, ram = self.fixture()
        for tick in ram["ticks"]:
            tick["new_law_us"] = 2  # Fixture: a newly instrumented leaf.
            tick["phases_us"]["new_phase_us"] = 1
        result = collapse.ram_summary(self.text(ram), recipe)
        self.assertEqual(result["stage_us"]["new_law"], 6)
        self.assertEqual(result["stage_us"]["new_phase"], 3)
        self.assertEqual(result["stage_us"]["unattributed"], 600)

    def test_ram_refuses_missing_nonfinite_overlapping_or_changed_timing_schema(self):
        recipe, ram = self.fixture()
        for change in (lambda t: t.pop("sweep_us"), lambda t: t.pop("crumble_us"),
                       lambda t: t.pop("phases_us"), lambda t: t.update(phases_us={}),
                       lambda t: t["phases_us"].update(physics_step_us=9),
                       lambda t: t["phases_us"].update(fixed_update_us=1000),
                       lambda t: t["phases_us"].update(fixed_update_us=float("nan")),
                       lambda t: t["phases_us"].update(sleep_us=1),
                       lambda t: t["phases_us"].update(unattributed_us=1),
                       lambda t: t.update(unattributed_us=1),
                       lambda t: t["phases_us"].update(intake_us=1),
                       lambda t: t["phases_us"].update(new_phase_us=1)):
            changed = copy.deepcopy(ram)
            changed["done"] = None
            change(changed["ticks"][1])
            with self.subTest(change=change), self.assertRaisesRegex(RuntimeError, "malformed ram stats"):
                collapse.ram_summary(self.text(changed), recipe)

    def test_ram_refuses_slow_impact_missing_ticks_and_incompatible_logs(self):
        recipe, ram = self.fixture()
        for field, value in [("impact", dict(tick=2, speed_m_s=8, normal_speed_m_s=8)), ("done", "unknown")]:
            changed = dict(ram, **{field: value})
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                collapse.ram_summary(self.text(changed), recipe)
        for change in (lambda r: r["ticks"].pop(1),
                       lambda r: r["ticks"][0].update(tanks=2),
                       lambda r: r["ticks"][0].update(physics_us=1),
                       lambda r: r["ticks"][0].update(tick_us=float("nan")),
                       lambda r: r.update(recipe={})):
            changed = copy.deepcopy(ram)
            change(changed)
            with self.assertRaises(RuntimeError):
                collapse.ram_summary(self.text(changed), recipe)
        for bad in ("server_stats {", self.text({})):
            with self.assertRaisesRegex(RuntimeError, "malformed"):
                collapse.ram_summary(bad, recipe)

    def test_slow_impact_refuses_before_observation_deadline(self):
        recipe, ram = self.fixture()
        ram.update(done=None, impact=dict(tick=2, speed_m_s=8, normal_speed_m_s=8))
        with self.assertRaisesRegex(RuntimeError, "full-speed admission"):
            collapse.ram_summary(self.text(ram), recipe)

    def test_ram_waits_for_server_completion_and_accepts_rest_or_deadline(self):
        recipe, ram = self.fixture()
        self.assertIsNone(collapse.ram_summary(self.text(dict(ram, done=None)), recipe))
        for end in ("rest", "deadline"):
            self.assertEqual(collapse.ram_summary(self.text(dict(ram, done=end)), recipe)["end"], end)

    def test_ram_refuses_nonfinite_or_reordered_send_samples(self):
        recipe, ram = self.fixture()
        for sends in ([dict(tick=3, us=float("nan"))], [dict(tick=3, us=1), dict(tick=2, us=1)]):
            with self.assertRaisesRegex(RuntimeError, "malformed"):
                collapse.ram_summary(self.text(dict(ram, sends=sends)), recipe)

    def test_ram_refuses_nonfinite_numbers_before_completion(self):
        # The runner must refuse invalid numbers in samples and metadata in an unfinished prefix.
        recipe, ram = self.fixture()
        for section, fields in (("plan", ("minimum_m_s", "governed_m_s", "runup_m", "spawn", "rotation")),
                                ("impact", ("speed_m_s", "normal_speed_m_s")),
                                ("ticks", ("tick_us", "physics_us", "sleep_us", "intake_us", "structure_us", "refit_us", "publish_us", "contacts_us", "sweep_us", "crumble_us", "collector_us", "dt_s")),
                                ("sends", ("us",))):
            for field in fields:
                for bad in (float("nan"), float("inf"), float("-inf"), True, 10**400, -(10**400)):
                    changed = copy.deepcopy(ram)
                    changed["done"] = None
                    values = changed[section]
                    if isinstance(values, list):
                        values = values[0]
                    if field in ("spawn", "rotation"):
                        values[field][0] = bad
                    else:
                        values[field] = bad
                    with self.subTest(section=section, field=field, bad=bad), self.assertRaisesRegex(RuntimeError, "log line 1.*" + field):
                        collapse.ram_summary(self.text(changed), recipe)

    def test_ram_impact_requires_membership_in_the_captured_ticks(self):
        recipe, ram = self.fixture()
        for bad in (0, -1, 1.5, 5, True, float("nan")):
            changed = copy.deepcopy(ram)
            changed["impact"]["tick"] = bad
            with self.subTest(tick=bad), self.assertRaisesRegex(RuntimeError, "impact.tick"):
                collapse.ram_summary(self.text(changed), recipe)

    def test_local_log_reader_parses_only_new_snapshots(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "server.log"
            reader = collapse.ServerLogs(path, None)
            self.assertEqual(reader.snapshot(100), "")
            path.write_text("first")
            self.assertEqual(reader.snapshot(100), "first")
            self.assertIsNone(reader.snapshot(100))
            path.write_text("second snapshot")
            self.assertEqual(reader.snapshot(100), "second snapshot")

    def test_ram_recipe_refuses_typos_and_invalid_operator_domains(self):
        recipe, _ = self.fixture()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "ram.json"
            for key, bad in [("typo", 1), ("instance", ""), ("runup_margin", 0.5),
                             ("minimum_speed_fraction", 1.1), ("settle_s", 0),
                             ("deadline_s", -1), ("heading_deg", True)]:
                path.write_text(json.dumps(dict(recipe, **{key: bad})))
                with self.subTest(field=key), self.assertRaises(ValueError):
                    collapse.ram_recipe(path)


if __name__ == "__main__":
    unittest.main()
