# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Steering-model gate report for track-sandbox harness captures (schema 2).

Consumes the JSONL a scripted `SANDBOX_HARNESS` run writes (src/track_sandbox/
harness.rs) and verifies the belt drive model's invariants as numbers — the
steer-feel counterpart to scripts/divergence/analyze.py: instead of eyeballing
whether the tank "turns right about right", every run is checked against what
the model (src/track/forces.rs + drive.rs) promises by construction.

Each file self-describes via its `meta` row (schema:2 — slew, half_tread, mu,
lateral_ratio, slip_saturation, plus the command script), so the analyzer needs
no scenario flags: pivot / turn / slalom / straight are detected from the
scripted commands.

HARD GATES (every run, every tick):
  - shaper      |Δshaped| per axis per tick ≤ slew/64, and shaped snaps exactly
                to raw once within one step (drive.rs `approach` semantics).
  - ellipse     per contact: (f_long/μ·load)² + (f_lat/μ·load·lat_ratio)² ≤ 1
                (forces.rs caps the ellipse; tolerance includes the JSONL
                print quantization: f at 0.05 N, load at 0.5 N).
  - dissipation friction never pumps energy: f_long·slip ≥ −tol and
                f_lat·slip_lat ≤ +tol (f_long tracks slip sign, f_lat opposes).
  - finite      every numeric field in every row is finite.

SCENARIO GATES (settled window = tick ≥ warmup+64, before t2, trimmed at
COURSE EXIT — the sandbox lane is finite, and a turning run eventually drives
off the authored terrain: the first sustained span (≥8 ticks) where total
support drops below 30% of weight or a side loses all contacts ends the
analyzable window; everything after is falling/wedged off-course, not steering):
  - yaw-sign    sign(yawrate_body) == −sign(steer) for ≥95% of samples, where
                yawrate_body = av · hull_up (hull_up = q * +Y — the raw `yawrate`
                field is world av.y and lies on slopes).
  - pivot       belts counter-rotate; |mean|L| − mean|R|| / mean|L| < 3%; hull
                translation drift < 5% of mean belt speed × window; pivot
                radius mean|v_xz|/mean|yawrate| < 0.25 × half_tread. (The
                kinematic ratio, NOT a path circle fit: a pivoting hull spins
                much faster than its centre orbits — measured ~2.4 rad/s spin
                vs ~0.1 rad/s orbit — so a fitted circle measures the slow
                precessing drift walk, which pivot_drift already bounds; the
                walk loop is reported alongside, not gated.)
  - turn        Kasa circle fit of the hull XZ path: fit RMS < 5% of radius;
                radius cross-checked against |v|/|yawrate| and the no-slip
                R_belt = half_tread·(vl+vr)/(vl−vr) (reported, not gated).
  - slalom      yawrate reverses sign after shaped steer crosses zero; net
                heading change over paired equal-length settled half-cycles < 5°.
  - contacts    both sides grounded (≥1 contact) for ≥95% of the (trimmed)
                window — else the run is marked INVALID (reported, steering
                gates skipped, not counted as a gate failure). A window
                trimmed below 128 ticks is likewise INVALID.

MULTI-RUN GATE: turn runs sharing a throttle must show fitted radius strictly
decreasing in |steer| (more steer, tighter turn).

Usage:
    uv run scripts/track/analyze_steer.py run1.jsonl [run2.jsonl ...]
        [--summary out.json]

Exit status is nonzero iff any gate fails (INVALID runs alone do not fail).
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

import numpy as np

TICK_HZ = 64.0  # the fixed tick both the harness and the shaper run on
SETTLE_TICKS = 64  # command-onset transient skipped before scenario windows
YAW_SIGN_FRACTION = 0.95
CONTACT_FRACTION = 0.95
MIN_WINDOW = 128  # a course-exit trim leaving less than this is not analyzable
# Course-exit detection: ≥ this many consecutive ticks with support below this
# fraction of vehicle weight (or a side fully airborne) means the tank left the
# authored lane — single-tick washboard hops never last this long.
EXIT_SPAN = 8
EXIT_SUPPORT_FRACTION = 0.3
# JSONL print quantization (harness.rs `arr` / contact row formats): worst-case
# half-ulp of the printed precision, folded into the ellipse tolerance.
Q_FORCE = 0.05  # f_long / f_lat at 1 decimal
Q_LOAD = 0.5  # load at 0 decimals


