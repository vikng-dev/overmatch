#!/usr/bin/env python3
"""The video game's destruction instrument (stdlib only).

Real match: --schedule scripts/perf/match-collapse.json --tanks 30 --duration 60.
Targets are local, or an operator-started Edgegap IP:PORT with --deployment ID. Before
starting that deployment, pass each --print-server-env output line to session.sh's --env.
The remote command reads logs only; it never deploys or stops a deployment.

Schedule times are wall seconds after the server admits the requested tank count into
combat. IDs are map instance IDs; one event breaks its IDs together. Targets must remain
untouched until their event. The game's normal construction laws receive work derived
from each support's volume pool, including buried supports, then launch and replicate
the unsupported matter. This instruments collapse, rather than projectile effectiveness.

Every report row is a complete server stats period, with its own nearest-rank percentiles;
the first period straddling admission is omitted. Other tick work includes game systems
and engine scheduling outside PhysicsSchedule. Send bytes are attempted encrypted payload
bytes before transport; raw rows also retain send time, RSS and quota throttling.
Hosted runs require kernel evidence of two vCPU. --rehearsal is explicitly unpriced local
plumbing on the recorded host; concurrent lanes make its timings unsuitable for pricing.
Without --schedule, the existing tankless HE diagnostic remains available.
"""

import argparse
import collections
import json
import math
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

from online_cost import ANSI

ROOT = Path(__file__).resolve().parents[2]
# Resolution: five wall seconds suppress single-frame stalls in the sustained-speed report.
SUSTAINED_S = 5
# Resolution: ignore deficits below 10%, above the one-decimal period quantization noise.
SAMPLE_SPEED = 0.9
# Resolution: one-second telemetry bins locate burst spikes without per-tick log traffic.
STATS_PERIOD_S = 1
# Resolution: bound the log reader's lag without busy-waiting beside the server.
POLL_S = 0.1
# Resolution: two log requests per minute reduce API pressure; stats retain server time.
# This is a conservative reader cadence, not an asserted API quota.
HOSTED_LOG_POLL_S = 30
# Resolution: bound one unresponsive log request without consuming the whole run watchdog.
LOG_FETCH_TIMEOUT_S = 30
# session.sh logs interface: EX_TEMPFAIL means HTTP 429; all other failures remain fatal.
LOGS_RATE_LIMITED = 75


def recipe_for(path, tanks):
    """The game's destruction schedule names data IDs; concurrency is one event's ID list."""
    value = json.loads(path.read_text())
    if not isinstance(value, dict) or set(value) != {"events"} or not isinstance(value["events"], list) or not value["events"]:
        raise ValueError(f"{path}: expected {{events: [{{at_s, instances}}, ...]}}")
    seen, previous = set(), -1
    for event in value["events"]:
        if not isinstance(event, dict) or set(event) != {"at_s", "instances"}:
            raise ValueError(f"{path}: each event must name at_s and instances")
        at, ids = event["at_s"], event["instances"]
        if isinstance(at, bool) or not isinstance(at, (int, float)) or not math.isfinite(at) or at < previous or at < 0:
            raise ValueError(f"{path}: at_s must be finite, nonnegative and ascending")
        if not isinstance(ids, list) or not ids or any(not isinstance(i, str) or not i or i in seen for i in ids) or len(set(ids)) != len(ids):
            raise ValueError(f"{path}: instances must be nonempty unique IDs, used only once")
        seen.update(ids)
        previous = at
    return dict(tanks=tanks, **value)


