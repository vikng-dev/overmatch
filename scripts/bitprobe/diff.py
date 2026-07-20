#!/usr/bin/env python3
"""Compare two overmatch bitprobe dumps without rounding any payload values."""

from __future__ import annotations

import argparse
import gzip
import json
import math
import struct
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO, Iterator

MAGIC = b"OMBP\x01\r\n\0"
FNV_OFFSET = 0xCBF29CE484222325
FNV_PRIME = 0x00000100000001B3
MASK64 = (1 << 64) - 1


class DumpError(Exception):
    """Malformed or unsupported dump."""


def read_exact(stream: BinaryIO, size: int, context: str) -> bytes:
    data = stream.read(size)
    if len(data) != size:
        raise DumpError(f"truncated {context}: wanted {size} bytes, got {len(data)}")
    return data


def fnv1a(words: tuple[int, ...]) -> int:
    value = FNV_OFFSET
    for word in words:
        for byte in struct.pack("<I", word):
            value ^= byte
            value = (value * FNV_PRIME) & MASK64
    return value


@dataclass(frozen=True)
class SeamPayload:
    stored_hash: int
    words: tuple[int, ...]


@dataclass(frozen=True)
class Tick:
    number: int
    seams: tuple[SeamPayload, ...]


class Dump:
    def __init__(self, path: Path):
        self.path = path
        with path.open("rb") as raw:
            compressed = raw.read(2) == b"\x1f\x8b"
        self.stream = gzip.open(path, "rb") if compressed else path.open("rb")
        try:
            magic = read_exact(self.stream, len(MAGIC), f"{path} magic")
            if magic != MAGIC:
                raise DumpError(f"{path}: bad magic {magic!r}")
            header_len = struct.unpack(
                "<I", read_exact(self.stream, 4, f"{path} header length")
            )[0]
            header_raw = read_exact(self.stream, header_len, f"{path} JSON header")
            self.header = json.loads(header_raw)
            if self.header.get("format") != "overmatch-bitprobe":
                raise DumpError(f"{path}: unsupported format {self.header.get('format')!r}")
            if self.header.get("version") != 1:
                raise DumpError(f"{path}: unsupported version {self.header.get('version')!r}")
            self.seam_schemas = self.header["seams"]
        except Exception:
            self.stream.close()
            raise

    def close(self) -> None:
        self.stream.close()

    def ticks(self) -> Iterator[Tick]:
        while True:
            tick_raw = self.stream.read(4)
            if not tick_raw:
                return
            if len(tick_raw) != 4:
                raise DumpError(f"{self.path}: truncated tick number")
            number = struct.unpack("<I", tick_raw)[0]
            seams: list[SeamPayload] = []
            for schema in self.seam_schemas:
                seam_name = schema["name"]
                seam_header = read_exact(
                    self.stream, 12, f"{self.path} tick {number} seam {seam_name} header"
                )
                stored_hash, word_count = struct.unpack("<QI", seam_header)
                payload = read_exact(
                    self.stream,
                    word_count * 4,
                    f"{self.path} tick {number} seam {seam_name} payload",
                )
                words = struct.unpack(f"<{word_count}I", payload) if word_count else ()
                if schema["layout"] == "records":
                    if not words:
                        raise DumpError(
                            f"{self.path}: tick {number} seam {seam_name} has no record count"
                        )
                    expected_words = 1 + words[0] * len(schema["record_fields"])
                else:
                    expected_words = len(schema["fields"])
                if word_count != expected_words:
                    raise DumpError(
                        f"{self.path}: tick {number} seam {seam_name} declares {word_count} "
                        f"words; schema requires {expected_words}"
                    )
                seams.append(SeamPayload(stored_hash, words))
            yield Tick(number, tuple(seams))


@dataclass(frozen=True)
class Location:
    label: str
    kind: str


@dataclass(frozen=True)
class Difference:
    tick: int
    seam_index: int
    word_index: int
    location: Location
    left: int | None
    right: int | None


def comparable_header(header: dict) -> dict:
    """Run identity and startup values are reported separately, not schema-compared."""
    return {
        key: value
        for key, value in header.items()
        if key not in {"run", "startup"}
    }


def first_startup_difference(left: dict, right: dict) -> tuple[int, dict | None, dict | None] | None:
    left_entries = left.get("startup", [])
    right_entries = right.get("startup", [])
    for index in range(max(len(left_entries), len(right_entries))):
        a = left_entries[index] if index < len(left_entries) else None
        b = right_entries[index] if index < len(right_entries) else None
        if a != b:
            return index, a, b
    return None


