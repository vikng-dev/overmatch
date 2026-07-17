# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Per-tick first-divergence anatomy of a client/server jitter-trace pair (src/trace.rs).

Where analyze.py graphs the render-jerk consequence of rollbacks, this script dissects the
*cause*: it aligns client and server FixedLast tick rows on the shared tick number (client rows
collapsed to the LAST per tick — the rollback-replayed/corrected sim value) and, for every shared
tick, computes the field-by-field client/server delta:

    |Δp|  rot(Δq)  |Δlv|  |Δav|  Δthr  Δstr  Δgnd  max|Δload|

It then reports, for the straight-flat-driving question:
  * the steady-state noise floor (median / p95 of each field over the constant-throttle cruise);
  * every "divergence onset" — the first tick |Δlv| crosses ONSET_DLV after >=SETTLE ticks below
    it — with the surrounding +/-4 ticks' full delta table and a first-mover verdict
    (thr/str first => input-timing; loads first with matched pose => belt-contact nondeterminism;
    lv first with matched thr+loads => solver; p only, growing => integration of an earlier vel
    offset);
  * the Delta-thr / input-timing verdict: count of cruise ticks where |Δthr|>1e-6 (a nonzero count
    means the deterministic TrackDrive command slew is tick-shifted between ends — input application is not
    aligned), plus the shift pattern at the ramp edges;
  * hull yaw/pitch drift on flat straight (the Δq decomposed into yaw / pitch / roll).

Usage:
    uv run scripts/jitter/anatomy.py CLIENT.jsonl SERVER.jsonl [--label NAME]
        [--cruise LO HI] [--onsets N]
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

import numpy as np

# --- thresholds -------------------------------------------------------------------------------
ONSET_DLV = 0.05    # m/s   onset trigger: quarter of the 0.20 m/s LinearVelocity rollback bar
SETTLE = 32         # ticks below ONSET_DLV required before an onset can re-arm
# "field moved" thresholds for the first-mover verdict — a field's departure from its noise floor.
MOVED = {
    "dthr": 1e-6,   # TrackDrive throttle is deterministic to the bit given aligned input
    "dstr": 1e-6,
    "dload": 100.0,  # N; per-SIDE belt support on a ~57 t tank rests ~280 kN, floor is ~single N
    "dlv": ONSET_DLV,
    "dav": 0.02,    # rad/s
    "dp": 2e-3,     # m
}


# --- quaternion helpers (layout [x, y, z, w]) -------------------------------------------------
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
    v = math.sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2])
    return 2.0 * math.atan2(v, abs(q[3]))


def q_between(a, b):
    return q_angle(q_mul(q_conj(a), b))


def q_to_ypr(q):
    """Intrinsic yaw(Y) / pitch(X) / roll(Z) of a unit quaternion [x,y,z,w], radians."""
    x, y, z, w = q
    # yaw (around Y)
    siny = 2.0 * (w * y + x * z)
    cosy = 1.0 - 2.0 * (x * x + y * y)
    yaw = math.atan2(siny, cosy)
    # pitch (around X)
    sinp = 2.0 * (w * x - y * z)
    sinp = max(-1.0, min(1.0, sinp))
    pitch = math.asin(sinp)
    # roll (around Z)
    sinr = 2.0 * (w * z + x * y)
    cosr = 1.0 - 2.0 * (x * x + z * z)
    roll = math.atan2(sinr, cosr)
    return yaw, pitch, roll


# --- parsing ----------------------------------------------------------------------------------
def _vec(x, n):
    if not isinstance(x, list) or len(x) != n:
        return None
    out = np.empty(n)
    for i, e in enumerate(x):
        if not isinstance(e, (int, float)) or not math.isfinite(e):
            return None
        out[i] = e
    return out


