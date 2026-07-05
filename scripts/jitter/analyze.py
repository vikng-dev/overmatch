# /// script
# requires-python = ">=3.11"
# dependencies = ["matplotlib"]
# ///
"""Diagnose MP hull jitter from jitter-trace JSONL (src/trace.rs).

Consumes the per-frame / per-tick / rollback rows the game's `SPIKE_TRACE`
recorder writes and produces (a) a stdout summary of quantitative jitter
metrics and (b) a multi-panel PNG timeline. The jitter it hunts is visual
hull snapping caused by the client-prediction stack: a rollback correction
that steps the rendered pose several centimetres inside a single ~8 ms frame,
producing a render-jerk (acceleration discontinuity) orders of magnitude above
what a real ~2 g tank hull can do. SP traces (the smooth baseline) carry none
of the net extras and analyse cleanly with the net-specific sections skipped.

Rollback REPLAY re-records ticks: a client that rolls back re-simulates already
recorded tick numbers and emits fresh `tick` rows stamped `rp=true` (the
corrected value). Where several rows share (tick, entity) we keep the LAST —
i.e. the replayed/corrected sim state, not the abandoned misprediction — so
divergence and context reflect what the authority replay settled on.

Usage:
    uv run scripts/jitter/analyze.py CLIENT.jsonl [--server SERVER.jsonl]
        [--out OUT.png] [--spike-threshold M_PER_S2] [--top N]
"""

from __future__ import annotations

import argparse
import bisect
import json
import math
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")  # headless: we only ever write a PNG
import matplotlib.pyplot as plt
import numpy as np

# --- Fixed palette (project dataviz method — non-negotiable colour assignment) ----------------
C_TRANSL = "#2a78d6"   # translational render jerk / divergence |Δp|
C_VERT = "#1baf7a"     # vertical-only jerk / divergence |Δlv|
C_ROT = "#4a3aa7"      # rotational jerk / divergence rot diff
C_CP = "#eb6834"       # correction |cp| (m)
C_CQ = "#e87ba4"       # correction cq angle
C_GND = "#008300"      # grounded-wheel count
C_HC = "#e34948"       # hull contacts
C_THR = "#52514e"      # throttle
C_SPIKE = "#d03b3b"    # spike dots + rollback event lines
C_INK = "#0b0b0b"      # primary text
C_INK2 = "#52514e"     # secondary text
C_SURFACE = "#fcfcfb"  # figure + axes background
C_GRID = "#d9d8d5"     # recessive grid

ROT_SPIKE_THRESHOLD = 20.0  # rad/s^2
DIV_P = 0.05   # m   rollback position threshold
DIV_Q = 0.05   # rad rollback rotation threshold
DIV_LV = 0.20  # m/s rollback velocity threshold


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
    """Shortest-arc rotation magnitude of a unit quaternion, in [0, pi] radians."""
    v = math.sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2])
    return 2.0 * math.atan2(v, abs(q[3]))


def q_between(a, b):
    """Shortest-arc angle between two orientations a -> b (radians)."""
    return q_angle(q_mul(q_conj(a), b))


# --- parsing ----------------------------------------------------------------------------------
def _finite_vec(x, n):
    """Return an np array of length n iff every element is a finite number, else None."""
    if not isinstance(x, list) or len(x) != n:
        return None
    out = np.empty(n)
    for i, e in enumerate(x):
        if not isinstance(e, (int, float)) or e is None or not math.isfinite(e):
            return None
        out[i] = e
    return out


def parse(path):
    """Parse a JSONL trace. Returns (meta, frames, ticks, rollbacks, stats).

    Unparseable lines and rows carrying a null/NaN pose are skipped and counted
    (a corrupt f32 serialises as JSON null; a hard-killed process tears its last
    line) — the parser never crashes on either. Tick rows are collapsed by
    (tick, entity) keeping the LAST occurrence, so a client's rollback-replay
    rows (rp=true) supersede the original misprediction for the same tick.
    """
    meta = None
    frames, ticks, rollbacks = [], [], []
    bad_lines = 0
    null_poses = 0
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
            elif k == "frame":
                p = _finite_vec(row.get("p"), 3)
                q = _finite_vec(row.get("q"), 4)
                t = row.get("t")
                dt = row.get("dt")
                if p is None or q is None or not isinstance(t, (int, float)) or not isinstance(dt, (int, float)):
                    null_poses += 1
                    continue
                frames.append({
                    "t": float(t), "dt": float(dt), "e": row.get("e"),
                    "p": p, "q": q, "ctl": bool(row.get("ctl", False)),
                    "tick": row.get("tick"),
                    "cp": _finite_vec(row.get("cp"), 3) if "cp" in row else None,
                    "cq": _finite_vec(row.get("cq"), 4) if "cq" in row else None,
                    "has_corr": ("cp" in row) or ("cq" in row),
                    # Per-entity confirmed authority (src/trace.rs, added for silent-desync hunt):
                    # latest confirmed Position/LinearVelocity and the tick they belong to. Absent
                    # in pre-instrumentation traces — parsed as None so the detector degrades to n/a.
                    "confp": _finite_vec(row.get("confp"), 3) if "confp" in row else None,
                    "confv": _finite_vec(row.get("confv"), 3) if "confv" in row else None,
                    "conft": row.get("conft"),
                })
            elif k == "tick":
                p = _finite_vec(row.get("p"), 3)
                q = _finite_vec(row.get("q"), 4)
                lv = _finite_vec(row.get("lv"), 3)
                if p is None or q is None:
                    null_poses += 1
                    continue
                ticks.append({
                    "tick": row.get("tick"), "e": row.get("e"), "p": p, "q": q,
                    "lv": lv, "gnd": row.get("gnd"), "hc": row.get("hc"),
                    "pen": row.get("pen"), "thr": row.get("thr"),
                    "ctl": bool(row.get("ctl", False)),
                    "rp": bool(row.get("rp", False)),
                })
            elif k == "rollback":
                rollbacks.append({
                    "t": row.get("t"), "tick": row.get("tick"),
                    "start": row.get("start"), "depth": row.get("depth"),
                    "cause": row.get("cause"), "trg": row.get("trg") or [],
                })
    n_raw_ticks = len(ticks)
    ticks = _dedupe_ticks(ticks)
    stats = {"bad_lines": bad_lines, "null_poses": null_poses,
             "replay_ticks": n_raw_ticks - len(ticks)}
    return meta, frames, ticks, rollbacks, stats


