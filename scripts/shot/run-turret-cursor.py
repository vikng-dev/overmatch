#!/usr/bin/env python3
"""Turret-on-the-cursor capture: one dedicated server and two hidden-window clients over real UDP,
each receiving through the seeded 80 ms / 10 ms receive conditioner (`net::harness`).

`fire`: the subject traverses its turret at its full powered rate toward a point behind it and
fires its main gun mid-traverse; per shot it reports the angle between the drawn barrel and the
server's bore at the fire tick, and between the drawn barrel and the tracer's first drawn segment,
on the shooter's own client (its own turret) and on the observer's (a remote turret, #330).

`pivot`: the subject turns in place while its turret holds a world point; from the observer's
per-frame drawn bore it reports the bore bearing's oscillation and steps (#132), beside the
drawn hull's.

The observer connects first and takes lane 0; the subject lands 8 m along +x
(`scripts/harness/flat-patch.ron`). Builds through serial-cargo, then holds the machine lock
for the run. Usage: python3 scripts/shot/run-turret-cursor.py fire|pivot --seed N --out DIR
"""

import argparse
import json
import math
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time

TICK_HZ = 64

# Fixture workloads. `fire`: the aim point sits 0.5 degrees right of dead astern and 7 degrees up
# (DERIVED from the point), so the turret traverses right, away from the observer at -8 m x, for
# a half turn; a click every 196 ticks outlasts the main gun's 192-tick reload, the first a second
# in, once the turret runs at full rate and the input path has settled. `pivot`: a world point
# 400 m along -z from the subject's spawn, held while the hull turns at the steer.
FIRE = "(drive: Fire(primary: true), aim: Point((7.0, 100.0, 800.0)), range_m: 800.0, " \
       "fire_tick: 64, fire_interval: 196, fire_until: {until}, ticks: {ticks})"
# Resolution: a shot leaves mid-traverse when the server's bore turned faster than this.
TRAVERSING_DEG_S = 1.0
PIVOT = "(drive: Pivot(steer: {steer}), aim: World((688.0, 12.0, -920.0)), ticks: {ticks})"
OBSERVE = "(drive: Idle, aim: Point((0.0, 0.0, -800.0)), ticks: {ticks})"


def jsonl(path):
    """A trace's rows. The server stops on SIGTERM mid-write, so its last line may be cut short."""
    lines = path.read_text().splitlines()
    rows = [json.loads(line) for line in lines[:-1] if line.strip()]
    try:
        rows.append(json.loads(lines[-1]))
    except (IndexError, json.JSONDecodeError):
        pass
    return rows


def unit(v):
    norm = math.sqrt(sum(c * c for c in v))
    return [c / norm for c in v]


def angle_deg(a, b):
    a, b = unit(a), unit(b)
    return math.degrees(math.acos(max(-1.0, min(1.0, sum(x * y for x, y in zip(a, b))))))


def bearing_deg(v):
    """World bearing of a direction on the ground plane: 0 along -z, positive toward +x."""
    return math.degrees(math.atan2(v[0], -v[2]))


def wrap(deg):
    return (deg + 180.0) % 360.0 - 180.0


def quat_forward(q):
    x, y, z, w = q
    # q * (0, 0, -1)
    return [-(2 * (x * z + w * y)), -(2 * (y * z - w * x)), -(1 - 2 * (x * x + y * y))]


def stats(values):
    if not values:
        return None
    mean = sum(values) / len(values)
    return {"n": len(values), "min": min(values), "mean": mean, "max": max(values),
            "rms": math.sqrt(sum(v * v for v in values) / len(values))}


def truth_at(series, itick):
    """A server per-tick value at a fractional cursor, along the shorter arc for bearings."""
    k = math.floor(itick)
    if k not in series or k + 1 not in series:
        return None
    a, b = series[k], series[k + 1]
    return a + wrap(b - a) * (itick - k)


def server_bores(server_ticks):
    by_tank = {}
    for row in server_ticks:
        if row["k"] == "tick" and row.get("bore"):
            by_tank.setdefault(row["e"], {})[row["tick"]] = row
    return by_tank


