# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""The shot-lifecycle instrument's offline join (src/shot_trace.rs, `SPIKE_SHOT_TRACE`).

This script joins a recorded client/server run and measures the visible shot lifecycle.
The transport has two deliberately separate policies: automatic-fire facts get three
bounded visual send opportunities; single-fire facts and private damage get one reliable
application send. It reports which nonempty-target calls Lightyear accepted into each policy.

It joins the role-suffixed client and server JSONL traces a `SPIKE_SHOT_TRACE` run
writes and reports, per shot:

  - LIFECYCLE RECONSTRUCTION: fired → broadcast → received (deduped?) → shell spawned →
    contact → held(n ticks) → re-seeded / confirmed / dissolved → end.
  - HOLD-TIME HISTOGRAM: how long shells ACTUALLY held, split by how the hold ended
    (re-seeded from a keyframe / resolved at a confirm / expired). This is the number
    that sizes `RICOCHET_HOLD_TICKS`: the window must cover the tail, and every
    `expired` is a shot whose picture was lost.
  - ARRIVAL LEAD: `recv_tick − server_tick` for keyframes and confirms — the client's
    prediction lead plus one-way latency, `(P − S) + OWL`. A client shell reaches the
    plate at local tick ≈ the server's bounce tick (both timelines index the same
    trajectory), so this distribution IS the cause of the hold histogram, and the hold
    window must cover it.
  - COPIES PER EVENT: how many nonempty-target application sends Lightyear accepted for each fact,
    split into reliable and visual policies. Keyframes are identified by their bounce
    sequence, so multiple bounces from one shot cannot be conflated.
  - CARRY-THROUGH: of every ricochet the authority sanctioned, what fraction actually
    re-seeded a client shell — vs dissolved before consumption, delivered but never
    consumed, or never delivered.
  - DEDUP: how often repeated visual copies were rejected by the receive gate.
  - EXACTLY-ONCE: shots whose cosmetic shell spawned more than once (a dedup failure) or
    never spawned, plus authored damaging
    shots that raised zero or multiple marker boundaries.
  - TRAIL CONSUMPTION: whether every received sanctioned bounce reached a captured ribbon
    station strictly after its authoritative re-anchor, rather than stopping at impact/path state.

Cross-process joining uses the stable match-local `(combatant, weapon, fire_tick)` key.
It is deliberately plain wire data rather than a receiver-local entity reference, so two
simultaneous same-slot shots remain distinct and a post-respawn outcome remains joinable.

Ticks are converted to milliseconds with the trace's own `tick_hz` (from the `meta` row),
so a re-tuned tick rate does not silently rescale the report.

Usage:
    uv run scripts/shot/analyze.py --client S.client.jsonl --server S.server.jsonl
        [--samples N] [--json]
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np

# DERIVED policy: the hold window is "adequate" if it covers this share of observed
# arrival leads.
COVERAGE_TARGET = 0.99
# DERIVED from `ballistics::TRACER_MAX_CALIBER`: at/above this, the shell owns the smoke ribbon
# whose post-bounce consumption rows are under analysis.
MAIN_GUN_MIN_CALIBER = 0.02


def load(path: Path) -> tuple[dict, list[dict]]:
    """Parse a JSONL trace into (meta row, rows). Torn tail lines are skipped, not fatal:
    a hard-killed process can lose its unflushed remainder (src/trace.rs `JsonlSink`)."""
    meta: dict = {}
    rows: list[dict] = []
    with path.open() as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue  # torn tail
            if row.get("k") == "meta":
                meta = row
            else:
                rows.append(row)
    return meta, rows


def shot_key(row: dict) -> tuple[int, int, int]:
    """The complete cross-process identity: combatant id, weapon slot, fire tick."""
    return (row["c"], row["w"], row["ft"])


def tick_diff(now: int, then: int) -> int:
    """Lightyear-style signed wrapping tick difference (`Tick - Tick -> i32`)."""
    delta = (int(now) - int(then)) & 0xFFFF_FFFF
    return delta if delta <= 0x7FFF_FFFF else delta - 0x1_0000_0000


def pct(values, q: float) -> float:
    a = np.asarray(values, dtype=float)
    return float(np.percentile(a, q)) if len(a) else float("nan")


