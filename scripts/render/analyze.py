# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Summarize render-pass cost from `SPIKE_RENDER_COST` JSONL (src/render_cost.rs).

Consumes the once-per-second `{t, name, cpu_ms, gpu_ms}` rows the client's
render-cost recorder writes and prints one percentile table per timing kind
(cpu_ms always; gpu_ms only where rows carry it — Vulkan/DX12; on macOS the
GPU column is null by construction). Rows are grouped by span name, sorted by
p50 descending, so the most expensive pass reads first.

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
        for line in fh:
            line = line.strip()
            if not line:
                continue
            row = json.loads(line)
            for kind in ("cpu_ms", "gpu_ms"):
                if row.get(kind) is not None:
                    series[kind][row["name"]].append(row[kind])
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
