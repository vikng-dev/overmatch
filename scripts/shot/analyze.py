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
# End-of-trace exclusion window: a server fire within this many ticks of the client trace's
# last recorded row (on either side) cannot fairly be scored as lost — the capture stopped
# before catch-up delivery could land. ASSUMED diagnostic tail allowance: 16 ticks.
# Twice this window is also the
# maximum client/server trace-end skew the strict verifier accepts before refusing the run.
TRACE_END_WINDOW_TICKS = 16


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

    # --- the client's OWN shots vs the shots it OBSERVES ----------------------------------
    # The shooter never receives a `fire_rx` for its own round (`receive_fire_events` drops the
    # echo of any tank it simulates locally), so own shots must be excluded from the delivery
    # accounting or every one of them reads as a lost fire. A `spawn src="own"` row is exactly
    # that marker.
    own = {k for k, kinds in client.items() if any(s.get("src") == "own" for s in kinds.get("spawn", []))}
    fired = set(server.keys()) & {k for k, kinds in server.items() if kinds.get("fire")}

    # The client's own tank id: the combatant carrying `spawn src="own"` rows. A single client
    # trace records exactly one locally-simulated shooter, so more than one id is a join defect.
    own_ids = {k[0] for k in own}
    assert len(own_ids) <= 1, f"multiple own-tank ids in one client trace: {sorted(own_ids)}"
    c_own = next(iter(own_ids)) if own_ids else None

    # --- own-tank fire identity under input attestation (ADR-0022/0029) -------------------
    # The server fails closed on unattested input ticks, so the client's own predicted fire is
    # NOT guaranteed to execute at its predicted tick: under jitter the server re-times it 1-3
    # ticks later, or rejects it outright and fires on its own later schedule. Tick-exact
    # identity binds authoritative facts, but the client's own predicted and executed fire
    # streams must be re-joined by tick proximity, not equality. Cross-tank fires (the client
    # never predicted them) keep strict tick-exact semantics below.
    own_matched = own & fired
    client_only_own = sorted(own - fired)
    server_only_own = sorted(k for k in fired - own if k[0] == c_own)

    # a. retimed_own_prediction: greedy 1:1 pairing per (c, w) stream in tick order — a
    #    server-only fire at U consumes the earliest still-unpaired client-only fire at T with
    #    1 <= U - T <= 3 (the measured attestation re-timing window).
    RETIME_MAX_TICKS = 3
    retime_pool: dict[tuple[int, int], list[tuple[int, int, int]]] = defaultdict(list)
    for k in client_only_own:
        retime_pool[(k[0], k[1])].append(k)
    retimed_pairs: list[tuple[tuple[int, int, int], tuple[int, int, int]]] = []
    for sk in server_only_own:
        pool = retime_pool[(sk[0], sk[1])]
        match = next(
            (ck for ck in pool if 1 <= tick_diff(sk[2], ck[2]) <= RETIME_MAX_TICKS), None
        )
        if match is not None:
            pool.remove(match)  # a pairing consumes both keys
            retimed_pairs.append((match, sk))
    retimed_server_keys = {sk for _ck, sk in retimed_pairs}
    retimed_deltas = [tick_diff(sk[2], ck[2]) for ck, sk in retimed_pairs]

    # b. rejected_own_prediction: client-only own keys left unpaired — the client predicted,
    #    the server declined the unattested tick (fail-closed) and never executed that round.
    rejected_own = sorted(k for pool in retime_pool.values() for k in pool)

    # c. own_echo: server-only own-tank keys left unpaired — the server's own re-scheduled
    #    rounds, observed by this client as fire_rx with the spawn correctly suppressed by the
    #    echo-drop.
    own_echo = [k for k in server_only_own if k not in retimed_server_keys]

    # DELIVERY for a re-timed or re-scheduled own round still means a fire_rx row: proximity
    # pairing explains WHY the key drifted off the prediction, it does not excuse the
    # transport. A missing fire_rx on either is a genuine loss and stays strict-fatal —
    # unless the server fired so close to the end of the client capture that delivery could
    # not have been recorded (see the exclusion window below).
    retimed_missing_rx = [sk for _ck, sk in retimed_pairs if not client[sk].get("fire_rx")]
    own_echo_delivered = [k for k in own_echo if client[k].get("fire_rx")]
    own_echo_missing = [k for k in own_echo if not client[k].get("fire_rx")]

    # The exclusion bound is TWO-SIDED (|Δ| <= window): a signed test would exclude any fire
    # arbitrarily far past the client trace's end, letting a stalled client capture excuse
    # every later loss.
    last_client_tick = max((r["t"] for r in crows if "t" in r), default=None)
    last_server_tick = max((r["t"] for r in srows if "t" in r), default=None)

    def at_trace_end(key: tuple[int, int, int]) -> bool:
        return (
            last_client_tick is not None
            and abs(tick_diff(last_client_tick, key[2])) <= TRACE_END_WINDOW_TICKS
        )

    excluded_at_trace_end = sorted(
        k for k in own_echo_missing + retimed_missing_rx if at_trace_end(k)
    )
    own_echo_lost = [k for k in own_echo_missing if k not in excluded_at_trace_end]
    retimed_lost = [k for k in retimed_missing_rx if k not in excluded_at_trace_end]

    # Overlap guard: every per-key verdict above presumes the two captures cover the same
    # span. If the client trace ends more than two exclusion windows before the server trace,
    # overlap is NOT established and the strict verifier refuses the run outright (fail loud,
    # never classify against a shrunken client denominator).
    client_trace_end_gap = (
        tick_diff(last_server_tick, last_client_tick)
        if last_server_tick is not None and last_client_tick is not None
        else 0
    )

    # d. Cross-tank fires: semantics completely unchanged — strict tick-exact exactly-once.
    #    A fire on a tick whose transport row records no public recipient at all (no client
    #    connected yet — e.g. a bot firing at server start) had no one to reach: it is not
    #    expected. A fire on a tick WITH recipients stays strict even if no send row exists,
    #    so a fact the server failed to send is still a loss.
    unaddressed_ticks = {
        r["t"]
        for r in srows
        if r.get("k") == "transport" and r["public_recipient_count"] == 0
    }
    unaddressed = {
        k
        for k in fired - own
        if k[0] != c_own and any(f["t"] in unaddressed_ticks for f in server[k]["fire"])
    }
    observed_expected = {k for k in fired - own if k[0] != c_own} - unaddressed

    # Damage/marker accounting joins on the AUTHORITATIVE key: a correctly re-timed or
    # re-scheduled own round's damage confirm rides the server's fire key, not the client's
    # predicted one. Resolve own identity to the tick-exact matches plus the retimed and echo
    # server keys, or a correct marker on a re-timed shot reads as stray.
    own_identity = own | retimed_server_keys | set(own_echo)

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
    # The direct measurement of the "~10-tick catch-up" that ADR-0032 records as DERIVED, never
    # measured. Own shots carry cu=0 by construction, so only observed fire matters here.
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
        "own_shots": len(own_matched),
        "own_predicted": len(own),
        # Own-tank identity drift under fail-closed input attestation (ADR-0022/0029):
        "retimed_own_prediction": len(retimed_pairs),
        "retimed_delta_hist": {
            int(d): retimed_deltas.count(d) for d in sorted(set(retimed_deltas))
        },
        "rejected_own_prediction": len(rejected_own),
        "rejected_own_keys": rejected_own,
        "own_echo": len(own_echo),
        "own_echo_delivered": len(own_echo_delivered),
        "own_echo_lost": len(own_echo_lost),
        "own_echo_lost_keys": own_echo_lost,
        "retimed_lost": len(retimed_lost),
        "retimed_lost_keys": retimed_lost,
        "excluded_at_trace_end": len(excluded_at_trace_end),
        "trace_end_window_ticks": TRACE_END_WINDOW_TICKS,
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


