#!/usr/bin/env python3
"""regress.py <conditions.tsv>... — the debris curve from the per-second stats lines.

For each condition (`<cond> <lever> <x.server.json>` per TSV line, run-debris-stress.sh's and
run-online-night.sh's layout) the window is the stats lines after the condition's marker in its
window log: `<x>.window.log` (local) or `<x>.server.log` (Edgegap), the marker `<x>.marker`. A line
without a `debris` object (lever off) has live = frozen = 0.

Prints per condition: lines, medians of live, frozen, tick p50/p95, physics p50, send p50, cpu.
Then per group (`-t<value>` suffix of the condition name; `-` without one), two least-squares fits
over every second of every condition in the group:
  (1) tick = a + b·live                      (condition medians, the curve)
  (2) tick = a + b·live + c·frozen           (per second, separating moving from frozen pieces)
for tick p50 and p95, physics p50 and cpu_pct."""
from __future__ import annotations

import json
import re
import statistics
import sys
from pathlib import Path

TOKEN = re.compile(r"server_stats\s+(\{.*\})")


def window_rows(json_path: Path) -> list[dict]:
    stem = str(json_path)[: -len(".server.json")]
    marker_p = Path(stem + ".marker")
    log_p = Path(stem + ".window.log")
    if not log_p.exists():
        log_p = Path(stem + ".server.log")
    if not log_p.exists():
        return []
    lines = log_p.read_text(errors="replace").splitlines()
    marker = marker_p.read_text(errors="replace").strip() if marker_p.exists() else ""
    start = 0
    if marker:
        for i in range(len(lines) - 1, -1, -1):
            if lines[i].strip() == marker:
                start = i + 1
                break
    rows = []
    for line in lines[start:]:
        m = TOKEN.search(line)
        if not m:
            continue
        try:
            d = json.loads(m.group(1))
        except json.JSONDecodeError:
            continue
        db = d.get("debris") or {}
        t = d.get("tick_us") or {}
        p = d.get("physics_us") or {}
        s = d.get("send_us") or {}
        if t.get("p50") is None:
            continue
        rows.append({"live": db.get("live", 0), "frozen": db.get("frozen", 0), "tick50": t.get("p50"),
                     "tick95": t.get("p95"), "phys50": p.get("p50"), "send50": s.get("p50"),
                     "cpu": d.get("cpu_pct"), "over": d.get("overruns"), "hz": d.get("tick_hz"),
                     "thr": d.get("throttled_usec")})
    return rows


def ols(X: list[list[float]], y: list[float]) -> tuple[list[float], float] | None:
    """Least squares via the normal equations (k ≤ 3); returns (coefficients, R²)."""
    k = len(X[0])
    xtx = [[sum(r[i] * r[j] for r in X) for j in range(k)] for i in range(k)]
    xty = [sum(r[i] * v for r, v in zip(X, y)) for i in range(k)]
    # Gauss-Jordan
    a = [row[:] + [xty[i]] for i, row in enumerate(xtx)]
    for c in range(k):
        piv = max(range(c, k), key=lambda r: abs(a[r][c]))
        if abs(a[piv][c]) < 1e-12:
            return None
        a[c], a[piv] = a[piv], a[c]
        for r in range(k):
            if r != c:
                f = a[r][c] / a[c][c]
                a[r] = [x - f * z for x, z in zip(a[r], a[c])]
    beta = [a[i][k] / a[i][i] for i in range(k)]
    my = sum(y) / len(y)
    ss_tot = sum((v - my) ** 2 for v in y)
    ss_res = sum((v - sum(b * x for b, x in zip(beta, r))) ** 2 for r, v in zip(X, y))
    return beta, (1 - ss_res / ss_tot if ss_tot else float("nan"))


def med(rows, key):
    vals = [r[key] for r in rows if r.get(key) is not None]
    return statistics.median(vals) if vals else None