def parse_ticks(path):
    """Return {tick: row} keeping the LAST row per tick (rollback-replay corrected value wins),
    for the busiest (controlled) tank entity, plus the rollback list."""
    rows = []
    rollbacks = []
    meta = None
    with open(path) as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                r = json.loads(line)
            except (json.JSONDecodeError, ValueError):
                continue
            k = r.get("k")
            if k == "meta":
                meta = r
            elif k == "tick":
                p = _vec(r.get("p"), 3)
                q = _vec(r.get("q"), 4)
                if p is None or q is None:
                    continue
                rows.append({
                    "tick": r.get("tick"), "e": r.get("e"),
                    "p": p, "q": q,
                    "lv": _vec(r.get("lv"), 3), "av": _vec(r.get("av"), 3),
                    "gnd": r.get("gnd"),
                    "loads": r.get("loads") or [],
                    "thr": r.get("thr"), "str": r.get("str"),
                    "hc": r.get("hc"), "pen": r.get("pen"),
                    "ctl": bool(r.get("ctl", False)),
                })
            elif k == "rollback":
                rollbacks.append({
                    "tick": r.get("tick"), "start": r.get("start"),
                    "depth": r.get("depth"), "cause": r.get("cause"),
                    "trg": r.get("trg") or [],
                })
    # choose entity: controlled if any, else busiest
    counts, ctl = {}, None
    for r in rows:
        counts[r["e"]] = counts.get(r["e"], 0) + 1
        if r["ctl"]:
            ctl = r["e"]
    if not counts:
        return meta, {}, rollbacks
    ent = ctl if ctl is not None else max(counts, key=counts.get)
    by_tick = {}
    for r in rows:
        if r["e"] == ent:
            by_tick[r["tick"]] = r          # last wins
    return meta, by_tick, rollbacks


# --- delta series -----------------------------------------------------------------------------
def deltas(cli, srv):
    """Per-shared-tick delta dict of aligned arrays, ordered by tick."""
    shared = sorted(set(cli) & set(srv))
    out = {kk: [] for kk in ("tick", "dp", "dq", "dlv", "dav", "dthr", "dstr", "dgnd", "dload")}
    for tk in shared:
        c, s = cli[tk], srv[tk]
        out["tick"].append(tk)
        out["dp"].append(float(np.linalg.norm(c["p"] - s["p"])))
        out["dq"].append(q_between(c["q"], s["q"]))
        out["dlv"].append(float(np.linalg.norm(c["lv"] - s["lv"])) if c["lv"] is not None and s["lv"] is not None else float("nan"))
        out["dav"].append(float(np.linalg.norm(c["av"] - s["av"])) if c["av"] is not None and s["av"] is not None else float("nan"))
        out["dthr"].append(abs((c["thr"] or 0.0) - (s["thr"] or 0.0)))
        out["dstr"].append(abs((c["str"] or 0.0) - (s["str"] or 0.0)))
        out["dgnd"].append(abs((c["gnd"] or 0) - (s["gnd"] or 0)))
        cl, sl = c["loads"], s["loads"]
        if cl and sl and len(cl) == len(sl):
            out["dload"].append(max(abs(a - b) for a, b in zip(cl, sl)))
        else:
            out["dload"].append(float("nan"))
    for kk in out:
        out[kk] = np.array(out[kk], dtype=float)
    return out


def pct(a, q):
    a = a[~np.isnan(a)]
    return float(np.percentile(a, q)) if len(a) else float("nan")


def med(a):
    return pct(a, 50)


# --- report -----------------------------------------------------------------------------------
FIELDS = [("dp", "|Δp| m"), ("dq", "rot(Δq) rad"), ("dlv", "|Δlv| m/s"),
          ("dav", "|Δav| rad/s"), ("dthr", "Δthr"), ("dstr", "Δstr"),
          ("dgnd", "Δgnd"), ("dload", "max|Δload| N")]


def cruise_mask(d, lo, hi):
    t = d["tick"]
    return (t >= lo) & (t <= hi)


def steady_state(d, lo, hi):
    m = cruise_mask(d, lo, hi)
    print(f"\n  NOISE FLOOR — steady-state deltas over cruise ticks [{lo}, {hi}]  (n={int(m.sum())})")
    print(f"    {'field':<16}{'median':>14}{'p95':>14}{'max':>14}")
    for key, lab in FIELDS:
        a = d[key][m]
        print(f"    {lab:<16}{med(a):>14.6g}{pct(a,95):>14.6g}{pct(a,100):>14.6g}")


def find_onsets(d):
    """Ticks where |Δlv| first exceeds ONSET_DLV after >=SETTLE consecutive ticks below it."""
    dlv = d["dlv"]
    onsets = []
    below = SETTLE  # start armed
    for i in range(len(dlv)):
        v = dlv[i]
        if np.isnan(v):
            continue
        if v > ONSET_DLV:
            if below >= SETTLE:
                onsets.append(i)
            below = 0
        else:
            below += 1
    return onsets