def analyze_fire(out):
    fires = [r for r in jsonl(out / "server-shots.server.jsonl") if r["k"] == "fire"]
    if len(fires) < 10:
        raise RuntimeError(f"{len(fires)} server fires, fewer than the 10 the capture needs")
    tanks = server_bores(jsonl(out / "server-ticks.server.jsonl"))
    first = fires[0]
    # The shooter's server root: the one whose bore before the fire tick is the fired bore.
    shooter = min(tanks, key=lambda e: angle_deg(
        tanks[e][first["t"] - 1]["bore"][3:], first["d"]) if first["t"] - 1 in tanks[e] else 1e9)
    bearing = {k: bearing_deg(r["bore"][3:]) for k, r in tanks[shooter].items()}
    shots, per_view = [], {}
    for view in ("subject", "observer"):
        drawn = {}
        for row in jsonl(out / f"{view}-shots.client.jsonl"):
            if row["k"] == "drawn" and row["bore"] is not None and len(row["p"]) >= 2:
                drawn.setdefault((row["c"], row["w"], row["ft"]), row)
        per_view[view] = drawn
    for fire in fires:
        key = (fire["c"], fire["w"], fire["ft"])
        rate = wrap(bearing[fire["t"]] - bearing[fire["t"] - 1]) * TICK_HZ \
            if fire["t"] in bearing and fire["t"] - 1 in bearing else None
        shot = {"fire_tick": fire["t"], "traverse_deg_s": rate}
        for view, drawn in per_view.items():
            row = drawn.get(key)
            if row is None:
                shot[view] = None
                continue
            muzzle, bore = row["bore"][:3], row["bore"][3:]
            p0, p1 = row["p"][0], row["p"][1]
            segment = [b - a for a, b in zip(p0, p1)]
            truth = truth_at(bearing, row["itick"])
            shot[view] = {
                "gun_vs_server_fire_deg": angle_deg(bore, fire["d"]),
                "gun_vs_tracer_deg": angle_deg(bore, segment),
                "gun_yaw_minus_fire_yaw_deg": wrap(bearing_deg(bore) - bearing_deg(fire["d"])),
                # The drawn bore against the server's bore at this frame's cursor: how far the
                # drawn turret runs ahead of the stream it presents.
                "gun_yaw_minus_cursor_yaw_deg":
                    None if truth is None else wrap(bearing_deg(bore) - truth),
                "cursor_minus_fire_ticks": row["itick"] - fire["t"],
                "tracer_start_from_drawn_muzzle_m": math.dist(muzzle, p0),
            }
        shot["traversing"] = rate is not None and abs(rate) > TRAVERSING_DEG_S
        shots.append(shot)
    traversing = [s for s in shots if s["traversing"]]
    if len(traversing) < 10:
        raise RuntimeError(f"{len(traversing)} shots left mid-traverse, fewer than 10")
    summary = {"shots": shots, "mid_traverse_shots": len(traversing)}
    for view in per_view:
        seen = [s[view] for s in traversing if s[view] is not None]
        summary[f"{view}_mid_traverse"] = {
            field: stats([s[field] for s in seen if s[field] is not None])
            for field in (seen[0] if seen else {})}
    summary["traverse_deg_s"] = stats([s["traverse_deg_s"] for s in traversing])
    return summary


def fire_text(summary):
    m = summary["MEASURED"]
    lines = [f"turret-cursor fire capture, seed {summary['seed']}: {summary['subject_script']}",
             f"{summary['conditioner']}; window {summary['window']}; MEASURED, degrees",
             "own = the shooter's client drawing its own turret; remote = the observer's client",
             "drawing the shooter. gun-fire: drawn barrel vs the server's bore at the fire tick;",
             "gun-tracer: drawn barrel vs the tracer's first drawn segment; lead: drawn yaw minus",
             "the server's yaw at the frame's view cursor; cur-ft: that cursor minus the fire tick.",
             "",
             f"{'tick':>5} {'deg/s':>6} | {'own gun-fire':>12} {'gun-tracer':>10} {'lead':>6} "
             f"{'cur-ft':>6} | {'remote gun-fire':>15} {'gun-tracer':>10} {'lead':>6} {'cur-ft':>6}"]
    for s in m["shots"]:
        row = f"{s['fire_tick']:>5} {s['traverse_deg_s'] or 0.0:>6.2f} |"
        for view, width in (("subject", 12), ("observer", 15)):
            v = s[view]
            row += " missing |" if v is None else (
                f" {v['gun_vs_server_fire_deg']:>{width}.3f} {v['gun_vs_tracer_deg']:>10.3f}"
                f" {v['gun_yaw_minus_cursor_yaw_deg'] or 0.0:>6.3f}"
                f" {v['cursor_minus_fire_ticks']:>6.2f} |")
        lines.append(row.rstrip(" |") + ("" if s["traversing"] else "   (at rest)"))
    lines.append("")
    lines.append(f"mid-traverse shots: {m['mid_traverse_shots']}, traverse "
                 f"{m['traverse_deg_s']['mean']:.2f} deg/s mean")
    for view, name in (("subject", "own"), ("observer", "remote")):
        st = m[f"{view}_mid_traverse"]
        for field in ("gun_vs_server_fire_deg", "gun_vs_tracer_deg",
                      "gun_yaw_minus_cursor_yaw_deg", "tracer_start_from_drawn_muzzle_m"):
            f = st[field]
            lines.append(f"{name:>6} {field}: min {f['min']:.3f} mean {f['mean']:.3f} "
                         f"max {f['max']:.3f}")
    return "\n".join(lines) + "\n"