def field_schema(schema: dict) -> list[dict]:
    if schema["layout"] == "records":
        return schema["record_fields"]
    return schema["fields"]


def locate(schema: dict, word_index: int, words: tuple[int, ...]) -> Location:
    if schema["layout"] == "records":
        if word_index == 0:
            return Location("record_count", "u32")
        fields = schema["record_fields"]
        width = len(fields)
        record = (word_index - 1) // width
        field_index = (word_index - 1) % width
        field = fields[field_index]
        base = 1 + record * width
        ids: list[str] = []
        for index, name in enumerate(("side", "station", "material", "column")):
            if index < width and base + index < len(words):
                ids.append(f"{name}={words[base + index]}")
        suffix = f" ({', '.join(ids)})" if ids else ""
        return Location(f"record[{record}]{suffix}.{field['name']}", field["type"])
    fields = schema["fields"]
    if word_index >= len(fields):
        return Location(f"extra_word[{word_index}]", "u32")
    field = fields[word_index]
    return Location(field["name"], field["type"])


def first_word_difference(
    tick: int,
    seam_index: int,
    schema: dict,
    left: tuple[int, ...],
    right: tuple[int, ...],
) -> Difference | None:
    for index in range(max(len(left), len(right))):
        a = left[index] if index < len(left) else None
        b = right[index] if index < len(right) else None
        if a != b:
            basis = left if a is not None else right
            return Difference(tick, seam_index, index, locate(schema, index, basis), a, b)
    return None


def word_types(schema: dict, words: tuple[int, ...]) -> Iterator[tuple[int, str]]:
    if schema["layout"] == "records":
        yield 0, "u32"
        fields = schema["record_fields"]
        width = len(fields)
        for index in range(1, len(words)):
            yield index, fields[(index - 1) % width]["type"]
    else:
        fields = schema["fields"]
        for index in range(len(words)):
            yield index, fields[index]["type"] if index < len(fields) else "u32"


def as_f32(word: int) -> float:
    return struct.unpack("<f", struct.pack("<I", word))[0]


def physical_linf(schema: dict, left: tuple[int, ...], right: tuple[int, ...]) -> float:
    if len(left) != len(right):
        return math.inf
    maximum = 0.0
    for index, kind in word_types(schema, left):
        if kind != "f32":
            continue
        # Equal raw NaNs are identical observations, not infinite physical growth.
        if left[index] == right[index]:
            continue
        a = as_f32(left[index])
        b = as_f32(right[index])
        delta = abs(b - a)
        if math.isnan(delta):
            return math.inf
        maximum = max(maximum, delta)
    return maximum


def decoded(kind: str, word: int | None) -> str:
    if word is None:
        return "<missing>"
    if kind == "f32":
        return f"{as_f32(word):.9g}"
    if kind == "i32":
        return str(struct.unpack("<i", struct.pack("<I", word))[0])
    if kind == "bool":
        return "true" if word else "false"
    return str(word)


def bits(word: int | None) -> str:
    return "<missing>" if word is None else f"0x{word:08x}"


def report_value_difference(kind: str, left: int | None, right: int | None) -> str:
    result = (
        f"left {bits(left)} ({decoded(kind, left)}), "
        f"right {bits(right)} ({decoded(kind, right)})"
    )
    if kind == "f32" and left is not None and right is not None:
        a = as_f32(left)
        b = as_f32(right)
        result += f", physical delta right-left={b - a:+.9g}, |delta|={abs(b - a):.9g}"
    return result


def growth_summary(samples: list[tuple[int, float]], first_tick: int) -> str:
    by_tick = dict(samples)
    final_tick, final_value = samples[-1]
    offsets = [0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024]
    points: list[str] = []
    for offset in offsets:
        tick = first_tick + offset
        if tick in by_tick:
            points.append(f"+{offset}:{by_tick[tick]:.9g}")
    if final_tick != first_tick + offsets[-1]:
        points.append(f"end@{final_tick}:{final_value:.9g}")
    peak_tick, peak_value = max(samples, key=lambda item: item[1])
    points.append(f"peak@{peak_tick}:{peak_value:.9g}")
    return "  ".join(points)