def allocation(text, rehearsal=False):
    """Only the kernel quota/cpuset and available_parallelism attest a hosted allocation."""
    lines = [ANSI.sub("", line) for line in text.splitlines() if "server: host available_parallelism=" in line]
    if len(lines) != 1:
        raise RuntimeError("allocation: expected exactly one server startup host line")
    line = lines[0]
    if rehearsal:
        return "UNPRICED LOCAL REHEARSAL: " + line
    available = re.search(r"available_parallelism=(\d+)\b", line)
    quota = re.search(r"cpu_quota=[\d.]+ cpus \((\d+)/(\d+) us\)", line)
    cpuset = re.search(r"cpuset=([0-9,-]+)(?:\s|$)", line)
    allowed = set()
    if cpuset:
        for span in cpuset[1].split(","):
            ends = [int(v) for v in span.split("-")]
            if len(ends) == 1:
                allowed.add(ends[0])
            elif len(ends) == 2 and ends[0] <= ends[1]:
                allowed.update(range(ends[0], ends[1] + 1))
            else:
                raise RuntimeError("allocation: malformed cpuset")
    # DERIVED requirement: exactly two usable CPUs, from a quota of two or a two-CPU cpuset.
    quota_cpus = int(quota[1]) / int(quota[2]) if quota and int(quota[2]) > 0 else None
    verified = (quota_cpus == 2 and (not allowed or len(allowed) >= 2)) or (len(allowed) == 2 and ("cpu_quota=unlimited" in line or quota_cpus is not None and quota_cpus >= 2))
    if not available or int(available[1]) != 2 or not verified:
        raise RuntimeError("allocation: two vCPU not verified from available_parallelism and quota/cpuset: " + line)
    return line


def match_rows(text, recipe):
    rows = []
    for number, line in enumerate(text.splitlines(), 1):
        line = ANSI.sub("", line)
        if "server_stats " not in line:
            continue
        try:
            row = json.loads(line.split("server_stats ", 1)[1])
            load = row["loadtest"]
            for key in ("sequence", "send_bytes", "awake_bodies", "construction_pieces", "constructions", "events_fired"):
                if type(load[key]) is not int or load[key] < 0:
                    raise ValueError(f"invalid loadtest.{key}")
            for key in ("tanks", "clients", "ticks"):
                if type(row[key]) is not int or row[key] < 0:
                    raise ValueError(f"invalid {key}")
            for key in ("period_s", "budget_us"):
                if not isinstance(row[key], (int, float)) or not math.isfinite(row[key]) or row[key] <= 0:
                    raise ValueError(f"missing positive {key}")
            if not isinstance(row["tick_hz"], (int, float)) or not math.isfinite(row["tick_hz"]) or row["tick_hz"] < 0:
                raise ValueError("missing nonnegative tick_hz")
            if load["elapsed_s"] is not None and (not isinstance(load["elapsed_s"], (int, float)) or not math.isfinite(load["elapsed_s"]) or load["elapsed_s"] < 0):
                raise ValueError("invalid loadtest.elapsed_s")
            if load["elapsed_s"] is not None and (not isinstance(load["window_start_s"], (int, float)) or not math.isfinite(load["window_start_s"]) or load["window_start_s"] >= load["elapsed_s"]):
                raise ValueError("invalid loadtest.window_start_s")
            if load["recipe"] != recipe:
                raise RuntimeError("server recipe differs from requested schedule/tanks")
            if load["sequence"] != len(rows) + 1:
                raise RuntimeError("missing, duplicated or reordered server stats window")
            rows.append(row)
        except (ValueError, KeyError, TypeError) as error:
            raise RuntimeError(f"server log line {number}: malformed/incompatible stats: {error}") from error
    return rows