def pivot_text(summary):
    m = summary["MEASURED"]
    lines = [f"turret-cursor pivot capture, seed {summary['seed']}: {summary['subject_script']}",
             f"{summary['conditioner']}; window {summary['window']}; MEASURED, degrees",
             "Series: the observer's per-frame drawn bore (and hull) bearing of the turning remote.",
             "vs-server: drawn minus the server's per-tick value at the frame's view cursor.",
             "wobble: drawn minus its 1 s centred mean. held frames: frames that moved under 10%",
             "of the mean rate's per-frame motion.",
             "",
             f"hull rate {m['hull_rate_deg_s']:.2f} deg/s; server bore drift over the window "
             f"{m['server_bore_drift_deg']:.3f}; {m['frames']} frames at {m['frame_s'] * 1e3:.2f} "
             "ms mean"]
    for name in ("bore", "hull"):
        r = m[name]
        steps_ = r["per_frame"]
        period_ = r["wobble_period"]
        lines += [
            f"{name}: drawn rate {r['drawn_rate_deg_s']:.3f} deg/s",
            f"  vs-server p2p {r['vs_server_p2p_deg']:.4f}, rms "
            f"{r['vs_server_at_cursor_deg']['rms']:.4f}",
            f"  wobble p2p {r['wobble_p2p_deg']:.4f}, rms {r['wobble_rms_deg']:.4f}, period "
            + ("none" if period_ is None else
               f"{period_['frames']} frames = {period_['s'] * 1e3:.1f} ms (r={period_['corr']:.2f})"),
            f"  per-frame step: max {steps_['max_step_deg']:.4f}, mean "
            f"{steps_['step_deg']['mean']:.4f} over {steps_['frames']} frames"
            + ("" if steps_["held_frames"] is None else
               f"; step over mean-rate motion min {steps_['step_over_mean_rate']['min']:.3f} max "
               f"{steps_['step_over_mean_rate']['max']:.3f}; held frames {steps_['held_frames']}")]
    return "\n".join(lines) + "\n"


def steps(series, rate_deg_s):
    """Frame-to-frame motion of a drawn bearing against the motion its mean rate predicts."""
    deltas = [(wrap(b[1] - a[1]), b[0] - a[0]) for a, b in zip(series, series[1:]) if b[0] > a[0]]
    moving = abs(rate_deg_s) > TRAVERSING_DEG_S
    # Each frame's motion over what the mean rate moves in that frame's interval: a staircase
    # holds near 0 for some frames and jumps well above 1 in others.
    ratio = [d / (rate_deg_s * dt) for d, dt in deltas] if moving else []
    return {"frames": len(deltas),
            "held_frames": sum(1 for r in ratio if abs(r) < 0.1) if moving else None,
            "max_step_deg": max((abs(d) for d, _ in deltas), default=None),
            "step_deg": stats([abs(d) for d, _ in deltas]),
            "step_over_mean_rate": stats(ratio)}