# --- loading -----------------------------------------------------------------------------------
def flatten_numeric(obj, out):
    """Collect every number reachable in a parsed JSON value (finiteness sweep)."""
    if isinstance(obj, (int, float)) and not isinstance(obj, bool):
        out.append(float(obj))
    elif isinstance(obj, list):
        for v in obj:
            flatten_numeric(v, out)
    elif isinstance(obj, dict):
        for v in obj.values():
            flatten_numeric(v, out)


def load_run(path: Path):
    """Parse one harness JSONL into meta + per-tick arrays + flat contact table."""
    meta = None
    rows = []
    with path.open() as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            rec = json.loads(line)
            t = rec.get("t")
            if t == "meta":
                meta = rec
            elif t == "k":
                rows.append(rec)
            # scan / vscan rows: terrain oracle validation, not steering — skip.
    if meta is None:
        raise ValueError(f"{path}: no meta row")
    if meta.get("schema") != 2:
        raise ValueError(f"{path}: expected schema 2, got {meta.get('schema')}")
    if not rows:
        raise ValueError(f"{path}: no k rows")

    n = len(rows)
    run = {
        "meta": meta,
        "n": n,
        "k": np.array([r["k"] for r in rows], dtype=np.int64),
        "hull": np.array([r["hull"] for r in rows]),
        "q": np.array([r["q"] for r in rows]),
        "av": np.array([r["av"] for r in rows]),
        "yaw": np.array([r["yaw"] for r in rows]),
        "raw": np.array([r["raw"] for r in rows]),
        "shaped": np.array([r["shaped"] for r in rows]),
        "vel": np.array([r["vel"] for r in rows]),
        "belt": np.array([r["belt"] for r in rows]),
        "sup": np.array([r["sup"] for r in rows]),
        "ncontacts": np.array([[len(r["contacts"][0]), len(r["contacts"][1])] for r in rows]),
    }
    # Flat contact table [load, slip, slip_lat, f_long, f_lat] across the whole run
    # (contact row layout: x,y,z,load,slip,normal_y,load_elastic,slip_lat,f_long,f_lat).
    flat = []
    for r in rows:
        for side in r["contacts"]:
            for c in side:
                flat.append((c[3], c[4], c[7], c[8], c[9]))
    run["contacts"] = np.array(flat) if flat else np.zeros((0, 5))

    # Finiteness sweep over EVERY numeric field the rows carry (chain, wheels, all).
    nonfinite = 0
    for r in rows:
        nums = []
        flatten_numeric(r, nums)
        a = np.asarray(nums)
        nonfinite += int(np.count_nonzero(~np.isfinite(a)))
    run["nonfinite"] = nonfinite
    return run


