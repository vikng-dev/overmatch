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

THE TAIL. tick-trace.sh stops the server with SIGINT and, 10 s later, SIGKILL, so a trace may end
in two ways that are not errors, both reported in the output: spans still open when the writer
stopped (`open_at_end`), and after a SIGKILL a torn last line (`torn_tail_line`). A span counts
at its end, so an open one counts nowhere: one begun inside the window would leave its period
short and is refused; one begun outside it is dropped. Every other malformed record, and every
end that does not close the innermost open span on its thread, is refused wherever it falls: a
thread's span stack runs through the whole trace, so one bad pair shifts every later span there.
"""
from __future__ import annotations

import bisect
import json
import math
import os
import re
import sys
from array import array
from collections import defaultdict
from collections.abc import Iterator
from typing import NamedTuple

NAME_FIELD = re.compile(r'^(system|system_commands|schedule): name=(.*)$')
# The phases tracing-chrome 0.7's Threaded style (bevy_log 0.20.1's chrome layer) writes besides B
# and E, which carry no span time; any other phase is refused.
IGNORED_PHASES = {"i": "an instant event (a log line)", "M": "a thread's name"}
TRACE_REPAIR = "recapture with bounded numeric fields and nesting by rerunning tick-trace.sh"
WINDOW_REPAIR = ('tick-trace.sh writes it; rerun the wrapper, or write {"w0_us": <start>, '
                 '"w1_us": <end>} in µs from the server\'s launch, start before end')


class TraceError(Exception):
    """The trace cannot be profiled; the message names the file and the line."""


class WindowError(Exception):
    """The window file cannot be read; the message names the file, the cause and the repair."""


class OutputError(Exception):
    """The profile cannot be written; the message names the path, the cause and the repair."""


class Span(NamedTuple):
    """One span begin (`B`) or end (`E`) record."""
    ph: str
    ts: float
    tid: int
    name: str


class Window(NamedTuple):
    """The measured window, µs from the server's launch; `raw` is the file as written."""
    w0_us: float
    w1_us: float
    raw: dict


def read_window(path: str) -> Window:
    try:
        with open(path, "rb") as f:
            raw = json.loads(f.read().decode("utf-8"))
        if not isinstance(raw, dict):
            raise ValueError("the window is not a JSON object")
        bounds = []
        for field in ("w0_us", "w1_us"):
            if type(raw.get(field)) not in (int, float):
                got = f"is {raw[field]!r}, not a number" if field in raw else "is missing"
                raise ValueError(f"field {field} {got}")
            bound = float(raw[field])
            if not math.isfinite(bound):
                raise ValueError(f"field {field} must be finite")
            bounds.append(bound)
        if not bounds[0] < bounds[1]:
            raise ValueError(f"w0_us {bounds[0]} is not before w1_us {bounds[1]}")
        return Window(*bounds, raw)
    except (OSError, ValueError, OverflowError, RecursionError) as e:
        line = e.lineno if isinstance(e, json.JSONDecodeError) else None
        if isinstance(e, UnicodeDecodeError):
            line = e.object[:e.start].count(b"\n") + 1
        location = f"{path}:{line}" if line is not None else path
        raise WindowError(f"{location}: {e}; {WINDOW_REPAIR}") from e