def main() -> int:
    groups: dict[str, list[tuple[str, str, list[dict]]]] = {}
    hdr = ["cond", "lever", "lines", "live", "frozen", "tick50", "tick95", "phys50", "send50", "cpu", "over_max", "hz"]
    print("\t".join(hdr))
    for tsv in sys.argv[1:]:
        tsv = Path(tsv)
        for line in tsv.read_text().splitlines():
            if not line.strip():
                continue
            cond, lever, path = line.split("\t")[:3]
            p = Path(path) if Path(path).is_absolute() else tsv.parent / path
            rows = window_rows(p)
            m = re.search(r"-t(\w+)$", cond)
            groups.setdefault(m.group(1) if m else "-", []).append((cond, lever, rows))
            if not rows:
                print(f"{cond}\t{lever}\t0")
                continue
            cells = [cond, lever, str(len(rows))]
            for k in ("live", "frozen", "tick50", "tick95", "phys50", "send50", "cpu"):
                v = med(rows, k)
                cells.append("-" if v is None else f"{v:.0f}")
            cells.append(str(max((r["over"] or 0) for r in rows)))
            cells.append(f"{med(rows, 'hz'):.1f}")
            print("\t".join(cells))
    for g, conds in groups.items():
        conds = [c for c in conds if c[2]]
        if not conds:
            continue
        print(f"\n== threads {g}: {len(conds)} conditions")
        for y in ("tick50", "tick95", "phys50", "cpu"):
            pts = [(med(rows, "live"), med(rows, y)) for _, _, rows in conds if med(rows, y) is not None]
            f1 = ols([[1.0, x] for x, _ in pts], [v for _, v in pts]) if len(pts) > 2 else None
            per = [r for _, _, rows in conds for r in rows if r.get(y) is not None]
            f2 = ols([[1.0, r["live"], r["frozen"]] for r in per], [r[y] for r in per]) if len(per) > 3 else None
            s1 = f"medians: a {f1[0][0]:8.1f}  b/live {f1[0][1]:7.3f}  R² {f1[1]:.3f}" if f1 else "medians: -"
            s2 = (f"per-second: a {f2[0][0]:8.1f}  b/live {f2[0][1]:7.3f}  c/frozen {f2[0][2]:7.3f}  R² {f2[1]:.3f}  n {len(per)}"
                  if f2 else "per-second: -")
            print(f"  {y:7s} {s1}   |   {s2}")
        lives = [med(rows, "live") for _, _, rows in conds]
        print(f"  live medians span {min(lives):.0f}–{max(lives):.0f}")
        # The curve per second: every armed second binned by its live count, the lever-off
        # seconds as their own row (their house rubble is uncounted: the census needs the lever).
        edges = [0, 1, 128, 256, 512, 1024, 2048, 4096, 8192, 10**9]
        print("  per-second bins (armed seconds by live; `off` = lever-off seconds):")
        print("    live_bin      n   live_med frozen_med tick50 tick95 phys50 send50 cpu   slope_vs_prev(us/piece)")
        prev = None
        bins = [("off", [r for c, lv, rows in conds if lv == "off" for r in rows])]
        armed = [r for c, lv, rows in conds if lv != "off" for r in rows]
        for lo, hi in zip(edges, edges[1:]):
            sel = [r for r in armed if lo <= r["live"] < hi]
            if sel:
                bins.append((f"{lo}-{hi - 1 if hi < 10**9 else 'max'}", sel))
        for name, sel in bins:
            if not sel:
                continue
            lm, t50 = med(sel, "live"), med(sel, "tick50")
            slope = "" if prev is None or lm == prev[0] else f"{(t50 - prev[1]) / (lm - prev[0]):+.2f}"
            print(f"    {name:12s} {len(sel):5d} {lm:9.0f} {med(sel, 'frozen'):9.0f} {t50:7.0f} {med(sel, 'tick95'):7.0f} "
                  f"{med(sel, 'phys50'):7.0f} {med(sel, 'send50'):6.0f} {med(sel, 'cpu'):5.0f}   {slope}")
            if name != "off":
                prev = (lm, t50)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
