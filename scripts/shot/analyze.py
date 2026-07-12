# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""The shot-lifecycle instrument's offline join (src/shot_trace.rs, `SPIKE_SHOT_TRACE`).

Two constants in the fire-replication slice were sized from theory and never measured
against a real link: `ballistics::RICOCHET_HOLD_TICKS` (16 — how long a client shell
freezes at armor waiting for the server's verdict) and `SanctionedShots::MAX_AGE_SECS`
(3.0 — how long an unconsumed outcome lingers). This script turns a recorded run into
the numbers that should size them.

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
  - COPIES PER EVENT: how many datagrams each fire / keyframe / confirm actually rode
    (`send` rows — one per event per burst). The server re-broadcasts its whole window
    every tick (`net::server::broadcast_fire_window`), so this is the redundancy the
    design promises, MEASURED — and a MINIMUM of 1 is the shape of the flake that
    motivated the clock-driven window (an isolated 88 bounce rode a single datagram, and
    one dropped packet lost the carry-through outright).
  - CARRY-THROUGH: of every ricochet the authority sanctioned, what fraction actually
    re-seeded a client shell — vs dissolved (the window ran out) vs never consumed
    (delivered, but no shell ever took it: the F3 pose-divergence class) vs never
    delivered (lost outright, redundancy window included).
  - DEDUP / REDUNDANCY REPAIR: how often the sliding window's duplicates were rejected
    (bytes spent) and how often a shot arrived ONLY because a later burst re-carried it
    (bytes earned — the window repairing a real loss).
  - EXACTLY-ONCE: shots whose cosmetic shell spawned more than once (a dedup failure) or
    never spawned (a delivery failure the window did not repair).

Cross-process joining: the `ShotId`'s `shooter` entity id is NOT comparable across the
two ECS worlds (the client's is its local replica of the server's tank), so shots are
paired on `(weapon, fire_tick)` — with the fire ORIGIN, recorded verbatim off the wire
on both ends, as the tiebreak. Residual ambiguity (two tanks firing the same weapon slot
on the same tick from the same point) is reported, never hidden.

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

# The hold-window sizing bar: a window is "adequate" if it covers this share of observed
# arrival leads. 99% is the same tail discipline the sync margin uses (jitter_multiple=2
# → 95% packet coverage; a hold miss is cheaper than a rollback, but it is a LOST PICTURE,
# so we ask for more).
COVERAGE_TARGET = 0.99


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


def shot_key(row: dict) -> tuple[int, int]:
    """The cross-process shot key: (weapon slot, fire tick). NOT the shooter entity — see
    the module docstring on why entity ids cannot cross the world boundary."""
    return (row["w"], row["ft"])


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
    server: dict[tuple[int, int], dict] = defaultdict(lambda: defaultdict(list))
    for r in srows:
        server[shot_key(r)][r["k"]].append(r)
    client: dict[tuple[int, int], dict] = defaultdict(lambda: defaultdict(list))
    for r in crows:
        client[shot_key(r)][r["k"]].append(r)

    # Key collisions: two DISTINCT shots that share (weapon, fire_tick) — only possible when
    # two tanks fire the same slot on the same tick. The fire origin (recorded verbatim off
    # the wire on both ends) separates them; report what is left rather than silently merging.
    ambiguous = 0
    for key, kinds in server.items():
        origins = {tuple(f.get("o", ())) for f in kinds.get("fire", [])}
        if len(origins) > 1:
            ambiguous += 1

    # --- the client's OWN shots vs the shots it OBSERVES ----------------------------------
    # The shooter never receives a `fire_rx` for its own round (`receive_fire_events` drops the
    # echo of any tank it simulates locally), so own shots must be excluded from the delivery
    # accounting or every one of them reads as a lost fire. A `spawn src="own"` row is exactly
    # that marker.
    own = {k for k, kinds in client.items() if any(s.get("src") == "own" for s in kinds.get("spawn", []))}
    fired = set(server.keys()) & {k for k, kinds in server.items() if kinds.get("fire")}
    observed_expected = fired - own

    # --- delivery / dedup / repair --------------------------------------------------------
    delivered, dup_rows, repaired, lost = 0, 0, 0, []
    for key in sorted(observed_expected):
        rx = client[key].get("fire_rx", [])
        if not rx:
            lost.append(key)
            continue
        delivered += 1
        dup_rows += sum(1 for r in rx if r.get("dup"))
        first_new = next((r for r in rx if not r.get("dup")), None)
        # The burst that carried this fire FRESH: if its newest fire tick is later than this
        # shot's own, the burst sent the moment it fired never arrived — a later burst's
        # redundancy window repaired the loss. That is the window earning its bytes.
        if first_new and int(first_new.get("bnew", 0)) > key[1]:
            repaired += 1

    # --- the receive gate's rejections ----------------------------------------------------
    # `drop` rows: events the client REFUSED at the gate. Today the only reason is an unresolvable
    # shooter (`net::client::shooter_is_live`) — a fire/keyframe/confirm that beat its shooter's tank
    # replica down the pipe and would otherwise have been keyed on a garbage entity (duplicate shells,
    # uncorrelatable bounces). A drop is NOT a loss: the server re-carries the event every tick of its
    # retain window, so the copy that arrives once the tank resolves is accepted, correctly keyed. What
    # matters is that the shot is eventually delivered — a shot with drops AND a fire_rx was repaired
    # by the window; a shot with drops and NOTHING else is one the guard cost outright.
    drops = defaultdict(int)
    for r in crows:
        if r["k"] == "drop":
            drops[f"{r.get('s', '?')}:{r.get('res', '?')}"] += 1
    dropped_keys = {shot_key(r) for r in crows if r["k"] == "drop"}
    dropped_then_delivered = sum(
        1 for k in dropped_keys if client.get(k, {}).get("fire_rx") or client.get(k, {}).get("spawn")
    )

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
    kf_lead = [r["t"] - r["bt"] for r in crows if r["k"] == "kf_rx" and not r.get("dup")]
    cf_lead = [r["t"] - r["it"] for r in crows if r["k"] == "cf_rx" and not r.get("dup")]

    # --- copies per event: the redundancy the window actually delivered --------------------
    # A `send` row is a TRANSMISSION (one per event per burst, written at the server's single send
    # site); `fire`/`kf`/`cf` are EMISSIONS (the tick the thing happened). Counting the sends of one
    # emitted event gives the datagram copies it rode — which is precisely what the clock-driven
    # window was built to raise off the floor: when the send was driven by the events themselves, an
    # isolated main-gun bounce rode ONE copy and a single dropped packet lost it for good.
    copies: dict[str, list[int]] = {"fire": [], "kf": [], "cf": []}
    for key, kinds in server.items():
        sends = kinds.get("send", [])
        for stream in ("fire", "cf"):
            if kinds.get(stream):  # at most one of each per shot
                copies[stream].append(sum(1 for r in sends if r.get("s") == stream))
        for kf in kinds.get("kf", []):  # a shot may bounce more than once
            seq = kf.get("seq", 0)
            copies["kf"].append(
                sum(1 for r in sends if r.get("s") == "kf" and r.get("seq") == seq)
            )
    # The USABLE copies of a bounce are only those that land before the observer's shell gives up:
    # the shell holds `hold_bound` ticks, so a copy sent more than that many ticks after the bounce
    # can never be consumed however faithfully it is delivered (see `RICOCHET_HOLD_TICKS`' doc).
    usable_kf = [
        sum(
            1
            for r in kinds.get("send", [])
            if r.get("s") == "kf" and r.get("seq") == kf.get("seq", 0) and r.get("c", 0) < hold_bound
        )
        for kinds in server.values()
        for kf in kinds.get("kf", [])
    ]

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

    # --- window sizing verdict ------------------------------------------------------------
    # The hold window must cover the arrival lead's tail: a keyframe that lands later than the
    # window is a dissolved shot. Sized off the OBSERVED leads (not the observed holds, which
    # are already truncated BY the window — the histogram cannot see past its own bound).
    need = pct(kf_lead + cf_lead, COVERAGE_TARGET * 100) if (kf_lead or cf_lead) else float("nan")

    return {
        "hz": hz,
        "hold_bound": hold_bound,
        "max_age_secs": cmeta.get("max_age_secs") or smeta.get("max_age_secs"),
        "overdue_ticks": cmeta.get("overdue_ticks") or smeta.get("overdue_ticks"),
        "ambiguous_keys": ambiguous,
        "server_fires": len(fired),
        "own_shots": len(own & fired),
        "expected": len(observed_expected),
        "delivered": delivered,
        "lost": len(lost),
        "dup_rows": dup_rows,
        "repaired": repaired,
        "multi_spawn": len(multi_spawn),
        "no_spawn": len(no_spawn),
        "holds": {k: v for k, v in holds.items()},
        "all_holds": all_holds,
        "kf_lead": kf_lead,
        "cf_lead": cf_lead,
        "drops": dict(drops),
        "dropped_shots": len(dropped_keys),
        "dropped_then_delivered": dropped_then_delivered,
        "copies": copies,
        "usable_kf_copies": usable_kf,
        # Whether the trace carries `send` rows AT ALL. A trace recorded before the server's single
        # send site was instrumented has none, and every copy count would read as a (meaningless) zero
        # — say "not instrumented", never "this event rode no datagrams".
        "sends_seen": sum(1 for r in srows if r["k"] == "send"),
        "carry": dict(carry),
        "term": dict(term),
        "ends": dict(ends),
        "need_ticks": need,
        "server": server,
        "client": client,
        "expected_keys": sorted(observed_expected),
    }


def reconstruct(key: tuple[int, int], s: dict, c: dict) -> list[str]:
    """One shot's life, in tick order across both processes.

    The per-tick `send` rows (one per event per burst — up to ~20 per event) are COLLAPSED into a
    single summary line per stream: they are a count, not a moment, and spelling them out would bury
    the lifecycle they carry."""
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
    out = [f"    shot weapon={key[0]} fire_tick={key[1]}"]
    for stream in ("fire", "kf", "cf"):
        sent = [r for r in s.get("send", []) if r.get("s") == stream]
        if sent:
            ticks = [r["t"] for r in sent]
            out.append(
                f"      [S] sent {stream:<4} ×{len(sent):<3} t={min(ticks)}..{max(ticks)}  "
                f"(datagram copies)"
            )
    for t, role, kind, r in events:
        extra = {
            k: v for k, v in r.items() if k not in ("k", "t", "sh", "w", "ft")
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
        f"({a['hold_bound'] * ms:.0f} ms)   OVERDUE_MARGIN_TICKS={a['overdue_ticks']}   "
        f"MAX_AGE_SECS={a['max_age_secs']}"
    )
    if a["ambiguous_keys"]:
        print(
            f"  WARNING: {a['ambiguous_keys']} (weapon, fire_tick) key(s) carry more than one fire "
            "origin — two shooters fired the same slot on the same tick; those shots are merged."
        )

    print("\n  DELIVERY  (shots the server broadcast that this client should observe)")
    print(f"    server fires                 {a['server_fires']}")
    print(f"      of which this client's own {a['own_shots']}  (never echoed back — excluded below)")
    print(f"    expected on this client      {a['expected']}")
    print(f"    delivered                    {a['delivered']}")
    print(f"    LOST (never arrived)         {a['lost']}")
    print(
        f"    redundancy REPAIRED          {a['repaired']}   "
        "(arrived only because a later burst re-carried it — the window earning its bytes)"
    )
    print(
        f"    duplicate events rejected    {a['dup_rows']}   "
        "(the window's cost: re-carried events the ShotId dedup dropped)"
    )
    if a["drops"]:
        print("\n  RECEIVE GATE  (events refused before they could key anything)")
        for reason, n in sorted(a["drops"].items(), key=lambda kv: -kv[1]):
            print(f"    dropped {reason:<28} {n:>6}")
        print(
            f"    distinct shots affected      {a['dropped_shots']}, of which "
            f"{a['dropped_then_delivered']} were still delivered afterwards "
            "(the redundancy window turning the race into a delay, not a loss)"
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
    if not np.isnan(a["need_ticks"]):
        verdict = "ADEQUATE" if a["need_ticks"] <= a["hold_bound"] else "TOO SHORT"
        print(
            f"\n    SIZING: covering {COVERAGE_TARGET:.0%} of observed arrivals needs "
            f"{a['need_ticks']:.0f} ticks ({a['need_ticks'] * ms:.0f} ms); "
            f"RICOCHET_HOLD_TICKS = {a['hold_bound']} → {verdict}"
        )

    print("\n  COPIES PER EVENT  (datagrams each event actually rode — the redundancy, measured)")
    if not a["sends_seen"]:
        print("      (no `send` rows — trace predates the server's single-send-site instrumentation)")
    else:
        for stream, label in (("fire", "fires"), ("kf", "keyframes"), ("cf", "confirms")):
            v = a["copies"].get(stream, [])
            if not v:
                print(f"    {label:<11}   (none)")
                continue
            arr = np.asarray(v, dtype=float)
            print(
                f"    {label:<11} n={len(arr):<5d} min={arr.min():3.0f}  p50={pct(arr, 50):5.1f}  "
                f"max={arr.max():3.0f}"
            )
        lone = [s for s in ("fire", "kf", "cf") if a["copies"].get(s) and min(a["copies"][s]) <= 1]
        if lone:
            print(
                f"    WARNING: some {'/'.join(lone)} event(s) rode a SINGLE datagram — that is the "
                "pre-fix shape (one packet drop loses the event outright)."
            )
        u = a["usable_kf_copies"]
        if u:
            arr = np.asarray(u, dtype=float)
            print(
                f"    of the keyframe copies, those the observer's shell could still CONSUME "
                f"(sent < {a['hold_bound']}t after the bounce): min={arr.min():.0f}  "
                f"p50={pct(arr, 50):.1f}  max={arr.max():.0f}"
            )

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
    args = ap.parse_args()

    cmeta, crows = load(Path(args.client))
    smeta, srows = load(Path(args.server))
    if not crows and not srows:
        print("no rows in either trace — was SPIKE_SHOT_TRACE armed?", file=sys.stderr)
        return 1

    a = analyze(cmeta, crows, smeta, srows)
    if args.json:
        payload = {k: v for k, v in a.items() if k not in ("server", "client", "expected_keys")}
        payload["hold_p50"] = pct(a["all_holds"], 50)
        payload["hold_p99"] = pct(a["all_holds"], 99)
        payload["kf_lead_p99"] = pct(a["kf_lead"], 99)
        print(json.dumps(payload, indent=2, default=float))
        return 0
    report(a, args.samples)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
