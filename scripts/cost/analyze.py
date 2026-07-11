# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Summarize machine-gun-march cost from `SPIKE_COST_TRACE` JSONL (src/cost.rs).

Consumes the per-fixed-tick rows the game's cost recorder writes and prints
percentile tables for (a) the whole FixedUpdate tick time (`us`) and (b) the
`integrate_projectiles` share of it (`mus`) — the direct march-cost attribution
that is immune to whole-tick scheduler noise because it is timed INSIDE the tick.

Each positional argument is `LABEL=path.jsonl` (or just `path.jsonl`, labelled by
stem). With two or more files, a DELTA table prints each scenario minus the
`--baseline` label (median-to-median), i.e. what firing adds over idle — the
answer to "what does the MG cost per tick". Pass `--rounds-per-s R` to convert the
march delta into a per-round cost.

Usage:
    uv run scripts/cost/analyze.py idle=idle.server.jsonl fire=fire.server.jsonl \
        --baseline idle [--rounds-per-s 25]
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


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


def mean(xs):
    return sum(xs) / len(xs) if xs else float("nan")


def linfit(xs, ys):
    """Ordinary least-squares slope + intercept of ys on xs (no numpy). Returns (slope, intercept, n)
    or None if xs has no spread. For the loft population sweep: slope = µs of march per additional
    concurrent projectile, intercept = the empty-query fixed cost."""
    n = len(xs)
    if n < 2:
        return None
    mx = mean(xs)
    my = mean(ys)
    sxx = sum((x - mx) ** 2 for x in xs)
    if sxx == 0:
        return None
    sxy = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    slope = sxy / sxx
    return slope, my - slope * mx, n


def parse(path):
    meta = {}
    ticks = []
    bad = 0
    with open(path) as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
            except (json.JSONDecodeError, ValueError):
                bad += 1
                continue
            if row.get("k") == "meta":
                meta = row
            elif row.get("k") == "tick":
                ticks.append(row)
    return meta, ticks, bad


def summarize(label, path):
    meta, ticks, bad = parse(path)
    if not ticks:
        print(f"  {label}: no tick rows in {path} (bad lines: {bad})", file=sys.stderr)
        return None
    us = [t["us"] for t in ticks]
    mus = [t["mus"] for t in ticks]
    np_ = [t["np"] for t in ticks]
    ne = [t["ne"] for t in ticks]
    nt = [t["nt"] for t in ticks]
    mc = [t["mc"] for t in ticks]
    hz = meta.get("tick_hz", 64.0)
    dur = len(ticks) / hz
    return {
        "label": label,
        "role": meta.get("role", "?"),
        "mgsc": meta.get("mgsc", False),
        "n": len(ticks),
        "dur": dur,
        "us": us,
        "mus": mus,
        "np": np_,
        "ne": ne,
        "nt": nt,
        "mc": mc,
        "bad": bad,
    }


def print_block(s):
    print(f"\n=== {s['label']}  (role={s['role']}  mg_shortcircuit={s['mgsc']}) ===")
    print(f"  ticks {s['n']}  ({s['dur']:.1f} s)   tanks {min(s['nt'])}-{max(s['nt'])}"
          f"   entities {min(s['ne'])}-{max(s['ne'])}   march re-runs(mc>1): "
          f"{sum(1 for m in s['mc'] if m > 1)}")
    print(f"  projectiles alive/tick: mean {mean(s['np']):.2f}  p50 {pct(s['np'],50):.0f}"
          f"  p99 {pct(s['np'],99):.0f}  max {max(s['np'])}")
    print(f"  {'metric':<22}{'p50':>10}{'p90':>10}{'p99':>10}{'max':>10}{'mean':>10}")
    print(f"  {'FixedUpdate tick us':<22}{pct(s['us'],50):>10.2f}{pct(s['us'],90):>10.2f}"
          f"{pct(s['us'],99):>10.2f}{max(s['us']):>10.2f}{mean(s['us']):>10.2f}")
    print(f"  {'march (mus)':<22}{pct(s['mus'],50):>10.3f}{pct(s['mus'],90):>10.3f}"
          f"{pct(s['mus'],99):>10.3f}{max(s['mus']):>10.3f}{mean(s['mus']):>10.3f}")
    share = 100.0 * mean(s['mus']) / mean(s['us']) if mean(s['us']) else float("nan")
    print(f"  march share of tick (mean): {share:.2f} %")
    # Per-concurrent-projectile march cost: regress march (mus) on live projectile count (np). Only
    # meaningful when np varies (the loft sweep) — the slope is µs of march per additional shell in
    # flight, the aim-independent number that scales to any population / tank count.
    fit = linfit(s["np"], s["mus"])
    if fit and max(s["np"]) - min(s["np"]) >= 4:
        slope, intercept, _ = fit
        print(f"  march vs live-projectile fit: {slope:.4f} µs/projectile  "
              f"(+ {intercept:.3f} µs empty)   [np {min(s['np'])}..{max(s['np'])}]")
        for p in (10, 30, 60, 150):
            if p <= max(s["np"]) * 1.05:
                print(f"      → np={p:>3}: march ≈ {slope * p + intercept:7.2f} µs/tick")


def print_deltas(blocks, base_label, rounds_per_s):
    base = next((b for b in blocks if b["label"] == base_label), None)
    if base is None:
        print(f"\n(no baseline '{base_label}' — skipping delta table)", file=sys.stderr)
        return
    print(f"\n=== DELTA vs baseline '{base_label}' (each scenario minus idle) ===")
    print(f"  {'scenario':<20}{'Δtick us p50':>14}{'Δtick us mean':>15}"
          f"{'Δmarch mean':>14}{'per-round march':>18}")
    for b in blocks:
        if b["label"] == base_label:
            continue
        d_us_p50 = pct(b["us"], 50) - pct(base["us"], 50)
        d_us_mean = mean(b["us"]) - mean(base["us"])
        d_mus_mean = mean(b["mus"]) - mean(base["mus"])
        # per-round from mean march delta: (µs/tick) * (ticks/s) / (rounds/s) = µs/round
        pr = (d_mus_mean * b["hz"] / rounds_per_s) if rounds_per_s else float("nan")
        print(f"  {b['label']:<20}{d_us_p50:>14.2f}{d_us_mean:>15.2f}"
              f"{d_mus_mean:>14.3f}{pr:>16.3f} µs")


def main():
    ap = argparse.ArgumentParser(description="Summarize MG-march cost from cost-trace JSONL.")
    ap.add_argument("files", nargs="+", help="LABEL=path.jsonl (or path.jsonl)")
    ap.add_argument("--baseline", help="label to subtract in the delta table")
    ap.add_argument("--rounds-per-s", type=float, default=0.0,
                    help="rounds/s in the firing scenarios, to derive per-round march cost")
    args = ap.parse_args()

    blocks = []
    for spec in args.files:
        if "=" in spec:
            label, path = spec.split("=", 1)
        else:
            label, path = Path(spec).stem, spec
        s = summarize(label, path)
        if s is None:
            continue
        s["hz"] = 64.0
        s["role_scale"] = 1.0
        blocks.append(s)
        print_block(s)

    if args.baseline:
        print_deltas(blocks, args.baseline, args.rounds_per_s)
    return 0


if __name__ == "__main__":
    sys.exit(main())
