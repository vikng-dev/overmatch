"""Offline deploy-contract and cost-guard checks: real session.sh, stubbed outbound commands."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class SessionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        script_dir = self.root / "scripts" / "edgegap"
        script_dir.mkdir(parents=True)
        self.script = script_dir / "session.sh"
        shutil.copy(Path(__file__).with_name("session.sh"), self.script)
        contract = self.root / "backend" / "contract"
        contract.mkdir(parents=True)
        shutil.copy(Path(__file__).resolve().parents[2] / "backend/contract/server-env.json", contract)
        binary = self.root / "bin"
        binary.mkdir()
        # Never start a watchdog process or touch the network; the shipped script still arms one.
        for name in ["sleep", "nohup"]:
            (binary / name).write_text("#!/bin/sh\nexit 0\n")
        (binary / "curl").write_text("""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
method = args[args.index('-X') + 1]
url = args[args.index('-X') + 2]
body = json.loads(args[args.index('--data-binary') + 1]) if '--data-binary' in args else None
with open(os.environ['CALLS'], 'a') as f:
    f.write(json.dumps({'method': method, 'url': url, 'body': body}) + '\\n')
if '/container-logs' in url:
    code = int(os.environ.get('LOG_STATUS', '200'))
    response = {'logs': 'fixture server log\\n'} if code == 200 else {'message': 'fixture log refusal'}
elif '/v1/deployments?' in url:
    scenario = os.environ['SCENARIO']
    responses = {'invalid_json': '{', 'missing_data': {'pagination': {'has_next': False}},
                 'missing_pagination': {'data': []}, 'invalid_row': {'data': [None], 'pagination': {'has_next': False}}}
    if scenario == 'empty':
        code, response = 200, {'data': [], 'pagination': {'has_next': False}}
    elif scenario == 'reconciled':
        posts = [json.loads(line) for line in Path(os.environ['CALLS']).read_text().splitlines() if json.loads(line)['method'] == 'POST']
        code, response = 200, {'data': [{'request_id': 'fixture', 'tags': posts[0]['body']['tags']}], 'pagination': {'has_next': False}}
    elif scenario == 'later_invalid_json':
        code = 200
        response = {'data': [{'request_id': 'other', 'tags': []}], 'pagination': {'has_next': True}} if 'page=1&' in url else '{'
    else:
        code, response = (200, responses[scenario]) if scenario in responses else (503, {'message': 'listing unavailable'})
elif method == 'POST':
    code, response = (502, {}) if os.environ.get('SCENARIO') != 'ready' else (200, {'request_id': 'fixture'})
else:
    code, response = 200, {'current_status': 'Status.READY', 'running': True, 'public_ip': '192.0.2.1',
                         'ports': {'game': {'external': 5888}}, 'tags': ['profile-fixture']}
