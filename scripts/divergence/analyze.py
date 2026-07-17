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
`TankSim`/`TrackDrive` no pose field exposes, via `hsim`); the magnitudes answer
"by how much?" for the pose/velocity part.

  - MISMATCH WINDOWS: each contiguous mismatched-tick span, attributed to the
    carried-state field families via the `hsim` decode (`hdrv`/`hsrv`/`hrld`/
    `hrec`/`hblt` — drive, servo, reload, recoil, track belt), with whether the
    window opens at the first shared tick (spawn/connect transient vs mid-run
    event) and how many surviving client rows were rollback replays. When the
    trace was recorded with SPIKE_TRACE_SIM_FIELDS the raw carried values ride
    the rows (`simf`) and each window also reports max per-family |Δ| magnitudes.

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

# The carried-state decode (src/trace.rs since the per-field split): `hsim` = the fixed-order
# combination of these five streams. Absent in older traces — attribution then reports "n/a".
SIM_SUBS = ("hdrv", "hsrv", "hrld", "hrec", "hblt")
SIM_LABEL = {"hdrv": "drive", "hsrv": "servo", "hrld": "reload", "hrec": "recoil", "hblt": "track-belt"}


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
                    "simsub": {s: row.get(s) for s in SIM_SUBS},
                    "rp": bool(row.get("rp", False)),
                    # Raw carried-state values (SPIKE_TRACE_SIM_FIELDS) + drive intent, for the
                    # carried-state magnitude report. None when the trace wasn't verbose.
                    "simf": row.get("simf"),
                    "thr": row.get("thr"), "str": row.get("str"),
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


def sim_field_deltas(c, s):
    """Max-abs per-family carried-state differences between two VERBOSE rows (SPIKE_TRACE_SIM_FIELDS).

    Returns {servo, reload, recoil, belt, drive} or None when either row lacks `simf`
    (`belt` here is the MG ammo count — the TRACK belt speed/phase lives in the `hblt` hash
    stream, not in `simf`); torn/null elements make the family read NaN rather than crash the join.

    Each `wpn` row is [reload_remaining, recoil_offset, recoil_velocity, belt_remaining] — the belt
    count (wpn[3]) gates fire and is what the `hrld` sub-hash flags, so a belt-only divergence must
    show a nonzero `belt` delta rather than a misleading reload/recoil 0.0.

    Since the belt-replication fix (`NetBelts`, `net::protocol::apply_net_belts`) the client's belt is
    PINNED to the server's newest CONFIRMED value every tick, so during an active MG burst a small
    `belt` delta (0-1 round) is EXPECTED and BENIGN: it is the pure replication lag (the confirmed
    value trails the server's live belt by a few ticks), transient-then-zero as fire pauses — NOT the
    old accumulate-until-swap divergence. A brief `reload` flag at the last-round boundary (the client
    predicting the final round into a swap a few ticks before the server confirms belt==0) is the same
    bounded, self-healing correction window. A PERSISTENT belt delta that grows across a burst is the
    real regression to hunt.
    """
    cf, sf = c.get("simf"), s.get("simf")
    if not cf or not sf:
        return None

    def max_abs(pairs):
        m = 0.0
        try:
            for a, b in pairs:
                if a is None or b is None:
                    return float("nan")
                m = max(m, abs(a - b))
        except TypeError:
            return float("nan")
        return m

    out = {"servo": max_abs((ea, eb)
                            for xa, xb in zip(cf.get("srv") or [], sf.get("srv") or [])
                            for ea, eb in zip(xa, xb))}
    rld, rec, belt = 0.0, 0.0, 0.0
    try:
        for wa, wb in zip(cf.get("wpn") or [], sf.get("wpn") or []):
            rld = max(rld, abs(wa[0] - wb[0]))
            rec = max(rec, abs(wa[1] - wb[1]), abs(wa[2] - wb[2]))
            # wpn[3] is belt_remaining (an integer round count); older traces omit it — treat a
            # missing 4th field as no belt divergence rather than crashing the join.
            if len(wa) > 3 and len(wb) > 3:
                belt = max(belt, abs(wa[3] - wb[3]))
    except TypeError:
        rld = rec = belt = float("nan")
    out["reload"], out["recoil"], out["belt"] = rld, rec, belt
    if c.get("thr") is not None and s.get("thr") is not None:
        out["drive"] = max(abs(c["thr"] - s["thr"]),
                           abs((c.get("str") or 0.0) - (s.get("str") or 0.0)))
    else:
        out["drive"] = float("nan")
    return out


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
        # Carried-state decode: tri-state — None when either trace predates the sub-hash split
        # (an absent field is UNKNOWN, not diverged).
        sim_match = {sub: (None if (c["simsub"][sub] is None or s["simsub"][sub] is None)
                           else c["simsub"][sub] == s["simsub"][sub])
                     for sub in SIM_SUBS}
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
                    "sim_match": sim_match, "rp": c["rp"],
                    "simd": None if h_match else sim_field_deltas(c, s),
                    "dp": dp, "dq": dq, "dlv": dlv, "dav": dav,
                    "flat": flat})
    return out


