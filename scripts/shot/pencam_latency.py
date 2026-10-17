# /// script
# requires-python = ">=3.11"
# ///
"""Pen cam delivery latency: the join of one server's and its clients' `SPIKE_SHOT_TRACE` rows.

Rows (src/net/shot_explanation.rs, src/net/shot_transport.rs, src/shot_trace.rs):

  server  kf / cf   an authority outcome resolved: a bounce (`seq`) or the end (`ab`, bounces
                    before it); the ordinal an explanation's `o` names
  server  xsend     one projection entered transport, to combatant `to`
  server  link      a client link connected (`ev` up) or ended (`ev` down)
  server  fire      a round fired (`cal`, metres: main gun at or above MAIN_GUN_MIN_CALIBER)
  client  xrx       an explanation stored, with its presence on arrival (`pres`)
  client  xpres     a stored explanation left pending: `presented` or `timed_out`
  client  xdraw     the client drew authority outcome `o` of a shot

Every row but `meta` carries `us`, the wall clock in microseconds; server and clients run on one
machine, so cross-process intervals are differences of `us`. Per explanation a client stored:

  wire    server impact (the kf/cf row of its outcome) -> arrival (xrx)
  hold    of which: server impact -> send (the record finishing on the authority)
  draw    arrival -> the client's own draw of the outcome (negative: drawn before it arrived)
  open    server impact -> the cam able to open: arrival if presented on arrival, the draw (xdraw)
          if presented later, the timeout's xpres row if timed out (expiry and that row run in one
          pass, so only a timeout reads xpres; a draw lands between passes)

Each curve is reported per weapon and role (`main/shooter`), and again per client trace
(`main/shooter/a`), since a client's interpolation delay sets its `draw`.

An explanation with no outcome (`o` null: the round reached this tank past its first crossing) has
no impact any client draws; it is counted apart and never presented.

The run is valid only if each client connected once and stayed connected while explanations were
sent: a reconnect takes a new spawn lane, and the scripted aims assume the first ones. The analyzer
exits non-zero, naming the reason, on more `link` ups than clients or any `link` down before the
last xsend. A server trace without `link` rows predates them: `--server-log` then counts the
server log's connects instead, and disconnects are reported unverified.

Usage:
    uv run scripts/shot/pencam_latency.py --server S.server.jsonl --client A.client.jsonl
        --client B.client.jsonl [--server-log S.log] [--label TEXT] [--json OUT]
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from collections import defaultdict
from pathlib import Path

# The caliber (m) at and above which a round is the main gun's; `analyze.py` names the same split.
MAIN_GUN_MIN_CALIBER = 0.02


def load(path: Path) -> tuple[dict, list[dict]]:
    """(meta row, other rows); a torn tail line from a killed process is skipped."""
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
                continue
            if row.get("k") == "meta":
                meta = row
            else:
                rows.append(row)
    return meta, rows


def shot(row: dict) -> tuple[int, int, int]:
    return (row["c"], row["w"], row["ft"])


def explanation(row: dict) -> tuple:
    """An explanation's identity: its shot, struck life, encounter and audience."""
    return (*shot(row), row["sc"], row["sl"], row["en"], row["aud"])


def percentile(values: list[float], q: float) -> float:
    """Linear interpolation between closest ranks (numpy's default)."""
    if not values:
        return math.nan
    s = sorted(values)
    pos = (len(s) - 1) * q / 100.0
    lo = math.floor(pos)
    hi = math.ceil(pos)
    return s[lo] + (s[hi] - s[lo]) * (pos - lo)


def curve(values_ms: list[float], tick_ms: float) -> dict:
    if not values_ms:
        return {"n": 0}
    p50, p90, top = percentile(values_ms, 50), percentile(values_ms, 90), max(values_ms)
    return {
        "n": len(values_ms),
        "min_ms": min(values_ms),
        "p50_ms": p50,
        "p90_ms": p90,
        "max_ms": top,
        "p50_t": p50 / tick_ms,
        "p90_t": p90 / tick_ms,
        "max_t": top / tick_ms,
    }


