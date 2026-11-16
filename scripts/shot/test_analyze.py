# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Scientific-integrity tests for the shot trace join."""

from __future__ import annotations

import sys
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from analyze import TraceInputError, analyze, load, verification_failures  # noqa: E402


def row(kind: str, tick: int, combatant: int, fire_tick: int, **extra) -> dict:
    if kind == "fire":
        extra.setdefault("smoke", False)
    if kind == "send":
        extra.setdefault("rcpt", 1)  # FIXTURE DATA: one recipient unless a case states another.
    if kind == "path_tx":
        extra.setdefault("impact", "bounce")
        extra.setdefault("ordinal", extra.get("seq", 0))
    if kind == "path_rx":
        extra.setdefault("result", "Duplicate" if extra.get("dup") else "Inserted")
    return {"k": kind, "t": tick, "c": combatant, "w": 0, "ft": fire_tick, **extra}


def received_own(tick: int, combatant: int, fire_tick: int) -> list[dict]:
    return [{"k": "controlled", "t": tick, "c": combatant},
            {"k": "capture_end", "t": tick + 10},
            row("fire_rx", tick, combatant, fire_tick),
            row("spawn", tick, combatant, fire_tick, src="obs")]


class JoinIntegrityTests(unittest.TestCase):
    def test_same_slot_same_tick_combatants_are_independent(self) -> None:
        # Two combatants fire the same slot on the same tick. Their stable ids make both joins exact.
        server = [
            row("fire", 10, 100, 10, o=[0.0, 0.0, 0.0], cal=0.088),
            row("dmg", 12, 100, 10, hp=5.0),
            row("fire", 10, 200, 10, o=[10.0, 0.0, 0.0], cal=0.088),
            row("path_tx", 11, 200, 10, seq=0),
            # One clean observed shot proves the report still analyzes uncontaminated groups.
            row("fire", 20, 300, 20, o=[20.0, 0.0, 0.0], cal=0.0079),
        ]
        client = [
            row("fire_rx", 11, 100, 10, dup=False, o=[0.0, 0.0, 0.0]),
            row("marker", 13, 100, 10),
            row("fire_rx", 11, 200, 10, dup=False, o=[10.0, 0.0, 0.0]),
            row("path_rx", 12, 200, 10, seq=0),
            row("path_done", 12, 200, 10, seq=0),
            row("fire_rx", 21, 300, 20, dup=False),
            row("spawn", 21, 300, 20, src="obs"),
        ]

        result = analyze({"tick_hz": 64}, client, {"tick_hz": 64}, server)

        self.assertEqual(result["server_fires"], 3)
        self.assertEqual(result["expected"], 3)
        self.assertEqual(result["delivered"], 3)
        self.assertEqual(result["damage"]["server"], 1)
        self.assertEqual(result["paths"], {"delivered": 1, "completed": 1})