def verification_failures(summary: dict) -> list[str]:
    """Return machine-readable violations for facts that a trace actually emitted.

    Empty categories are valid: this verifier checks evidence from one scenario, not a required
    scenario matrix. Receive duplicates are expected visual-copy traffic; only duplicate shell spawns
    and duplicate shooter-marker boundaries violate the exactly-once contracts.

    Fatal: cross-tank lost/no_spawn, multi_spawn, and any own-tank round — echoed OR
    proximity-paired as retimed — with no fire_rx (a genuine delivery loss). NOT fatal,
    reported only: delivered retimed and rejected own predictions — those are the fail-closed
    input-attestation design (ADR-0022/0029) working as specified — and keys excluded at the
    end of the trace, where delivery could not have been captured. A client trace that ends
    more than two exclusion windows before the server's is refused outright: without capture
    overlap none of the per-key verdicts are trustworthy.
    """
    failures: list[str] = []

    def add(name: str, count: int) -> None:
        if count:
            failures.append(f"{name}={count}")

    gap = int(summary["client_trace_end_gap_ticks"])
    if gap > 2 * TRACE_END_WINDOW_TICKS:
        add("client_trace_ended_early_by_ticks", gap)
    add("lost_shots", summary["lost"])
    add("own_echo_lost", summary["own_echo_lost"])
    add("retimed_lost", summary["retimed_lost"])
    add("duplicate_shots", summary["multi_spawn"])
    add("no_spawn_shots", summary["no_spawn"])

    paths = summary["paths"]
    add("public_path_failures", paths.get("missing_delivery", 0) + paths.get("refused", 0))

    damage = summary["damage"]
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
        print(json.dumps(payload, indent=2, default=float))
    else:
        report(a, args.samples)
    if args.strict and failures:
        print("strict verification failed: " + ", ".join(failures), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