def analyze(server_path: Path, client_paths: list[Path]) -> dict:
    smeta, srows = load(server_path)
    hz = float(smeta.get("tick_hz") or 64)
    tick_ms = 1000.0 / hz

    impact_us: dict[tuple, int] = {}
    for r in srows:
        if r["k"] == "kf":
            key = (*shot(r), r["seq"])
        elif r["k"] == "cf":
            key = (*shot(r), r["ab"])
        else:
            continue
        impact_us[key] = min(impact_us.get(key, r["us"]), r["us"])
    caliber = {shot(r): r["cal"] for r in srows if r["k"] == "fire"}

    def weapon(key: tuple) -> str:
        cal = caliber.get(key[:3])
        if cal is None:
            return "unknown"
        return "main" if cal >= MAIN_GUN_MIN_CALIBER else "mg"

    fires = defaultdict(int)
    for key in caliber:
        fires[weapon(key)] += 1
    sends = {explanation(r): r for r in srows if r["k"] == "xsend"}
    sent_by = defaultdict(int)
    for key in sends:
        sent_by[(weapon(key), key[-1])] += 1

    received: set[tuple] = set()
    samples: dict[tuple, dict[str, list[float]]] = defaultdict(lambda: defaultdict(list))
    counts: dict[tuple, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    clients = []
    for path in client_paths:
        _, crows = load(path)
        arrivals = [r for r in crows if r["k"] == "xrx"]
        released = {explanation(r): r for r in crows if r["k"] == "xpres"}
        drawn_us: dict[tuple, int] = {}
        for r in crows:
            if r["k"] == "xdraw":
                key = (*shot(r), r["o"])
                drawn_us[key] = min(drawn_us.get(key, r["us"]), r["us"])
        shooters = sorted({r["c"] for r in arrivals if r["aud"] == "shooter"})
        clients.append({"trace": str(path), "explanations": len(arrivals), "as_shooter": shooters})
        name = path.name.split(".")[0]
        for r in arrivals:
            key = explanation(r)
            received.add(key)
            role = "shooter" if r["aud"] == "shooter" else "victim"
            # Every sample lands in its role's group and in that role's group on this client alone.
            groups_of = [(weapon(key), role), (weapon(key), role, name)]

            def count(flag: str) -> None:
                for group in groups_of:
                    counts[group][flag] += 1

            def add(metric: str, since_us: int, until_us: int) -> None:
                for group in groups_of:
                    samples[group][metric].append((until_us - since_us) / 1000.0)

            count("stored")
            if r["aud"] == "killed_victim":
                count("killed_victim")
            send = sends.get(key)
            if send is None:
                count("no_send_row")
            if r["o"] is None:
                count("no_outcome")
                continue
            impact = impact_us.get((*shot(r), r["o"]))
            if impact is None:
                count("no_impact_row")
            else:
                add("wire", impact, r["us"])
                if send is not None:
                    add("hold", impact, send["us"])
            if send is not None:
                add("transit", send["us"], r["us"])
            draw = drawn_us.get((*shot(r), r["o"]))
            if draw is None:
                count("never_drawn")
            else:
                add("draw", r["us"], draw)
            if r["pres"] == "presented":
                count("presented_on_arrival")
                opened = r["us"]
            elif (later := released.get(key)) is None:
                count("unreleased")
                continue
            elif later["pres"] == "timed_out":
                count("timed_out")
                opened = later["us"]
            elif draw is None:
                count("presented_undrawn")
                continue
            else:
                count("presented")
                opened = draw
            if impact is not None:
                add("open", impact, opened)

    groups = {}
    for group in sorted(set(samples) | set(counts)):
        c = dict(counts[group])
        released = c.get("presented_on_arrival", 0) + c.get("presented", 0) + c.get("timed_out", 0)
        groups["/".join(group)] = {
            "counts": c,
            "timed_out_fraction": (c.get("timed_out", 0) / released) if released else math.nan,
            "curves": {name: curve(v, tick_ms) for name, v in samples[group].items()},
        }
    lost = [list(key) for key in sends if key not in received]
    links = [r for r in srows if r["k"] == "link"]
    return {
        "tick_hz": hz,
        "links": {
            "up": sum(r["ev"] == "up" for r in links),
            "down_before_last_send": sum(
                r["ev"] == "down" and r["us"] < max((x["us"] for x in sends.values()), default=0)
                for r in links
            ),
        }
        if links
        else None,
        "fires": dict(fires),
        "sent": {f"{w}/{aud}": n for (w, aud), n in sorted(sent_by.items())},
        "sent_not_stored": lost,
        "clients": clients,
        "groups": groups,
    }


def connect_failures(result: dict, clients: int, server_log: Path | None) -> list[str]:
    """Why the run's connects invalidate it; empty when each client connected once and stayed."""
    links = result["links"]
    if links is None:
        if server_log is None:
            return ["the server trace has no `link` rows and no --server-log was given"]
        connects = server_log.read_text(errors="replace").count("client connected")
        result["links"] = {"up": connects, "down_before_last_send": None}
        print(f"  legacy trace: {connects} connects in {server_log}; disconnects unverified")
    failures = []
    if result["links"]["up"] != clients:
        failures.append(f"{result['links']['up']} client connects for {clients} clients")
    if result["links"]["down_before_last_send"]:
        failures.append(
            f"{result['links']['down_before_last_send']} client disconnects before the last send"
        )
    return failures


def report(result: dict, label: str) -> None:
    print(f"pen cam delivery latency — {label}  (tick {1000.0 / result['tick_hz']:.3f} ms)")
    print(f"  server fires: {result['fires']}   explanations sent: {result['sent']}")
    print(f"  sent but never stored by a client: {len(result['sent_not_stored'])}")
    for client in result["clients"]:
        print(
            f"  {client['trace']}: {client['explanations']} stored, "
            f"shooter combatant(s) {client['as_shooter']}"
        )
    for name, group in result["groups"].items():
        print(f"\n  {name}: {group['counts']}  timed out {group['timed_out_fraction']:.1%}")
        for metric in ("wire", "hold", "transit", "draw", "open"):
            cv = group["curves"].get(metric)
            if not cv or not cv["n"]:
                continue
            print(
                f"    {metric:<8} n={cv['n']:<4} p50 {cv['p50_ms']:7.1f} ms {cv['p50_t']:5.1f} t   "
                f"p90 {cv['p90_ms']:7.1f} ms {cv['p90_t']:5.1f} t   "
                f"max {cv['max_ms']:7.1f} ms {cv['max_t']:5.1f} t   (min {cv['min_ms']:.1f} ms)"
            )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--client", type=Path, action="append", required=True)
    parser.add_argument("--server-log", type=Path)
    parser.add_argument("--label", default="")
    parser.add_argument("--json", type=Path)
    args = parser.parse_args()
    result = analyze(args.server, args.client)
    report(result, args.label)
    failures = connect_failures(result, len(args.client), args.server_log)
    if sum(c["explanations"] for c in result["clients"]) == 0:
        failures.append("no explanation was stored: the run measured nothing")
    if args.json:
        args.json.write_text(json.dumps(result, indent=2))
    for failure in failures:
        print(f"\nINVALID RUN: {failure}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
