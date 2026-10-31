# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Tests for the bitprobe comparator's reading of the three `transmission_state` dump forms.

A dump's `transmission_state` seam is one of: today's (the schema declares `band_confirm_ticks` and
the payload writes it), a genuine older one (neither does), or a malformed one (the payload writes
the word, the schema omits it). The comparator must read all three, declare the word only in the
malformed form, and still refuse to compare two genuinely different schemas.

    python3 scripts/bitprobe/test_diff.py
"""

from __future__ import annotations

import gzip
import json
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

DIFF = Path(__file__).parent / "diff.py"
MAGIC = b"OMBP\x01\r\n\0"

# Fixture data: a short `transmission_state` schema around the field the reader may declare, and a
# second seam so the declared word is not the payload's last.
WITHOUT_BAND = [
    {"name": "demand_n", "type": "f32"},
    {"name": "grade_confirm_ticks", "type": "u32"},
    {"name": "grade_target", "type": "u32"},
]
WITH_BAND = [
    WITHOUT_BAND[0],
    WITHOUT_BAND[1],
    {"name": "band_confirm_ticks", "type": "u32"},
    WITHOUT_BAND[2],
]
POSE = [{"name": "x", "type": "f32"}]
TICKS = 4


def fnv1a64(words: list[int]) -> int:
    value = 0xCBF29CE484222325
    for byte in struct.pack(f"<{len(words)}I", *words):
        value = ((value ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return value


def transmission_words(tick: int, with_band: bool, band: int = 7) -> list[int]:
    demand = struct.unpack("<I", struct.pack("<f", 0.25 * tick))[0]
    return [demand, tick, band, 3] if with_band else [demand, tick, 3]


def write_dump(path: Path, schema: list[dict], payload_has_band: bool, band_at: dict | None = None) -> Path:
    header = {
        "format": "overmatch-bitprobe",
        "version": 1,
        "tick_count": TICKS,
        "run": {"arch": "fixture"},
        "startup": [],
        "seams": [
            {"name": "transmission_state", "layout": "fields", "fields": schema},
            {"name": "pose", "layout": "fields", "fields": POSE},
        ],
    }
    raw = json.dumps(header).encode()
    with gzip.open(path, "wb") as out:
        out.write(MAGIC + struct.pack("<I", len(raw)) + raw)
        for tick in range(TICKS):
            band = (band_at or {}).get(tick, 7)
            seams = [
                transmission_words(tick, payload_has_band, band),
                [struct.unpack("<I", struct.pack("<f", float(tick)))[0]],
            ]
            out.write(struct.pack("<I", tick))
            for words in seams:
                out.write(struct.pack("<QI", fnv1a64(words), len(words)))
                out.write(struct.pack(f"<{len(words)}I", *words))
    return path


def diff(left: Path, right: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(DIFF), str(left), str(right)],
        capture_output=True,
        text=True,
        check=False,
    )


class DumpForms(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.current = write_dump(root / "current.obp.gz", WITH_BAND, payload_has_band=True)
        self.genuine = write_dump(root / "genuine.obp.gz", WITHOUT_BAND, payload_has_band=False)
        self.malformed = write_dump(root / "malformed.obp.gz", WITHOUT_BAND, payload_has_band=True)
        self.root = root

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def assert_identical(self, left: Path, right: Path) -> None:
        result = diff(left, right)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"tick payloads: IDENTICAL ({TICKS} ticks)", result.stdout)

    def test_each_form_reads_identical_against_itself(self) -> None:
        # A width check that rejects a well-formed dump, or a declaration applied to a payload
        # that does not carry the word, fails one of these three.
        for dump in (self.current, self.genuine, self.malformed):
            with self.subTest(dump=dump.name):
                self.assert_identical(dump, dump)

    def test_malformed_form_compares_as_todays_schema(self) -> None:
        # The declared word must land after `grade_confirm_ticks`, or the two diverge on the
        # first tick at the word the reader misplaced.
        self.assert_identical(self.malformed, self.current)
        self.assert_identical(self.current, self.malformed)

    def test_a_changed_undeclared_word_is_named(self) -> None:
        changed = write_dump(
            self.root / "changed.obp.gz", WITHOUT_BAND, payload_has_band=True, band_at={2: 8}
        )
        result = diff(self.current, changed)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("tick=2 seam=transmission_state index=2 field=band_confirm_ticks", result.stdout)

    def test_genuine_form_never_compares_against_todays_schema(self) -> None:
        # Declaring the word on a payload without it would make an older schema look like
        # today's; the comparator must refuse the pair as schemas that differ.
        result = diff(self.genuine, self.current)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("schemas/scenario metadata differ", result.stderr)

    def test_a_payload_wider_than_the_accepted_exception_is_refused(self) -> None:
        wide = self.root / "wide.obp.gz"
        write_dump(wide, WITHOUT_BAND[:2], payload_has_band=True)
        result = diff(wide, wide)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("declares 4 words; schema requires 2", result.stderr)


if __name__ == "__main__":
    unittest.main()