def complete_windows(rows, recipe, duration, clients):
    """Keep whole server periods after admission; never estimate global percentiles from p95s."""
    active = [r for r in rows if r["loadtest"]["elapsed_s"] is not None]
    windows = []
    for row in active:
        load = row["loadtest"]
        elapsed = load["elapsed_s"]
        # The first stats period can straddle admission. It is retained in rows.json, but is not
        # a full workload window. Subsequent periods are contiguous and end at server timestamps.
        if load["window_start_s"] < 0:
            continue
        if row["tanks"] != recipe["tanks"] or row["clients"] != clients or not load["combat"]:
            raise RuntimeError("window lost its requested tank population, real link or combat")
        for key in ("period_s", "ticks", "budget_us", "cpu_pct", "tick_hz"):
            if not isinstance(row[key], (int, float)) or not math.isfinite(row[key]) or row[key] < 0:
                raise RuntimeError(f"window missing finite {key}")
        if row["ticks"] == 0 or row["period_s"] == 0:
            raise RuntimeError("window has no completed ticks or wall time")
        for key in ("tick_us", "physics_us", "send_us"):
            if row[key] is None or any(not isinstance(row[key][q], (int, float)) or not math.isfinite(row[key][q]) or row[key][q] < 0 for q in ("p50", "p95", "p99", "total")):
                raise RuntimeError(f"window missing {key} timing samples")
        if row["physics_us"]["total"] > row["tick_us"]["total"]:
            raise RuntimeError("physics total exceeds the containing fixed ticks")
        if windows:
            # Resolution: one microsecond allows f64 subtraction noise in monotonic clock edges.
            if abs(load["window_start_s"] - windows[-1]["loadtest"]["elapsed_s"]) > 1e-6:
                raise RuntimeError("incomplete/noncontiguous server windows")
        windows.append(row)
        if elapsed >= duration:
            break
    if not windows or windows[-1]["loadtest"]["elapsed_s"] < duration:
        return None
    last = windows[-1]["loadtest"]
    if last["events_fired"] != len(recipe["events"]):
        raise RuntimeError("schedule incomplete: not all events fired")
    expected = {i for e in recipe["events"] for i in e["instances"]}
    targets = {t["id"]: t for t in last["targets"]}
    if set(targets) != expected or any(t["falls"] == 0 for t in targets.values()):
        raise RuntimeError("schedule incomplete: each named construction must have a derived departure")
    return windows


