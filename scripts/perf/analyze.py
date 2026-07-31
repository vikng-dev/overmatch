# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Summarize frame-budget sweep streams from `SPIKE_FRAME_COST` JSONL (src/frame_cost.rs).

Consumes the raw one-row-per-frame `{t, frame_ms}` streams `scripts/perf/run-frame-sweep.sh`
captures (one file per ladder condition) and prints:
  * per-condition p50/p95/p99/worst frame ms, with fps-equivalents and % of the 16.67 ms
    (60 fps) and 8.33 ms (120 fps) budgets;
  * a delta table against the ladder's baseline condition.

Percentiles live HERE, never in the recorder: the product signal is tail stutter, and any
in-process averaging destroys it. Validity is enforced like the scripts/shot analyzers: a
missing file or a post-warmup stream shorter than --min-rows fails loud (a short stream is a
crashed or stalled run, not evidence).

Usage:
    uv run scripts/perf/analyze.py [--warmup-s 10] [--min-rows 400] \
        [--baseline m350-2] out/m350-2.client.jsonl out/off-30.client.jsonl ...

Condition names are file basenames minus `.client.jsonl`/`.jsonl`.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

BUDGETS_MS = (16.667, 8.333)
STATS = (("p50", 50), ("p95", 95), ("p99", 99), ("worst", 100))


def fail(msg: str) -> None:
    print(f"INVALID: {msg}", file=sys.stderr)
    sys.exit(1)


def pct(xs: list[float], q: float) -> float:
    xs = sorted(xs)
    if q >= 100:
        return xs[-1]
    # linear interpolation between closest ranks (same rule as scripts/render/analyze.py)
    pos = (len(xs) - 1) * q / 100.0
    lo = int(pos)
    hi = min(lo + 1, len(xs) - 1)
    frac = pos - lo
    return xs[lo] * (1 - frac) + xs[hi] * frac


def condition_name(path: Path) -> str:
    name = path.name
    for suffix in (".client.jsonl", ".jsonl"):
        if name.endswith(suffix):
            return name[: -len(suffix)]
    return name


def load_stream(path: Path, warmup_s: float, min_rows: int) -> list[float]:
    if not path.is_file():
        fail(f"{path} does not exist")
    frames: list[tuple[float, float]] = []
    with open(path) as fh:
        for lineno, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
                frames.append((float(row["t"]), float(row["frame_ms"])))
            except (json.JSONDecodeError, KeyError, TypeError, ValueError) as err:
                fail(f"{path}:{lineno} unparseable frame row ({err})")
    if not frames:
        fail(f"{path} is empty")
    # Warmup is relative to the stream's own first row: `t` is app-elapsed time, and the first
    # seconds cover asset load, shader compilation and the settle after spawn.
    t0 = frames[0][0]
    kept = [ms for t, ms in frames if t - t0 >= warmup_s]
    if len(kept) < min_rows:
        fail(
            f"{path}: {len(kept)} post-warmup rows < required {min_rows} "
            f"({len(frames)} total; crashed, stalled, or killed early?)"
        )
    return kept


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("streams", nargs="+", type=Path)
    parser.add_argument("--warmup-s", type=float, default=10.0)
    parser.add_argument("--min-rows", type=int, default=400)
    parser.add_argument(
        "--baseline",
        help="condition name the delta table subtracts (default: the first stream)",
    )
    args = parser.parse_args()

    conditions: dict[str, dict[str, float]] = {}
    counts: dict[str, int] = {}
    for path in args.streams:
        name = condition_name(path)
        if name in conditions:
            fail(f"duplicate condition name {name!r} ({path})")
        kept = load_stream(path, args.warmup_s, args.min_rows)
        conditions[name] = {label: pct(kept, q) for label, q in STATS}
        counts[name] = len(kept)

    baseline = args.baseline or condition_name(args.streams[0])
    if baseline not in conditions:
        fail(f"baseline {baseline!r} is not among the streams ({', '.join(conditions)})")

    width = max(len("condition"), *(len(name) for name in conditions))
    stat_labels = [label for label, _ in STATS]

    print(f"\nframe time, ms (post-warmup {args.warmup_s:g}s discarded)")
    print(f"{'condition':<{width}} {'n':>7} " + " ".join(f"{s:>8}" for s in stat_labels))
    for name, stats in conditions.items():
        cells = " ".join(f"{stats[s]:>8.2f}" for s in stat_labels)
        print(f"{name:<{width}} {counts[name]:>7} {cells}")

    print("\nfps-equivalent (1000 / frame ms)")
    print(f"{'condition':<{width}} " + " ".join(f"{s:>8}" for s in stat_labels))
    for name, stats in conditions.items():
        cells = " ".join(f"{1000.0 / stats[s]:>8.1f}" for s in stat_labels)
        print(f"{name:<{width}} {cells}")

    for budget in BUDGETS_MS:
        print(f"\n% of {budget:.2f} ms budget ({1000.0 / budget:.0f} fps); >100 = over budget")
        print(f"{'condition':<{width}} " + " ".join(f"{s:>8}" for s in stat_labels))
        for name, stats in conditions.items():
            cells = " ".join(f"{100.0 * stats[s] / budget:>8.1f}" for s in stat_labels)
            print(f"{name:<{width}} {cells}")

    print(f"\ndelta vs baseline {baseline!r}, ms (positive = slower than baseline)")
    print(f"{'condition':<{width}} " + " ".join(f"{'d' + s:>8}" for s in stat_labels))
    base = conditions[baseline]
    for name, stats in conditions.items():
        if name == baseline:
            continue
        cells = " ".join(f"{stats[s] - base[s]:>+8.2f}" for s in stat_labels)
        print(f"{name:<{width}} {cells}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
