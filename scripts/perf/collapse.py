#!/usr/bin/env python3
"""The video game's destruction instrument, local runner and stats reader (stdlib only)."""

import argparse
import collections
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
# Resolution: five wall seconds suppress single-frame stalls in the sustained-speed report.
SUSTAINED_S = 5
# Resolution: ignore deficits below 10%, above the one-decimal period quantization noise.
SAMPLE_SPEED = 0.9


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
               OVERMATCH_SERVER_STATS_S="1", OVERMATCH_HE_STRESS=f"{args.bursts}:{args.period}",
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
                while time.monotonic() - start < args.duration:
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
                    # Sample once a sustained slowdown is visible. If none appears, sample the
                    # settled observation after the requested burst count, labelled in the output.
                    sustained = window and sum(r["period_s"] for r in window) >= SUSTAINED_S
                    slow = sustained and speed(window) < SAMPLE_SPEED
                    if args.sample and not sampled and sustained and (slow or bursts >= args.bursts):
                        print("Sampling sustained slowdown." if slow else "No slowdown yet; sampling after the bursts.", file=sys.stderr)
                        sample = subprocess.Popen(["/usr/bin/sample", str(server.pid), "3", "1", "-file", str(out / "sample.txt")],
                                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                        sampled = True
                    time.sleep(0.1)
                if server.poll() is not None:
                    raise RuntimeError(f"SERVER EXITED EARLY (status {server.returncode})")
        if not rows:
            raise RuntimeError("SERVER PRINTED NO STATS")
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
    parser.add_argument("--duration", type=positive, default=60, help="wall seconds, including server startup (default 60)")
    parser.add_argument("--bursts", type=count, default=20, help="operator shot count (default 20)")
    parser.add_argument("--period", type=positive, default=1, help="simulation seconds between shots (default 1)")
    parser.add_argument("--sample", action="store_true", help="macOS sample during sustained slowdown, or after bursts if still real time")
    parser.add_argument("--out", type=Path, help="artifact directory; default target/collapse-<timestamp>")
    parser.add_argument("--holding-lock", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    out = (args.out or ROOT / "target" / time.strftime("collapse-%Y%m%d-%H%M%S")).resolve()
    out.mkdir(parents=True, exist_ok=True)
    if args.sample and not Path("/usr/bin/sample").is_file():
        parser.error("--sample needs macOS /usr/bin/sample")
    if args.holding_lock:
        run(args, out)
        return 0
    env = os.environ.copy()
    env["OVERMATCH_CARGO_LOCK_WAIT_S"] = "10800"
    start = time.monotonic()
    built = subprocess.run([str(ROOT / "scripts/serial-cargo"), "build", "--locked", "--bin", "overmatch-server", "--features", "dev_ui,bitprobe"], cwd=ROOT, env=env)
    elapsed = time.monotonic() - start
    print(f"Build/freshness wall time (including lock wait): {elapsed:.2f} s", file=sys.stderr)
    (out / "build-seconds.txt").write_text(f"{elapsed:.3f}\n")
    if built.returncode:
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
    except (OSError, RuntimeError, subprocess.TimeoutExpired, ValueError) as error:
        print(f"collapse: {error}", file=sys.stderr)
        sys.exit(1)