def report_match(rows):
    print("MEASURED instrument observations; each row is one complete server window. CPU is % of one core; shares use summed tick/physics wall time; send is queued encrypted payload bytes, excluding IP/UDP headers.")
    print("| end s | span s | tanks | tick p50/p95/p99 ms | budget ms | CPU % | physics/other tick % | awake | pieces | send B | Hz |")
    print("|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for row in rows:
        load = row["loadtest"]
        tick = row["tick_us"]
        share = 100 * row["physics_us"]["total"] / tick["total"]
        print(f"| {load['elapsed_s']:.2f} | {load['elapsed_s'] - load['window_start_s']:.2f} | {row['tanks']} | "
              + "/".join(f"{tick[q] / 1000:.3f}" for q in ("p50", "p95", "p99"))
              + f" | {row['budget_us'] / 1000:.3f} | {row['cpu_pct']:.1f} | {share:.1f}/{100-share:.1f} | {load['awake_bodies']} | {load['construction_pieces']} | {load['send_bytes']} | {row['tick_hz']:.1f} |")


def run_match(args, out, recipe):
    """Reuse the online runner's scripted GPU-less links and session.sh log snapshots."""
    import socket
    children, logs = [], []
    local = args.target == "local"
    address = "127.0.0.1:5888" if local else args.target
    env = os.environ.copy()
    # Inherited stress/trace levers would silently add a second workload.
    for key in list(env):
        if key.startswith(("OVERMATCH_", "SPIKE_")):
            del env[key]
    env.update(OVERMATCH_UNATTENDED="1", OVERMATCH_WINDOW="headless", OVERMATCH_MUTE="1", BEVY_ASSET_ROOT=str(ROOT), NO_COLOR="1")
    env["OVERMATCH_LOADTEST"] = json.dumps(recipe, separators=(",", ":"))
    serverlog = out / "server.log"
    (out / "rows.json").unlink(missing_ok=True)
    client_count = args.clients or args.tanks
    # DERIVED: the script outlives both admission and observation at the server's tick rate.
    tick_ns = int(re.search(r"const TICK: Duration = Duration::from_nanos\(([\d_]+)\)", (ROOT / "src/net/server.rs").read_text())[1].replace("_", ""))
    ticks = math.ceil((args.duration + 2 * args.startup_grace + 60) * 1e9 / tick_ns)
    script = out / "load.harness.ron"
    script.write_text(f"(ticks: {ticks}, drive: Arc, aim: Sweep, fire_tick: {ticks})\n")
    def spawn(binary, logpath, extra):
        log = logpath.open("w")
        logs.append(log)
        child = subprocess.Popen([str(args.bin / binary)], cwd=ROOT, env=dict(env, **extra), stdout=log, stderr=subprocess.STDOUT)
        children.append(child)
        return child
    next_log_poll, log_poll_s = 0.0, HOSTED_LOG_POLL_S
    def snapshot(deadline):
        nonlocal next_log_poll, log_poll_s
        if not local:
            now = time.monotonic()
            if now < next_log_poll or now >= deadline:
                return None  # Polls never bypass the cadence or the run watchdog.
            try:
                subprocess.run([str(ROOT / "scripts/edgegap/session.sh"), "logs", args.deployment, str(serverlog)],
                               check=True, timeout=min(LOG_FETCH_TIMEOUT_S, deadline - now), stdout=subprocess.DEVNULL)
            except subprocess.CalledProcessError as error:
                if error.returncode != LOGS_RATE_LIMITED:
                    raise
                # Resolution: exponential backoff reduces pressure until success or the wall deadline.
                log_poll_s *= 2
                next_log_poll = time.monotonic() + log_poll_s
                print(f"Hosted logs rate limited (HTTP 429); retry in {log_poll_s:g} s if within the run deadline.", file=sys.stderr, flush=True)
                return None
            log_poll_s = HOSTED_LOG_POLL_S
            next_log_poll = time.monotonic() + log_poll_s
        return serverlog.read_text() if serverlog.exists() else ""
    start = time.monotonic()
    try:
        if local:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
                sock.bind(("127.0.0.1", 5888))
            spawn("overmatch-server", serverlog, dict(OVERMATCH_SERVER_BIND="127.0.0.1", OVERMATCH_SERVER_STATS_S="1", OVERMATCH_BOT=str(args.tanks-client_count)))
        # Read the real startup line before spending time admitting clients.
        admission_deadline = start + args.startup_grace
        while True:
            if any(c.poll() is not None for c in children) or time.monotonic() >= admission_deadline:
                raise RuntimeError("no verified server startup line before admission deadline")
            text = snapshot(admission_deadline)
            if text is not None and "server: host available_parallelism=" in text:
                verified_host = allocation(text, args.rehearsal)
                print(verified_host, flush=True)
                break
            time.sleep(min(POLL_S, max(0, admission_deadline - time.monotonic())))
        clients = [spawn("overmatch", out / f"client{i}.log", dict(OVERMATCH_SERVER=address, OVERMATCH_HARNESS=str(script))) for i in range(client_count)]
        deadline = start + 2 * args.startup_grace + args.duration
        while True:
            if any(c.poll() is not None for c in children):
                raise RuntimeError("server or load client exited before the window closed")
            if time.monotonic() >= deadline:
                raise RuntimeError("incomplete windows or admission: server did not finish the observation by the wall deadline")
            text = snapshot(deadline)
            if text is not None:
                if allocation(text, args.rehearsal) != verified_host:
                    raise RuntimeError("server startup allocation changed during the run")
                rows = match_rows(text, recipe)
                windows = complete_windows(rows, recipe, args.duration, client_count)
                if windows:
                    for i, _ in enumerate(clients):
                        log = (out / f"client{i}.log").read_text()
                        if log.count("client: connected (entity") != 1 or "harness: released" not in log:
                            raise RuntimeError(f"client{i}: requires one connection and synchronized combat release")
                    (out / "rows.json").write_text(json.dumps(rows, indent=2) + "\n")
                    report_match(windows)
                    last = windows[-1]["loadtest"]
                    print(f"COMPLETE: {args.tanks} tanks, {client_count} real links, {last['constructions']} constructions, {len(windows)} complete windows, {last['events_fired']} events; game construction outcomes {json.dumps(last['targets'])}; artifacts: {out}")
                    return
            time.sleep(min(POLL_S, max(0, deadline - time.monotonic())))
    except BaseException:
        print(f"MATCH RUN INVALID; server/client logs: {out}", file=sys.stderr)
        raise
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.kill()
            child.wait()
        for log in logs:
            log.close()


def positive(text):
    value = float(text)
    if not math.isfinite(value) or value <= 0:
        raise argparse.ArgumentTypeError("must be finite and positive")
    return value


def count(text):
    value = int(text)
    if not 0 < value <= 2**32 - 1:
        raise argparse.ArgumentTypeError("must be a positive u32")
    return value


def speed(rows):
    return sum(r["ticks"] * r["budget_us"] / 1e6 for r in rows) / sum(
        r["period_s"] for r in rows
    )


def milliseconds(row, field):
    summary = row[field]
    return None if summary is None else summary["p50"] / 1000


def display(value, places=2):
    return "-" if value is None else f"{value:.{places}f}"


def report(rows):
    print("| time s | bursts | awake | asleep | speed | tick ms | physics ms | send ms | RSS MiB |")
    print("|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for row in rows:
        print("| " + " | ".join([
            display(row["wall_s"], 1), str(row["bursts"]),
            str(row["debris"]["live"]), str(row["debris"]["sleeping"]),
            display(speed([row])) + "x", display(milliseconds(row, "tick_us")),
            display(milliseconds(row, "physics_us")), display(milliseconds(row, "send_us")),
            display(row["rss_mb"], 1),
        ]) + " |")
    window = collections.deque()
    worst = None
    for row in rows:
        window.append(row)
        while len(window) > 1 and sum(r["period_s"] for r in list(window)[1:]) >= SUSTAINED_S:
            window.popleft()
        if sum(r["period_s"] for r in window) >= SUSTAINED_S:
            if worst is None or speed(window) < speed(worst):
                worst = list(window)
    if worst is None:
        print(f"No {SUSTAINED_S} s sustained window; increase --duration.")
        return
    # These are duration-weighted means of each period's p50, not whole-run percentiles.
    def mean(field):
        present = [r for r in worst if r[field] is not None]
        if not present:
            return None
        return sum(milliseconds(r, field) * r["period_s"] for r in present) / sum(r["period_s"] for r in present)
    tick, physics, send = [mean(f) for f in ("tick_us", "physics_us", "send_us")]
    other = None if tick is None or physics is None else tick - physics
    print(f"Worst sustained speed: {speed(worst):.3f}x at {worst[0]['wall_s'] - worst[0]['period_s']:.1f}–{worst[-1]['wall_s']:.1f} s; "
          f"mean period p50: tick {display(tick)} ms, physics {display(physics)} ms, "
          f"other tick work {display(other)} ms, send {display(send)} ms (outside tick).")


def top_symbols(path):
    text = path.read_text()
    anchor = "Sort by top of stack, same collapsed (when >= 5):"
    if anchor in text:
        lines = text.split(anchor, 1)[1].split("Binary Images:", 1)[0].strip().splitlines()[:20]
    else:
        lines = text.split("Call graph:", 1)[-1].splitlines()[:25]
    demangled = subprocess.run(["/usr/bin/c++filt"], input="\n".join(lines),
                               text=True, capture_output=True, check=True).stdout
    print("macOS sample top symbols (includes waiting threads):\n" + demangled)


def run(args, out):
    rows = []
    bursts = fired = 0
    sample = None
    sampled = False
    window = collections.deque()
    env = os.environ.copy()
    env.update(OVERMATCH_UNATTENDED="1", OVERMATCH_WINDOW="headless", OVERMATCH_MUTE="1",
               OVERMATCH_SERVER_BIND="127.0.0.1", OVERMATCH_BOT="0", OVERMATCH_DEBRIS_STRESS="off",
               OVERMATCH_SERVER_STATS_S=str(STATS_PERIOD_S), OVERMATCH_HE_STRESS=f"{args.bursts}:{args.period}",
               BEVY_ASSET_ROOT=str(ROOT), RUST_BACKTRACE="1")
    # Refuse a busy port before the child can fail, without contacting another server.
    import socket
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind(("127.0.0.1", 5888))
    logpath = out / "server.log"
    start = time.monotonic()
    server = None
    try:
        with logpath.open("w") as log:
            server = subprocess.Popen([str(ROOT / "target/debug/overmatch-server")], cwd=ROOT,
                                      env=env, stdout=log, stderr=subprocess.STDOUT)
            with logpath.open() as reader:
                while True:
                    if server.poll() is not None:
                        raise RuntimeError(f"SERVER EXITED EARLY (status {server.returncode})")
                    for line in reader.readlines():
                        if line.startswith("he_stress "):
                            census = json.loads(line.removeprefix("he_stress "))
                            bursts, fired = census["bursts"], census["fired"]
                        elif line.startswith("server_stats "):
                            row = json.loads(line.removeprefix("server_stats "))
                            row.update(wall_s=time.monotonic() - start, bursts=bursts)
                            rows.append(row)
                            window.append(row)
                            while len(window) > 1 and sum(r["period_s"] for r in list(window)[1:]) >= SUSTAINED_S:
                                window.popleft()
                    now = time.monotonic()
                    if now - start >= args.duration:
                        break
                    # Sample once a sustained slowdown is visible. If none appears, sample the
                    # settled observation after the requested burst count, labelled in the output.
                    sustained = window and sum(r["period_s"] for r in window) >= SUSTAINED_S
                    slow = sustained and speed(window) < SAMPLE_SPEED
                    if args.sample and not sampled and sustained and (slow or bursts >= args.bursts):
                        print("Sampling sustained slowdown." if slow else "No slowdown yet; sampling after the bursts.", file=sys.stderr)
                        sample = subprocess.Popen(["/usr/bin/sample", str(server.pid), "3", "1", "-file", str(out / "sample.txt")],
                                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                        sampled = True
                    time.sleep(min(POLL_S, args.duration - (now - start)))
                if server.poll() is not None:
                    raise RuntimeError(f"SERVER EXITED EARLY (status {server.returncode})")
        if not rows:
            raise RuntimeError("SERVER PRINTED NO STATS")
        tail_s = time.monotonic() - start - rows[-1]["wall_s"]
        # A report is due every stats period, with at most one reader poll of delivery lag.
        # Beyond that bound the final interval is censored, not evidence of healthy speed.
        if tail_s > STATS_PERIOD_S + POLL_S:
            raise RuntimeError(f"UNMEASURED TRAILING INTERVAL: no stats for {tail_s:.2f} s at deadline; run incomplete")
        if sample:
            _, error = sample.communicate(timeout=15)
            if sample.returncode:
                raise RuntimeError("macOS sample failed: " + error.decode())
            top_symbols(out / "sample.txt")
        elif args.sample:
            raise RuntimeError("No sample window reached; increase --duration")
        if server.poll() is not None:
            raise RuntimeError(f"SERVER EXITED EARLY (status {server.returncode})")
        report(rows)
        print(f"Completed {time.monotonic() - start:.2f} s; fired {fired}/{args.bursts}, actual bursts {bursts}; artifacts: {out}")
        (out / "rows.json").write_text(json.dumps(rows, indent=2) + "\n")
    except BaseException:
        lines = logpath.read_text().splitlines()
        # Keep the cause even when a long backtrace follows it.
        causes = [line for line in lines[:-80] if any(word in line.lower() for word in ("panic", "fatal", "error"))]
        print("\n!!! COLLAPSE RUN FAILED !!!\n" + "\n".join(causes + lines[-80:]), file=sys.stderr)
        raise
    finally:
        if sample and sample.poll() is None:
            sample.kill()
            sample.wait()
        if server and server.poll() is None:
            server.kill()
            server.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--duration", type=positive, default=60, help="observation wall seconds after match admission; tankless diagnosis includes startup (default 60)")
    parser.add_argument("--bursts", type=count, default=20, help="operator shot count (default 20)")
    parser.add_argument("--period", type=positive, default=1, help="simulation seconds between shots (default 1)")
    parser.add_argument("--sample", action="store_true", help="macOS sample during sustained slowdown, or after bursts if still real time")
    parser.add_argument("--out", type=Path, help="artifact directory; default target/collapse-<timestamp>")
    parser.add_argument("--schedule", type=Path, help="real-match mode: JSON {events: [{at_s, instances}]} for the game's construction workload")
    parser.add_argument("--target", default="local", help="local, or ip:port of a deployment the operator already started")
    parser.add_argument("--deployment", help="existing Edgegap deployment ID; only its logs are read")
    parser.add_argument("--tanks", type=count, default=30, help="total tanks, including bots and real links (owner workload: 30)")
    parser.add_argument("--clients", type=count, help="real window-less clients; remaining tanks are bots (default: all tanks)")
    parser.add_argument("--startup-grace", type=positive, default=90, help="admission and trailing-window watchdog seconds")
    parser.add_argument("--bin", type=Path, default=ROOT / "target/debug", help="binary directory (default target/debug; other directories must already be built)")
    parser.add_argument("--rehearsal", action="store_true", help="local plumbing only: print unpriced observations without claiming a two-vCPU allocation")
    parser.add_argument("--print-server-env", action="store_true", help="print the recipe/environment for the operator's deployment, then exit without a build or run")
    parser.add_argument("--holding-lock", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.bin = args.bin.resolve()
    if args.clients is not None and args.clients > args.tanks:
        parser.error("--clients cannot exceed --tanks")
    if args.rehearsal and args.target != "local":
        parser.error("--rehearsal is local only; hosted runs must verify two vCPU")
    if args.target != "local" and (not args.schedule or not args.deployment):
        parser.error("remote runs require --schedule and --deployment")
    recipe = recipe_for(args.schedule, args.tanks) if args.schedule else None
    if args.deployment and args.target == "local":
        parser.error("--deployment requires its explicit remote --target IP:PORT")
    if args.rehearsal and not recipe:
        parser.error("--rehearsal requires a real-match --schedule")
    if recipe and args.sample:
        parser.error("--sample belongs to the tankless diagnostic; match mode observes server windows")
    if args.print_server_env:
        if not recipe:
            parser.error("--print-server-env requires --schedule")
        print("OVERMATCH_LOADTEST=" + json.dumps(recipe, separators=(",", ":")))
        print(f"OVERMATCH_BOT={args.tanks - (args.clients or args.tanks)}")
        print("OVERMATCH_SERVER_STATS_S=1")
        return 0
    if recipe and args.duration <= recipe["events"][-1]["at_s"] + STATS_PERIOD_S:
        parser.error("--duration must leave a complete stats period after the last scheduled break")
    out = (args.out or ROOT / "target" / time.strftime("collapse-%Y%m%d-%H%M%S")).resolve()
    out.mkdir(parents=True, exist_ok=True)
    if args.sample and not Path("/usr/bin/sample").is_file():
        parser.error("--sample needs macOS /usr/bin/sample")
    if args.holding_lock:
        if recipe:
            run_match(args, out, recipe)
        else:
            run(args, out)
        return 0
    env = os.environ.copy()
    env["OVERMATCH_CARGO_LOCK_WAIT_S"] = "10800"
    start = time.monotonic()
    build = [str(ROOT / "scripts/serial-cargo"), "build", "--locked"]
    if not recipe or args.target == "local":
        build.extend(["--bin", "overmatch-server"])
    if recipe:
        build.extend(["--bin", "overmatch"])
    build.extend(["--features", "dev_ui,bitprobe"])
    built = subprocess.run(build, cwd=ROOT, env=env) if args.bin == ROOT / "target/debug" else None
    elapsed = time.monotonic() - start
    print(f"Build/freshness wall time (including lock wait): {elapsed:.2f} s", file=sys.stderr)
    (out / "build-seconds.txt").write_text(f"{elapsed:.3f}\n")
    if built and built.returncode:
        return built.returncode
    command = ["/usr/bin/lockf", "-k", "-t", "10800", "/tmp/overmatch-cargo.lock", sys.executable,
               str(Path(__file__).resolve()), *sys.argv[1:], "--out", str(out), "--holding-lock"]
    child = subprocess.Popen(command, cwd=ROOT, start_new_session=True)
    try:
        return child.wait()
    finally:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()


if __name__ == "__main__":
    # The lock holder must survive long enough to reap its child on operator cancellation.
    def interrupted(signum, frame):
        raise KeyboardInterrupt(f"signal {signum}")
    signal.signal(signal.SIGTERM, interrupted)
    try:
        sys.exit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError, ValueError, KeyError, TypeError) as error:
        print(f"collapse: {error}", file=sys.stderr)
        sys.exit(1)