def pct(arr, q):
    a = np.asarray([x for x in arr if not (isinstance(x, float) and math.isnan(x))])
    return float(np.percentile(a, q)) if len(a) else float("nan")


def mag_stats(joined, key):
    vals = [j[key] for j in joined]
    return pct(vals, 50), pct(vals, 99), pct(vals, 100)


def mismatch_windows(joined):
    """Contiguous mismatched-tick spans, each attributed to the carried-state field families.

    Per window: the tick range, whether it opens at the first shared tick (a spawn/connect
    transient rather than a mid-run event), how many of its surviving client rows were rollback
    replays, the per-family mismatch tally (None = trace predates the decode), and — when the
    trace is verbose — the max per-family carried-state magnitudes.
    """
    windows = []
    run = []
    for j in joined:
        if j["h_match"]:
            if run:
                windows.append(run)
                run = []
        elif run and j["tick"] != run[-1]["tick"] + 1:
            windows.append(run)
            run = [j]
        else:
            run.append(j)
    if run:
        windows.append(run)

    first_tick = joined[0]["tick"] if joined else None
    out = []
    for w in windows:
        have_decode = any(v is not None for j in w for v in j["sim_match"].values())
        tally = ({SIM_LABEL[s]: sum(1 for j in w if j["sim_match"][s] is False)
                  for s in SIM_SUBS} if have_decode else None)
        # pose/velocity families too, so a physics-diverging window is named as such
        pose_tally = {SUB_LABEL[s]: sum(1 for j in w if not j["sub_match"][s])
                      for s in SUBS}
        mags = None
        deltas = [j["simd"] for j in w if j["simd"]]
        if deltas:
            mags = {k: max((d[k] for d in deltas), default=float("nan"))
                    for k in ("servo", "reload", "recoil", "belt", "drive")}
        out.append({
            "lo": w[0]["tick"], "hi": w[-1]["tick"], "n": len(w),
            "opens_at_first_shared": w[0]["tick"] == first_tick,
            "rp_rows": sum(1 for j in w if j["rp"]),
            "sim_tally": tally, "pose_tally": pose_tally, "mags": mags,
        })
    return out


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
        "windows": mismatch_windows(joined),
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
          f"({s['n_trans']} ticks: hull contact / tracks lifting / airborne)")

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

    if s["windows"]:
        print(f"\n    MISMATCH WINDOWS ({len(s['windows'])} contiguous span(s))")
        for w in s["windows"]:
            opens = "opens@first-shared-tick" if w["opens_at_first_shared"] else "mid-run"
            print(f"      {w['lo']}..{w['hi']}  ({w['n']} ticks, {opens}, "
                  f"{w['rp_rows']} replay rows)")
            pose = "   ".join(f"{k}={v}" for k, v in w["pose_tally"].items() if v)
            print(f"        components: {pose or '(none?)'}")
            if w["sim_tally"] is not None:
                sim = "   ".join(f"{k}={v}" for k, v in w["sim_tally"].items())
                print(f"        carried-state fields: {sim}")
            else:
                print("        carried-state fields: n/a (trace predates the hsim decode)")
            if w["mags"]:
                m = w["mags"]
                print(f"        max |Δ|: servo {m['servo']:.3e}  reload {m['reload']:.3e}  "
                      f"recoil {m['recoil']:.3e}  belt {m['belt']:.3e}  "
                      f"drive {m['drive']:.3e}")

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