def compare(left_path: Path, right_path: Path) -> int:
    left = Dump(left_path)
    right = Dump(right_path)
    try:
        if comparable_header(left.header) != comparable_header(right.header):
            raise DumpError("dump schemas/scenario metadata differ")
        seam_schemas = left.seam_schemas
        seam_names = [schema["name"] for schema in seam_schemas]
        run_left = left.header.get("run", {})
        run_right = right.header.get("run", {})
        print(f"runs: left={run_left} right={run_right}")

        startup_difference = first_startup_difference(left.header, right.header)
        if startup_difference is None:
            count = len(left.header.get("startup", []))
            print(f"startup dump: IDENTICAL ({count} named raw values)")
        else:
            index, a, b = startup_difference
            print(f"startup dump: DIVERGED at index {index}")
            if a is None or b is None:
                print(f"  left={a!r}")
                print(f"  right={b!r}")
            elif a.get("name") != b.get("name") or a.get("kind") != b.get("kind"):
                print(f"  left={a!r}")
                print(f"  right={b!r}")
            else:
                print(f"  {a['name']} ({a['kind']}): {report_value_difference(a['kind'], a['bits'], b['bits'])}")

        first_by_seam: list[Difference | None] = [None] * len(seam_schemas)
        growth: list[list[tuple[int, float]]] = [[] for _ in seam_schemas]
        left_ticks = left.ticks()
        right_ticks = right.ticks()
        total_ticks = 0
        expected_tick = 0
        while True:
            a = next(left_ticks, None)
            b = next(right_ticks, None)
            if a is None and b is None:
                break
            if a is None or b is None:
                raise DumpError(
                    f"tick count differs after {total_ticks} complete ticks: "
                    f"left={'EOF' if a is None else a.number}, right={'EOF' if b is None else b.number}"
                )
            if a.number != b.number:
                raise DumpError(f"tick sequence differs: left {a.number}, right {b.number}")
            if a.number != expected_tick:
                raise DumpError(
                    f"tick sequence is not contiguous: expected {expected_tick}, got {a.number}"
                )
            expected_tick += 1
            total_ticks += 1
            for seam_index, schema in enumerate(seam_schemas):
                a_payload = a.seams[seam_index]
                b_payload = b.seams[seam_index]
                if (
                    a_payload.stored_hash != b_payload.stored_hash
                    or a_payload.words != b_payload.words
                ):
                    difference = first_word_difference(
                        a.number,
                        seam_index,
                        schema,
                        a_payload.words,
                        b_payload.words,
                    )
                    if difference is None:
                        raise DumpError(
                            f"tick {a.number} seam {schema['name']}: hashes differ but payloads match"
                        )
                    if first_by_seam[seam_index] is None:
                        first_by_seam[seam_index] = difference
                if first_by_seam[seam_index] is not None:
                    growth[seam_index].append(
                        (
                            a.number,
                            physical_linf(schema, a_payload.words, b_payload.words),
                        )
                    )

        expected_count = left.header.get("tick_count")
        if total_ticks != expected_count:
            raise DumpError(
                f"dump ended after {total_ticks} ticks; header requires {expected_count}"
            )
        all_differences = [difference for difference in first_by_seam if difference is not None]
        if all_differences:
            first = min(all_differences, key=lambda item: (item.tick, item.seam_index))
            print("first tick divergence:")
            print(
                f"  tick={first.tick} seam={seam_names[first.seam_index]} "
                f"index={first.word_index} field={first.location.label}"
            )
            print(
                "  "
                + report_value_difference(first.location.kind, first.left, first.right)
            )
        else:
            print(f"tick payloads: IDENTICAL ({total_ticks} ticks)")

        print("per-seam first divergence:")
        for seam_index, name in enumerate(seam_names):
            difference = first_by_seam[seam_index]
            if difference is None:
                print(f"  {name:22} identical")
            else:
                print(
                    f"  {name:22} tick {difference.tick:5}  index {difference.word_index:6}  "
                    f"{difference.location.label}"
                )

        if all_differences:
            print("divergence growth (per-tick f32 L-infinity; +N is ticks after that seam's first divergence):")
            for seam_index, name in enumerate(seam_names):
                difference = first_by_seam[seam_index]
                if difference is not None:
                    print(
                        f"  {name:22} "
                        f"{growth_summary(growth[seam_index], difference.tick)}"
                    )

        return 1 if startup_difference is not None or all_differences else 0
    finally:
        left.close()
        right.close()


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Report the first raw-bit difference and later seam growth between bitprobe dumps."
    )
    parser.add_argument("left", type=Path, help="first .obp dump (for example macOS ARM)")
    parser.add_argument("right", type=Path, help="second .obp dump (for example Linux x86_64)")
    args = parser.parse_args()
    try:
        return compare(args.left, args.right)
    except (OSError, KeyError, ValueError, json.JSONDecodeError, DumpError) as error:
        print(f"bitprobe diff error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
