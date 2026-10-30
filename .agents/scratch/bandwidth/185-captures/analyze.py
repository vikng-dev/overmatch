"""Client download per remote tank per tick from bytes.sh output.

Usage: python3 -I analyze.py <out-dir>
Reads <out-dir>/edges.tsv (cond, t0, in0, out0, t1, in1, out1) and each condition's server log
(the server_stats lines in the measurement window: those after the marker line). Prints per
condition the down rate, the server tick rate and tank count in the window, bytes per tick, and a
least-squares slope of bytes per tick against tank count: bytes per added tank per tick.
"""

import json
import sys
from pathlib import Path

out = Path(sys.argv[1])
rows = []
for line in (out / "edges.tsv").read_text().splitlines():
    cond, t0, in0, out0, t1, in1, out1 = line.split("\t")
    span = float(t1) - float(t0)
    down = (int(in1) - int(in0)) / span
    up = (int(out1) - int(out0)) / span
    log = (out / f"{cond}.server.log").read_text().splitlines()
    marker = (out / f"{cond}.marker").read_text().strip()
    stats = [l.split("server_stats ", 1)[1] for l in log if "server_stats " in l]
    marker_json = marker.split("server_stats ", 1)[1] if "server_stats " in marker else None
    start = stats.index(marker_json) + 1 if marker_json in stats else 0
    window = [json.loads(s) for s in stats[start : start + int(span)]]
    hz = sum(d["tick_hz"] for d in window) / len(window)
    tanks = sorted({d["tanks"] for d in window})
    overruns = sum(d["overruns"] for d in window)
    rows.append((cond, tanks, down, up, hz, overruns))
    print(
        f"{cond}: tanks {tanks} down {down / 1000:.2f} kB/s up {up / 1000:.2f} kB/s "
        f"server {hz:.2f} Hz overruns {overruns} -> {down / hz:.1f} B/tick down"
    )

xs = [max(r[1]) for r in rows]
ys = [r[2] / r[4] for r in rows]
n = len(xs)
mx = sum(xs) / n
my = sum(ys) / n
sxx = sum((x - mx) ** 2 for x in xs)
slope = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sxx
icpt = my - slope * mx
print(f"slope {slope:.1f} B per tank per tick, intercept {icpt:.1f} B/tick (least squares, {n} points)")