def fmt_ticks(values, hz: float) -> str:
    if not len(values):
        return "      (none)"
    a = np.asarray(values, dtype=float)
    ms = 1000.0 / hz
    return (
        f"  n={len(a):<6d} p50={pct(a, 50):5.1f}t  p95={pct(a, 95):5.1f}t  "
        f"p99={pct(a, 99):5.1f}t  max={a.max():5.0f}t"
        f"   ({pct(a, 50) * ms:5.1f} / {pct(a, 95) * ms:5.1f} / "
        f"{pct(a, 99) * ms:5.1f} / {a.max() * ms:5.1f} ms)"
    )


def histogram(values, bound: int) -> list[str]:
    """A text histogram of hold ticks, one bucket per tick up to the configured window
    bound (the constant under test), plus an over-the-bound bucket."""
    if not len(values):
        return ["      (no holds recorded)"]
    counts = defaultdict(int)
    for v in values:
        counts[int(v)] += 1
    top = max(counts.values())
    lines = []
    for t in range(0, bound + 2):
        n = counts.get(t, 0)
        if t == bound + 1:
            n = sum(c for k, c in counts.items() if k > bound)
            label = f">{bound}"
        else:
            label = str(t)
        bar = "#" * int(round(40 * n / top)) if top else ""
        lines.append(f"      {label:>4}t  {n:>6}  {bar}")
    return lines