class StrictVerificationTests(unittest.TestCase):
    def test_own_damage_confirm_duplicate_receive_rows_reaches_one_marker(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.088, smoke=True),
            row("dmg", 12, 100, 10, hp=5.0),
        ]
        client = [
            *received_own(10, 100, 10),
            row("dmg_rx", 13, 100, 10, own=True, dup=False, dt=12),
            row("dmg_rx", 14, 100, 10, own=True, dup=True, dt=12),
            row("marker", 13, 100, 10),
        ]

        result = analyze({"tick_hz": 64}, client, {"tick_hz": 64}, server)

        self.assertEqual(
            result["damage"],
            {
                "server": 1,
                "authored_expected": 1,
                "delivered": 1,
                "marked": 1,
                "missing_delivery": 0,
                "missing_marker": 0,
                "duplicate_markers": 0,
                "stray_markers": 0,
            },
        )
        self.assertEqual(verification_failures(result), [])

    def test_strict_rejects_missing_duplicate_and_stray_shooter_markers(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.088, smoke=True),
            row("dmg", 12, 100, 10, hp=5.0),
        ]
        client = [
            *received_own(10, 100, 10),
            row("dmg_rx", 13, 100, 10, own=True, dup=False, dt=12),
            row("marker", 13, 100, 10),
            row("marker", 14, 100, 10),
            row("marker", 15, 200, 20),
        ]

        result = analyze({"tick_hz": 64}, client, {"tick_hz": 64}, server)

        self.assertEqual(result["damage"]["missing_marker"], 0)
        self.assertEqual(result["damage"]["duplicate_markers"], 1)
        self.assertEqual(result["damage"]["stray_markers"], 1)
        self.assertEqual(
            verification_failures(result),
            ["duplicate_shooter_markers=1", "stray_shooter_markers=1"],
        )

    def test_strict_rejects_missing_shooter_marker(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.088, smoke=True),
            row("dmg", 12, 100, 10, hp=5.0),
        ]
        client = [
            *received_own(10, 100, 10),
            row("dmg_rx", 13, 100, 10, own=True, dup=False, dt=12),
        ]

        result = analyze({"tick_hz": 64}, client, {"tick_hz": 64}, server)

        self.assertEqual(result["damage"]["missing_marker"], 1)
        self.assertEqual(verification_failures(result), ["missing_shooter_markers=1"])

    def test_main_gun_sanctioned_bounce_reaches_post_bounce_trail_station(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.088, smoke=True),
            row("path_tx", 12, 100, 10, seq=0),
        ]
        client = [
            row("fire_rx", 13, 100, 10), row("spawn", 13, 100, 10),
            row("path_rx", 13, 100, 10, seq=0, dup=False, bt=12),
            row("path_done", 13, 100, 10, seq=0, res="bounce", held=1),
            row("trail", 13, 100, 10, seq=0, res="post_bounce_consumed"),
        ]

        result = analyze({"tick_hz": 64}, client, {"tick_hz": 64}, server)

        self.assertEqual(result["trail"], {"consumed": 1})
        self.assertEqual(verification_failures(result), [])

    def test_strict_rejects_main_gun_trail_with_no_row_or_unrendered_result(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.088, smoke=True),
            row("path_tx", 12, 100, 10, seq=0),
            row("fire", 20, 100, 20, cal=0.088, smoke=True),
            row("path_tx", 22, 100, 20, seq=0),
        ]
        client = [
            row("fire_rx", 13, 100, 10), row("spawn", 13, 100, 10),
            row("path_rx", 13, 100, 10, seq=0, dup=False, bt=12),
            row("path_done", 13, 100, 10, seq=0, res="bounce", held=1),
            row("fire_rx", 23, 100, 20), row("spawn", 23, 100, 20),
            row("path_rx", 23, 100, 20, seq=0, dup=False, bt=22),
            row("path_done", 23, 100, 20, seq=0, res="bounce", held=1),
            row("trail", 23, 100, 20, seq=0, res="post_bounce_unrendered"),
        ]

        result = analyze({"tick_hz": 64}, client, {"tick_hz": 64}, server)

        self.assertEqual(result["trail"], {"no_row": 1, "unrendered": 1})
        self.assertEqual(
            verification_failures(result),
            ["smoke_trail_failures=2"],
        )

    def test_completed_capture_requires_playback_of_received_paths(self):
        for impact in ("bounce", "state"):
            server = [row("fire", 10, 100, 10), row("path_tx", 12, 100, 10, seq=0, impact=impact)]
            client = [row("fire_rx", 13, 100, 10), row("spawn", 13, 100, 10),
                      row("path_rx", 13, 100, 10, seq=0), {"k": "capture_end", "t": 20}]
            with self.subTest(impact=impact):
                self.assertIn("pending_public_paths=1", verification_failures(analyze({}, client, {}, server)))
                client.append(row("path_done", 15, 100, 10, seq=0))
                self.assertEqual(verification_failures(analyze({}, client, {}, server)), [])

    def test_smoke_choice_controls_ribbon_evidence_independently_of_calibre(self):
        for smoke, caliber, failures in [(True, 0.008, ["smoke_trail_failures=1"]),
                                         (False, 0.088, [])]:
            server = [row("fire", 10, 100, 10, cal=caliber, smoke=smoke),
                      row("path_tx", 12, 100, 10, seq=0)]
            client = [row("fire_rx", 13, 100, 10), row("spawn", 13, 100, 10),
                      row("path_rx", 13, 100, 10, seq=0),
                      row("path_done", 15, 100, 10, seq=0)]
            with self.subTest(smoke=smoke):
                self.assertEqual(verification_failures(analyze({}, client, {}, server)), failures)

    def test_strict_reports_every_lifecycle_failure_class(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.0079),
            row("fire", 20, 100, 20, cal=0.0079),
            row("fire", 30, 100, 30, cal=0.0079),
            row("fire", 40, 100, 40, cal=0.0079),
            row("path_tx", 42, 100, 40, seq=0),
            row("fire", 50, 100, 50, cal=0.0079),
            row("fire", 50, 200, 50, cal=0.0079),
        ]
        client = [
            row("fire_rx", 21, 100, 20, dup=False),
            row("spawn", 21, 100, 20, src="obs"),
            row("spawn", 21, 100, 20, src="obs"),
            row("fire_rx", 31, 100, 30, dup=False),
            row("fire_rx", 41, 100, 40, dup=False),
            row("spawn", 41, 100, 40, src="obs"),
        ]

        result = analyze({"tick_hz": 64}, client, {"tick_hz": 64}, server)

        self.assertEqual(
            verification_failures(result),
            [
                "lost_shots=3",
                "duplicate_shots=1",
                "no_spawn_shots=1",
                "public_path_failures=1",
            ],
        )