def first_mover(d, i):
    """Within +/-4 ticks of onset index i, the field that first departs its noise floor."""
    lo = max(0, i - 4)
    hi = min(len(d["tick"]), i + 5)
    first = {}
    for key in ("dthr", "dstr", "dload", "dav", "dp", "dlv"):
        thr = MOVED[key]
        idx = None
        for j in range(lo, hi):
            v = d[key][j]
            if not np.isnan(v) and v > thr:
                idx = j
                break
        if idx is not None:
            first[key] = idx
    if not first:
        return "none", first
    winner = min(first, key=lambda k: (first[k], k))
    return winner, first


def verdict_for(winner):
    return {
        "dthr": "INPUT-TIMING (throttle shifted between ends)",
        "dstr": "INPUT-TIMING (steer shifted between ends)",
        "dload": "BELT-CONTACT nondeterminism (support loads diverge first, pose still matched)",
        "dav": "SOLVER (angular velocity diverges first)",
        "dlv": "SOLVER (linear velocity diverges with matched thr+loads)",
        "dp": "INTEGRATION (position drifts from an earlier velocity offset)",
        "none": "sub-threshold (no field crossed its floor in-window)",
    }.get(winner, winner)


def print_onset_table(d, i, drv_lo=None, drv_hi=None):
    lo = max(0, i - 4)
    hi = min(len(d["tick"]), i + 5)
    onset_tick = int(d["tick"][i])
    winner, first = first_mover(d, i)
    where = ""
    if drv_lo is not None:
        where = "  [IN DRIVE]" if drv_lo <= onset_tick <= drv_hi else "  [PRE-DRIVE SETTLE]"
    print(f"\n  DIVERGENCE ONSET @ tick {onset_tick}{where}  (|Δlv|={d['dlv'][i]:.4f} m/s)")
    hdr = f"    {'tick':>7}{'|Δp|':>11}{'rot Δq':>11}{'|Δlv|':>11}{'|Δav|':>11}{'Δthr':>10}{'Δstr':>10}{'Δgnd':>7}{'max|Δload|':>13}"
    print(hdr)
    for j in range(lo, hi):
        mark = "  <==" if j == i else ""
        print(f"    {int(d['tick'][j]):>7}{d['dp'][j]:>11.5f}{d['dq'][j]:>11.5f}"
              f"{d['dlv'][j]:>11.5f}{d['dav'][j]:>11.5f}{d['dthr'][j]:>10.5f}"
              f"{d['dstr'][j]:>10.5f}{int(d['dgnd'][j]):>7}{d['dload'][j]:>13.2f}{mark}")
    fm = ", ".join(f"{k}@{int(d['tick'][first[k]])}" for k in sorted(first, key=lambda k: first[k]))
    print(f"    first to move: [{fm}]")
    print(f"    VERDICT: {verdict_for(winner)}")


def dthr_anatomy(d, cli, srv, ramp_edges):
    """Count cruise ticks where |Δthr|>1e-6; show the shift pattern around each ramp edge."""
    print("\n  Δthr / INPUT-TIMING anatomy")
    nz = int(np.sum(d["dthr"] > 1e-6))
    print(f"    ticks with |Δthr|>1e-6 over ALL shared ticks: {nz} / {len(d['tick'])}")
    for edge in ramp_edges:
        print(f"    around ramp edge tick {edge}:")
        print(f"      {'tick':>7}{'cli thr':>12}{'srv thr':>12}{'Δthr':>12}")
        for tk in range(edge - 4, edge + 5):
            c, s = cli.get(tk), srv.get(tk)
            if c is None or s is None:
                continue
            ct, st = c["thr"] or 0.0, s["thr"] or 0.0
            print(f"      {tk:>7}{ct:>12.5f}{st:>12.5f}{ct - st:>12.5f}")


def yaw_drift(d, cli, srv, lo, hi):
    """Hull yaw/pitch/roll drift between ends over the cruise."""
    print(f"\n  HULL ORIENTATION DRIFT — yaw/pitch/roll |client−server| over cruise [{lo},{hi}]")
    dy, dpi, dr = [], [], []
    for tk in d["tick"]:
        tk = int(tk)
        if tk < lo or tk > hi:
            continue
        c, s = cli.get(tk), srv.get(tk)
        if c is None or s is None:
            continue
        cy, cp, cr = q_to_ypr(c["q"])
        sy, sp, sr = q_to_ypr(s["q"])
        dy.append(abs(cy - sy))
        dpi.append(abs(cp - sp))
        dr.append(abs(cr - sr))
    if not dy:
        print("    (no cruise ticks)")
        return
    for lab, a in (("yaw", dy), ("pitch", dpi), ("roll", dr)):
        a = np.array(a)
        print(f"    {lab:<6} median {med(a):.6g} rad  p95 {pct(a,95):.6g} rad  max {pct(a,100):.6g} rad")