def analyze(cmeta: dict, crows: list[dict], smeta: dict, srows: list[dict]) -> dict:
    hz = float(cmeta.get("tick_hz") or smeta.get("tick_hz") or 64)
    hold_bound = int(cmeta.get("hold_ticks") or smeta.get("hold_ticks") or 16)
    # --- index every row by shot key -----------------------------------------------------
    server: dict[tuple[int, int, int], dict] = defaultdict(lambda: defaultdict(list))
    for r in srows:
        if r["k"] in ("send", "transport"):
            continue
        server[shot_key(r)][r["k"]].append(r)
    client: dict[tuple[int, int, int], dict] = defaultdict(lambda: defaultdict(list))
    for r in crows:
        client[shot_key(r)][r["k"]].append(r)

    # Send rows preserve the normal ShotId and add `age`, `rel`, and (for keyframes) `seq`.
    sends_by_key: dict[tuple[int, int, int], list[dict]] = defaultdict(list)
    for r in srows:
        if r.get("k") != "send":
            continue
        sends_by_key[shot_key(r)].append(r)
    transport_rows = [r for r in srows if r.get("k") == "transport"]
    transport_totals = (
        "visual_selected",
        "visual_facts_send_accepted",
        "visual_batches_send_accepted",
        "visual_wire_bytes_send_accepted_upper_bound",
        "visual_expired",
        "visual_budget_deferred_producers",
        "reliable_public_queued",
        "private_damage_queued",
        "public_no_recipient_facts",
        "private_damage_no_recipient_facts",
        "send_call_errors",
        "send_call_error_facts",
    )
    transport = {
        "rows": len(transport_rows),
        "max_visual_queue_depth": max(
            (
                max(int(row.get("visual_queue_before", 0)), int(row.get("visual_queue_after", 0)))
                for row in transport_rows
            ),
            default=0,
        ),
        **{
            field: sum(int(row.get(field, 0)) for row in transport_rows)
            for field in transport_totals
        },
        "max_public_recipient_count": max(
            (int(row.get("public_recipient_count", 0)) for row in transport_rows), default=0
        ),
    }
    for field in (
        "visual_copy_opportunities",
        "visual_ttl_ticks",
        "visual_batch_wire_limit",
        "visual_tick_wire_limit",
    ):
        value = next((row[field] for row in transport_rows if field in row), None)
        if value is not None:
            transport[field] = value

    # --- the client's OWN shots vs the shots it OBSERVES ----------------------------------
    # The shooter never receives a `fire_rx` for its own round (`receive_fire_events` drops the
    # echo of any tank it simulates locally), so own shots must be excluded from the delivery
    # accounting or every one of them reads as a lost fire. A `spawn src="own"` row is exactly
    # that marker.
    own = {k for k, kinds in client.items() if any(s.get("src") == "own" for s in kinds.get("spawn", []))}
    fired = set(server.keys()) & {k for k, kinds in server.items() if kinds.get("fire")}
    observed_expected = fired - own

    # --- delivery / dedup -----------------------------------------------------------------
    delivered, dup_rows, lost = 0, 0, []
    for key in sorted(observed_expected):
        rx = client[key].get("fire_rx", [])
        if not rx:
            lost.append(key)
            continue
        delivered += 1
        dup_rows += sum(1 for r in rx if r.get("dup"))

    # --- exactly-once shell spawn ---------------------------------------------------------
    multi_spawn = [k for k in observed_expected if len(client[k].get("spawn", [])) > 1]
    no_spawn = [
        k
        for k in observed_expected
        if client[k].get("fire_rx") and not client[k].get("spawn") and not client[k].get("end")
    ]

    # --- hold-time histogram --------------------------------------------------------------
    holds = {"bounce": [], "terminal": [], "expired": []}
    for r in crows:
        if r["k"] == "hold":
            holds.setdefault(r.get("res", "?"), []).append(r.get("held", 0))
    all_holds = [h for v in holds.values() for h in v]

    # --- arrival lead (the hold window's cause) -------------------------------------------
    kf_lead = [tick_diff(r["t"], r["bt"]) for r in crows if r["k"] == "kf_rx" and not r.get("dup")]
    cf_lead = [tick_diff(r["t"], r["it"]) for r in crows if r["k"] == "cf_rx" and not r.get("dup")]
    dmg_lead = [
        tick_diff(r["t"], r["dt"])
        for r in crows
        if r["k"] == "dmg_rx" and r.get("own") and not r.get("dup")
    ]

    # --- copies per emitted fact -----------------------------------------------------------
    # `send` is written only after a successful application send. `rel` is the actual transport
    # policy, not a guess based on weapon type. A keyframe's sequence is part of its identity;
    # falling back to zero would silently merge a later bounce with bounce zero.
    copies: dict[str, dict[str, list[int]]] = {
        "reliable": {"fire": [], "kf": [], "cf": [], "dmg": []},
        "visual": {"fire": [], "kf": [], "cf": [], "dmg": []},
    }
    missing_kf_send_sequence = 0
    for key, kinds in server.items():
        sends = sends_by_key.get(key, [])
        for stream in ("fire", "cf", "dmg"):
            for emission in kinds.get(stream, []):
                matching = [r for r in sends if r.get("s") == stream]
                for policy, reliable in (("reliable", True), ("visual", False)):
                    copies[policy][stream].append(
                        sum(
                            r.get("rel") is reliable and int(r.get("rcpt", 1)) > 0
                            for r in matching
                        )
                    )
        for kf in kinds.get("kf", []):  # a shot may bounce more than once
            seq = kf.get("seq")
            matching = [r for r in sends if r.get("s") == "kf" and r.get("seq") == seq]
            if any(r.get("s") == "kf" and "seq" not in r for r in sends):
                missing_kf_send_sequence += 1
            for policy, reliable in (("reliable", True), ("visual", False)):
                copies[policy]["kf"].append(
                    sum(r.get("rel") is reliable and int(r.get("rcpt", 1)) > 0 for r in matching)
                )

    # --- carry-through: the fate of every sanctioned ricochet -----------------------------
    def consumed_bounce(kinds: dict, seq: int) -> str | None:
        for r in kinds.get("hold", []):
            if r.get("res") == "bounce" and r.get("seq") == seq:
                return "reseeded_hold"
        for r in kinds.get("contact", []):
            if r.get("res") == "pre_bounce" and r.get("seq") == seq:
                return "reseeded_prearmed"
        for r in kinds.get("overdue", []):
            if r.get("res") == "bounce" and r.get("seq") == seq:
                return "reseeded_overdue"
        return None

    # Own shots are KEPT here, unlike in the delivery accounting above: the shooter's own shell
    # consumes its own keyframes (that is the fall-of-shot read the whole carry-through exists
    # for — `receive_fire_events` deliberately does not drop keyframes for locally-fired tanks).
    carry = defaultdict(int)
    for key, kinds in server.items():
        for kf in kinds.get("kf", []):
            seq = kf.get("seq", 0)
            ckinds = client.get(key, {})
            got = any(r.get("seq") == seq and not r.get("dup") for r in ckinds.get("kf_rx", []))
            outcome = consumed_bounce(ckinds, seq)
            if outcome:
                carry[outcome] += 1
            elif not got:
                carry["never_delivered"] += 1
            elif any(r.get("res") == "expired" for r in ckinds.get("hold", [])):
                carry["dissolved"] += 1
            else:
                carry["never_consumed"] += 1

    # --- terminals ------------------------------------------------------------------------
    term = defaultdict(int)
    for key, kinds in server.items():
        for _cf in kinds.get("cf", []):
            ckinds = client.get(key, {})
            got = any(not r.get("dup") for r in ckinds.get("cf_rx", []))
            resolved = (
                any(r.get("res") == "terminal" for r in ckinds.get("hold", []))
                or any(r.get("res") == "pre_term" for r in ckinds.get("contact", []))
                or any(r.get("res") == "terminal" for r in ckinds.get("overdue", []))
            )
            if resolved:
                term["resolved"] += 1
            elif not got:
                term["never_delivered"] += 1
            elif any(r.get("res") == "expired" for r in ckinds.get("hold", [])):
                term["dissolved"] += 1
            else:
                term["never_consumed"] += 1

    # --- how each observed shell's picture ENDED ------------------------------------------
    ends = defaultdict(int)
    for r in crows:
        if r["k"] == "end":
            ends[r.get("why", "?")] += 1

    # --- shooter marker delivery: discrete damage facts, never reconstructed from NetCrew --------
    damaging_keys = {key for key, kinds in server.items() if kinds.get("dmg")}
    authored_damage = damaging_keys & own
    fresh_own_damage = {
        key
        for key in authored_damage
        if any(r.get("own") and not r.get("dup") for r in client[key].get("dmg_rx", []))
    }
    marked_damage = {key for key in authored_damage if client[key].get("marker")}
    duplicate_markers = sum(max(0, len(client[key].get("marker", [])) - 1) for key in authored_damage)
    stray_markers = sum(
        len(kinds.get("marker", [])) for key, kinds in client.items() if key not in authored_damage
    )
    damage = {
        "server": sum(len(kinds.get("dmg", [])) for kinds in server.values()),
        "authored_expected": len(authored_damage),
        "delivered": len(fresh_own_damage),
        "marked": len(marked_damage),
        "missing_delivery": len(authored_damage - fresh_own_damage),
        "missing_marker": len(authored_damage - marked_damage),
        "duplicate_markers": duplicate_markers,
        "stray_markers": stray_markers,
    }

    # --- renderer boundary: sanctioned main-gun bounce -> captured post-bounce ribbon station -----
    trail = defaultdict(int)
    for key, kinds in server.items():
        is_main_gun = any(float(fire.get("cal", 0.0)) >= MAIN_GUN_MIN_CALIBER for fire in kinds.get("fire", []))
        if not is_main_gun:
            continue
        for kf in kinds.get("kf", []):
            seq = kf.get("seq", 0)
            rows = [r for r in client.get(key, {}).get("trail", []) if r.get("seq") == seq]
            if any(r.get("res") == "post_bounce_consumed" for r in rows):
                trail["consumed"] += 1
            elif any(r.get("res") == "post_bounce_unrendered" for r in rows):
                trail["unrendered"] += 1
            elif any(r.get("res") == "bounce_anchor_missing" for r in rows):
                trail["anchor_missing"] += 1
            elif any(r.get("res") == "ribbon_missing_at_end" for r in client.get(key, {}).get("trail", [])):
                trail["ribbon_missing"] += 1
            else:
                trail["no_row"] += 1
    catchup_holds = sum(
        1 for r in crows if r["k"] == "catchup" and r.get("res") == "armor_hold"
    )

    # --- window sizing verdict ------------------------------------------------------------
    # The hold window must cover the arrival lead's tail: a keyframe that lands later than the
    # window is a dissolved shot. Sized off the OBSERVED leads (not the observed holds, which
    # are already truncated BY the window — the histogram cannot see past its own bound).
    need = pct(kf_lead + cf_lead, COVERAGE_TARGET * 100) if (kf_lead or cf_lead) else float("nan")

    # Reattach sends only for lifecycle reconstruction after all shot accounting above. This keeps
    # an unscoped aggregate `transport` row and any malformed send from creating a phantom shot.
    for key, sends in sends_by_key.items():
        server[key]["send"].extend(sends)

    return {
        "hz": hz,
        "hold_bound": hold_bound,
        "overdue_ticks": cmeta.get("overdue_ticks") or smeta.get("overdue_ticks"),
        "server_fires": len(fired),
        "own_shots": len(own & fired),
        "expected": len(observed_expected),
        "delivered": delivered,
        "lost": len(lost),
        "dup_rows": dup_rows,
        "multi_spawn": len(multi_spawn),
        "no_spawn": len(no_spawn),
        "holds": {k: v for k, v in holds.items()},
        "all_holds": all_holds,
        "kf_lead": kf_lead,
        "cf_lead": cf_lead,
        "dmg_lead": dmg_lead,
        "copies": copies,
        "missing_kf_send_sequence": missing_kf_send_sequence,
        "transport": transport,
        # Whether the trace carries `send` rows AT ALL. A trace recorded before the server's single
        # send site was instrumented has none, and every copy count would read as a (meaningless) zero
        # — say "not instrumented", never "this event rode no datagrams".
        "sends_seen": sum(1 for r in srows if r["k"] == "send"),
        "carry": dict(carry),
        "term": dict(term),
        "ends": dict(ends),
        "damage": damage,
        "trail": dict(trail),
        "catchup_holds": catchup_holds,
        "need_ticks": need,
        "server": server,
        "client": client,
        "expected_keys": sorted(observed_expected),
    }