class OwnFireIdentityTests(unittest.TestCase):
    """The game's damage model: every client presents server shots, including its own."""

    def test_controlled_identity_scores_missing_private_receipts_without_local_fire(self):
        # MG verification must detect a missing receipt even if the client received no private fact.
        server = [row("fire", 10, 100, 10), row("dmg", 11, 100, 10)]
        client = [
            {"k": "controlled", "t": 1, "c": 100},
            row("fire_rx", 12, 100, 10),
            row("spawn", 13, 100, 10, src="obs"),
        ]
        result = analyze({}, client, {}, server)
        self.assertEqual(result["own_shots"], 1)
        self.assertEqual(result["damage"]["authored_expected"], 1)
        self.assertEqual(verification_failures(result), [
            "incomplete_capture=1", "missing_shooter_damage_confirms=1", "missing_shooter_markers=1"])

    def test_mg_capture_refuses_empty_damage_and_missing_completion_evidence(self):
        # An armor runner must not certify a run that only exercised ground contacts.
        result = analyze({}, [{"k": "controlled", "t": 1, "c": 100}], {}, [])
        self.assertEqual(verification_failures(result),
                         ["incomplete_capture=1", "no_shooter_damage=1"])

    def test_declared_shooter_cannot_pass_without_controlled_identity(self):
        result = analyze({}, [{"k": "capture_end", "t": 10, "shooter": True}], {}, [])
        self.assertEqual(verification_failures(result),
                         ["missing_controlled_identity=1", "no_shooter_damage=1"])

    def test_own_and_remote_fires_require_delivery_and_exactly_one_spawn_even_at_end(self):
        # No prediction-era echo exemption may hide a missing spawn or a final lost shot.
        server = [row("fire", 10, 100, 10), row("fire", 10, 200, 10)]
        client = [{"k": "controlled", "t": 1, "c": 100}, row("fire_rx", 12, 100, 10)]
        result = analyze({}, client, {}, server)
        self.assertEqual(verification_failures(result), ["lost_shots=1", "no_spawn_shots=1",
                                                               "incomplete_capture=1", "no_shooter_damage=1"])

    def test_strict_refuses_a_client_trace_that_ends_early(self):
        result = analyze({}, [{"k": "capture_end", "t": 100}], {},
                         [row("fire", 200, 100, 200)])
        self.assertIn("client_trace_ended_early_by_ticks=100", verification_failures(result))

    def test_quiet_observation_tail_is_capture_coverage_not_a_lost_client(self):
        server = [row("fire", 10, 100, 10), {"k": "reliable_outbox", "t": 90}]
        client = [row("fire_rx", 12, 100, 10), row("spawn", 13, 100, 10, src="obs"),
                  {"k": "capture_end", "t": 100}]
        self.assertEqual(verification_failures(analyze({}, client, {}, server)), [])

    def test_fire_with_no_connected_recipient_is_unaddressed_not_lost(self):
        server = [row("fire", 1, 100, 1),
                  {"k": "transport", "t": 1, "public_recipient_count": 0},
                  row("fire", 10, 100, 10),
                  {"k": "transport", "t": 10, "public_recipient_count": 1}]
        result = analyze({}, [{"k": "capture_end", "t": 12}], {}, server)
        self.assertEqual(result["unaddressed"], 1)
        self.assertEqual(result["expected"], 1)
        self.assertEqual(verification_failures(result), ["lost_shots=1"])


