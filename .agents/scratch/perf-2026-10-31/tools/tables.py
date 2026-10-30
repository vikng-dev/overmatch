#!/usr/bin/env python3
"""tables.py <p30.json> <p2.json> <plain30.server.json> — the markdown tables of server-profile.md.

Rows key on (kind, name). Per tank: (mean@30 − mean@2) / 28, DERIVED. Names are shortened to
their last path segments (generic arguments kept short)."""
from __future__ import annotations

import json
import re
import sys

p30 = json.load(open(sys.argv[1]))
p2 = json.load(open(sys.argv[2]))
plain = json.load(open(sys.argv[3]))["stats"]

BOT_SYSTEMS = {"overmatch::net::server::drive_bot", "overmatch::net::server::spawn_bots",
               "overmatch::net::server::schedule_bot_respawn", "overmatch::net::server::respawn_dead_bots"}


def short(name: str) -> str:
    name = re.sub(r"\b(?:[a-z_][a-z0-9_]*::)+", "", name)  # drop module paths
    name = name.replace("par_for_each: query=", "par: ")
    return name if len(name) <= 110 else name[:107] + "..."


def index(d):
    return {(r["kind"], r["name"]): r for r in d["systems"]}


i30, i2 = index(p30), index(p2)
busy30 = p30["leaf_busy_us"]["mean"]
busy2 = p2["leaf_busy_us"]["mean"]
tick_plain = plain["tick_us.p50"]["median"]


def row_cells(r, r2):
    m2 = r2["mean_us"] if r2 else 0.0
    per_tank = (r["mean_us"] - m2) / 28
    return per_tank, m2


print("### Top 20 by mean\n")
print("| # | System (schedule, kind) | runs/tick | mean µs | p95 µs | share of busy | mean µs @ 2 tanks | per added tank µs (DERIVED) |")
print("|---|---|---|---|---|---|---|---|")
rows = sorted(p30["systems"], key=lambda r: -r["mean_us"])
for k, r in enumerate(rows[:20], 1):
    r2 = i2.get((r["kind"], r["name"]))
    per_tank, m2 = row_cells(r, r2)
    kind = "" if r["kind"] == "system" else f", {r['kind']}"
    print(f"| {k} | `{short(r['name'])}` ({r['schedule']}{kind}) | {r['runs_per_tick']:.2f} | {r['mean_us']:.0f} | "
          f"{r['p95_us']:.0f} | {100 * r['mean_us'] / busy30:.1f} % | {m2:.0f} | {per_tank:+.1f} |")

print("\n### Top 20 by p95\n")
print("| # | System (schedule, kind) | mean µs | p95 µs | max µs |")
print("|---|---|---|---|---|")
for k, r in enumerate(sorted(p30["systems"], key=lambda r: -r["p95_us"])[:20], 1):
    kind = "" if r["kind"] == "system" else f", {r['kind']}"
    print(f"| {k} | `{short(r['name'])}` ({r['schedule']}{kind}) | {r['mean_us']:.0f} | {r['p95_us']:.0f} | {r['max_us']:.0f} |")

print("\n### Per added tank, top 15 (DERIVED: (mean@30 − mean@2) / 28)\n")
print("| # | System (schedule) | per tank µs | mean @30 | mean @2 |")
print("|---|---|---|---|---|")
slopes = []
for key, r in i30.items():
    r2 = i2.get(key)
    per_tank, m2 = row_cells(r, r2)
    slopes.append((per_tank, r, m2))
slopes.sort(key=lambda t: -t[0])
for k, (s, r, m2) in enumerate(slopes[:15], 1):
    kind = "" if r["kind"] == "system" else f", {r['kind']}"
    print(f"| {k} | `{short(r['name'])}` ({r['schedule']}{kind}) | {s:+.1f} | {r['mean_us']:.0f} | {m2:.0f} |")

by_sched: dict[str, float] = {}
for r in p30["systems"]:
    by_sched[r["schedule"]] = by_sched.get(r["schedule"], 0.0) + r["mean_us"]
print("\n### Busy time by schedule (leaf systems + their commands + parallel batches), µs per tick\n")
print("| Schedule | 30 tanks | share | 2 tanks |")
print("|---|---|---|---|")
by2: dict[str, float] = {}
for r in p2["systems"]:
    by2[r["schedule"]] = by2.get(r["schedule"], 0.0) + r["mean_us"]
for s, v in sorted(by_sched.items(), key=lambda kv: -kv[1]):
    if v < 5:
        continue
    print(f"| {s} | {v:.0f} | {100 * v / busy30:.1f} % | {by2.get(s, 0.0):.0f} |")
print(f"| **all** | **{busy30:.0f}** | 100 % | **{busy2:.0f}** |")

print("\n### Schedule wall time per tick period (traced), µs\n")
print("| Schedule | runs/tick @30 | mean @30 | p95 @30 | mean @2 |")
print("|---|---|---|---|---|")
s2 = {s["name"]: s for s in p2["schedules"]}
for s in p30["schedules"]:
    if s["mean_us"] < 20:
        continue
    print(f"| {s['name']} | {s['runs_per_tick']:.2f} | {s['mean_us']:.0f} | {s['p95_us']:.0f} | {s2.get(s['name'], {}).get('mean_us', 0):.0f} |")

bots30 = sum(r["mean_us"] for r in p30["systems"] if r["name"] in BOT_SYSTEMS)
ai30 = sum(r["mean_us"] for r in p30["systems"] if r["name"] == "overmatch::net::server::drive_bot")
print(f"\nbots (drive_bot + spawn/schedule/respawn incl. commands): {bots30:.1f} µs/tick = {100 * bots30 / busy30:.2f} % of busy; "
      f"drive_bot alone {ai30:.1f} µs")
for r in p30["systems"]:
    if r["name"] in BOT_SYSTEMS:
        print(f"   {r['kind']:<16} {short(r['name'])}: mean {r['mean_us']:.1f} p95 {r['p95_us']:.1f}")
phys = p30["physics_leaf_us"]["mean"]
print(f"physics leaf (PhysicsSchedule + SubstepSchedule): {phys:.0f} µs/tick = {100 * phys / busy30:.1f} % of busy; @2: {p2['physics_leaf_us']['mean']:.0f}")
print(f"busy @30 {busy30:.0f} (p95 {p30['leaf_busy_us']['p95']:.0f}); @2 {busy2:.0f}; periods {p30['periods']} / {p2['periods']}; "
      f"lines {p30['trace_lines']} / {p2['trace_lines']}; mismatches {p30['pair_mismatches']} / {p2['pair_mismatches']}")
other = {r["name"]: r for r in p30["containers_and_other"]}
for nm in ("multithreaded executor: ", "update: "):
    if nm in other:
        print(f"   {nm} mean {other[nm]['mean_us']:.0f} runs/tick {other[nm]['runs_per_tick']:.2f}")