def verification_failures(summary: dict) -> list[str]:
    """Return machine-readable violations for facts that a trace actually emitted.

    Empty categories are valid: this verifier checks evidence from one scenario, not a required
    scenario matrix. Receive duplicates are expected visual-copy traffic; only duplicate shell spawns
    and duplicate shooter-marker boundaries violate the exactly-once contracts.
    """
    failures: list[str] = []

    def add(name: str, count: int) -> None:
        if count:
            failures.append(f"{name}={count}")

    add("lost_shots", summary["lost"])
    add("duplicate_shots", summary["multi_spawn"])
    add("no_spawn_shots", summary["no_spawn"])

    carry = summary["carry"]
    add(
        "sanctioned_bounce_carry_failures",
        sum(carry.get(outcome, 0) for outcome in ("dissolved", "never_consumed", "never_delivered")),
    )

    damage = summary["damage"]
    add("missing_shooter_damage_confirms", damage["missing_delivery"])
    add("missing_shooter_markers", damage["missing_marker"])
    add("duplicate_shooter_markers", damage["duplicate_markers"])
    add("stray_shooter_markers", damage["stray_markers"])

    trail = summary["trail"]
    add(
        "main_gun_trail_failures",
        sum(trail.get(outcome, 0) for outcome in ("unrendered", "anchor_missing", "ribbon_missing", "no_row")),
    )
    return failures