class TransportCopyTests(unittest.TestCase):
    def test_copy_counts_keep_multiple_bounces_separate_and_split_policy(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.0079),
            row("path_tx", 12, 100, 10, seq=0),
            row("path_tx", 14, 100, 10, seq=1),
            # Automatic-fire visual facts get three bounded copies.
            row("send", 10, 100, 10, s="fire", age=0, rel=False, bb=100),
            row("send", 11, 100, 10, s="fire", age=1, rel=False, bb=100),
            row("send", 12, 100, 10, s="fire", age=2, rel=False, bb=100),
            row("send", 12, 100, 10, s="path", seq=0, age=0, rel=False, bb=100),
            row("send", 13, 100, 10, s="path", seq=0, age=1, rel=False, bb=100),
            row("send", 14, 100, 10, s="path", seq=0, age=2, rel=False, bb=100),
            row("send", 14, 100, 10, s="path", seq=1, age=0, rel=False, bb=100),
            row("send", 15, 100, 10, s="path", seq=1, age=1, rel=False, bb=100),
            row("send", 16, 100, 10, s="path", seq=1, age=2, rel=False, bb=100),
            # A single-fire damage fact is one reliable application send.
            row("dmg", 17, 100, 10, hp=5.0),
            row("send", 17, 100, 10, s="dmg", age=0, rel=True, bb=None),
        ]

        result = analyze({"tick_hz": 64}, [], {"tick_hz": 64}, server)

        self.assertEqual(result["copies"]["visual"]["fire"], [3])
        self.assertEqual(result["copies"]["visual"]["path"], [3, 3])
        self.assertEqual(result["copies"]["reliable"]["dmg"], [1])
        self.assertEqual(result["copies"]["reliable"]["path"], [0, 0])
        self.assertEqual(result["missing_path_send_sequence"], 0)

    def test_missing_keyframe_send_sequence_is_not_assigned_to_bounce_zero(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.0079),
            row("path_tx", 12, 100, 10, seq=0),
            row("path_tx", 14, 100, 10, seq=1),
            row("send", 12, 100, 10, s="path", age=0, rel=False, bb=100),
        ]

        result = analyze({"tick_hz": 64}, [], {"tick_hz": 64}, server)

        self.assertEqual(result["copies"]["visual"]["path"], [0, 0])
        self.assertEqual(result["missing_path_send_sequence"], 2)

    def test_transport_summary_aggregates_whole_trace_and_preserves_config(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.0079),
            {
                "k": "transport",
                "t": 10,
                "visual_queue_before": 2,
                "visual_queue_after": 4,
                "visual_selected": 3,
                "visual_facts_send_accepted": 2,
                "visual_batches_send_accepted": 1,
                "visual_wire_bytes_send_accepted_upper_bound": 100,
                "visual_expired": 1,
                "visual_budget_deferred_producers": 5,
                "reliable_public_queued": 1,
                "private_damage_queued": 2,
                "public_recipient_count": 2,
                "public_no_recipient_facts": 3,
                "private_damage_no_recipient_facts": 1,
                "send_call_errors": 1,
                "send_call_error_facts": 2,
                "visual_copy_opportunities": 3,
                "visual_ttl_ticks": 16,
                "visual_batch_wire_limit": 1100,
                "visual_tick_wire_limit": 4400,
            },
            {
                "k": "transport",
                "t": 11,
                "visual_queue_before": 7,
                "visual_queue_after": 1,
                "visual_selected": 4,
                "visual_facts_send_accepted": 3,
                "visual_batches_send_accepted": 2,
                "visual_wire_bytes_send_accepted_upper_bound": 250,
                "visual_expired": 2,
                "visual_budget_deferred_producers": 6,
                "reliable_public_queued": 3,
                "private_damage_queued": 1,
                "public_recipient_count": 5,
                "public_no_recipient_facts": 4,
                "private_damage_no_recipient_facts": 2,
                "send_call_errors": 0,
                "send_call_error_facts": 1,
                "visual_copy_opportunities": 3,
                "visual_ttl_ticks": 16,
                "visual_batch_wire_limit": 1100,
                "visual_tick_wire_limit": 4400,
            },
        ]

        result = analyze({"tick_hz": 64}, [], {"tick_hz": 64}, server)

        self.assertEqual(result["server_fires"], 1)
        self.assertEqual(
            result["transport"],
            {
                "rows": 2,
                "max_visual_queue_depth": 7,
                "visual_selected": 7,
                "visual_facts_send_accepted": 5,
                "visual_batches_send_accepted": 3,
                "visual_wire_bytes_send_accepted_upper_bound": 350,
                "visual_expired": 3,
                "visual_budget_deferred_producers": 11,
                "reliable_public_queued": 4,
                "private_damage_queued": 3,
                "max_public_recipient_count": 5,
                "public_no_recipient_facts": 7,
                "private_damage_no_recipient_facts": 3,
                "send_call_errors": 1,
                "send_call_error_facts": 3,
                "visual_copy_opportunities": 3,
                "visual_ttl_ticks": 16,
                "visual_batch_wire_limit": 1100,
                "visual_tick_wire_limit": 4400,
            },
        )

    def test_zero_recipient_send_is_not_an_application_opportunity(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.0079),
            row("send", 10, 100, 10, s="fire", age=0, rel=False, bb=100, rcpt=0),
            row("send", 11, 100, 10, s="fire", age=1, rel=False, bb=100, rcpt=2),
        ]

        result = analyze({"tick_hz": 64}, [], {"tick_hz": 64}, server)

        self.assertEqual(result["copies"]["visual"]["fire"], [1])

    def test_strict_cli_preserves_json_and_returns_nonzero_for_a_violation(self) -> None:
        server = [row("fire", 10, 100, 10, cal=0.0079)]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            client_path = root / "client.jsonl"
            server_path = root / "server.jsonl"
            client_path.write_text("")
            server_path.write_text("\n".join(json.dumps(item) for item in server) + "\n")

            run = subprocess.run(
                [
                    sys.executable,
                    str(Path(__file__).with_name("analyze.py")),
                    "--client",
                    str(client_path),
                    "--server",
                    str(server_path),
                    "--json",
                    "--strict",
                ],
                check=False,
                capture_output=True,
                text=True,
            )

        self.assertEqual(run.returncode, 1)
        self.assertEqual(json.loads(run.stdout)["lost"], 1)
        self.assertIn("lost_shots=1", run.stderr)