def rollbacks_report(rollbacks):
    print(f"\n  ROLLBACKS (client): count {len(rollbacks)}")
    if not rollbacks:
        return
    causes, comp = {}, {}
    depths = []
    for r in rollbacks:
        causes[r["cause"]] = causes.get(r["cause"], 0) + 1
        if isinstance(r["depth"], (int, float)):
            depths.append(r["depth"])
        for pair in r["trg"]:
            if len(pair) >= 2:
                comp.setdefault(pair[0], []).append(pair[1])
    print("    causes: " + ", ".join(f"{c}={n}" for c, n in sorted(causes.items())))
    if depths:
        print(f"    depth ticks: mean {np.mean(depths):.1f}  max {int(max(depths))}")
    for c in sorted(comp, key=lambda c: -len(comp[c])):
        print(f"    trigger {c:<16} count {len(comp[c]):>4}  median mag {np.median(comp[c]):.4f}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("client")
    ap.add_argument("server")
    ap.add_argument("--label", default=None)
    ap.add_argument("--cruise", nargs=2, type=int, default=None,
                    help="cruise window [lo hi] for the noise floor (default: auto from server thr)")
    ap.add_argument("--edges", nargs="*", type=int, default=None,
                    help="ramp-edge ticks for the Δthr shift check (default: auto from server thr)")
    ap.add_argument("--onsets", type=int, default=6, help="max onset tables to print")
    args = ap.parse_args()

    cmeta, cli, rollbacks = parse_ticks(Path(args.client))
    smeta, srv, _ = parse_ticks(Path(args.server))
    d = deltas(cli, srv)
    label = args.label or Path(args.client).stem

    # The trace `tick` is the lightyear network tick (offset per run, not the scripted counter), so
    # the driving window is discovered from the server's own throttle rather than hardcoded.
    drv = sorted(tk for tk, r in srv.items() if abs(r["thr"] or 0.0) > 0.5)
    if drv:
        drv_lo, drv_hi = drv[0], drv[-1]
    else:
        drv_lo, drv_hi = (int(d["tick"][0]), int(d["tick"][-1])) if len(d["tick"]) else (0, 0)
    # Cruise: inside the drive window, past the ~1 s throttle ramp-up and before the ramp-down.
    lo, hi = args.cruise if args.cruise else (drv_lo + 64, drv_hi - 16)
    edges = args.edges if args.edges is not None else [drv_lo, drv_hi]

    line = "=" * 92
    print(line)
    print(f"  FIRST-DIVERGENCE ANATOMY — {label}")
    print(line)
    print(f"  client ticks {len(cli)}  server ticks {len(srv)}  shared {len(d['tick'])}")
    print(f"  driving window (server thr!=0): ticks [{drv_lo}, {drv_hi}]   cruise [{lo}, {hi}]")

    steady_state(d, lo, hi)
    rollbacks_report(rollbacks)
    dthr_anatomy(d, cli, srv, edges)
    yaw_drift(d, cli, srv, lo, hi)

    onsets = find_onsets(d)
    in_drive = [i for i in onsets if drv_lo <= int(d["tick"][i]) <= drv_hi]
    print(f"\n  DIVERGENCE ONSETS (|Δlv| first > {ONSET_DLV} after >={SETTLE} ticks below): "
          f"{len(onsets)} total, {len(in_drive)} inside the drive window")
    if len(d["tick"]):
        span = (drv_hi - drv_lo) / 64.0
        print(f"    in-drive onset cadence: {len(in_drive)/span:.2f} / s over {span:.1f} s of driving"
              if span > 0 else "")
    # Prefer in-drive onsets (the ordinary-driving question); fall back to all.
    show = in_drive[:args.onsets] if in_drive else onsets[:args.onsets]
    for i in show:
        print_onset_table(d, i, drv_lo, drv_hi)
    print(line)


if __name__ == "__main__":
    sys.exit(main())