def reconstruct(key: tuple[int, int, int], s: dict, c: dict) -> list[str]:
    """One shot's life, in tick order across both processes.

    The `send` rows are collapsed per fact and transport policy: they are a count, not a lifecycle
    moment, and spelling each visual copy out would bury the useful evidence."""
    events = []
    for kind, rows in s.items():
        if kind == "send":
            continue
        for r in rows:
            events.append((r["t"], "S", kind, r))
    for kind, rows in c.items():
        for r in rows:
            events.append((r["t"], "C", kind, r))
    events.sort(key=lambda e: (e[0], e[1] != "S"))
    out = [f"    shot combatant={key[0]} weapon={key[1]} fire_tick={key[2]}"]
    for stream in ("fire", "kf", "cf", "dmg"):
        emissions = s.get(stream, [])
        for emission in emissions:
            seq = emission.get("seq") if stream == "kf" else None
            sent = [
                r for r in s.get("send", [])
                if r.get("s") == stream and (stream != "kf" or r.get("seq") == seq)
            ]
            if sent:
                visual = sum(r.get("rel") is False and int(r.get("rcpt", 1)) > 0 for r in sent)
                reliable = sum(r.get("rel") is True and int(r.get("rcpt", 1)) > 0 for r in sent)
                detail = f" seq={seq}" if stream == "kf" else ""
                out.append(
                    f"      [S] accepted {stream:<4}{detail} visual×{visual} reliable×{reliable}"
                )
    for t, role, kind, r in events:
        extra = {
            k: v for k, v in r.items() if k not in ("k", "t", "c", "w", "ft")
        }
        detail = "  ".join(f"{k}={v}" for k, v in extra.items())
        out.append(f"      t={t:<8} [{role}] {kind:<8} {detail}")
    return out


