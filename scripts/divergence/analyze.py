# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""The divergence instrument's offline join/diff (src/trace.rs per-tick state hash).

A determinism effort drives client/server sim divergence toward zero; this is the
measurement it drives. It joins the role-suffixed client and server JSONL traces
a `SPIKE_TRACE` run writes, pairs each logical tank ACROSS the two ECS worlds
(their entity ids differ — 4294966669 vs 4294966650 for the same tank — so the
join never uses `e`; it pairs on the world-independent `own` field the rows
carry), and per shared tick reports:

  - hash MATCH RATE: the fraction of shared ticks whose combined state hash `h`
    is bit-identical on both ends — overall and split by workload window
    (flat-ground cruise vs contact transient). Flat cruise is expected ~100%
    (measured bit-exact); divergence concentrates at contact transients.
  - FIRST DIVERGENCE: the earliest shared tick where `h` differs, and which
    sub-component (pos / rot / lv / av / sim) diverged there — localized by the
    per-component sub-hashes.
  - per-component ERROR MAGNITUDE (|Δp|, rotation-angle delta, |Δlv|, |Δav|;
    p50/p99/max) computed from the pose/velocity fields the rows already carry,
    overall and per window — the size of the divergence the hash flags.

The hash answers "did anything differ?" exactly (including the carried
`TankSim`/`DriveState` no pose field exposes, via `hsim`); the magnitudes answer
"by how much?" for the pose/velocity part.

Rollback REPLAY re-records ticks (client rows stamped `rp=true`): where rows
share (tick, entity) the LAST wins — the corrected replay value, matching
src/trace.rs and scripts/jitter/analyze.py.

Usage:
    uv run scripts/divergence/analyze.py --client C.client.jsonl --server C.server.jsonl
        [--warmup-ticks N] [--json]
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

import numpy as np

# Rollback thresholds (net/protocol.rs) — printed alongside the magnitudes for context, not gates.
DIV_P = 0.05   # m
DIV_Q = 0.05   # rad
DIV_LV = 0.20  # m/s

SUBS = ("hpos", "hrot", "hlv", "hav", "hsim")
SUB_LABEL = {"hpos": "pos", "hrot": "rot", "hlv": "lv", "hav": "av", "hsim": "sim"}


# --- quaternion helpers (layout [x, y, z, w], matching src/trace.rs) ---------------------------
def q_conj(q):
    return np.array([-q[0], -q[1], -q[2], q[3]])


def q_mul(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return np.array([
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ])


def q_angle(q):
    """Shortest-arc rotation magnitude of a unit quaternion, in [0, pi] radians."""
    v = math.sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2])
    return 2.0 * math.atan2(v, abs(q[3]))


def q_between(a, b):
    """Shortest-arc angle between two orientations a -> b (radians).

    Bit-identical inputs return exactly 0.0: the composed formula leaves ~1e-17 rad of
    float-addition non-associativity residue on IDENTICAL quats ((wy + xz) - yw - zx round-off),
    which would print as a phantom rotation floor in a baseline where rotation is measured
    bit-exact. A REAL divergence is never masked — one flipped f32 quat bit is ~1e-7 rad, ten
    orders above the residue this early-out removes.
    """
    if np.array_equal(a, b):
        return 0.0
    return q_angle(q_mul(q_conj(a), b))


# --- parsing ----------------------------------------------------------------------------------
def _finite_vec(x, n):
    """np array of length n iff every element is a finite number, else None (a corrupt f32
    serialises as JSON null; a hard-killed process tears its last line)."""
    if not isinstance(x, list) or len(x) != n:
        return None
    out = np.empty(n)
    for i, e in enumerate(x):
        if not isinstance(e, (int, float)) or e is None or not math.isfinite(e):
            return None
        out[i] = e
    return out


