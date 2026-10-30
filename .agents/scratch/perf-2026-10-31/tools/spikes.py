#!/usr/bin/env python3
"""spikes.py <trace.json> <window.json> [min_ms] — every system / commands span inside the window
whose exclusive time (minus nested system/schedule/commands spans on its thread) is at least
min_ms (default 5), in time order, plus the FixedMain spans at least 3 × min_ms long."""
import json
import sys
from collections import defaultdict

trace, window = sys.argv[1], sys.argv[2]
min_us = float(sys.argv[3] if len(sys.argv) > 3 else 5) * 1000
win = json.load(open(window))
w0, w1 = win["w0_us"], win["w1_us"]
stacks = defaultdict(list)
out = []
with open(trace, "rb") as f:
    for raw in f:
        raw = raw.strip().rstrip(b",")
        if not raw.startswith(b"{"):
            continue
        try:
            ev = json.loads(raw)
        except json.JSONDecodeError:
            continue
        ph = ev.get("ph")
        if ph == "B":
            stacks[ev["tid"]].append([ev["name"], ev["ts"], 0.0])
        elif ph == "E":
            st = stacks[ev["tid"]]
            if not st:
                continue
            name, start, child = st.pop()
            dur = ev["ts"] - start
            kind = name.split(":", 1)[0]
            if st and kind in ("system", "schedule", "system_commands"):
                st[-1][2] += dur
            if not (w0 <= start <= w1):
                continue
            if kind in ("system", "system_commands") and dur - child >= min_us:
                out.append((start, dur - child, name))
            elif name == "schedule: name=FixedMain" and dur >= 3 * min_us:
                out.append((start, dur, name))
out.sort()
for ts, us, name in out:
    print(f"{(ts - w0) / 1e6:8.3f} s  {us / 1000:8.2f} ms  {name}")
