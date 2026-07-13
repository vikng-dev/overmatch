# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
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

from analyze import analyze, verification_failures  # noqa: E402


def row(kind: str, tick: int, combatant: int, fire_tick: int, **extra) -> dict:
    return {"k": kind, "t": tick, "c": combatant, "w": 0, "ft": fire_tick, **extra}


class JoinIntegrityTests(unittest.TestCase):
    def test_same_slot_same_tick_combatants_are_independent(self) -> None:
        # Two combatants fire the same slot on the same tick. Their stable ids make both joins exact.
        server = [
            row("fire", 10, 100, 10, o=[0.0, 0.0, 0.0], cal=0.088),
            row("dmg", 12, 100, 10, hp=5.0),
            row("fire", 10, 200, 10, o=[10.0, 0.0, 0.0], cal=0.088),
            row("kf", 11, 200, 10, seq=0),
            # One clean observed shot proves the report still analyzes uncontaminated groups.
            row("fire", 20, 300, 20, o=[20.0, 0.0, 0.0], cal=0.0079),
        ]
        client = [
            row("fire_rx", 11, 100, 10, dup=False, o=[0.0, 0.0, 0.0]),
            row("marker", 13, 100, 10),
            row("fire_rx", 11, 200, 10, dup=False, o=[10.0, 0.0, 0.0]),
            row("hold", 12, 200, 10, res="bounce", held=1, seq=0),
            row("fire_rx", 21, 300, 20, dup=False),
            row("spawn", 21, 300, 20, src="obs"),
        ]

        result = analyze({"tick_hz": 64}, client, {"tick_hz": 64}, server)

        self.assertEqual(result["server_fires"], 3)
        self.assertEqual(result["expected"], 3)
        self.assertEqual(result["delivered"], 3)
        self.assertEqual(result["damage"]["server"], 1)
        self.assertEqual(result["all_holds"], [1])
        self.assertEqual(result["carry"], {"reseeded_hold": 1})


class StrictVerificationTests(unittest.TestCase):
    def test_own_damage_confirm_duplicate_receive_rows_reaches_one_marker(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.088),
            row("dmg", 12, 100, 10, hp=5.0),
        ]
        client = [
            row("spawn", 10, 100, 10, src="own"),
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
            row("fire", 10, 100, 10, cal=0.088),
            row("dmg", 12, 100, 10, hp=5.0),
        ]
        client = [
            row("spawn", 10, 100, 10, src="own"),
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
            row("fire", 10, 100, 10, cal=0.088),
            row("dmg", 12, 100, 10, hp=5.0),
        ]
        client = [
            row("spawn", 10, 100, 10, src="own"),
            row("dmg_rx", 13, 100, 10, own=True, dup=False, dt=12),
        ]

        result = analyze({"tick_hz": 64}, client, {"tick_hz": 64}, server)

        self.assertEqual(result["damage"]["missing_marker"], 1)
        self.assertEqual(verification_failures(result), ["missing_shooter_markers=1"])

    def test_main_gun_sanctioned_bounce_reaches_post_bounce_trail_station(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.088),
            row("kf", 12, 100, 10, seq=0),
        ]
        client = [
            row("spawn", 10, 100, 10, src="own"),
            row("kf_rx", 13, 100, 10, seq=0, dup=False, bt=12),
            row("hold", 13, 100, 10, seq=0, res="bounce", held=1),
            row("trail", 13, 100, 10, seq=0, res="post_bounce_consumed"),
        ]

        result = analyze({"tick_hz": 64}, client, {"tick_hz": 64}, server)

        self.assertEqual(result["trail"], {"consumed": 1})
        self.assertEqual(verification_failures(result), [])

    def test_strict_rejects_main_gun_trail_with_no_row_or_unrendered_result(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.088),
            row("kf", 12, 100, 10, seq=0),
            row("fire", 20, 100, 20, cal=0.088),
            row("kf", 22, 100, 20, seq=0),
        ]
        client = [
            row("spawn", 10, 100, 10, src="own"),
            row("kf_rx", 13, 100, 10, seq=0, dup=False, bt=12),
            row("hold", 13, 100, 10, seq=0, res="bounce", held=1),
            row("spawn", 20, 100, 20, src="own"),
            row("kf_rx", 23, 100, 20, seq=0, dup=False, bt=22),
            row("hold", 23, 100, 20, seq=0, res="bounce", held=1),
            row("trail", 23, 100, 20, seq=0, res="post_bounce_unrendered"),
        ]

        result = analyze({"tick_hz": 64}, client, {"tick_hz": 64}, server)

        self.assertEqual(result["trail"], {"no_row": 1, "unrendered": 1})
        self.assertEqual(
            verification_failures(result),
            ["main_gun_trail_failures=2"],
        )

    def test_strict_reports_every_lifecycle_failure_class(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.0079),
            row("fire", 20, 100, 20, cal=0.0079),
            row("fire", 30, 100, 30, cal=0.0079),
            row("fire", 40, 100, 40, cal=0.0079),
            row("kf", 42, 100, 40, seq=0),
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
                "sanctioned_bounce_carry_failures=1",
            ],
        )


class TransportCopyTests(unittest.TestCase):
    def test_copy_counts_keep_multiple_bounces_separate_and_split_policy(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.0079),
            row("kf", 12, 100, 10, seq=0),
            row("kf", 14, 100, 10, seq=1),
            # Automatic-fire visual facts get three bounded copies.
            row("send", 10, 100, 10, s="fire", age=0, rel=False, bb=100),
            row("send", 11, 100, 10, s="fire", age=1, rel=False, bb=100),
            row("send", 12, 100, 10, s="fire", age=2, rel=False, bb=100),
            row("send", 12, 100, 10, s="kf", seq=0, age=0, rel=False, bb=100),
            row("send", 13, 100, 10, s="kf", seq=0, age=1, rel=False, bb=100),
            row("send", 14, 100, 10, s="kf", seq=0, age=2, rel=False, bb=100),
            row("send", 14, 100, 10, s="kf", seq=1, age=0, rel=False, bb=100),
            row("send", 15, 100, 10, s="kf", seq=1, age=1, rel=False, bb=100),
            row("send", 16, 100, 10, s="kf", seq=1, age=2, rel=False, bb=100),
            # A single-fire damage fact is one reliable application send.
            row("dmg", 17, 100, 10, hp=5.0),
            row("send", 17, 100, 10, s="dmg", age=0, rel=True, bb=None),
        ]

        result = analyze({"tick_hz": 64}, [], {"tick_hz": 64}, server)

        self.assertEqual(result["copies"]["visual"]["fire"], [3])
        self.assertEqual(result["copies"]["visual"]["kf"], [3, 3])
        self.assertEqual(result["copies"]["reliable"]["dmg"], [1])
        self.assertEqual(result["copies"]["reliable"]["kf"], [0, 0])
        self.assertEqual(result["missing_kf_send_sequence"], 0)

    def test_missing_keyframe_send_sequence_is_not_assigned_to_bounce_zero(self) -> None:
        server = [
            row("fire", 10, 100, 10, cal=0.0079),
            row("kf", 12, 100, 10, seq=0),
            row("kf", 14, 100, 10, seq=1),
            row("send", 12, 100, 10, s="kf", age=0, rel=False, bb=100),
        ]

        result = analyze({"tick_hz": 64}, [], {"tick_hz": 64}, server)

        self.assertEqual(result["copies"]["visual"]["kf"], [0, 0])
        self.assertEqual(result["missing_kf_send_sequence"], 2)

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


if __name__ == "__main__":
    unittest.main()
