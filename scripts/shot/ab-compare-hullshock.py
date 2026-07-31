#!/usr/bin/env python3
"""A/B accounting for the inert-comparator change, from the per-fact k=fact rows.

Per seed and arm: staged/retired counts and route split, release verdicts (on-impact vs
on-budget with waits), requested count, sequence continuity among staged facts, HullShock trg
incidence, and the belt-first facts' stage-vs-spark ordering.

Usage: ab-compare-hullshock.py <base-dir> [arm-dir ...]
Each arm dir holds seed1..seed5 directories from run-hullshock-capture.sh.
"""

import json
import sys
from collections import Counter
from pathlib import Path

base = Path(sys.argv[1])
arms = sys.argv[2:] or ["evcap-baseline", "evcap-treatment"]


def load(seed_dir):
    facts, rollbacks = [], []
    for line in (seed_dir / "target-trace.client.jsonl").read_text(errors="replace").splitlines():
        try:
            r = json.loads(line)
        except json.JSONDecodeError:
            continue
        if r.get("k") == "fact":
            facts.append(r)
        elif r.get("k") == "rollback":
            rollbacks.append(r)
    return facts, rollbacks


def summarize(seed_dir):
    facts, rollbacks = load(seed_dir)
    staged = [f for f in facts if f["ev"] == "staged"]
    retired = [f for f in facts if f["ev"] == "retired"]
    released = [f for f in facts if f["ev"] == "released"]
    sparks = [f for f in facts if f["ev"] == "spark"]
    routes = Counter(f["route"] for f in retired)
    seqs = sorted(f["seq"] for f in staged)
    gaps = [s for s in range(seqs[0], seqs[-1] + 1) if s not in set(seqs)] if seqs else []
    hshk_trg = sum(
        1 for r in rollbacks if any(t[0] == "HullShock" for t in (r.get("trg") or []))
    )
    on_budget = [f for f in released if not f["shown"]]
    waits = [f["waited"] for f in released]
    unterminated = set(f["seq"] for f in staged) - set(f["seq"] for f in retired)

    # Belt-first facts: a staged fact whose OWN span had no spark drawn yet at staging time.
    spark_ticks = sorted(s["at"] for s in sparks)
    belt_first = []
    for f in staged:
        span = f.get("span")
        if span is None:
            continue
        covered = any(span[0] <= t <= span[1] for t in spark_ticks)
        drawn_before = any(
            span[0] <= s["at"] <= span[1] and s["tick"] <= f["staged_at"] for s in sparks
        )
        if covered and not drawn_before:
            belt_first.append(f["seq"])

    return {
        "staged": len(staged),
        "retired": len(retired),
        "routes": dict(routes),
        "unterminated": sorted(unterminated),
        "seq_gaps_among_staged": gaps,
        "released_on_budget": len(on_budget),
        "budget_seqs": [f["seq"] for f in on_budget],
        "wait_max": max(waits) if waits else 0,
        "requested": sum(1 for f in facts if f["ev"] == "requested"),
        "dropped": sum(1 for f in facts if f["ev"] == "dropped"),
        "rollbacks": len(rollbacks),
        "hullshock_trg_rollbacks": hshk_trg,
        "belt_first_staged_before_spark": belt_first,
    }


for arm in arms:
    print(f"=== {arm} ===")
    for seed in range(1, 6):
        d = base / arm / f"seed{seed}"
        if not (d / "target-trace.client.jsonl").exists():
            print(f"seed {seed}: MISSING")
            continue
        s = summarize(d)
        print(f"seed {seed}: {json.dumps(s)}")
