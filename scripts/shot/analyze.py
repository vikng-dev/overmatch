# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""The game's damage model: join fire, public-path playback, damage receipts and trail evidence.

Automatic fire announcements use bounded visual copies. Resolved paths and private damage use
reliable delivery. Public paths are identified by (ShotId, sequence), with no client armour test
or contact wait. The report distinguishes arrival from playback at the shared presentation cursor.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np

# DERIVED from `ballistics::TRACER_MAX_CALIBER`: at/above this, the shell owns the smoke ribbon
# whose post-bounce consumption rows are under analysis.
MAIN_GUN_MIN_CALIBER = 0.02
# Resolution: reject captures whose endpoints differ by more than the existing 32-tick
# diagnostic overlap allowance. Missing facts remain strict failures even inside this allowance.
TRACE_END_MAX_SKEW_TICKS = 32


def load(path: Path) -> tuple[dict, list[dict]]:
    """Parse a JSONL trace into (meta row, rows). Torn tail lines are skipped, not fatal:
    a hard-killed process can lose its unflushed remainder (src/trace.rs `JsonlSink`)."""
    meta: dict = {}
    rows: list[dict] = []
    with path.open() as f:
        for number, line in enumerate(f, 1):
            if not line.strip():
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError as error:
                if any(rest.strip() for rest in f):
                    raise ValueError(f"{path}:{number}: malformed row before the tail") from error
                break  # A killed producer may leave only its final row torn.
            if not isinstance(row, dict):
                raise ValueError(f"{path}:{number}: a trace row must be an object")
            if row.get("k") in ("send", "transport"):
                field = "rcpt" if row["k"] == "send" else "public_recipient_count"
                if type(row.get(field)) is not int or row[field] < 0:
                    raise ValueError(f"{path}:{number}: {field} must be a nonnegative integer")
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


