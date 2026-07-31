# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Summarize render-pass cost from `SPIKE_RENDER_COST` JSONL (src/render_cost.rs).

Consumes the recorder's fresh-only rows — `{t, name, cpu_ms, gpu_ms}` where
`cpu_ms`/`gpu_ms` are ARRAYS of raw per-frame millisecond measurements taken
since the previous sample window (null where a kind produced nothing fresh;
on macOS the GPU side is null by construction) — and prints one percentile
table per timing kind. Percentiles are computed over the flattened raw
values, so every observation is a real single-frame timing counted exactly
once; a span that stopped running simply stops contributing rows, it does
not echo its last value into the statistics.

Fails loud (exit 1) if the capture holds zero fresh observations, and
rejects rows from the pre-fresh schema (scalar cpu_ms — overlapping rolling
averages) instead of quietly summarizing them.

Rows are grouped by span name, sorted by p50 descending, so the most
expensive pass reads first.

Usage:
    uv run scripts/render/analyze.py capture.client.jsonl
"""

from __future__ import annotations

import json
import sys
from collections import defaultdict


def pct(xs, q):
    if not xs:
        return float("nan")
    xs = sorted(xs)
    if q <= 0:
        return xs[0]
    if q >= 100:
        return xs[-1]
    # linear interpolation between closest ranks
    pos = (len(xs) - 1) * q / 100.0
    lo = int(pos)
    hi = min(lo + 1, len(xs) - 1)
    frac = pos - lo
    return xs[lo] * (1 - frac) + xs[hi] * frac


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    series = {"cpu_ms": defaultdict(list), "gpu_ms": defaultdict(list)}
    with open(sys.argv[1]) as fh:
        for line_no, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            row = json.loads(line)
            for kind in ("cpu_ms", "gpu_ms"):
                values = row.get(kind)
                if values is None:
                    continue
                if not isinstance(values, list):
                    print(
                        f"{sys.argv[1]}:{line_no}: scalar {kind} — this is the old "
                        "rolling-average schema (overlapping ~2 s windows, stale "
                        "passes re-emitted forever); re-capture with the fresh-only "
                        "recorder before trusting any percentile.",
                        file=sys.stderr,
                    )
                    return 1
                series[kind][row["name"]].extend(values)
    if not any(series.values()):
        print(
            f"{sys.argv[1]}: zero fresh render-cost observations — the recorder "
            "emits only measurements newer than the previous window, so an empty "
            "capture means the render diagnostics never produced fresh data "
            "(no render app, capture too short, or recorder not armed).",
            file=sys.stderr,
        )
        return 1
    for kind, by_name in series.items():
        if not by_name:
            continue
        print(f"\n{kind}  ({sys.argv[1]})")
        print(f"{'span':<44} {'n':>5} {'p50':>8} {'p90':>8} {'p99':>8}")
        rows = sorted(by_name.items(), key=lambda kv: -pct(kv[1], 50))
        for name, xs in rows:
            print(
                f"{name:<44} {len(xs):>5} {pct(xs, 50):>8.3f} "
                f"{pct(xs, 90):>8.3f} {pct(xs, 99):>8.3f}"
            )
    return 0


if __name__ == "__main__":
    sys.exit(main())
