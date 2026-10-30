#!/usr/bin/env python3
"""Per-system cost per server tick from a bevy `trace_chrome` trace (tracing-chrome 0.7, Threaded).

usage: trace_profile.py <trace.json> <window.json> <out.json>

Spans: `system: name=X` (one run of system X), `system_commands: name=X` (its deferred commands),
`schedule: name=S` (one run of schedule S), `par_for_each: …` (a parallel query batch), others.
B/E pairs nest per thread (tid). A span's EXCLUSIVE time is its duration minus the durations of
the system / schedule / commands spans nested in it on the same thread, so a par_for_each batch
run on the system's own thread stays in the system and a schedule run inside an exclusive system
(RunFixedMainLoop, avian's run_physics_schedule) does not.

A CONTAINER is a system whose span holds a schedule span: it is a schedule runner, reported with
the schedules, never ranked with the leaf systems.

THE TICK PERIOD: from one `schedule: name=FixedMain` start to the next. Every span is attributed
to the period its start falls in, so a period holds one fixed tick and the ~4 frames (4 ms runner)
around it. Only periods that start inside the window (window.json, µs from the server's launch;
the trace clock starts at the LogPlugin's build, a few ms after launch) are kept.

THE SCHEDULE of a span: the innermost schedule span open (on any thread) when it started. Bevy
runs a schedule only with exclusive world access, so at most one nested chain of schedules is
open at a time on a headless server (no render sub-app).
"""
from __future__ import annotations

import bisect
import json
import math
import re
import sys
from collections import defaultdict

NAME_FIELD = re.compile(r'^(system|system_commands|schedule): name=(.*)$')


def clean(v: str) -> str:
    v = v.strip()
    if len(v) >= 2 and v[0] == '"' and v[-1] == '"':
        v = v[1:-1]
    return v


def classify(name: str) -> tuple[str, str]:
    m = NAME_FIELD.match(name)
    if m:
        return m.group(1), clean(m.group(2))
    if name.startswith("par_for_each"):
        return "par", name
    return "other", name


def pct(values: list[float], q: float) -> float:
    if not values:
        return float("nan")
    s = sorted(values)
    k = (len(s) - 1) * q
    f = math.floor(k)
    c = min(f + 1, len(s) - 1)
    return s[f] + (s[c] - s[f]) * (k - f)