Path(args[args.index('-o') + 1]).write_text(response if isinstance(response, str) else json.dumps(response))
print(code, end='')
""")
        for executable in binary.iterdir():
            executable.chmod(0o755)
        token = self.root / "fixture.token"
        token.write_text("fixture-only-token")
        self.calls_path = self.root / "calls.jsonl"
        self.env = dict(os.environ, PATH=f"{binary}:{os.environ['PATH']}",
                        EDGEGAP_TOKEN_FILE=str(token), EDGEGAP_SESSION_DIR=str(self.root / "sessions"),
                        CALLS=str(self.calls_path), SCENARIO="ready")

    def deploy(self, *args):
        return subprocess.run(["bash", str(self.script), "deploy", "fixture-version", "--city", "Paris", *args],
                              env=self.env, text=True, capture_output=True, timeout=10)

    def calls(self):
        return [json.loads(line) for line in self.calls_path.read_text().splitlines()] if self.calls_path.exists() else []

    def test_logs_rate_limit_has_distinct_status_and_preserves_snapshot(self):
        # Log consumers retry only HTTP 429; every refusal preserves the last good snapshot.
        dest = self.root / "server.log"
        for http_status, expected in [(429, 75), (503, 1), (404, 1), (200, 0)]:
            with self.subTest(http_status=http_status):
                dest.write_text("previous snapshot")
                self.env["LOG_STATUS"] = str(http_status)
                run = subprocess.run(["bash", str(self.script), "logs", "fixture", str(dest)],
                                     env=self.env, text=True, capture_output=True, timeout=10)
                self.assertEqual(run.returncode, expected, run.stderr)
                self.assertEqual(dest.read_text(), "fixture server log\n\n" if http_status == 200 else "previous snapshot")
                if http_status == 429:
                    self.assertIn("rate limited", run.stderr)
                    self.assertIn("HTTP 429", run.stderr)
                self.assertFalse(dest.with_suffix(".log.tmp").exists())

    def test_stats_remains_a_non_hosted_lever(self):
        # Profiling operators can set stats after it becomes part of the Worker's contract.
        for period in ["2", "0"]:
            with self.subTest(period=period):
                run = self.deploy("--stats-s", period)
                self.assertEqual(run.returncode, 0, run.stderr)
                variables = [c for c in self.calls() if c["method"] == "POST"][-1]["body"]["env_vars"]
                stats = [v for v in variables if v["key"] == "OVERMATCH_SERVER_STATS_S"]
                self.assertEqual(stats, [] if period == "0" else [{"key": "OVERMATCH_SERVER_STATS_S", "value": period, "is_hidden": False}])

    def test_hosted_keys_still_refuse_before_any_deploy(self):
        run = self.deploy("--env", "OVERMATCH_SERVER_ID=fixture")
        self.assertNotEqual(run.returncode, 0)
        self.assertIn("refusing OVERMATCH_SERVER_ID", run.stderr)
        self.assertEqual(self.calls(), [])

    def test_bad_watchdog_limit_refuses_before_any_deploy(self):
        for limit in ["0", "invalid"]:
            with self.subTest(limit=limit):
                run = self.deploy("--max-min", limit)
                self.assertNotEqual(run.returncode, 0)
                self.assertIn("watchdog minutes must be a positive integer", run.stderr)
                self.assertEqual(self.calls(), [])

    def test_failed_reconciliation_never_starts_another_deployment(self):
        self.env["SCENARIO"] = "lost"
        run = self.deploy("--city", "Tokyo")
        self.assertNotEqual(run.returncode, 0)
        self.assertIn("deployment outcome unknown; refusing another deploy", run.stderr)
        self.assertEqual(len([c for c in self.calls() if c["method"] == "POST"]), 1)

    def test_malformed_listing_never_becomes_an_absent_deployment(self):
        # A lost POST followed by malformed HTTP 200 must not permit a second deployment.
        for scenario in ["invalid_json", "missing_data", "missing_pagination", "invalid_row", "later_invalid_json"]:
            with self.subTest(scenario=scenario):
                self.calls_path.unlink(missing_ok=True)
                self.env["SCENARIO"] = scenario
                run = self.deploy("--city", "Tokyo")
                self.assertNotEqual(run.returncode, 0)
                self.assertIn("deployment outcome unknown; refusing another deploy", run.stderr)
                self.assertEqual(len([c for c in self.calls() if c["method"] == "POST"]), 1)

    def test_valid_listing_recovers_the_single_uncertain_deployment(self):
        self.env["SCENARIO"] = "reconciled"
        run = self.deploy("--city", "Tokyo")
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(run.stdout.strip(), "fixture")
        self.assertEqual(len([c for c in self.calls() if c["method"] == "POST"]), 1)

    def test_valid_empty_listing_is_an_absent_deployment(self):
        self.env["SCENARIO"] = "empty"
        run = self.deploy("--city", "Tokyo")
        self.assertNotEqual(run.returncode, 0)
        self.assertIn("no city turned READY", run.stderr)
        self.assertNotIn("deployment outcome unknown", run.stderr)
        self.assertEqual(len([c for c in self.calls() if c["method"] == "POST"]), 2)

    def test_bad_he_refuses_before_any_deploy(self):
        for recipe in ["2:0", "2:NaN", "2:1_0", "2:١", "4294967296:1"]:
            with self.subTest(recipe=recipe):
                run = self.deploy("--he", recipe)
                self.assertNotEqual(run.returncode, 0)
                self.assertIn("--he wants", run.stderr)
                self.assertEqual(self.calls(), [])

    def test_he_is_emitted_for_the_non_hosted_operator(self):
        run = self.deploy("--he", "2:0.5")
        self.assertEqual(run.returncode, 0, run.stderr)
        variables = [c for c in self.calls() if c["method"] == "POST"][-1]["body"]["env_vars"]
        self.assertIn({"key": "OVERMATCH_HE_STRESS", "value": "2:0.5", "is_hidden": False}, variables)


if __name__ == "__main__":
    unittest.main()