def report(a: dict, samples: int) -> None:
    hz = a["hz"]
    ms = 1000.0 / hz
    line = "=" * 92
    print(line)
    print("  SHOT-LIFECYCLE INSTRUMENT — client/server ShotId join")
    print(line)
    print(
        f"  tick_hz={hz:.0f}   configured: RICOCHET_HOLD_TICKS={a['hold_bound']} "
        f"({a['hold_bound'] * ms:.0f} ms)   OVERDUE_MARGIN_TICKS={a['overdue_ticks']}"
    )

    print("\n  DELIVERY  (shots the server broadcast that this client should observe)")
    print(f"    analyzable server fires      {a['server_fires']}")
    print(f"      of which this client's own {a['own_shots']}  (never echoed back — excluded below)")
    print(f"    expected on this client      {a['expected']}")
    print(f"    delivered                    {a['delivered']}")
    print(f"    LOST (never arrived)         {a['lost']}")
    print(
        f"    duplicate events rejected    {a['dup_rows']}   "
        "(repeated visual copies rejected by ShotId dedup)"
    )

    print("\n  EXACTLY-ONCE SHELL SPAWN")
    print(f"    shots spawning >1 shell      {a['multi_spawn']}   (must be 0 — a ShotId dedup failure)")
    print(f"    delivered but never spawned  {a['no_spawn']}   (bore/tick guard rejected the event)")

    print("\n  HOLD-TIME HISTOGRAM  (ticks a client shell froze at armor awaiting the verdict)")
    for res in ("bounce", "terminal", "expired"):
        v = a["holds"].get(res, [])
        print(f"    ended {res:<9}{fmt_ticks(v, hz)}")
    print(f"    all holds      {fmt_ticks(a['all_holds'], hz)}")
    print()
    for l in histogram(a["all_holds"], a["hold_bound"]):
        print(l)

    print("\n  ARRIVAL LEAD  (recv tick − server tick = (P − S) + one-way latency)")
    print(f"    ricochet keyframes {fmt_ticks(a['kf_lead'], hz)}")
    print(f"    impact confirms    {fmt_ticks(a['cf_lead'], hz)}")
    print(f"    damage confirms    {fmt_ticks(a['dmg_lead'], hz)}")
    if not np.isnan(a["need_ticks"]):
        verdict = "ADEQUATE" if a["need_ticks"] <= a["hold_bound"] else "TOO SHORT"
        print(
            f"\n    SIZING: covering {COVERAGE_TARGET:.0%} of observed arrivals needs "
            f"{a['need_ticks']:.0f} ticks ({a['need_ticks'] * ms:.0f} ms); "
            f"RICOCHET_HOLD_TICKS = {a['hold_bound']} → {verdict}"
        )

    print("\n  APPLICATION SEND ACCEPTANCES PER FACT  (nonempty target, split by policy)")
    if not a["sends_seen"]:
        print("      (no `send` rows — trace predates the server's single-send-site instrumentation)")
    else:
        for policy, label in (("visual", "visual (automatic; up to 3)"), ("reliable", "reliable (single/damage; 1)")):
            print(f"    {label}")
            for stream, stream_label in (("fire", "fires"), ("kf", "keyframes"), ("cf", "terminals"), ("dmg", "damages")):
                v = a["copies"][policy][stream]
                if not v:
                    continue
                arr = np.asarray(v, dtype=float)
                print(f"      {stream_label:<11} n={len(arr):<5d} min={arr.min():.0f}  p50={pct(arr, 50):.1f}  max={arr.max():.0f}")
        if a["missing_kf_send_sequence"]:
            print(f"    INCOMPLETE SCHEMA: {a['missing_kf_send_sequence']} keyframe emission(s) cannot be matched: `send.seq` is absent.")
    transport = a["transport"]
    if transport["rows"]:
        print(
            "    transport trace totals: "
            + "  ".join(
                f"{field}={transport[field]}"
                for field in (
                    "visual_selected",
                    "visual_facts_send_accepted",
                    "visual_batches_send_accepted",
                    "visual_wire_bytes_send_accepted_upper_bound",
                    "visual_expired",
                    "visual_budget_deferred_producers",
                    "reliable_public_queued",
                    "private_damage_queued",
                    "public_no_recipient_facts",
                    "private_damage_no_recipient_facts",
                    "send_call_errors",
                    "send_call_error_facts",
                )
            )
        )
        print(
            f"    peak visual queue depth={transport['max_visual_queue_depth']}  "
            f"peak targeted public recipients={transport['max_public_recipient_count']}"
        )
        config = "  ".join(
            f"{field}={transport[field]}"
            for field in (
                "visual_copy_opportunities",
                "visual_ttl_ticks",
                "visual_batch_wire_limit",
                "visual_tick_wire_limit",
            )
            if field in transport
        )
        if config:
            print(f"    recorded transport config (DERIVED): {config}")

    print("\n  CARRY-THROUGH  (fate of every ricochet the authority sanctioned)")
    total = sum(a["carry"].values())
    for k in (
        "reseeded_hold",
        "reseeded_prearmed",
        "reseeded_overdue",
        "dissolved",
        "never_consumed",
        "never_delivered",
    ):
        n = a["carry"].get(k, 0)
        share = f"{100.0 * n / total:5.1f}%" if total else "    —"
        print(f"    {k:<20} {n:>6}  {share}")
    if total:
        ok = sum(a["carry"].get(k, 0) for k in ("reseeded_hold", "reseeded_prearmed", "reseeded_overdue"))
        print(f"    carry-through rate   {100.0 * ok / total:5.1f}%  ({ok}/{total})")

    print("\n  TERMINALS  (fate of every ImpactConfirm the authority emitted)")
    ttotal = sum(a["term"].values())
    for k in ("resolved", "dissolved", "never_consumed", "never_delivered"):
        n = a["term"].get(k, 0)
        share = f"{100.0 * n / ttotal:5.1f}%" if ttotal else "    —"
        print(f"    {k:<20} {n:>6}  {share}")

    print("\n  SHOOTER MARKERS  (discrete authority damage facts for this client's authored shots)")
    damage = a["damage"]
    print(f"    server damage facts          {damage['server']:>6}")
    print(f"    authored by this client      {damage['authored_expected']:>6}")
    print(f"    fresh own confirms received  {damage['delivered']:>6}")
    print(f"    marker boundaries reached    {damage['marked']:>6}")
    print(f"    MISSING delivery             {damage['missing_delivery']:>6}")
    print(f"    MISSING marker after receive {damage['missing_marker']:>6}")
    print(f"    duplicate markers            {damage['duplicate_markers']:>6}")
    print(f"    stray/non-authored markers   {damage['stray_markers']:>6}")

    print("\n  MAIN-GUN TRAIL  (renderer consumed a station strictly after each sanctioned bounce)")
    trail = a["trail"]
    for outcome in ("consumed", "unrendered", "anchor_missing", "ribbon_missing", "no_row"):
        print(f"    {outcome:<20} {trail.get(outcome, 0):>6}")
    print(f"    catch-up armor holds         {a['catchup_holds']:>6}")

    print("\n  SHELL ENDINGS  (how each client shell's picture ended)")
    for why, n in sorted(a["ends"].items(), key=lambda kv: -kv[1]):
        print(f"    {why:<20} {n:>6}")

    if samples:
        print("\n  LIFECYCLE RECONSTRUCTION  (first {} shots)".format(samples))
        for key in a["expected_keys"][:samples]:
            for l in reconstruct(key, a["server"].get(key, {}), a["client"].get(key, {})):
                print(l)
    print()


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Shot-lifecycle instrument: client/server ShotId join (SPIKE_SHOT_TRACE)."
    )
    ap.add_argument("--client", required=True, help="client role-suffixed JSONL trace")
    ap.add_argument("--server", required=True, help="server role-suffixed JSONL trace")
    ap.add_argument("--samples", type=int, default=3, help="reconstruct this many shot lifecycles (0 = none)")
    ap.add_argument("--json", action="store_true", help="emit the summary as JSON instead of text")
    ap.add_argument(
        "--strict",
        action="store_true",
        help="exit nonzero when the trace violates a verifiable shot-lifecycle contract",
    )
    args = ap.parse_args()

    cmeta, crows = load(Path(args.client))
    smeta, srows = load(Path(args.server))
    if not crows and not srows:
        print("no rows in either trace — was SPIKE_SHOT_TRACE armed?", file=sys.stderr)
        return 1

    a = analyze(cmeta, crows, smeta, srows)
    failures = verification_failures(a)
    if args.json:
        payload = {k: v for k, v in a.items() if k not in ("server", "client", "expected_keys")}
        payload["hold_p50"] = pct(a["all_holds"], 50)
        payload["hold_p99"] = pct(a["all_holds"], 99)
        payload["kf_lead_p99"] = pct(a["kf_lead"], 99)
        payload["dmg_lead_p99"] = pct(a["dmg_lead"], 99)
        print(json.dumps(payload, indent=2, default=float))
    else:
        report(a, args.samples)
    if args.strict and failures:
        print("strict verification failed: " + ", ".join(failures), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