def _dedupe_ticks(ticks):
    """Collapse rows sharing (tick, entity) to the LAST occurrence.

    A client that rolls back re-records already-seen tick numbers during replay
    (rp=true); the replayed row is the corrected sim state, so it wins. Original,
    server, and SP traces have unique (tick, e) and pass through unchanged. Dict
    insertion order is preserved (first-seen position, last-seen value).
    """
    by_key = {}
    for t in ticks:
        by_key[(t["tick"], t["e"])] = t
    return list(by_key.values())


def choose_entity(frames):
    """Pick the tank to analyse: prefer the controlled one, else the busiest.

    Returns (entity_bits, chosen_frames, n_other_entities).
    """
    counts = {}
    ctl_ent = None
    for f in frames:
        counts[f["e"]] = counts.get(f["e"], 0) + 1
        if f["ctl"]:
            ctl_ent = f["e"]
    if not counts:
        return None, [], 0
    ent = ctl_ent if ctl_ent is not None else max(counts, key=counts.get)
    chosen = [f for f in frames if f["e"] == ent]
    return ent, chosen, len(counts) - 1


# --- percentile helper ------------------------------------------------------------------------
def pct(arr, q):
    if len(arr) == 0:
        return float("nan")
    return float(np.percentile(arr, q))


# --- frame-derived metrics --------------------------------------------------------------------
def frame_metrics(frames):
    """Render-jerk and rotational-jerk series over strictly-increasing valid frames.

    Guards dt<=0 and duplicated/backwards wall-time. Returns a dict of aligned
    arrays; each jerk sample sits at the *later* frame of the pair that produced
    it (index k in the accepted-frame list, k>=2).
    """
    # accepted frames: strictly increasing t and positive dt
    acc = []
    prev_t = -math.inf
    for f in frames:
        if f["dt"] <= 0.0 or f["t"] <= prev_t:
            continue
        acc.append(f)
        prev_t = f["t"]

    n = len(acc)
    # velocity + angular velocity per accepted pair (defined for k>=1)
    v = [None] * n
    w = [None] * n
    for k in range(1, n):
        dt = acc[k]["dt"]
        v[k] = (acc[k]["p"] - acc[k - 1]["p"]) / dt
        w[k] = q_between(acc[k - 1]["q"], acc[k]["q"]) / dt

    # jerk (discontinuity) per accepted triple (defined for k>=2)
    t_j, a_transl, a_vert, a_horiz, a_rot = [], [], [], [], []
    j_frame_idx = []
    for k in range(2, n):
        dt = acc[k]["dt"]
        dv = v[k] - v[k - 1]
        t_j.append(acc[k]["t"])
        a_transl.append(np.linalg.norm(dv) / dt)
        a_vert.append(abs(dv[1]) / dt)
        a_horiz.append(math.hypot(dv[0], dv[2]) / dt)
        a_rot.append(abs(w[k] - w[k - 1]) / dt)
        j_frame_idx.append(k)

    return {
        "acc": acc,
        "t": np.array([acc[k]["t"] for k in range(n)]),
        "py": np.array([acc[k]["p"][1] for k in range(n)]),
        "dt": np.array([acc[k]["dt"] for k in range(n)]),
        "t_j": np.array(t_j),
        "a_transl": np.array(a_transl),
        "a_vert": np.array(a_vert),
        "a_horiz": np.array(a_horiz),
        "a_rot": np.array(a_rot),
        "j_frame_idx": j_frame_idx,
    }


def correction_series(chosen):
    """Per-frame correction magnitude series (all valid frames of chosen entity)."""
    t, cp_mag, cq_ang, active = [], [], [], []
    for f in chosen:
        t.append(f["t"])
        cp = float(np.linalg.norm(f["cp"])) if f["cp"] is not None else 0.0
        cq = q_angle(f["cq"]) if f["cq"] is not None else 0.0
        cp_mag.append(cp)
        cq_ang.append(cq)
        active.append(1 if f["has_corr"] else 0)
    return {
        "t": np.array(t), "cp": np.array(cp_mag),
        "cq": np.array(cq_ang), "active": np.array(active),
    }