def parse_ticks(path):
    """Parse a JSONL trace's `tick` rows. Returns (meta, ticks, stats).

    Collapses rows sharing (tick, entity) to the LAST occurrence so a client's
    rollback-replay rows (rp=true) supersede the original misprediction. Rows with
    a null/NaN pose are skipped and counted; the hash fields are kept as exact
    Python ints (u64 — arbitrary precision in JSON, no float rounding).
    """
    meta = None
    by_key = {}
    bad_lines = 0
    null_poses = 0
    n_raw = 0
    with open(path, "r") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
            except (json.JSONDecodeError, ValueError):
                bad_lines += 1
                continue
            k = row.get("k")
            if k == "meta":
                meta = row
            elif k == "tick":
                p = _finite_vec(row.get("p"), 3)
                q = _finite_vec(row.get("q"), 4)
                lv = _finite_vec(row.get("lv"), 3)
                av = _finite_vec(row.get("av"), 3)
                if p is None or q is None:
                    null_poses += 1
                    continue
                n_raw += 1
                rec = {
                    "tick": row.get("tick"), "e": row.get("e"),
                    "p": p, "q": q, "lv": lv, "av": av,
                    "gnd": row.get("gnd"), "hc": row.get("hc"),
                    "own": row.get("own"),
                    "ctl": bool(row.get("ctl", False)),
                    "h": row.get("h"),
                    "sub": {s: row.get(s) for s in SUBS},
                }
                by_key[(rec["tick"], rec["e"])] = rec
    ticks = list(by_key.values())
    stats = {"bad_lines": bad_lines, "null_poses": null_poses,
             "replay_ticks": n_raw - len(ticks)}
    return meta, ticks, stats


# --- cross-world tank pairing -----------------------------------------------------------------
def pair_tanks(client_ticks, server_ticks):
    """Pair each logical tank across the two worlds, returning [(name, c_rows, s_rows), ...].

    The join is world-independent: it never compares entity ids (they differ per
    world). It pairs on `own` — the player's own tank is `own=true` on both ends
    (client `Controlled` / server `ControlledBy`); the ownerless bot is `own=false`
    on both. For the solo case (one own tank, at most one bot) that is exact. If a
    trace predates the `own` field, fall back to the single busiest entity per side
    (the historical one-tank assumption).
    """
    def group(rows):
        by_e = {}
        for r in rows:
            by_e.setdefault(r["e"], []).append(r)
        return by_e

    def busiest(by_e):
        return max(by_e, key=lambda e: len(by_e[e])) if by_e else None

    c_by_e, s_by_e = group(client_ticks), group(server_ticks)
    have_own = (any(r["own"] is not None for r in client_ticks)
                and any(r["own"] is not None for r in server_ticks))

    if not have_own:
        ce, se = busiest(c_by_e), busiest(s_by_e)
        if ce is None or se is None:
            return []
        return [("tank (own field absent — busiest-entity pairing)",
                 c_by_e[ce], s_by_e[se])]

    pairs = []
    for own_val, label in ((True, "own (player) tank"), (False, "bot / remote tank")):
        c = [r for r in client_ticks if bool(r["own"]) == own_val]
        s = [r for r in server_ticks if bool(r["own"]) == own_val]
        if c and s:
            pairs.append((label, c, s))
    return pairs


# --- per-tick join + diff ---------------------------------------------------------------------
def join(c_rows, s_rows, warmup_ticks):
    """Join one tank's client/server rows on tick number; return a per-shared-tick record array.

    warmup_ticks drops the first ticks (spawn settle / camera fly-in / initial
    rollback burst) — one-time transients that are not the steady divergence.
    """
    cmap = {r["tick"]: r for r in c_rows if r["tick"] is not None}
    smap = {r["tick"]: r for r in s_rows if r["tick"] is not None}
    shared = sorted(set(cmap) & set(smap))
    if shared and warmup_ticks:
        lo = shared[0] + warmup_ticks
        shared = [t for t in shared if t >= lo]

    out = []
    for tk in shared:
        c, s = cmap[tk], smap[tk]
        h_match = (c["h"] is not None and c["h"] == s["h"])
        sub_match = {sub: (c["sub"][sub] is not None and c["sub"][sub] == s["sub"][sub])
                     for sub in SUBS}
        dp = float(np.linalg.norm(c["p"] - s["p"]))
        dq = q_between(c["q"], s["q"])
        dlv = (float(np.linalg.norm(c["lv"] - s["lv"]))
               if c["lv"] is not None and s["lv"] is not None else float("nan"))
        dav = (float(np.linalg.norm(c["av"] - s["av"]))
               if c["av"] is not None and s["av"] is not None else float("nan"))
        # Workload window from the sim's own contact signal on BOTH ends.
        gnd_ok = (c["gnd"] == 16 and s["gnd"] == 16)
        hc_zero = ((c["hc"] in (0, None)) and (s["hc"] in (0, None)))
        flat = gnd_ok and hc_zero
        out.append({"tick": tk, "h_match": h_match, "sub_match": sub_match,
                    "dp": dp, "dq": dq, "dlv": dlv, "dav": dav,
                    "flat": flat})
    return out