class Trace:
    """A tracing-chrome file's records: `[`, the records joined by `,\\n`, then `\\n]`. Iterating
    yields (line, span) for each B and E record, skipping IGNORED_PHASES; afterwards `lines` is the
    line count and `torn_tail` the line of a torn last record, or None."""

    def __init__(self, path: str) -> None:
        self.path = path
        self.lines = 0
        self.torn_tail: int | None = None

    def _admit(self, record: object) -> Span | None:
        """The record as a Span, or None for a phase in IGNORED_PHASES."""
        ph = record.get("ph") if isinstance(record, dict) else None
        if not isinstance(ph, str):
            raise ValueError(f"a record's phase is {type(ph).__name__}, not a string")
        if ph in IGNORED_PHASES:
            return None
        if ph not in ("B", "E"):
            raise ValueError(f"phase {ph!r} is neither a span begin or end (B, E) nor one the "
                             f"profiler ignores ({', '.join(IGNORED_PHASES)})")
        span = Span(ph, record.get("ts"), record.get("tid"), record.get("name"))
        if type(span.ts) not in (int, float) or type(span.tid) is not int or type(span.name) is not str:
            raise ValueError(f"a {ph} record needs a numeric ts, an integer tid and a string name, "
                             f"not {span}")
        try:
            ts = float(span.ts)
        except OverflowError as e:
            raise ValueError("ts exceeds finite float range") from e
        if not math.isfinite(ts):
            raise ValueError("ts must be finite")
        span.name.encode("utf-8")  # Names reach the operator's UTF-8 summary, not just JSON.
        return span._replace(ts=ts)

    def __iter__(self) -> Iterator[tuple[int, Span]]:
        closed = torn = None  # the closing `]`'s line; (line, why) of a record that did not parse
        self.lines = 0
        self.torn_tail = None
        line = None
        try:
            with open(self.path, "rb") as f:
                while True:
                    line = self.lines + 1
                    raw = next(f, None)
                    if raw is None:
                        break
                    self.lines = line
                    if torn:
                        line = torn[0]
                        raise ValueError(f"malformed record ({torn[1]}), and line {self.lines} "
                                         "follows it: only the last may be torn")
                    if closed:
                        raise ValueError(f"a line after the closing ] on line {closed}")
                    raw = raw.strip()
                    if raw == b"]":
                        closed = self.lines
                        continue
                    if self.lines == 1 and raw == b"[":
                        continue
                    try:
                        record = json.loads(raw.removesuffix(b","))
                    except (json.JSONDecodeError, UnicodeDecodeError) as e:
                        torn = (self.lines, str(e))
                        continue
                    except (ValueError, RecursionError) as e:
                        raise ValueError(f"record exceeds JSON decoder limits ({e})") from e
                    span = self._admit(record)
                    if span is not None:
                        yield self.lines, span
            self.torn_tail = torn[0] if torn else None
        except (OSError, ValueError, OverflowError, RecursionError) as e:
            location = f"{self.path}:{line}" if line is not None else self.path
            raise TraceError(f"{location}: {e}; {TRACE_REPAIR}") from e


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