def tick_to_wall(frames):
    """Interpolator client-tick -> wall seconds, from frame (tick, t) pairs.

    Frames carry both a predicted tick and a wall time; averaging t per tick
    gives a monotone map used to place tick-indexed series on the shared x axis
    and to project server ticks into client wall time.
    """
    by_tick = {}
    for f in frames:
        tk = f["tick"]
        if tk is None:
            continue
        by_tick.setdefault(tk, []).append(f["t"])
    if not by_tick:
        return None
    xs = np.array(sorted(by_tick))
    ys = np.array([float(np.mean(by_tick[tk])) for tk in xs])
    lo, hi = xs[0], xs[-1]

    def f(tick_query):
        tq = np.asarray(tick_query, dtype=float)
        out = np.interp(tq, xs, ys)
        out = np.where((tq < lo) | (tq > hi), np.nan, out)
        return out

    f.lo, f.hi = lo, hi
    return f


# --- report helpers ---------------------------------------------------------------------------
def fmt(x, w=10, p=3):
    if x is None or (isinstance(x, float) and math.isnan(x)):
        return f"{'-':>{w}}"
    return f"{x:>{w}.{p}f}"


def main():
    ap = argparse.ArgumentParser(description="Diagnose MP hull jitter from jitter-trace JSONL.")
    ap.add_argument("client", help="client (or SP) JSONL trace")
    ap.add_argument("--server", help="server JSONL trace (enables divergence analysis)")
    ap.add_argument("--out", help="output PNG (default: <input-stem>.png alongside input)")
    ap.add_argument("--spike-threshold", type=float, default=50.0,
                    help="translational render-jerk spike threshold, m/s^2 (default 50)")
    ap.add_argument("--top", type=int, default=12, help="rows in the TOP-N spikes table (default 12)")
    args = ap.parse_args()

    in_path = Path(args.client)
    meta, frames, ticks, rollbacks, pstats = parse(in_path)
    role = (meta or {}).get("role", "unknown")
    is_net = role in ("client",)  # SP has no net extras; server has no frame rows

    if not frames:
        print(f"error: no usable frame rows in {in_path} (role={role}); nothing to analyse.",
              file=sys.stderr)
        # a server-only file has no frames — that's a misuse of the positional arg
        return 2

    ent, chosen, n_other = choose_entity(frames)
    chosen_ticks = [t for t in ticks if t["e"] == ent]

    fm = frame_metrics(chosen)
    cs = correction_series(chosen)
    t2w = tick_to_wall(chosen)

    # --- server load + divergence -------------------------------------------------------------
    server_meta = server_ticks = None
    div = None
    if args.server:
        smeta, _sf, sticks, _sr, sstats = parse(Path(args.server))
        server_meta = smeta
        pstats["server_bad_lines"] = sstats["bad_lines"]
        pstats["server_null_poses"] = sstats["null_poses"]
        # server entity: the busiest tick entity
        scounts = {}
        for t in sticks:
            scounts[t["e"]] = scounts.get(t["e"], 0) + 1
        if scounts:
            sent = max(scounts, key=scounts.get)
            server_ticks = [t for t in sticks if t["e"] == sent]
            div = divergence(chosen_ticks, server_ticks)

    # --- spike detection + table --------------------------------------------------------------
    spikes = collect_spikes(fm, chosen, args.spike_threshold)
    top_rows = spike_table(spikes, rollbacks, chosen_ticks, args.top)

    # --- stdout report ------------------------------------------------------------------------
    print_report(in_path, meta, role, is_net, ent, n_other, chosen, chosen_ticks,
                 fm, cs, rollbacks, spikes, div, server_meta, server_ticks,
                 pstats, args, top_rows)

    # --- PNG ----------------------------------------------------------------------------------
    out_path = Path(args.out) if args.out else in_path.with_suffix(".png")
    build_png(out_path, in_path, role, is_net, fm, cs, chosen_ticks, rollbacks,
              spikes, div, server_ticks, t2w, args)
    print(f"\nPNG written: {out_path}")
    return 0


def divergence(client_ticks, server_ticks):
    """Per-shared-tick client/server sim divergence."""
    cmap = {t["tick"]: t for t in client_ticks}
    smap = {t["tick"]: t for t in server_ticks}
    shared = sorted(set(cmap) & set(smap))
    if not shared:
        return {"shared": np.array([]), "dp": np.array([]), "dq": np.array([]),
                "dlv": np.array([]), "n": 0}
    dp, dq, dlv = [], [], []
    for tk in shared:
        c, s = cmap[tk], smap[tk]
        dp.append(float(np.linalg.norm(c["p"] - s["p"])))
        dq.append(q_between(c["q"], s["q"]))
        if c["lv"] is not None and s["lv"] is not None:
            dlv.append(float(np.linalg.norm(c["lv"] - s["lv"])))
        else:
            dlv.append(float("nan"))
    return {"shared": np.array(shared), "dp": np.array(dp), "dq": np.array(dq),
            "dlv": np.array(dlv), "n": len(shared)}


SILENT_MIN_DUR_S = 0.5  # a sustained-desync window must last at least this long to count


