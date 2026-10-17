# /// script
# requires-python = ">=3.11"
# ///
"""Tests for the pen cam delivery-latency join: when the cam opens, and the run's connect check.

    uv run scripts/shot/test_pencam_latency.py
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from pencam_latency import analyze, connect_failures  # noqa: E402

SHOT = {"c": 1, "w": 0, "ft": 100}
WHO = {"sc": 2, "sl": 0, "en": 0, "o": 0, "aud": "victim"}


def write(directory: Path, name: str, rows: list[dict]) -> Path:
    path = directory / name
    path.write_text("".join(json.dumps(row) + "\n" for row in rows))
    return path


def server_rows(*links: tuple[str, int]) -> list[dict]:
    return [
        {"k": "meta", "tick_hz": 64},
        {"k": "fire", "t": 100, "us": 1_000, **SHOT, "cal": 0.088},
        {"k": "cf", "t": 110, "us": 10_000, **SHOT, "ab": 0},
        {"k": "xsend", "t": 110, "us": 10_500, **SHOT, **WHO, "to": 2},
        *({"k": "link", "t": 0, "us": us, "ev": ev, "link": 7} for ev, us in links),
    ]


def client_rows(arrival: str, released: str | None) -> list[dict]:
    rows = [{"k": "xrx", "t": 120, "us": 40_000, **SHOT, **WHO, "pres": arrival}]
    rows.append({"k": "xdraw", "t": 130, "us": 60_000, **SHOT, "o": 0})
    if released is not None:
        rows.append({"k": "xpres", "t": 131, "us": 75_000, **SHOT, **WHO, "pres": released})
    return rows


class OpenTests(unittest.TestCase):
    def open_ms(self, arrival: str, released: str | None) -> dict:
        with tempfile.TemporaryDirectory() as d:
            server = write(Path(d), "s.jsonl", server_rows(("up", 0)))
            client = write(Path(d), "a.jsonl", client_rows(arrival, released))
            return analyze(server, [client])["groups"]["main/victim"]

    def test_a_later_presentation_opens_at_the_draw_not_the_polling_pass(self) -> None:
        group = self.open_ms("pending", "presented")
        self.assertEqual(group["curves"]["open"]["p50_ms"], 50.0)
        self.assertEqual(group["counts"]["presented"], 1)

    def test_a_timeout_opens_at_its_release(self) -> None:
        group = self.open_ms("pending", "timed_out")
        self.assertEqual(group["curves"]["open"]["p50_ms"], 65.0)
        self.assertEqual(group["counts"]["timed_out"], 1)

    def test_presented_on_arrival_opens_at_the_arrival(self) -> None:
        group = self.open_ms("presented", None)
        self.assertEqual(group["curves"]["open"]["p50_ms"], 30.0)


class ConnectTests(unittest.TestCase):
    def failures(self, *links: tuple[str, int], log: str | None = None) -> list[str]:
        with tempfile.TemporaryDirectory() as d:
            server = write(Path(d), "s.jsonl", server_rows(*links))
            client = write(Path(d), "a.jsonl", client_rows("pending", "presented"))
            server_log = None
            if log is not None:
                server_log = Path(d) / "server.log"
                server_log.write_text(log)
            return connect_failures(analyze(server, [client]), 2, server_log)

    def test_two_connects_that_stay_connected_are_valid(self) -> None:
        self.assertEqual(self.failures(("up", 0), ("up", 1), ("down", 20_000)), [])

    def test_a_reconnect_invalidates_the_run(self) -> None:
        self.assertEqual(
            self.failures(("up", 0), ("up", 1), ("up", 5_000)),
            ["3 client connects for 2 clients"],
        )

    def test_a_disconnect_before_the_last_send_invalidates_the_run(self) -> None:
        self.assertEqual(
            self.failures(("up", 0), ("up", 1), ("down", 5_000)),
            ["1 client disconnects before the last send"],
        )

    def test_a_trace_without_link_rows_needs_the_server_log(self) -> None:
        self.assertEqual(
            self.failures(),
            ["the server trace has no `link` rows and no --server-log was given"],
        )
        connected = "server: client connected: Netcode(1) (link 1v0)\n"
        self.assertEqual(self.failures(log=connected * 2), [])
        self.assertEqual(self.failures(log=connected * 3), ["3 client connects for 2 clients"])


if __name__ == "__main__":
    unittest.main()