class TraceInputTests(unittest.TestCase):
    def test_conflicting_controlled_ids_refuse_with_file_and_identities(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "client.jsonl"
            path.write_text('{"k":"controlled","t":1,"c":7}\n'
                            '{"k":"controlled","t":2,"c":8}\n')
            with self.assertRaisesRegex(TraceInputError, r"client.jsonl:2:.*controlled.*7.*8.*recapture"):
                load(path)

    def test_missing_shot_identity_refuses_instead_of_shrinking_denominator(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "server.jsonl"
            for field in ("t", "c", "w", "ft", "smoke"):
                fire = row("fire", 10, 100, 10, smoke=False)
                del fire[field]
                path.write_text(json.dumps(fire) + "\n")
                with self.subTest(field=field), self.assertRaisesRegex(
                        TraceInputError, rf"server.jsonl:1:.*fire.*{field}.*recapture"):
                    load(path)

    def test_only_the_final_row_may_be_torn(self):
        # The game's damage-model report must not certify a stream with missing middle evidence.
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.jsonl"
            path.write_text('{"k":"meta"}\n{torn\n')
            self.assertEqual(load(path), ({"k": "meta"}, []))
            path.write_text('{torn\n{"k":"fire"}\n')
            with self.assertRaisesRegex(ValueError, "malformed row before the tail"):
                load(path)

    def test_missing_recipient_evidence_is_refused(self):
        # The game's damage-model delivery report cannot invent a recipient for a send.
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.jsonl"
            for kind, field in [("send", "rcpt"), ("transport", "public_recipient_count")]:
                for value in (None, -1, True, 1.5, "1"):
                    item = row(kind, 10, 100, 10, **{field: value})
                    if value is None:
                        del item[field]
                    path.write_text(json.dumps(item) + "\n")
                    with self.subTest(kind=kind, value=value), self.assertRaisesRegex(
                            TraceInputError, rf"trace.jsonl:1:.*{kind}\.{field}.*recapture"):
                        load(path)

    def test_own_damage_requires_a_checked_tick_before_lead_accounting(self):
        # The game's damage-model report reads dt only for own receipts, duplicates included at admission.
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "client.jsonl"
            for value in (None, -1, True, 1.5, "1"):
                item = row("dmg_rx", 12, 100, 10, own=True, dt=value)
                if value is None:
                    del item["dt"]
                path.write_text(json.dumps(item) + "\n")
                with self.subTest(value=value), self.assertRaisesRegex(
                        TraceInputError, r"client.jsonl:1:.*dmg_rx\.dt.*recapture"):
                    load(path)
            item["dt"] = 0
            path.write_text(json.dumps(item) + "\n")
            self.assertEqual(load(path)[1], [item])
            del item["dt"]
            item["own"] = False
            path.write_text(json.dumps(item) + "\n")
            self.assertEqual(load(path)[1], [item])

    def test_bounce_requires_a_checked_ordinal_before_ribbon_accounting(self):
        # The game's damage-model smoke report must join each bounce to its ribbon evidence.
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "server.jsonl"
            fire = row("fire", 10, 100, 10, smoke=True)
            for value in (None, -1, True, 1.5, "1"):
                item = row("path_tx", 12, 100, 10, seq=0, ordinal=value)
                if value is None:
                    del item["ordinal"]
                path.write_text(json.dumps(fire) + "\n" + json.dumps(item) + "\n")
                with self.subTest(value=value), self.assertRaisesRegex(
                        TraceInputError, r"server.jsonl:2:.*path_tx\.ordinal.*recapture"):
                    load(path)
            item["ordinal"] = 0
            path.write_text(json.dumps(fire) + "\n" + json.dumps(item) + "\n")
            self.assertEqual(analyze({}, [], *load(path))["trail"], {"no_row": 1})

    def test_path_impact_cannot_silently_skip_smoke_verification(self):
        # The game's damage-model report must distinguish a checked non-bounce from missing evidence.
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "server.jsonl"
            fire = row("fire", 10, 100, 10, smoke=True)
            for value in (None, "unknown", 1, [], {}):
                item = row("path_tx", 12, 100, 10, seq=0, impact=value)
                if value is None:
                    del item["impact"]
                path.write_text(json.dumps(fire) + "\n" + json.dumps(item) + "\n")
                with self.subTest(value=value), self.assertRaisesRegex(
                        TraceInputError, r"server.jsonl:2:.*path_tx\.impact.*recapture"):
                    load(path)
            for impact in ("bounce", "through", "stopped", "ground", "state"):
                item = row("path_tx", 12, 100, 10, seq=0, impact=impact)
                if impact != "bounce":
                    del item["ordinal"]
                path.write_text(json.dumps(fire) + "\n" + json.dumps(item) + "\n")
                with self.subTest(impact=impact):
                    result = analyze({}, [], *load(path))
                    self.assertEqual(result["trail"], {"no_row": 1} if impact == "bounce" else {})


if __name__ == "__main__":
    unittest.main()
