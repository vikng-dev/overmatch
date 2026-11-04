#!/usr/bin/env python3
"""The game's damage model: hidden live walk evidence, with no frame/tick cost sampling."""

import argparse
from collections import Counter
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import time


def run_one(root, out, seconds, index):
    prefix = out / f"run-{index}"
    log_path = prefix.with_suffix(".log")
    trace_path = Path(f"{prefix}.server.jsonl")
    trace_path.unlink(missing_ok=True)
    plan = out / "wall.ron"
    # Fixture cues exercise real wall crossings and structural changes, without taking frames.
    plan.write_text('(wall: (shots: [Round(round: "88mm_AP", at: 10.0), '
                    'Charge(kg: 1.0, core: "88mm_burst_core", at: 20.0)]))\n')
    env = dict(os.environ, OVERMATCH_UNATTENDED="1", OVERMATCH_WINDOW="headless", OVERMATCH_MUTE="1",
               OVERMATCH_SERVER_BIND="127.0.0.1", OVERMATCH_BOT="30",
               SPIKE_SHOT_TRACE=str(prefix), OVERMATCH_CAPTURE=str(plan),
               BEVY_ASSET_ROOT=str(root), RUST_BACKTRACE="1")
    for key in ("SPIKE_COST_TRACE", "SPIKE_FRAME_COST", "SPIKE_RENDER_COST",
                "OVERMATCH_SERVER_STATS_S", "OVERMATCH_HARNESS", "SPIKE_TRACE"):
        env.pop(key, None)
    started = time.monotonic()
    with log_path.open("w") as log:
        server = subprocess.Popen([str(root / "target/debug/overmatch-server")],
                                  cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            while "spawned circling bot 29 " not in log_path.read_text():
                if server.poll() is not None:
                    raise RuntimeError(f"server exited during startup: {log_path}")
                if time.monotonic() - started > 120:
                    raise RuntimeError(f"bots did not spawn: {log_path}")
                time.sleep(0.2)
            live = time.monotonic()
            while time.monotonic() - live < seconds:
                if server.poll() is not None:
                    raise RuntimeError(f"server exited during live run: {log_path}")
                time.sleep(0.2)
            elapsed = time.monotonic() - live
        finally:
            if server.poll() is None:
                # Bevy's terminal handler exits the app and flushes the final trace buffers.
                server.send_signal(signal.SIGINT)
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
    rows = [json.loads(line) for line in trace_path.read_text().splitlines()]
    crossings = Counter(kind for row in rows if row["k"] == "walk_crossing"
                        for kind in row["kinds"])
    census = [row for row in rows if row["k"] == "walk_world"]
    stops = sum(row["k"] == "walk_first_contact_stop" for row in rows)
    panics = log_path.read_text().count("panicked at")
    result = {"run": index, "measured_live_seconds": elapsed,
              "first_contact_stops": stops, "walk_panics": panics,
              "crossing_events_by_kind": {kind: crossings[kind]
                  for kind in ("tank", "construction", "other", "retired")},
              "constructions_placed": max((row["constructions"] for row in census), default=0),
              "max_tanks": max((row["tanks"] for row in census), default=0),
              "construction_changes": max((row["construction_changes"] for row in census), default=0),
              "trace": str(trace_path), "log": str(log_path)}
    prefix.with_suffix(".json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
    if stops or panics or result["max_tanks"] < 30 or not result["constructions_placed"] or not result["construction_changes"]:
        raise RuntimeError(f"live walk gate failed: {prefix.with_suffix('.json')}")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=Path("/tmp/overmatch-walk"))
    parser.add_argument("--seconds", type=float, default=170.0)
    parser.add_argument("--holding-lock", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not 160.0 <= args.seconds < float("inf"):
        parser.error("--seconds must be finite and at least 160")
    root = Path(__file__).resolve().parents[2]
    if not args.holding_lock:
        env = dict(os.environ, OVERMATCH_CARGO_LOCK_WAIT_S="10800",
                   OVERMATCH_UNATTENDED="1", OVERMATCH_WINDOW="headless")
        subprocess.run([str(root / "scripts/serial-cargo"), "build", "--locked",
                        "--bin", "overmatch-server", "--features", "dev_ui,bitprobe"],
                       cwd=root, env=env, check=True)
        os.execv("/usr/bin/lockf", ["lockf", "-k", "-t", "10800",
                 "/tmp/overmatch-cargo.lock", sys.executable, str(Path(__file__).resolve()),
                 "--holding-lock", "--out", str(args.out.resolve()), "--seconds", str(args.seconds)])
    args.out.mkdir(parents=True, exist_ok=True)
    results = [run_one(root, args.out, args.seconds, index) for index in (1, 2)]
    (args.out / "summary.json").write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