def main() -> int:
    trace_path, window_path, out_path = sys.argv[1:4]
    win = json.load(open(window_path))
    w0, w1 = win["w0_us"], win["w1_us"]

    from array import array

    ticks: list[float] = []  # FixedMain starts, in trace order
    stacks: dict[int, list[list]] = defaultdict(list)
    open_sched: list[tuple[float, str, int]] = []  # (start, label, tid), in start order
    acc: dict[tuple[str, str], array] = {}
    runs_abs: dict[tuple[str, str], array] = {}
    sched_of: dict[tuple[str, str], dict[str, int]] = defaultdict(lambda: defaultdict(int))
    container: dict[str, bool] = {}
    sched_acc: dict[str, array] = {}
    sched_runs_abs: dict[str, array] = {}
    mismatches = 0
    lines = 0
    last_ts = 0.0

    def add(table: dict, key, index: int, value: float) -> None:
        a = table.get(key)
        if a is None:
            a = table[key] = array("d")
        if len(a) <= index:
            a.extend([0.0] * (index + 1 - len(a) + 256))
        a[index] += value

    with open(trace_path, "rb") as f:
        for raw in f:
            lines += 1
            raw = raw.strip()
            if raw.endswith(b","):
                raw = raw[:-1]
            if not raw.startswith(b"{"):
                continue
            try:
                ev = json.loads(raw)
            except json.JSONDecodeError:
                continue  # a torn last line (the writer was killed)
            ph = ev.get("ph")
            if ph not in ("B", "E"):
                continue
            ts = ev["ts"]
            last_ts = max(last_ts, ts)
            tid = ev["tid"]
            stack = stacks[tid]
            if ph == "B":
                kind, name = classify(ev["name"])
                sched = open_sched[-1][1] if open_sched else "-"
                stack.append([kind, name, ts, 0.0, sched, False])
                if kind == "schedule":
                    open_sched.append((ts, name, tid))
                    if name == "FixedMain":
                        ticks.append(ts)
                continue
            if not stack:
                mismatches += 1
                continue
            kind, name, start, child, sched, has_sched = stack.pop()
            if classify(ev["name"])[1] != name:
                mismatches += 1
            dur = ts - start
            if stack and kind in ("system", "schedule", "system_commands"):
                stack[-1][3] += dur
                if kind == "schedule":
                    stack[-1][5] = True
            if kind == "schedule":
                for i in range(len(open_sched) - 1, -1, -1):
                    if open_sched[i][1] == name and open_sched[i][2] == tid:
                        del open_sched[i]
                        break
            if not (w0 - 20_000 <= start <= w1 + 20_000) or not ticks:
                continue
            p = bisect.bisect_right(ticks, start) - 1
            if p < 0:
                continue
            if kind == "schedule":
                add(sched_acc, name, p, dur)
                add(sched_runs_abs, name, p, 1.0)
                continue
            if kind == "par" and stack and stack[-1][0] == "system":
                continue  # already inside its system's exclusive time
            if kind == "system" and has_sched:
                container[name] = True
            key = (kind, name)
            add(acc, key, p, dur - child)
            add(runs_abs, key, p, 1.0)
            sched_of[key][sched] += 1

    ticks.sort()
    first = bisect.bisect_left(ticks, w0)
    last = bisect.bisect_right(ticks, w1) - 1  # the period starting at ticks[last] may run past w1
    periods = list(range(first, last))  # complete periods inside the window
    n = len(periods)
    if n < 10:
        print(f"only {n} tick periods in the window", file=sys.stderr)
        return 1

    def window_of(a: array) -> list[float]:
        vals = list(a[first:last])
        return vals + [0.0] * (n - len(vals))

    per = {key: window_of(a) for key, a in acc.items()}
    runs = {key: sum(window_of(a)) for key, a in runs_abs.items()}
    sched_per = {name: window_of(a) for name, a in sched_acc.items()}
    sched_count = {name: sum(window_of(a)) for name, a in sched_runs_abs.items()}

    physics_scheds = {"PhysicsSchedule", "SubstepSchedule"}
    leaf_total = [0.0] * n
    physics_total = [0.0] * n
    rows = []
    for (kind, name), vals in per.items():
        if kind == "system" and container.get(name):
            continue
        if kind == "other":
            continue
        top_sched = max(sched_of[(kind, name)].items(), key=lambda kv: kv[1])[0]
        for i, v in enumerate(vals):
            leaf_total[i] += v
            if top_sched in physics_scheds:
                physics_total[i] += v
        rows.append({
            "kind": kind,
            "name": name,
            "schedule": top_sched,
            "runs_per_tick": runs[(kind, name)] / n,
            "mean_us": sum(vals) / n,
            "p50_us": pct(vals, 0.5),
            "p95_us": pct(vals, 0.95),
            "max_us": max(vals),
        })
    other_rows = []
    for (kind, name), vals in per.items():
        if kind != "other" and not (kind == "system" and container.get(name)):
            continue
        other_rows.append({
            "kind": "container" if kind == "system" else kind,
            "name": name,
            "runs_per_tick": runs[(kind, name)] / n,
            "mean_us": sum(vals) / n,
            "p95_us": pct(vals, 0.95),
        })
    mean_leaf = sum(leaf_total) / n
    for r in rows:
        r["share_of_busy"] = r["mean_us"] / mean_leaf if mean_leaf else float("nan")
    rows.sort(key=lambda r: -r["mean_us"])
    other_rows.sort(key=lambda r: -r["mean_us"])
    schedules = []
    for name, vals in sched_per.items():
        schedules.append({
            "name": name,
            "runs_per_tick": sched_count[name] / n,
            "mean_us": sum(vals) / n,
            "p50_us": pct(vals, 0.5),
            "p95_us": pct(vals, 0.95),
            "max_us": max(vals),
        })
    schedules.sort(key=lambda r: -r["mean_us"])
    period_us = [ticks[i + 1] - ticks[i] for i in periods]
    out = {
        "trace": trace_path,
        "window": win,
        "trace_lines": lines,
        "trace_last_ts_us": last_ts,
        "pair_mismatches": mismatches,
        "periods": n,
        "period_us_mean": sum(period_us) / n,
        "leaf_busy_us": {"mean": mean_leaf, "p50": pct(leaf_total, 0.5), "p95": pct(leaf_total, 0.95), "max": max(leaf_total)},
        "physics_leaf_us": {"mean": sum(physics_total) / n, "p50": pct(physics_total, 0.5), "p95": pct(physics_total, 0.95)},
        "systems": rows,
        "containers_and_other": other_rows,
        "schedules": schedules,
    }
    json.dump(out, open(out_path, "w"), indent=1)
    print(f"{trace_path}: {lines} lines, {n} tick periods (mean {out['period_us_mean']:.0f} µs), "
          f"leaf busy mean {mean_leaf:.0f} µs/tick, physics leaf {out['physics_leaf_us']['mean']:.0f}, "
          f"{len(rows)} leaf keys, {mismatches} B/E mismatches")
    for r in rows[:25]:
        print(f"  {r['mean_us']:8.1f} {r['p95_us']:8.1f} {100*r['share_of_busy']:5.1f}% {r['runs_per_tick']:5.2f} {r['schedule']:<18} {r['kind']:<15} {r['name']}")
    for s in schedules[:15]:
        print(f"  sched {s['mean_us']:8.1f} {s['p95_us']:8.1f} {s['runs_per_tick']:5.2f} {s['name']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