def analyze(cmeta: dict, crows: list[dict], smeta: dict, srows: list[dict]) -> dict:
    hz = float(cmeta.get("tick_hz") or smeta.get("tick_hz") or 64)
    # --- index every row by shot key -----------------------------------------------------
    server: dict[tuple[int, int, int], dict] = defaultdict(lambda: defaultdict(list))
    for r in srows:
        # `send` joins below; `transport` and `reliable_outbox` are per-tick aggregates with no
        # ShotId (`src/shot_trace.rs`).
        if r["k"] in ("send", "transport", "reliable_outbox") or not all(k in r for k in ("c", "w", "ft")):
            continue
        server[shot_key(r)][r["k"]].append(r)
    client: dict[tuple[int, int, int], dict] = defaultdict(lambda: defaultdict(list))
    for r in crows:
        if all(k in r for k in ("c", "w", "ft")):
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

    # Own fire uses the same server identity and presentation path as remote fire (ADR-0037).
    # Control is recorded at the input edge, independently of whether any receipt arrives.
    own_ids = {r["c"] for r in crows if r["k"] == "controlled"}
    assert len(own_ids) <= 1, f"multiple controlled ids in one capture: {sorted(own_ids)}"
    c_own = next(iter(own_ids), None)
    fired = {k for k, kinds in server.items() if kinds.get("fire")}
    own_identity = {k for k in fired if k[0] == c_own}

    last_client_tick = max((r["t"] for r in crows if "t" in r), default=None)
    last_server_tick = max((r["t"] for r in srows if "t" in r), default=None)
    client_trace_end_gap = (
        tick_diff(last_server_tick, last_client_tick)
        if last_server_tick is not None and last_client_tick is not None else 0
    )
    unaddressed_ticks = {
        r["t"] for r in srows
        if r["k"] == "transport" and r["public_recipient_count"] == 0
    }
    unaddressed = {
        k for k in fired
        if any(f["t"] in unaddressed_ticks for f in server[k]["fire"])
    }
    observed_expected = fired - unaddressed

    # --- delivery / dedup -----------------------------------------------------------------
    delivered, lost = 0, []
    for key in sorted(observed_expected):
        rx = client[key].get("fire_rx", [])
        if not rx:
            lost.append(key)
            continue
        delivered += 1
    # Dedup rejections are counted trace-wide: the visual copies of an own-tank echo are just
    # as much real dedup traffic as those of a cross-tank fire.
    dup_rows = sum(1 for r in crows if r["k"] == "fire_rx" and r.get("dup"))

    # --- exactly-once shell spawn ---------------------------------------------------------
    multi_spawn = [k for k in observed_expected if len(client[k].get("spawn", [])) > 1]
    no_spawn = [
        k
        for k in observed_expected
        if client[k].get("fire_rx") and not client[k].get("spawn") and not client[k].get("end")
    ]

    # --- fire catch-up: (P − S) + one-way latency, per observed FireEvent -----------------
    # Receive age is the local timeline minus the authoritative fire tick, for every shooter.
    fire_cu = [r["cu"] for r in crows if r["k"] == "fire_rx" and not r.get("dup") and "cu" in r]

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
        "reliable": {"fire": [], "path": [], "dmg": []},
        "visual": {"fire": [], "path": [], "dmg": []},
    }
    missing_path_send_sequence = 0
    for key, kinds in server.items():
        sends = sends_by_key.get(key, [])
        for stream in ("fire", "dmg"):
            for emission in kinds.get(stream, []):
                matching = [r for r in sends if r.get("s") == stream]
                for policy, reliable in (("reliable", True), ("visual", False)):
                    copies[policy][stream].append(
                        sum(
                            r.get("rel") is reliable and r["rcpt"] > 0
                            for r in matching
                        )
                    )
        for kf in kinds.get("path_tx", []):  # a shot may bounce more than once
            seq = kf.get("seq")
            matching = [r for r in sends if r.get("s") == "path" and r.get("seq") == seq]
            if any(r.get("s") == "path" and "seq" not in r for r in sends):
                missing_path_send_sequence += 1
            for policy, reliable in (("reliable", True), ("visual", False)):
                copies[policy]["path"].append(
                    sum(r.get("rel") is reliable and r["rcpt"] > 0 for r in matching)
                )

    # A path may arrive before fire or after its cursor crossing. Sequence joins are exact.
    paths = defaultdict(int)
    for key, kinds in server.items():
        if key not in observed_expected and key not in own_identity:
            continue
        for sent in kinds.get("path_tx", []):
            seq = sent.get("seq")
            received = [r for r in client.get(key, {}).get("path_rx", []) if r.get("seq") == seq]
            if any(r.get("result") == "Inserted" for r in received):
                paths["delivered"] += 1
            elif received:
                paths["refused"] += 1
            else:
                paths["missing_delivery"] += 1
            if any(r.get("seq") == seq for r in client.get(key, {}).get("path_done", [])):
                paths["completed"] += 1
            elif received and sent.get("impact") != "state":
                # A still-future effect at the trace boundary is observable as pending.
                paths["pending"] += 1

    # --- how each observed shell's picture ENDED ------------------------------------------
    ends = defaultdict(int)
    for r in crows:
        if r["k"] == "end":
            ends[r.get("why", "?")] += 1

    # --- shooter marker delivery: discrete damage facts, never reconstructed from NetCrew --------
    damaging_keys = {key for key, kinds in server.items() if kinds.get("dmg")}
    authored_damage = damaging_keys & own_identity
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
        for kf in kinds.get("path_tx", []):
            if kf.get("impact") != "bounce":
                continue
            seq = kf["ordinal"]
            rows = [r for r in client.get(key, {}).get("trail", []) if r.get("seq") == seq]
            if any(r.get("res") == "post_bounce_consumed" for r in rows):
                trail["consumed"] += 1
            elif any(r.get("res") == "post_bounce_unrendered" for r in rows):
                trail["unrendered"] += 1
            elif any(r.get("res") == "ribbon_missing_at_end" for r in client.get(key, {}).get("trail", [])):
                trail["ribbon_missing"] += 1
            else:
                trail["no_row"] += 1
    # Reattach sends only for lifecycle reconstruction after all shot accounting above. This keeps
    # an unscoped aggregate `transport` row and any malformed send from creating a phantom shot.
    for key, sends in sends_by_key.items():
        server[key]["send"].extend(sends)

    return {
        "hz": hz,
        "server_fires": len(fired),
        "own_shots": len(own_identity),
        "controlled_combatant": c_own,
        "capture_completed": any(r["k"] == "capture_end" for r in crows),
        "client_trace_end_gap_ticks": client_trace_end_gap,
        "expected": len(observed_expected),
        "unaddressed": len(unaddressed),
        "delivered": delivered,
        "lost": len(lost),
        "dup_rows": dup_rows,
        "multi_spawn": len(multi_spawn),
        "no_spawn": len(no_spawn),
        "fire_cu": fire_cu,
        "dmg_lead": dmg_lead,
        "copies": copies,
        "missing_path_send_sequence": missing_path_send_sequence,
        "transport": transport,
        # Whether the trace carries `send` rows AT ALL. A trace recorded before the server's single
        # send site was instrumented has none, and every copy count would read as a (meaningless) zero
        # — say "not instrumented", never "this event rode no datagrams".
        "sends_seen": sum(1 for r in srows if r["k"] == "send"),
        "paths": dict(paths),
        "ends": dict(ends),
        "damage": damage,
        "trail": dict(trail),
        "server": server,
        "client": client,
        "expected_keys": sorted(observed_expected),
    }