def pct(arr, q):
    a = np.asarray([x for x in arr if not (isinstance(x, float) and math.isnan(x))])
    return float(np.percentile(a, q)) if len(a) else float("nan")


def mag_stats(joined, key):
    vals = [j[key] for j in joined]
    return pct(vals, 50), pct(vals, 99), pct(vals, 100)


def summarize(joined):
    """Match rates, first divergence, sub-component tally, and magnitude stats for one tank."""
    n = len(joined)
    matched = sum(1 for j in joined if j["h_match"])
    flat = [j for j in joined if j["flat"]]
    trans = [j for j in joined if not j["flat"]]

    first = next((j for j in joined if not j["h_match"]), None)
    first_subs = ([SUB_LABEL[s] for s in SUBS if not first["sub_match"][s]]
                  if first else [])

    # Among mismatched ticks, which sub-components diverged (the "|Δav|-first" signature check).
    mismatched = [j for j in joined if not j["h_match"]]
    sub_tally = {SUB_LABEL[s]: sum(1 for j in mismatched if not j["sub_match"][s])
                 for s in SUBS}

    def rate(rows):
        return (sum(1 for j in rows if j["h_match"]) / len(rows)) if rows else float("nan")

    return {
        "n": n, "matched": matched,
        "rate_all": (matched / n) if n else float("nan"),
        "n_flat": len(flat), "rate_flat": rate(flat),
        "n_trans": len(trans), "rate_trans": rate(trans),
        "first": first, "first_subs": first_subs,
        "sub_tally": sub_tally, "n_mismatch": len(mismatched),
        "mag": {k: mag_stats(joined, k) for k in ("dp", "dq", "dlv", "dav")},
        "mag_trans": {k: mag_stats(trans, k) for k in ("dp", "dq", "dlv", "dav")},
    }


# --- report -----------------------------------------------------------------------------------
def fmt_pct(x):
    return "  n/a  " if math.isnan(x) else f"{x * 100:6.2f}%"


def fmt_e(x):
    return f"{'n/a':>12}" if math.isnan(x) else f"{x:>12.3e}"