# --- geometry helpers --------------------------------------------------------------------------
def hull_up(q: np.ndarray) -> np.ndarray:
    """Rotate +Y by each quaternion row [x,y,z,w] (rotation-matrix middle column)."""
    x, y, z, w = q[:, 0], q[:, 1], q[:, 2], q[:, 3]
    return np.stack(
        [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
        axis=1,
    )


def kasa_fit(x: np.ndarray, z: np.ndarray):
    """Algebraic (Kasa) circle fit of a planar path: returns (cx, cz, R, rms)."""
    a = np.column_stack([x, z, np.ones_like(x)])
    b = x * x + z * z
    sol, *_ = np.linalg.lstsq(a, b, rcond=None)
    cx, cz = sol[0] / 2.0, sol[1] / 2.0
    r2 = sol[2] + cx * cx + cz * cz
    radius = math.sqrt(max(r2, 0.0))
    rms = float(np.sqrt(np.mean((np.hypot(x - cx, z - cz) - radius) ** 2)))
    return float(cx), float(cz), radius, rms


def detect_scenario(meta) -> str:
    thr, steer = meta["throttle"], meta["steer"]
    t2 = meta["t2"]
    end = meta["warmup"] + meta["ticks"]
    if t2 < end and steer != 0.0 and meta["steer2"] == -steer:
        return "slalom"
    if thr == 0.0 and steer != 0.0:
        return "pivot"
    if thr != 0.0 and steer != 0.0:
        return "turn"
    return "straight"


# --- gates -------------------------------------------------------------------------------------
def gate(gates, name, ok, value):
    gates[name] = {"pass": bool(ok), "value": value}
    return ok


def check_hard_gates(run, gates):
    meta = run["meta"]
    step = meta["slew"] / TICK_HZ

    # (1) Shaper: per-tick slew bound + exact convergence within one step.
    shaped, raw = run["shaped"], run["raw"]
    d = np.abs(np.diff(shaped, axis=0))
    max_excess = float(np.max(d) - step) if d.size else 0.0
    slew_ok = bool(np.all(d <= step + 1e-6))
    gap = np.abs(raw[1:] - shaped[:-1])  # target vs previous shaped, per axis
    must_converge = gap <= step - 1e-6
    converged = np.abs(shaped[1:] - raw[1:]) <= 1e-6
    conv_viol = int(np.count_nonzero(must_converge & ~converged))
    gate(
        gates,
        "shaper",
        slew_ok and conv_viol == 0,
        {"step": step, "max_step_excess": max_excess, "convergence_violations": conv_viol},
    )

    # (2) Friction ellipse per contact (skip unloaded contacts; tolerance folds in
    # the print quantization of f (±0.05 N) and load (±0.5 N) — which dominates e
    # for feather-weight contacts, so max_e_loaded (load ≥ 100 N, quantization
    # ≤ ~1%) is the headline number and raw max_e is quantization noise).
    c = run["contacts"]
    loaded = c[c[:, 0] >= 1.0] if c.size else c
    if loaded.size:
        load, f_long, f_lat = loaded[:, 0], loaded[:, 3], loaded[:, 4]
        grip = meta["mu"] * load
        grip_lat = grip * meta["lateral_ratio"]
        e = (f_long / grip) ** 2 + (f_lat / grip_lat) ** 2
        slack = (
            (2.0 * np.abs(f_long) * Q_FORCE + Q_FORCE**2) / grip**2
            + (2.0 * np.abs(f_lat) * Q_FORCE + Q_FORCE**2) / grip_lat**2
            + 2.0 * e * Q_LOAD / load
        )
        viol = int(np.count_nonzero(e > 1.0 + 1e-4 + slack))
        emax = float(np.max(e))
        heavy = load >= 100.0
        emax_loaded = float(np.max(e[heavy])) if np.any(heavy) else 0.0
    else:
        viol, emax, emax_loaded = 0, 0.0, 0.0
    gate(
        gates,
        "ellipse",
        viol == 0,
        {"contacts": int(loaded.shape[0]), "max_e_loaded": emax_loaded, "max_e": emax, "violations": viol},
    )

    # (3) Dissipation: f_long follows slip sign, f_lat opposes slip_lat (forces.rs).
    if c.size:
        fscale = max(1.0, float(np.max(np.abs(c[:, 3:5]))))
        tol = 1e-3 * fscale
        long_viol = int(np.count_nonzero(c[:, 3] * c[:, 1] < -tol))
        lat_viol = int(np.count_nonzero(c[:, 4] * c[:, 2] > tol))
    else:
        tol, long_viol, lat_viol = 0.0, 0, 0
    gate(
        gates,
        "dissipation",
        long_viol == 0 and lat_viol == 0,
        {"tol": tol, "long_violations": long_viol, "lat_violations": lat_viol},
    )

    # (4) Finiteness.
    gate(gates, "finite", run["nonfinite"] == 0, {"nonfinite_values": run["nonfinite"]})


def course_exit_tick(run):
    """First tick of the first sustained support-collapse span, or n if none.

    The sandbox lane is finite: a turning run eventually drives off the authored
    terrain, after which the capture is falling/wedged geometry, not steering.
    Single-tick washboard hops never sustain EXIT_SPAN consecutive ticks.
    """
    weight = run["meta"]["weight"]
    bad = (run["sup"] < EXIT_SUPPORT_FRACTION * weight) | (run["ncontacts"].min(axis=1) == 0)
    runlen = 0
    for i in range(run["n"]):
        runlen = runlen + 1 if bad[i] else 0
        if runlen >= EXIT_SPAN:
            return i - EXIT_SPAN + 1
    return run["n"]


def settled_window(run, exit_tick):
    """Index range [start, stop) of the settled, on-course first command phase."""
    meta = run["meta"]
    start = meta["warmup"] + SETTLE_TICKS
    stop = min(meta["t2"], meta["warmup"] + meta["ticks"], run["n"], exit_tick)
    return int(start), int(stop)


def check_contact_validity(run, gates, start, stop):
    nc = run["ncontacts"][start:stop]
    frac = float(np.mean(np.all(nc >= 1, axis=1))) if len(nc) else 0.0
    ok = frac >= CONTACT_FRACTION and (stop - start) >= MIN_WINDOW
    gates["contacts"] = {
        "pass": bool(ok),
        "value": {"both_sides_fraction": frac, "window_ticks": stop - start},
    }
    return ok  # validity, not a steering-gate failure


def yawrate_body(run):
    return np.einsum("ij,ij->i", run["av"], hull_up(run["q"]))


def check_yaw_sign(run, gates, start, stop, steer, name="yaw_sign"):
    """steady_fraction (second half of the window) separates turn-in lag —
    yawrate still indistinguishable from washboard noise while speed builds —
    from a genuine wrong-way response."""
    yr = yawrate_body(run)[start:stop]
    frac = float(np.mean(np.sign(yr) == -np.sign(steer))) if len(yr) else 0.0
    half = yr[len(yr) // 2 :]
    steady = float(np.mean(np.sign(half) == -np.sign(steer))) if len(half) else 0.0
    gate(
        gates,
        name,
        frac >= YAW_SIGN_FRACTION,
        {
            "fraction": frac,
            "steady_fraction": steady,
            "mean_yawrate_body": float(np.mean(yr)) if len(yr) else 0.0,
        },
    )


def check_pivot(run, gates, start, stop):
    meta = run["meta"]
    belt = run["belt"][start:stop]
    ml, mr = float(np.mean(belt[:, 0])), float(np.mean(belt[:, 1]))
    al, ar = float(np.mean(np.abs(belt[:, 0]))), float(np.mean(np.abs(belt[:, 1])))
    gate(gates, "pivot_counter_rotate", ml * mr < 0.0, {"mean_belt_l": ml, "mean_belt_r": mr})
    asym = abs(al - ar) / al if al > 0 else float("inf")
    gate(gates, "pivot_belt_symmetry", asym < 0.03, {"asymmetry": asym, "mean_abs_l": al, "mean_abs_r": ar})

    xz = run["hull"][start:stop][:, [0, 2]]
    drift = float(np.linalg.norm(xz[-1] - xz[0]))
    duration = (stop - start) / TICK_HZ
    budget = 0.05 * ((al + ar) / 2.0) * duration
    gate(gates, "pivot_drift", drift < budget, {"drift_m": drift, "budget_m": budget})

    # Pivot radius: the KINEMATIC ratio mean|v_xz| / mean|yawrate| — how far the
    # instantaneous rotation centre sits from the hull centre. A circle fit is
    # the wrong instrument here: the hull spins far faster than its centre
    # orbits the fit centre, so the fitted circle is the slow precessing drift
    # walk (reported below), not a turning radius.
    v = run["vel"][start:stop]
    speed = np.hypot(v[:, 0], v[:, 2])
    yr = np.abs(yawrate_body(run)[start:stop])
    radius = float(np.mean(speed) / max(np.mean(yr), 1e-9))
    cx, cz, loop_r, _ = kasa_fit(xz[:, 0], xz[:, 1])
    orbit = np.unwrap(np.arctan2(xz[:, 1] - cz, xz[:, 0] - cx))
    gate(
        gates,
        "pivot_radius",
        radius < 0.25 * meta["half_tread"],
        {
            "radius_m": radius,
            "limit_m": 0.25 * meta["half_tread"],
            "walk_loop_radius_m": loop_r,
            "walk_speed_mps": float(np.mean(speed)),
            "spin_vs_orbit_rad_s": [float(np.mean(yr)), float((orbit[-1] - orbit[0]) / ((stop - start) / TICK_HZ))],
        },
    )
    return radius


def check_turn(run, gates, start, stop):
    meta = run["meta"]
    xz = run["hull"][start:stop][:, [0, 2]]
    _, _, radius, rms = kasa_fit(xz[:, 0], xz[:, 1])
    gate(gates, "turn_fit_rms", rms < 0.05 * radius, {"radius_m": radius, "fit_rms_m": rms})

    # Cross-checks (reported, not gated): kinematic |v|/|yawrate| and the no-slip
    # differential radius from the belt speeds — the gap between R_fit and R_belt
    # IS the model's understeer (lateral slip pushing the hull wide).
    yr = yawrate_body(run)[start:stop]
    v = run["vel"][start:stop]
    speed = np.hypot(v[:, 0], v[:, 2])
    r_kin = float(np.mean(speed) / max(np.mean(np.abs(yr)), 1e-9))
    belt = run["belt"][start:stop]
    vl, vr = float(np.mean(belt[:, 0])), float(np.mean(belt[:, 1]))
    r_belt = abs(meta["half_tread"] * (vl + vr) / (vl - vr)) if vl != vr else float("inf")
    gates["turn_radius_crosscheck"] = {
        "pass": True,
        "value": {"r_fit": radius, "r_kinematic": r_kin, "r_belt_noslip": r_belt, "mean_speed": float(np.mean(speed))},
    }
    return radius


def check_slalom(run, gates, start, stop, exit_tick):
    meta = run["meta"]
    steer, steer2 = meta["steer"], meta["steer2"]
    t2, end = int(meta["t2"]), min(run["n"], int(exit_tick))

    # Yaw reversal: after the SHAPED steer crosses zero (the slewed reversal, not
    # the raw t2 edge), the body yaw rate must settle onto the opposite sign.
    shaped_steer = run["shaped"][:, 1]
    crossed = np.nonzero((np.arange(run["n"]) >= t2) & (shaped_steer * steer < 0.0))[0]
    if len(crossed) == 0:
        gate(gates, "slalom_reversal", False, {"error": "shaped steer never crossed zero after t2"})
        return
    cross = int(crossed[0])
    w0 = min(cross + SETTLE_TICKS, end)
    yr2 = yawrate_body(run)[w0:end]
    frac = float(np.mean(np.sign(yr2) == -np.sign(steer2))) if len(yr2) else 0.0
    gate(
        gates,
        "slalom_reversal",
        frac >= YAW_SIGN_FRACTION,
        {"cross_tick": cross, "fraction": frac, "mean_yawrate_body": float(np.mean(yr2)) if len(yr2) else 0.0},
    )

    # Net heading over paired equal-length settled half-cycles: equal windows on
    # each side of t2 (both past their onset transient) should cancel to < 5 deg.
    # The per-half mean speed/yawrate are reported so an asymmetry can be told
    # apart: phase 1 starts from rest (spin-up) and the two halves sweep
    # different headings across the washboard.
    half = min(t2 - start, end - (t2 + SETTLE_TICKS))
    if half < MIN_WINDOW:
        gate(gates, "slalom_net_heading", False, {"error": f"phase pairing too short ({half} ticks)"})
        return
    yaw = np.unwrap(run["yaw"])
    w1 = slice(start, start + half)
    w2 = slice(t2 + SETTLE_TICKS, t2 + SETTLE_TICKS + half)
    d1 = float(yaw[w1.stop - 1] - yaw[w1.start])
    d2 = float(yaw[w2.stop - 1] - yaw[w2.start])
    net = math.degrees(d1 + d2)
    yr = yawrate_body(run)
    v = run["vel"]
    speed = np.hypot(v[:, 0], v[:, 2])
    gate(
        gates,
        "slalom_net_heading",
        abs(net) < 5.0,
        {
            "net_deg": net,
            "half_cycle_deg": [math.degrees(d1), math.degrees(d2)],
            "half_len_ticks": half,
            "mean_yawrate": [float(np.mean(yr[w1])), float(np.mean(yr[w2]))],
            "mean_speed": [float(np.mean(speed[w1])), float(np.mean(speed[w2]))],
        },
    )


# --- report ------------------------------------------------------------------------------------
def fmt_value(v):
    if isinstance(v, dict):
        return "  ".join(f"{k}={fmt_value(x)}" for k, x in v.items())
    if isinstance(v, float):
        return f"{v:.4g}"
    if isinstance(v, list):
        return "[" + ", ".join(fmt_value(x) for x in v) + "]"
    return str(v)


def analyze_file(path: Path):
    run = load_run(path)
    meta = run["meta"]
    scenario = detect_scenario(meta)
    gates = {}
    check_hard_gates(run, gates)
    exit_tick = course_exit_tick(run)
    start, stop = settled_window(run, exit_tick)
    valid = check_contact_validity(run, gates, start, stop)

    radius = None
    if valid:
        if scenario != "straight":
            check_yaw_sign(run, gates, start, stop, meta["steer"])
        if scenario == "pivot":
            radius = check_pivot(run, gates, start, stop)
        elif scenario == "turn":
            radius = check_turn(run, gates, start, stop)
        elif scenario == "slalom":
            check_slalom(run, gates, start, stop, exit_tick)

    print(f"\n=== {path.name} ===")
    print(
        f"  scenario={scenario}  throttle={meta['throttle']:g} steer={meta['steer']:g}"
        + (f" -> t2={meta['t2']} throttle2={meta['throttle2']:g} steer2={meta['steer2']:g}" if meta["t2"] < run["n"] else "")
        + f"  rows={run['n']}  window=[{start},{stop})"
        + (f"  COURSE EXIT at tick {exit_tick}" if exit_tick < run["n"] else "")
    )
    if not valid:
        print("  RUN INVALID: a track side lost ground contact for >5% of the window; steering gates skipped")
    for name, g in gates.items():
        status = "PASS" if g["pass"] else "FAIL"
        print(f"  [{status}] {name:22s} {fmt_value(g['value'])}")

    failed = [n for n, g in gates.items() if not g["pass"] and n != "contacts"]
    return {
        "scenario": scenario,
        "valid": valid,
        "course_exit_tick": exit_tick if exit_tick < run["n"] else None,
        "gates": gates,
        "throttle": meta["throttle"],
        "steer": meta["steer"],
        "radius_fit": radius,
        "failed": failed,
    }


def check_monotonicity(results):
    """Fitted turn radius strictly decreasing in |steer| among turn runs sharing a throttle."""
    groups = {}
    for name, r in results.items():
        if r["scenario"] == "turn" and r["valid"] and r["radius_fit"] is not None:
            groups.setdefault(round(r["throttle"], 3), []).append((abs(r["steer"]), r["radius_fit"], name))
    out = {}
    for thr, runs in groups.items():
        if len(runs) < 2:
            continue
        runs.sort()
        radii = [r for _, r, _ in runs]
        ok = all(radii[i] > radii[i + 1] for i in range(len(radii) - 1))
        out[str(thr)] = {
            "pass": ok,
            "value": [{"steer": s, "radius_m": r, "file": n} for s, r, n in runs],
        }
        status = "PASS" if ok else "FAIL"
        chain = "  ".join(f"|steer|={s:g} R={r:.2f}m" for s, r, _ in runs)
        print(f"\n[{status}] monotonicity (throttle={thr:g}): {chain}")
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("files", nargs="+", type=Path, help="harness JSONL captures (schema 2)")
    ap.add_argument("--summary", type=Path, help="write the machine-readable summary JSON here")
    args = ap.parse_args()

    results = {}
    for path in args.files:
        results[path.name] = analyze_file(path)

    monotonicity = check_monotonicity(results)

    summary = {
        name: {
            "scenario": r["scenario"],
            "valid": r["valid"],
            "course_exit_tick": r["course_exit_tick"],
            "gates": r["gates"],
        }
        for name, r in results.items()
    }
    summary["monotonicity"] = monotonicity
    if args.summary:
        args.summary.write_text(json.dumps(summary, indent=2))
        print(f"\nsummary written to {args.summary}")

    failures = [(n, f) for n, r in results.items() for f in r["failed"]]
    failures += [("monotonicity", thr) for thr, g in monotonicity.items() if not g["pass"]]
    invalid = [n for n, r in results.items() if not r["valid"]]
    print(f"\n{len(results)} run(s): {len(failures)} gate failure(s), {len(invalid)} invalid run(s)")
    for n, f in failures:
        print(f"  FAIL {n}: {f}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