def silent_desync_windows(div, chosen_ticks, server_ticks, chosen_frames, rollbacks, tick_hz):
    """Maximal windows of sustained same-tick client/server position desync with NO rollback.

    A window is a run of consecutive shared ticks where |client p − server p| > DIV_P,
    containing no rollback (a rollback tick at either endpoint or in an internal gap
    terminates the run), lasting at least SILENT_MIN_DUR_S. Each is the fingerprint of a
    *silent* desync: the position rollback condition (DIV_P m in protocol.rs) sat tripped
    for the whole window yet no rollback fired.

    Per window we discriminate the three failure branches using the client's own confirmed
    fields (confp/conft), degrading every conf-derived stat to None when the trace predates
    that instrumentation (so pre-fields captures still report the window from tick data):
      - `conft` advance   — stalled (updates stopped arriving) vs moving.
      - median |confp − server_p(conft)| — is the confirmed value the server's actual value
        at that tick (staleness / quantization check).
      - median |confp − client_p(conft)| — what the rollback check compared; > DIV_P here
        with no rollback means the check saw a mismatch and declined to fire.
    """
    shared = [int(t) for t in div["shared"]]
    dp = list(div["dp"])
    if not shared:
        return []
    period = 1.0 / tick_hz if tick_hz else 1.0 / 64.0
    rb_sorted = sorted(int(r["tick"]) for r in rollbacks if r["tick"] is not None)
    rb_set = set(rb_sorted)

    def rollback_in_open(a, b):
        """Any rollback tick strictly inside the open interval (a, b)."""
        idx = bisect.bisect_right(rb_sorted, a)
        return idx < len(rb_sorted) and rb_sorted[idx] < b

    # A shared tick is eligible if it is over threshold and is not itself a rollback tick.
    over = [dp[i] > DIV_P and shared[i] not in rb_set for i in range(len(shared))]

    cmap = {t["tick"]: t for t in chosen_ticks}
    smap = {t["tick"]: t for t in server_ticks}
    # Client frame rows indexed by predicted tick, carrying the confirmed fields; a tick can
    # own several frames (render > tick rate), so keep them all and aggregate per window.
    frames_by_tick = {}
    for f in chosen_frames:
        if f["tick"] is not None:
            frames_by_tick.setdefault(f["tick"], []).append(f)

    windows = []
    i, n = 0, len(shared)
    while i < n:
        if not over[i]:
            i += 1
            continue
        j = i
        while j + 1 < n and over[j + 1] and not rollback_in_open(shared[j], shared[j + 1]):
            j += 1
        lo_tick, hi_tick = shared[i], shared[j]
        dur = (hi_tick - lo_tick + 1) * period
        if dur >= SILENT_MIN_DUR_S:
            windows.append(_build_silent_window(
                lo_tick, hi_tick, dur, shared, dp, i, j,
                frames_by_tick, cmap, smap))
        i = j + 1
    return windows


def _build_silent_window(lo_tick, hi_tick, dur, shared, dp, i, j,
                         frames_by_tick, cmap, smap):
    """Assemble one silent-desync window's report record (see silent_desync_windows)."""
    max_dp = max(dp[i:j + 1])

    # Confirmed-field stats from the client frame rows whose predicted tick lands in the window.
    conft_vals = []
    confp_vs_server = []   # |confp − server_p(conft)| : is confirmed == server's value there?
    confp_vs_client = []   # |confp − client_p(conft)| : what the rollback check compared
    have_conf = False
    for tk in range(lo_tick, hi_tick + 1):
        for f in frames_by_tick.get(tk, []):
            ct = f["conft"]
            if ct is None:
                continue
            have_conf = True
            conft_vals.append(int(ct))
            cp = f["confp"]
            if cp is None:
                continue
            srow = smap.get(int(ct))
            if srow is not None and srow["p"] is not None:
                confp_vs_server.append(float(np.linalg.norm(cp - srow["p"])))
            crow = cmap.get(int(ct))
            if crow is not None and crow["p"] is not None:
                confp_vs_client.append(float(np.linalg.norm(cp - crow["p"])))

    conft_min = min(conft_vals) if conft_vals else None
    conft_max = max(conft_vals) if conft_vals else None
    med_server = float(np.median(confp_vs_server)) if confp_vs_server else None
    med_client = float(np.median(confp_vs_client)) if confp_vs_client else None

    # Verdict — the three branches from the task brief.
    if not have_conf:
        verdict = "no conf fields (tick-only) — cannot discriminate branch"
    elif conft_min is not None and conft_max == conft_min:
        verdict = "updates stalled"
    elif med_client is not None and med_client > DIV_P:
        verdict = "mismatch ignored by check"
    else:
        verdict = "confirmed matches client history (server-side or quantization issue)"

    return {
        "lo": lo_tick, "hi": hi_tick, "dur": dur, "max_dp": max_dp,
        "conft_min": conft_min, "conft_max": conft_max,
        "conft_advanced": (conft_max - conft_min) if conft_vals else None,
        "med_server": med_server, "med_client": med_client,
        "have_conf": have_conf, "verdict": verdict,
    }