def verification_failures(summary: dict, require_shooter_damage: bool = False) -> list[str]:
    """Check every authoritative fire, path and own private receipt, without tail exclusions.

    Generic captures may have empty categories. The MG runner additionally requires a completed
    shooter capture and nonempty private-damage evidence, so a missed target cannot pass it.
    """
    failures: list[str] = []

    def add(name: str, count: int) -> None:
        if count:
            failures.append(f"{name}={count}")

    gap = int(summary["client_trace_end_gap_ticks"])
    if gap > TRACE_END_MAX_SKEW_TICKS:
        add("client_trace_ended_early_by_ticks", gap)
    add("lost_shots", summary["lost"])
    add("duplicate_shots", summary["multi_spawn"])
    add("no_spawn_shots", summary["no_spawn"])

    paths = summary["paths"]
    add("public_path_failures", paths.get("missing_delivery", 0) + paths.get("refused", 0))

    damage = summary["damage"]
    if require_shooter_damage:
        add("missing_controlled_identity", int(summary["controlled_combatant"] is None))
        add("incomplete_capture", int(not summary["capture_completed"]))
        add("no_shooter_damage", int(damage["authored_expected"] == 0))
    add("missing_shooter_damage_confirms", damage["missing_delivery"])
    add("missing_shooter_markers", damage["missing_marker"])
    add("duplicate_shooter_markers", damage["duplicate_markers"])
    add("stray_shooter_markers", damage["stray_markers"])

    trail = summary["trail"]
    add(
        "main_gun_trail_failures",
        sum(trail.get(outcome, 0) for outcome in ("unrendered", "ribbon_missing", "no_row")),
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
    for stream in ("fire", "path", "dmg"):
        emissions = s.get("path_tx" if stream == "path" else stream, [])
        for emission in emissions:
            seq = emission.get("seq") if stream == "path" else None
            sent = [
                r for r in s.get("send", [])
                if r.get("s") == stream and (stream != "path" or r.get("seq") == seq)
            ]
            if sent:
                visual = sum(r.get("rel") is False and r["rcpt"] > 0 for r in sent)
                reliable = sum(r.get("rel") is True and r["rcpt"] > 0 for r in sent)
                detail = f" seq={seq}" if stream == "path" else ""
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


def report(summary: dict, samples: int) -> None:
    payload = {k: v for k, v in summary.items() if k not in ("server", "client", "expected_keys")}
    print(json.dumps(payload, indent=2, default=float))
    for key in summary["expected_keys"][:samples]:
        for line in reconstruct(key, summary["server"].get(key, {}), summary["client"].get(key, {})):
            print(line)


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
    ap.add_argument("--require-shooter-damage", action="store_true",
                    help="require a completed shooter capture and at least one damaging shot")
    args = ap.parse_args()

    cmeta, crows = load(Path(args.client))
    smeta, srows = load(Path(args.server))
    if not crows and not srows:
        print("no rows in either trace — was SPIKE_SHOT_TRACE armed?", file=sys.stderr)
        return 1

    a = analyze(cmeta, crows, smeta, srows)
    failures = verification_failures(a, args.require_shooter_damage)
    if args.json:
        payload = {k: v for k, v in a.items() if k not in ("server", "client", "expected_keys")}
        print(json.dumps(payload, indent=2, default=float))
    else:
        report(a, args.samples)
    if args.strict and failures:
        print("strict verification failed: " + ", ".join(failures), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