def period(residual, dt):
    """First autocorrelation peak above 0.3 of a detrended series, in frames and seconds."""
    n = len(residual)
    mean = sum(residual) / n
    centred = [r - mean for r in residual]
    power = sum(c * c for c in centred)
    if power == 0.0:
        return None
    corr = [sum(centred[i] * centred[i + lag] for i in range(n - lag)) / power
            for lag in range(1, min(n // 2, int(2.0 / dt)))]
    for lag in range(1, len(corr) - 1):
        if corr[lag] > 0.3 and corr[lag] >= corr[lag - 1] and corr[lag] >= corr[lag + 1]:
            return {"frames": lag + 1, "s": (lag + 1) * dt, "corr": corr[lag]}
    return None


def oscillation(series, window_s):
    """A drawn bearing minus its centred moving average, where the window fits inside the series:
    what a viewer sees as wobble."""
    times = [t for t, _ in series]
    out = []
    lo = hi = 0
    for t, value in series:
        if t - window_s / 2 < times[0] or t + window_s / 2 > times[-1]:
            continue
        while times[lo] < t - window_s / 2:
            lo += 1
        while hi < len(times) and times[hi] <= t + window_s / 2:
            hi += 1
        around = [wrap(v - value) for _, v in series[lo:hi]]
        out.append(-sum(around) / len(around))
    return out


def analyze_pivot(out):
    tanks = server_bores(jsonl(out / "server-ticks.server.jsonl"))
    # The pivoting root: the one whose hull turns furthest.
    def turned(rows):
        yaws = [bearing_deg(quat_forward(rows[k]["q"])) for k in sorted(rows)]
        return sum(abs(wrap(b - a)) for a, b in zip(yaws, yaws[1:]))
    subject = max(tanks, key=lambda e: turned(tanks[e]))
    rows = tanks[subject]
    hull_truth = {k: bearing_deg(quat_forward(r["q"])) for k, r in rows.items()}
    bore_truth = {k: bearing_deg(r["bore"][3:]) for k, r in rows.items()}
    ticks = sorted(hull_truth)
    # The steady turn: from 2 s after the hull starts turning to 1 s before the script ends.
    moving = [k for a, k in zip(ticks, ticks[1:])
              if abs(wrap(hull_truth[k] - hull_truth[a])) * TICK_HZ > 1.0]
    if not moving:
        raise RuntimeError("the subject's hull never turned")
    start, end = moving[0] + 2 * TICK_HZ, moving[-1] - TICK_HZ
    rate = wrap(hull_truth[end] - hull_truth[start]) / ((end - start) / TICK_HZ)
    frames = [r for r in jsonl(out / "observer-ticks.client.jsonl")
              if r["k"] == "frame" and not r.get("ctl") and r.get("bore") and "itick" in r
              and start <= r["itick"] <= end]
    if len(frames) < 100:
        raise RuntimeError(f"{len(frames)} observer frames of the turning remote")
    dt = (frames[-1]["t"] - frames[0]["t"]) / (len(frames) - 1)
    bore = [(f["t"], bearing_deg(f["bore"][3:])) for f in frames]
    hull = [(f["t"], bearing_deg(quat_forward(f["q"]))) for f in frames]
    result = {"hull_rate_deg_s": rate, "frames": len(frames), "frame_s": dt,
              "server_bore_drift_deg": wrap(bore_truth[end] - bore_truth[start])}
    for name, series, truth in (("bore", bore, bore_truth), ("hull", hull, hull_truth)):
        residual = []
        for f, (_, drawn) in zip(frames, series):
            at = truth_at(truth, f["itick"])
            if at is not None:
                residual.append(wrap(drawn - at))
        wobble = oscillation(series, 1.0)
        moving_rate = rate if name == "hull" else (
            wrap(series[-1][1] - series[0][1]) / (series[-1][0] - series[0][0]))
        result[name] = {
            "vs_server_at_cursor_deg": stats(residual),
            "vs_server_p2p_deg": max(residual) - min(residual),
            "wobble_p2p_deg": max(wobble) - min(wobble),
            "wobble_rms_deg": stats(wobble)["rms"],
            "wobble_period": period(wobble, dt),
            "drawn_rate_deg_s": moving_rate,
            "per_frame": steps(series, moving_rate),
        }
    return result


def run(args, root, env):
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    for stale in out.glob("*.jsonl"):
        stale.unlink()
    if args.scenario == "fire":
        subject = FIRE.format(until=args.ticks - 160, ticks=args.ticks)
    else:
        subject = PIVOT.format(steer=args.steer, ticks=args.ticks)
    (out / "subject.ron").write_text(subject + "\n")
    (out / "observer.ron").write_text(OBSERVE.format(ticks=args.ticks) + "\n")
    ready = out / "workload.ready"
    ready.unlink(missing_ok=True)
    # Seeded receive jitter, one stream per process (`net::harness`).
    net = dict(SPIKE_LATENCY_MS="80", SPIKE_JITTER_MS="10")
    children = []
    logs = {name: (out / f"{name}.log").open("w") for name in ("server", "observer", "subject")}

    def await_log(name, text, child, seconds):
        by = time.monotonic() + seconds
        while text not in (out / f"{name}.log").read_text():
            if child.poll() is not None or time.monotonic() > by:
                raise RuntimeError(f"{name} never logged {text!r}: {out / (name + '.log')}")
            time.sleep(0.1)

    def client(name, seed):
        child = subprocess.Popen(
            [str(root / "target/debug/overmatch")], cwd=root,
            env=dict(env, **net, SPIKE_JITTER_SEED=str(seed), OVERMATCH_WINDOW="hidden",
                     OVERMATCH_HARNESS=str(out / f"{name}.ron"), SPIKE_WORKLOAD_READY=str(ready),
                     SPIKE_TRACE=str(out / f"{name}-ticks"),
                     SPIKE_SHOT_TRACE=str(out / f"{name}-shots")),
            stdout=logs[name], stderr=subprocess.STDOUT)
        children.append(child)
        return child

    try:
        server = subprocess.Popen(
            [str(root / "target/debug/overmatch-server")], cwd=root,
            env=dict(env, OVERMATCH_HARNESS=str(root / "scripts/harness/flat-patch.ron"),
                     SPIKE_TRACE=str(out / "server-ticks"),
                     SPIKE_SHOT_TRACE=str(out / "server-shots")),
            stdout=logs["server"], stderr=subprocess.STDOUT)
        children.append(server)
        # ASSUMED boot allowances: 30 s for the server, 60 s for a client's connection.
        await_log("server", "listening", server, 30)
        observer = client("observer", 2 * args.seed)
        await_log("server", "client connected", observer, 60)
        subject_child = client("subject", 2 * args.seed + 1)
        for name, child in (("observer", observer), ("subject", subject_child)):
            await_log(name, "harness: input slot", child, 60)
        ready.touch()
        # DERIVED bound: the script at 64 Hz plus the harness watchdog's 20 s launch slack.
        deadline = time.monotonic() + args.ticks / TICK_HZ + 60
        for name, child in (("subject", subject_child), ("observer", observer)):
            status = child.wait(timeout=max(1.0, deadline - time.monotonic()))
            if status != 0:
                raise RuntimeError(f"{name} exited {status}: {out / (name + '.log')}")
        server.send_signal(signal.SIGTERM)
        server.wait(timeout=10)
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.send_signal(signal.SIGINT)
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
        for log in logs.values():
            log.close()
    summarize(args, out)


def summarize(args, out):
    measured = analyze_fire(out) if args.scenario == "fire" else analyze_pivot(out)
    summary = {"scenario": args.scenario, "seed": args.seed,
               "subject_script": (out / "subject.ron").read_text().strip(),
               "conditioner": "SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10, seeded", "window": "hidden",
               "MEASURED": measured, "captures": str(out)}
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    text = fire_text(summary) if args.scenario == "fire" else pivot_text(summary)
    (out / "summary.txt").write_text(text)
    print(text, flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scenario", choices=("fire", "pivot"))
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--ticks", type=int, default=None)
    # Fixture steer demand for `pivot`: full, the fastest turn in place (MEASURED 5.8 deg/s).
    parser.add_argument("--steer", type=float, default=1.0)
    parser.add_argument("--analyze-only", action="store_true")
    parser.add_argument("--holding-lock", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    # Fixture lengths: `fire` spans the half-turn traverse at the idle-powered rate (MEASURED about
    # 31 s) and two shots at rest; `pivot` the 4 s lead and about 20 s of turn.
    args.ticks = args.ticks or (2400 if args.scenario == "fire" else 1600)
    root = Path(__file__).resolve().parents[2]
    if args.analyze_only:
        summarize(args, args.out.resolve())
        return
    env = dict(os.environ, OVERMATCH_CARGO_LOCK_WAIT_S="10800", OVERMATCH_UNATTENDED="1",
               OVERMATCH_MUTE="1", OVERMATCH_SERVER_BIND="127.0.0.1",
               OVERMATCH_SERVER="127.0.0.1:5888")
    for key in ("SPIKE_COST_TRACE", "SPIKE_FRAME_COST", "SPIKE_RENDER_COST", "OVERMATCH_CAPTURE",
                "OVERMATCH_BOT", "OVERMATCH_SERVER_STATS_S", "SPIKE_TRACE_SIM_FIELDS"):
        env.pop(key, None)
    if not args.holding_lock:
        subprocess.run([str(root / "scripts/serial-cargo"), "build", "--locked", "--features",
                        "dev_ui,bitprobe", "--bin", "overmatch", "--bin", "overmatch-server"],
                       cwd=root, env=env, check=True)
        os.execve("/usr/bin/lockf", ["lockf", "-k", "-t", "10800", "/tmp/overmatch-cargo.lock",
                  sys.executable, str(Path(__file__).resolve()), *sys.argv[1:], "--holding-lock"],
                  env)
    # A server outside the machine lock must not supply this run's evidence.
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as port:
        port.bind(("127.0.0.1", 5888))
    run(args, root, env)


if __name__ == "__main__":
    main()