def print_silent_desync(windows):
    """The SILENT DESYNC WINDOWS report section (only reached when --server was given)."""
    print("\n  SILENT DESYNC WINDOWS (|Δp|>%.2f m sustained >=%.1f s, NO rollback inside)"
          % (DIV_P, SILENT_MIN_DUR_S))
    if not windows:
        print("    (none — no sustained rollback-free position desync)")
        return
    print(f"    found {len(windows)} window(s):")
    for w in windows:
        print(f"      ticks {w['lo']}–{w['hi']}  dur {w['dur']:.2f} s  "
              f"max|Δp| {w['max_dp']:.3f} m")
        if not w["have_conf"]:
            conft_s = "conft n/a"
            server_s = "|confp−server| n/a"
            client_s = "|confp−client| n/a"
        else:
            adv = w["conft_advanced"]
            conft_s = (f"conft {w['conft_min']}→{w['conft_max']} "
                       f"({'stalled' if adv == 0 else f'+{adv} ticks'})")
            server_s = ("|confp−server_p(conft)| median "
                        + ("n/a" if w["med_server"] is None else f"{w['med_server']:.3f} m"))
            client_s = ("|confp−client_p(conft)| median "
                        + ("n/a" if w["med_client"] is None else f"{w['med_client']:.3f} m"))
        print(f"        {conft_s}")
        print(f"        {server_s}")
        print(f"        {client_s}")
        print(f"        VERDICT: {w['verdict']}")


def collect_spikes(fm, chosen, thr):
    """Translational + rotational render-jerk spikes as a merged, magnitude-sorted list."""
    out = []
    for i, tj in enumerate(fm["t_j"]):
        fidx = fm["j_frame_idx"][i]
        f = fm["acc"][fidx]
        if fm["a_transl"][i] > thr:
            out.append({"t": float(tj), "mag": float(fm["a_transl"][i]),
                        "axis": "transl", "tick": f["tick"], "corr": f["has_corr"]})
        if fm["a_rot"][i] > ROT_SPIKE_THRESHOLD:
            out.append({"t": float(tj), "mag": float(fm["a_rot"][i]),
                        "axis": "rot", "tick": f["tick"], "corr": f["has_corr"]})
    out.sort(key=lambda s: s["mag"], reverse=True)
    return out


def _top_trigger(trg):
    if not trg:
        return None
    best = max(trg, key=lambda pair: pair[1] if len(pair) > 1 else 0.0)
    return f"{best[0]} {best[1]:.3f}"


def spike_table(spikes, rollbacks, chosen_ticks, top_n):
    """Enrich the top-N spikes with the rollback / correction / sim context that coincided."""
    tickmap = {t["tick"]: t for t in chosen_ticks}
    tick_nums = np.array(sorted(t["tick"] for t in chosen_ticks if t["tick"] is not None)) \
        if chosen_ticks else np.array([])
    rb_t = np.array([r["t"] for r in rollbacks if r["t"] is not None])
    rows = []
    for sp in spikes[:top_n]:
        # nearest rollback within +/- 0.15 s
        rb_str = "-"
        if len(rb_t):
            j = int(np.argmin(np.abs(rb_t - sp["t"])))
            if abs(rb_t[j] - sp["t"]) <= 0.15:
                r = rollbacks[j]
                rb_str = f"Δt={rb_t[j] - sp['t']:+.3f} {r['cause']} [{_top_trigger(r['trg']) or '-'}]"
        # sim context: the tick row for this spike frame (nearest tick number)
        gnd = hc = pen = None
        tk = sp["tick"]
        row = tickmap.get(tk)
        if row is None and len(tick_nums) and tk is not None:
            row = tickmap[int(tick_nums[int(np.argmin(np.abs(tick_nums - tk)))])]
        if row is not None:
            gnd, hc, pen = row.get("gnd"), row.get("hc"), row.get("pen")
        rows.append({"t": sp["t"], "mag": sp["mag"], "axis": sp["axis"],
                     "rb": rb_str, "corr": sp["corr"], "gnd": gnd, "hc": hc, "pen": pen})
    return rows