def print_tank(label, s):
    print(f"\n  TANK: {label}")
    print(f"    shared ticks (post-warmup): {s['n']}")
    if s["n"] == 0:
        print("    (no overlapping ticks — nothing to compare)")
        return
    print("\n    HASH MATCH RATE (fraction of shared ticks bit-identical on both ends)")
    print(f"      overall           {fmt_pct(s['rate_all'])}   ({s['matched']}/{s['n']})")
    print(f"      flat-ground cruise{fmt_pct(s['rate_flat'])}   "
          f"({s['n_flat']} ticks: gnd=16 & hc=0 both ends)")
    print(f"      contact transient {fmt_pct(s['rate_trans'])}   "
          f"({s['n_trans']} ticks: hull contact / wheels lifting / airborne)")

    print("\n    FIRST DIVERGENCE")
    if s["first"] is None:
        print("      (none — every shared tick is bit-identical)")
    else:
        f = s["first"]
        print(f"      tick {f['tick']}   diverged sub-component(s): "
              f"{', '.join(s['first_subs']) or '(none — combined only)'}")
        print(f"        |Δp| {f['dp']:.3e} m   rot {f['dq']:.3e} rad   "
              f"|Δlv| {f['dlv']:.3e} m/s   |Δav| {f['dav']:.3e} rad/s")

    if s["n_mismatch"]:
        tally = "   ".join(f"{name}={cnt}" for name, cnt in s["sub_tally"].items())
        print(f"\n    SUB-COMPONENT DIVERGENCE TALLY ({s['n_mismatch']} mismatched ticks)")
        print(f"      {tally}")

    def mag_block(title, mag):
        print(f"\n    {title}")
        print(f"      {'metric':<12}{'p50':>12}{'p99':>12}{'max':>12}   threshold")
        print(f"      {'|Δp| m':<12}{fmt_e(mag['dp'][0])}{fmt_e(mag['dp'][1])}"
              f"{fmt_e(mag['dp'][2])}   {DIV_P} m")
        print(f"      {'rot rad':<12}{fmt_e(mag['dq'][0])}{fmt_e(mag['dq'][1])}"
              f"{fmt_e(mag['dq'][2])}   {DIV_Q} rad")
        print(f"      {'|Δlv| m/s':<12}{fmt_e(mag['dlv'][0])}{fmt_e(mag['dlv'][1])}"
              f"{fmt_e(mag['dlv'][2])}   {DIV_LV} m/s")
        print(f"      {'|Δav| rad/s':<12}{fmt_e(mag['dav'][0])}{fmt_e(mag['dav'][1])}"
              f"{fmt_e(mag['dav'][2])}")

    mag_block("PER-COMPONENT ERROR MAGNITUDE (all shared ticks)", s["mag"])
    if s["n_trans"]:
        mag_block("PER-COMPONENT ERROR MAGNITUDE (contact-transient ticks only)", s["mag_trans"])


def main():
    ap = argparse.ArgumentParser(description="Divergence instrument: client/server per-tick hash join.")
    ap.add_argument("--client", required=True, help="client role-suffixed JSONL trace")
    ap.add_argument("--server", required=True, help="server role-suffixed JSONL trace")
    ap.add_argument("--warmup-ticks", type=int, default=64,
                    help="drop this many leading shared ticks (spawn/settle transient; default 64)")
    ap.add_argument("--json", action="store_true", help="emit the summary as JSON instead of text")
    args = ap.parse_args()

    cmeta, cticks, cstats = parse_ticks(Path(args.client))
    smeta, sticks, sstats = parse_ticks(Path(args.server))

    pairs = pair_tanks(cticks, sticks)

    if args.json:
        payload = {"client": args.client, "server": args.server,
                   "tanks": []}
        for label, c_rows, s_rows in pairs:
            s = summarize(join(c_rows, s_rows, args.warmup_ticks))
            s.pop("first")  # not JSON-friendly (numpy arrays); rates/tally are the machine payload
            payload["tanks"].append({"label": label, **{k: v for k, v in s.items()
                                                         if k not in ("mag", "mag_trans")}})
        print(json.dumps(payload, indent=2, default=float))
        return 0

    line = "=" * 78
    print(line)
    print("  DIVERGENCE INSTRUMENT — per-tick client/server state-hash join")
    print(line)
    print(f"  client: {Path(args.client).name}   role={(cmeta or {}).get('role', '?')}   "
          f"tick_hz={(cmeta or {}).get('tick_hz', '?')}")
    print(f"  server: {Path(args.server).name}   role={(smeta or {}).get('role', '?')}")
    print(f"  client rows: {len(cticks)} tanks-x-ticks "
          f"({cstats['replay_ticks']} replay collapsed, {cstats['bad_lines']} bad, "
          f"{cstats['null_poses']} null pose)")
    print(f"  server rows: {len(sticks)} tanks-x-ticks "
          f"({sstats['replay_ticks']} replay collapsed, {sstats['bad_lines']} bad, "
          f"{sstats['null_poses']} null pose)")
    print(f"  warmup dropped: first {args.warmup_ticks} shared ticks")

    if not pairs:
        print("\n  error: could not pair any tank across the two traces "
              "(no shared 'own' groups and no entities).", file=sys.stderr)
        return 2

    for label, c_rows, s_rows in pairs:
        print_tank(label, summarize(join(c_rows, s_rows, args.warmup_ticks)))
    print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