def profile(trace_path: str, window_path: str, out_path: str) -> int:
    w0, w1, win = read_window(window_path)

    def in_window(ts: float) -> bool:
        return w0 <= ts <= w1

    trace = Trace(trace_path)
    ticks: list[float] = []  # FixedMain starts, in trace order
    stacks: dict[int, list[list]] = defaultdict(list)
    open_sched: list[tuple[float, str, int]] = []  # (start, label, tid), in start order
    acc: dict[tuple[str, str], array] = {}
    runs_abs: dict[tuple[str, str], array] = {}
    sched_of: dict[tuple[str, str], dict[str, int]] = defaultdict(lambda: defaultdict(int))
    container: dict[str, bool] = {}
    sched_acc: dict[str, array] = {}
    sched_runs_abs: dict[str, array] = {}
    last_ts = 0.0

    def add(table: dict, key, index: int, value: float) -> None:
        a = table.get(key)
        if a is None:
            a = table[key] = array("d")
        if len(a) <= index:
            a.extend([0.0] * (index + 1 - len(a) + 256))
        a[index] += value

    for line, (ph, ts, tid, span_name) in trace:
        last_ts = max(last_ts, ts)
        stack = stacks[tid]
        kind, name = classify(span_name)
        if ph == "B":
            sched = open_sched[-1][1] if open_sched else "-"
            stack.append([kind, name, ts, 0.0, sched, False, line])
            if kind == "schedule":
                open_sched.append((ts, name, tid))
                if name == "FixedMain":
                    ticks.append(ts)
            continue
        if not stack or stack[-1][:2] != [kind, name]:
            inner = (f"{stack[-1][0]} {stack[-1][1]!r}, begun at {stack[-1][2]} µs on line "
                     f"{stack[-1][6]}") if stack else "none"
            raise TraceError(f"{trace_path}:{line}: thread {tid} ends {kind} {name!r} at {ts} µs, "
                             f"but its innermost open span is {inner}; {TRACE_REPAIR}")
        _, _, start, child, sched, has_sched, _ = stack.pop()
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
        if not in_window(start) or not ticks:
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

    open_at_end = 0
    for tid, stack in stacks.items():
        for kind, name, start, *_, line in stack:
            if in_window(start):
                raise TraceError(f"{trace_path}:{line}: thread {tid}'s {kind} {name!r}, begun at "
                                 f"{start} µs inside the window, never ends; {TRACE_REPAIR}")
            open_at_end += 1

    ticks.sort()
    first = bisect.bisect_left(ticks, w0)
    last = bisect.bisect_right(ticks, w1) - 1  # the period starting at ticks[last] may run past w1
    periods = range(first, last)  # complete periods inside the window
    n = len(periods)
    if n < 10:
        raise TraceError(f"{trace_path}: only {n} tick periods in window {window_path}; "
                         "recapture a longer window with tick-trace.sh")

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
    other_rows = []
    for (kind, name), vals in per.items():
        if kind == "other" or (kind == "system" and container.get(name)):
            other_rows.append({
                "kind": "container" if kind == "system" else kind,
                "name": name,
                "runs_per_tick": runs[(kind, name)] / n,
                "mean_us": sum(vals) / n,
                "p95_us": pct(vals, 0.95),
            })
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
        "trace_lines": trace.lines,
        "trace_last_ts_us": last_ts,
        "torn_tail_line": trace.torn_tail,
        "open_at_end": open_at_end,
        "periods": n,
        "period_us_mean": sum(period_us) / n,
        "leaf_busy_us": {"mean": mean_leaf, "p50": pct(leaf_total, 0.5), "p95": pct(leaf_total, 0.95), "max": max(leaf_total)},
        "physics_leaf_us": {"mean": sum(physics_total) / n, "p50": pct(physics_total, 0.5), "p95": pct(physics_total, 0.95)},
        "systems": rows,
        "containers_and_other": other_rows,
        "schedules": schedules,
    }
    try:
        with open(out_path, "w", encoding="utf-8") as f:
            json.dump(out, f, indent=1)
        print(f"{trace_path}: {trace.lines} lines, {n} tick periods (mean {out['period_us_mean']:.0f} µs), "
              f"leaf busy mean {mean_leaf:.0f} µs/tick, physics leaf {out['physics_leaf_us']['mean']:.0f}, "
              f"{len(rows)} leaf keys, {open_at_end} spans open at the end, "
              f"torn last line: {trace.torn_tail or 'none'}")
        for r in rows[:25]:
            print(f"  {r['mean_us']:8.1f} {r['p95_us']:8.1f} {100*r['share_of_busy']:5.1f}% {r['runs_per_tick']:5.2f} {r['schedule']:<18} {r['kind']:<15} {r['name']}")
        for s in schedules[:15]:
            print(f"  sched {s['mean_us']:8.1f} {s['p95_us']:8.1f} {s['runs_per_tick']:5.2f} {s['name']}")
        sys.stdout.flush()
    except (OSError, ValueError, RecursionError) as e:
        if isinstance(e, BrokenPipeError):
            # Shutdown retries buffered stdout; keep that flush from replacing the refusal.
            with open(os.devnull, "w") as devnull:
                os.dup2(devnull.fileno(), sys.stdout.fileno())
        raise OutputError(f"{out_path}: cannot write the profile or summary ({e}); create its "
                          "directory or pass a writable UTF-8 output and terminal") from e
    return 0


def main() -> int:
    if len(sys.argv) != 4:
        print("usage: trace_profile.py <trace.json> <window.json> <out.json>", file=sys.stderr)
        return 2
    try:
        return profile(*sys.argv[1:])
    except (TraceError, WindowError, OutputError) as e:
        print(f"trace_profile: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
