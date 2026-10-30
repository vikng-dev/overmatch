#!/usr/bin/env python3
"""series.py <server.log> [every] — one row per `every`-th stats line: index, live, frozen, thawed,
released, despawned, tick p50/p95, physics p50, send p50, cpu, entities."""
import json
import re
import sys

TOKEN = re.compile(r"server_stats\s+(\{.*\})")
every = int(sys.argv[2]) if len(sys.argv) > 2 else 1
i = 0
print("i live frozen thawed released despawned tick50 tick95 phys50 send50 cpu ent")
for line in open(sys.argv[1], errors="replace"):
    m = TOKEN.search(line)
    if not m:
        continue
    d = json.loads(m.group(1))
    if i % every == 0:
        db = d.get("debris") or {}
        t = d.get("tick_us") or {}
        p = d.get("physics_us") or {}
        s = d.get("send_us") or {}
        print(i, db.get("live", "-"), db.get("frozen", "-"), db.get("thawed", "-"), db.get("released", "-"),
              db.get("despawned", "-"), t.get("p50"), t.get("p95"), p.get("p50"), s.get("p50"), d.get("cpu_pct"),
              d.get("entities"))
    i += 1
