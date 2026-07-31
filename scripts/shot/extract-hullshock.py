#!/usr/bin/env python3
"""Per-seed extraction for the HullShock delivery evidence capture (handoff item 1).

Reads a seed directory produced by run-hullshock-capture.sh and prints one JSON object with:
- the FINAL OrderingTally numbers from the target client's diagnostics line
- counts of the adoption debug log lines the memo names
- rollback-row stats from the target's SPIKE_TRACE: count, causes, per-component
  trg attribution, and the HullShock trigger count specifically
"""

import json
import re
import sys
from collections import Counter
from pathlib import Path

seed_dir = Path(sys.argv[1])

out = {"seed_dir": str(seed_dir)}

# --- OrderingTally from the last ROLLBACK-fired diagnostics line ---
tally_re = re.compile(
    r"shoves_on_impact=(\d+), shoves_on_budget=(\d+), shoves_bypassed=(\d+), "
    r"\s*shoves_undelivered=(\d+), max_shove_wait_ticks=(\d+)"
)
target_log = (seed_dir / "target.log").read_text(errors="replace")
last = None
for m in tally_re.finditer(target_log):
    last = m
if last:
    out["tally"] = {
        "released_on_impact": int(last.group(1)),
        "released_on_budget": int(last.group(2)),
        "bypassed": int(last.group(3)),
        "undelivered": int(last.group(4)),
        "max_wait_ticks": int(last.group(5)),
    }
else:
    out["tally"] = None

# --- adoption log line counts (target client) ---
patterns = {
    "adopted": "adopted authoritative fact",
    "forced_rollback_installed": "forced rollback installed",
    "external_event_cause": "ExternalEvent",
    "bypassed_lines": "was BYPASSED",
    "landed_unordered": "landed UNORDERED",
    "adopted_not_delivered": "ADOPTED BUT NOT DELIVERED",
    "not_deliverable_waiting": "not deliverable",
    "replay_window_drop": "replay window",
    "rollback_fired_lines": "ROLLBACK fired",
}
out["log_counts"] = {k: target_log.count(v) for k, v in patterns.items()}

# --- rollback rows from the target trace ---
rows = []
trace_path = seed_dir / "target-trace.client.jsonl"
if trace_path.exists():
    for line in trace_path.read_text(errors="replace").splitlines():
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if row.get("k") == "rollback":
            rows.append(row)

causes = Counter(r.get("cause") for r in rows)
trg_components = Counter()
hullshock_rollbacks = 0
depths = [r["depth"] for r in rows if r.get("depth") is not None]
for r in rows:
    trg = r.get("trg") or []
    names = {t[0] for t in trg}
    for name in names:
        trg_components[name] += 1
    if "HullShock" in names:
        hullshock_rollbacks += 1

out["trace"] = {
    "rollback_rows": len(rows),
    "causes": dict(causes),
    "trg_component_rollbacks": dict(trg_components),
    "rollbacks_with_hullshock_trg": hullshock_rollbacks,
    "no_trg_rollbacks": sum(1 for r in rows if not r.get("trg")),
    "depth_max": max(depths) if depths else None,
    "depth_mean": (sum(depths) / len(depths)) if depths else None,
}

# --- shot summary sanity (hits happened at all) ---
summary = seed_dir / "summary.json"
if summary.exists() and summary.stat().st_size > 0:
    try:
        s = json.loads(summary.read_text())
        out["shot_summary_keys"] = {
            k: s[k]
            for k in s
            if isinstance(s[k], (int, float)) and ("hit" in k or "shot" in k or "confirm" in k)
        }
    except json.JSONDecodeError:
        out["shot_summary_keys"] = "unparseable"
else:
    out["shot_summary_keys"] = None

print(json.dumps(out, indent=2))