# --- report -----------------------------------------------------------------------------------
def print_report(in_path, meta, role, is_net, ent, n_other, chosen, chosen_ticks,
                 fm, cs, rollbacks, spikes, div, server_meta, server_ticks,
                 pstats, args, top_rows):
    line = "=" * 78
    print(line)
    print(f"  MP HULL JITTER ANALYSIS  —  {in_path.name}")
    print(line)
    tick_hz = (meta or {}).get("tick_hz", "?")
    ver = (meta or {}).get("ver", "?")
    print(f"  role={role}   tick_hz={tick_hz}   ver={ver}")
    if not is_net:
        if role == "sp":
            print("  NOTE: SP baseline trace — net-specific sections "
                  "(rollback / correction / divergence) skipped.")
        else:
            print(f"  NOTE: role '{role}' carries no net extras — net sections skipped.")
    print(f"  chosen entity: {ent}   ({n_other} other tank(s) ignored)")
    print(f"  frame rows (chosen): {len(chosen)}   tick rows (chosen): {len(chosen_ticks)}")
    print(f"  skipped: {pstats['bad_lines']} unparseable line(s), "
          f"{pstats['null_poses']} null/NaN pose row(s)")
    if pstats.get("replay_ticks"):
        print(f"  collapsed: {pstats['replay_ticks']} rollback-replay tick row(s) "
              f"(kept last per tick,entity)")
    if "server_bad_lines" in pstats:
        print(f"  server skipped: {pstats['server_bad_lines']} unparseable, "
              f"{pstats['server_null_poses']} null pose")

    # timing
    t = fm["t"]
    if len(t) >= 2:
        dur = t[-1] - t[0]
        dt = fm["dt"][1:]
        fps_mean = 1.0 / np.mean(dt) if np.mean(dt) > 0 else float("nan")
        print(f"\n  DURATION {dur:8.2f} s   frames {len(t)}   "
              f"fps mean {fps_mean:5.1f}   dt p50 {pct(dt,50)*1000:5.2f} ms "
              f"p99 {pct(dt,99)*1000:5.2f} ms")

    # render jerk
    at, ar = fm["a_transl"], fm["a_rot"]
    n_tspike = sum(1 for s in spikes if s["axis"] == "transl")
    n_rspike = sum(1 for s in spikes if s["axis"] == "rot")
    minutes = (t[-1] - t[0]) / 60.0 if len(t) >= 2 else float("nan")
    print("\n  RENDER JERK (acceleration discontinuity — the jitter signal)")
    print(f"    {'metric':<14}{'p50':>10}{'p90':>10}{'p99':>10}{'max':>12}"
          f"{'spikes':>9}{'/min':>9}")
    print(f"    {'transl m/s^2':<14}{fmt(pct(at,50))}{fmt(pct(at,90))}{fmt(pct(at,99))}"
          f"{fmt(pct(at,100),12)}{n_tspike:>9}{fmt(n_tspike/minutes if minutes else float('nan'),9,1)}")
    print(f"    {'rot rad/s^2':<14}{fmt(pct(ar,50))}{fmt(pct(ar,90))}{fmt(pct(ar,99))}"
          f"{fmt(pct(ar,100),12)}{n_rspike:>9}{fmt(n_rspike/minutes if minutes else float('nan'),9,1)}")
    print(f"    thresholds: transl > {args.spike_threshold:g} m/s^2, "
          f"rot > {ROT_SPIKE_THRESHOLD:g} rad/s^2")

    if is_net:
        # rollbacks
        print("\n  ROLLBACKS")
        if rollbacks:
            causes = {}
            depths = []
            comp_counts, comp_mags = {}, {}
            for r in rollbacks:
                causes[r["cause"]] = causes.get(r["cause"], 0) + 1
                if isinstance(r["depth"], (int, float)):
                    depths.append(r["depth"])
                for pair in r["trg"]:
                    if len(pair) >= 2:
                        comp_counts[pair[0]] = comp_counts.get(pair[0], 0) + 1
                        comp_mags.setdefault(pair[0], []).append(pair[1])
            cause_str = ", ".join(f"{c}={n}" for c, n in sorted(causes.items()))
            print(f"    count {len(rollbacks)}   causes: {cause_str}")
            if depths:
                print(f"    depth (ticks): mean {np.mean(depths):.1f}   max {int(max(depths))}")
            print(f"    trigger attribution (exact per-rollback — the slot is cleared before each "
                  f"rollback check, so trg is precisely the components that tripped it; caps at "
                  f"64/rollback):")
            print(f"      {'component':<18}{'count':>8}{'median mag':>14}")
            for comp in sorted(comp_counts, key=lambda c: -comp_counts[c]):
                unit = {"Position": "m", "Rotation": "rad", "LinearVelocity": "m/s",
                        "AngularVelocity": "rad/s"}.get(comp, "")
                print(f"      {comp:<18}{comp_counts[comp]:>8}"
                      f"{fmt(float(np.median(comp_mags[comp])),10)} {unit}")
        else:
            print("    count 0")

        # correction
        print("\n  VISUAL CORRECTION (rollback error decaying on the rendered pose)")
        frac = float(np.mean(cs["active"])) if len(cs["active"]) else float("nan")
        cp_active = cs["cp"][cs["active"] == 1]
        cq_active = cs["cq"][cs["active"] == 1]
        print(f"    active in {frac*100:5.1f}% of frames")
        print(f"    |cp| m   : p95 {fmt(pct(cp_active,95),8)}  max {fmt(pct(cp_active,100),8)}")
        print(f"    cq  rad  : p95 {fmt(pct(cq_active,95),8)}  max {fmt(pct(cq_active,100),8)}  "
              f"(= {math.degrees(pct(cq_active,100)) if len(cq_active) else float('nan'):.1f} deg max)")

        # divergence
        print("\n  CLIENT/SERVER SIM DIVERGENCE (shared ticks)")
        if div is None:
            print("    (no --server given)")
        elif div["n"] == 0:
            print("    no overlapping ticks between client and server traces")
        else:
            dp, dq, dlv = div["dp"], div["dq"], div["dlv"]
            print(f"    shared ticks: {div['n']}")
            print(f"      {'metric':<14}{'p50':>10}{'p95':>10}{'max':>12}{'frac>thr':>11}")
            fp = float(np.mean(dp > DIV_P))
            fq = float(np.mean(dq > DIV_Q))
            flv = float(np.mean(dlv[~np.isnan(dlv)] > DIV_LV)) if np.any(~np.isnan(dlv)) else float("nan")
            print(f"      {'|Δp| m':<14}{fmt(pct(dp,50))}{fmt(pct(dp,95))}{fmt(pct(dp,100),12)}"
                  f"{fmt(fp*100,9,1)} %")
            print(f"      {'rot rad':<14}{fmt(pct(dq,50))}{fmt(pct(dq,95))}{fmt(pct(dq,100),12)}"
                  f"{fmt(fq*100,9,1)} %")
            print(f"      {'|Δlv| m/s':<14}{fmt(pct(dlv,50))}{fmt(pct(dlv,95))}{fmt(pct(dlv,100),12)}"
                  f"{fmt(flv*100,9,1)} %")
            print(f"    thresholds: |Δp|>{DIV_P} m, rot>{DIV_Q} rad, |Δlv|>{DIV_LV} m/s")

        # silent-desync windows — tick-based detection, conf-based branch discrimination.
        # Only meaningful with a server trace (needs same-tick divergence); absent otherwise.
        if div is not None and div["n"] > 0:
            tick_hz = (meta or {}).get("tick_hz")
            windows = silent_desync_windows(div, chosen_ticks, server_ticks, chosen,
                                            rollbacks, tick_hz)
            print_silent_desync(windows)

    # top spikes
    print(f"\n  TOP {len(top_rows)} SPIKES — what coincided with each snap")
    print(f"    {'t(s)':>8}{'mag':>11}{'axis':>7}   {'nearest rollback (±0.15s)':<40}"
          f"{'corr':>5}{'gnd':>5}{'hc':>4}{'pen(m)':>9}")
    for r in top_rows:
        unit = "m/s²" if r["axis"] == "transl" else "rad/s²"
        corr = "yes" if r["corr"] else " no"
        gnd = "-" if r["gnd"] is None else str(r["gnd"])
        hc = "-" if r["hc"] is None else str(r["hc"])
        pen = "-" if r["pen"] is None else f"{r['pen']:.3f}"
        print(f"    {r['t']:>8.3f}{r['mag']:>11.1f}{r['axis']:>7}   {r['rb']:<40}"
              f"{corr:>5}{gnd:>5}{hc:>4}{pen:>9}")
    if not top_rows:
        print("    (none — no spikes above threshold)")
    print(line)


# --- PNG --------------------------------------------------------------------------------------
def _style_axis(ax, ylabel, title=None):
    ax.set_facecolor(C_SURFACE)
    ax.grid(True, color=C_GRID, linewidth=0.6, zorder=0)
    ax.set_axisbelow(True)
    for spine in ("top", "right"):
        ax.spines[spine].set_visible(False)
    for spine in ("left", "bottom"):
        ax.spines[spine].set_color(C_INK2)
        ax.spines[spine].set_linewidth(0.8)
    ax.tick_params(colors=C_INK2, labelsize=8)
    ax.set_ylabel(ylabel, color=C_INK, fontsize=9)
    if title:
        ax.set_title(title, color=C_INK, fontsize=9, loc="left", pad=3)


def _rollback_lines(ax, rollbacks):
    for r in rollbacks:
        if r["t"] is not None:
            ax.axvline(r["t"], color=C_SPIKE, alpha=0.35, linewidth=0.8, zorder=1)


def _legend(ax):
    leg = ax.legend(loc="upper right", fontsize=7.5, framealpha=0.9,
                    facecolor=C_SURFACE, edgecolor=C_GRID)
    for txt in leg.get_texts():
        txt.set_color(C_INK)


def build_png(out_path, in_path, role, is_net, fm, cs, chosen_ticks, rollbacks,
              spikes, div, server_ticks, t2w, args):
    # tick-indexed series -> wall time
    ct = [t for t in chosen_ticks if t["tick"] is not None]
    ct.sort(key=lambda r: r["tick"])
    ck_tick = np.array([r["tick"] for r in ct])
    ck_wall = t2w(ck_tick) if (t2w and len(ck_tick)) else np.array([])
    gnd = np.array([r["gnd"] if r["gnd"] is not None else np.nan for r in ct])
    hc = np.array([r["hc"] if r["hc"] is not None else np.nan for r in ct])
    thr = np.array([r["thr"] if r["thr"] is not None else np.nan for r in ct])

    have_ctx = is_net and len(ck_wall) and np.any(~np.isnan(ck_wall))
    have_div = is_net and div is not None and div["n"] > 0
    have_srv_h = server_ticks is not None and t2w is not None

    # assemble panel list
    panels = []  # (key,)
    panels.append("transl")
    panels.append("rot")
    if is_net:
        panels.append("cp")
        panels.append("cq")
    if have_ctx:
        panels.append("ctx")
        panels.append("thr")
    if have_div:
        panels.append("div")
    panels.append("height")

    n = len(panels)
    heights = {"cp": 1.2, "cq": 1.2, "thr": 1.0}
    hr = [heights.get(p, 2.0) for p in panels]
    fig, axes = plt.subplots(n, 1, sharex=True, figsize=(16, sum(hr) * 0.95),
                             gridspec_kw={"height_ratios": hr})
    if n == 1:
        axes = [axes]
    fig.patch.set_facecolor(C_SURFACE)

    xmin = float(fm["t"][0]) if len(fm["t"]) else 0.0
    xmax = float(fm["t"][-1]) if len(fm["t"]) else 1.0

    # spike frame times/mags for dots
    sp_t = np.array([s["t"] for s in spikes if s["axis"] == "transl"])
    sp_m = np.array([s["mag"] for s in spikes if s["axis"] == "transl"])
    spr_t = np.array([s["t"] for s in spikes if s["axis"] == "rot"])
    spr_m = np.array([s["mag"] for s in spikes if s["axis"] == "rot"])

    for ax, key in zip(axes, panels):
        _style_axis(ax, "")
        if key == "transl":
            at = fm["a_transl"]
            log_y = len(at) and np.max(at) > 0 and pct(at, 50) > 0 and (np.max(at) / pct(at, 50) > 100)
            _rollback_lines(ax, rollbacks)
            ax.plot(fm["t_j"], at, color=C_TRANSL, linewidth=1.2, label="translational", zorder=3)
            ax.plot(fm["t_j"], fm["a_vert"], color=C_VERT, linewidth=1.0,
                    label="vertical-only", zorder=2)
            if len(sp_t):
                ax.scatter(sp_t, sp_m, s=40, color=C_SPIKE, zorder=5, label="spike")
            if log_y:
                ax.set_yscale("log")
            _style_axis(ax, "render jerk\n(m/s²)")
            _legend(ax)
        elif key == "rot":
            _rollback_lines(ax, rollbacks)
            ax.plot(fm["t_j"], fm["a_rot"], color=C_ROT, linewidth=1.2, zorder=3)
            if len(spr_t):
                ax.scatter(spr_t, spr_m, s=40, color=C_SPIKE, zorder=5)
            _style_axis(ax, "rot jerk\n(rad/s²)",
                        title="rotational render jerk (rad/s²)  ·  red lines = rollbacks")
        elif key == "cp":
            _rollback_lines(ax, rollbacks)
            ax.plot(cs["t"], cs["cp"], color=C_CP, linewidth=1.2, zorder=3)
            _style_axis(ax, "|cp|\n(m)", title="correction error — translation |cp| (m)")
        elif key == "cq":
            _rollback_lines(ax, rollbacks)
            ax.plot(cs["t"], np.degrees(cs["cq"]), color=C_CQ, linewidth=1.2, zorder=3)
            _style_axis(ax, "cq\n(deg)", title="correction error — rotation cq (deg)")
        elif key == "ctx":
            _rollback_lines(ax, rollbacks)
            ax.step(ck_wall, gnd, where="post", color=C_GND, linewidth=1.2,
                    label="grounded wheels", zorder=3)
            ax.step(ck_wall, hc, where="post", color=C_HC, linewidth=1.2,
                    label="hull contacts", zorder=2)
            ax.set_ylim(-0.5, 16.5)
            _style_axis(ax, "count\n(0–16)")
            _legend(ax)
        elif key == "thr":
            ax.plot(ck_wall, thr, color=C_THR, linewidth=1.2, zorder=3)
            ax.set_ylim(-0.05, 1.05)
            _style_axis(ax, "throttle\n(0–1)", title="throttle intent (0–1)")
        elif key == "div":
            dw = t2w(div["shared"])
            _rollback_lines(ax, rollbacks)
            ax.plot(dw, div["dp"], color=C_TRANSL, linewidth=1.2, label="|Δp| m", zorder=3)
            ax.axhline(DIV_P, color=C_INK2, linestyle="--", linewidth=0.9, zorder=2,
                       label=f"rollback thr {DIV_P} m")
            _style_axis(ax, "|Δp|\n(m)", title="client/server position divergence (m)")
            _legend(ax)
        elif key == "height":
            _rollback_lines(ax, rollbacks)
            ax.plot(fm["t"], fm["py"], color=C_TRANSL, linewidth=1.2,
                    label="rendered p_y (client)", zorder=3)
            if have_srv_h:
                st = [t for t in server_ticks if t["tick"] is not None]
                st.sort(key=lambda r: r["tick"])
                sk = np.array([r["tick"] for r in st])
                sw = t2w(sk)
                spy = np.array([r["p"][1] for r in st])
                mask = ~np.isnan(sw)
                ax.plot(sw[mask], spy[mask], color=C_GND, linewidth=1.2,
                        label="server sim p_y", zorder=2)
            _style_axis(ax, "hull\nheight (m)", title="hull height — what the eye saw")
            _legend(ax)

    axes[-1].set_xlabel("wall time (s)", color=C_INK, fontsize=9)
    axes[-1].set_xlim(xmin, xmax)

    # title block
    n_sp = len(spikes)
    dur = (fm["t"][-1] - fm["t"][0]) if len(fm["t"]) >= 2 else 0.0
    fig.suptitle(
        f"{in_path.name}   ·   role={role}   ·   {dur:.1f}s   ·   "
        f"rollbacks={len(rollbacks) if is_net else 'n/a'}   ·   spikes={n_sp}",
        color=C_INK, fontsize=12, x=0.01, ha="left", y=0.997,
    )
    fig.tight_layout(rect=(0, 0, 1, 0.985))
    fig.savefig(out_path, dpi=130, facecolor=C_SURFACE)
    plt.close(fig)


if __name__ == "__main__":
    sys.exit(main())
