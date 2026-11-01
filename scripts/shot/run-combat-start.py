#!/usr/bin/env python3
"""The game's damage-model instruments, game state only: prove a headless Fire script starts
at combat release. Build through serial-cargo, then run under the machine lock with the shipped
mode. No frame/tick cost sampling. Usage: python3 scripts/shot/run-combat-start.py [--out DIR].
"""

import argparse
import json
import math
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys
import time


def match(pattern, text):
    found = re.search(pattern, text)
    if found is None:
        raise RuntimeError(f"missing combat-start evidence: {pattern}")
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=Path("/tmp/overmatch-combat-start"))
    parser.add_argument("--holding-lock", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    env = dict(os.environ, OVERMATCH_CARGO_LOCK_WAIT_S="10800",
               OVERMATCH_UNATTENDED="1", OVERMATCH_WINDOW="headless")
    if not args.holding_lock:
        subprocess.run([str(root / "scripts/serial-cargo"), "build", "--locked",
                        "--features", "dev_ui,bitprobe", "--bin", "overmatch",
                        "--bin", "overmatch-server"], cwd=root, env=env, check=True)
        os.execve("/usr/bin/lockf", ["lockf", "-k", "-t", "10800",
                  "/tmp/overmatch-cargo.lock", sys.executable, str(Path(__file__).resolve()),
                  "--holding-lock", "--out", str(args.out.resolve())], env)
    # A server outside the machine lock must not supply this run's evidence.
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as port:
        port.bind(("127.0.0.1", 5888))
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    for key in ("SPIKE_COST_TRACE", "SPIKE_FRAME_COST", "SPIKE_RENDER_COST", "SPIKE_TRACE",
                "OVERMATCH_CAPTURE", "OVERMATCH_HARNESS", "OVERMATCH_BOT",
                "OVERMATCH_SERVER_STATS_S"):
        env.pop(key, None)
    env.update(BEVY_ASSET_ROOT=str(root), OVERMATCH_SERVER_BIND="127.0.0.1",
               OVERMATCH_SERVER="127.0.0.1:5888")
    harness = out / "fire.ron"
    # ASSUMED workload length: 256 combat ticks. The first click deliberately tests tick zero.
    ticks = 256
    harness.write_text(f"(drive: Fire(primary: true), fire_tick: 0, ticks: {ticks})\n")
    for role in ("client", "server"):
        (out / f"{role}.{role}.jsonl").unlink(missing_ok=True)
    fill_up = float(match(r"fill_up_s:\s*([\d.]+)",
                          (root / "assets/modes/battle.ron").read_text())[1])
    # DERIVED outer bound: ASSUMED 120 s load allowance + shipped fill-up + 64 Hz script.
    deadline = 120 + math.ceil(fill_up) + math.ceil(ticks / 64)
    children = []
    with (out / "server.log").open("w") as server_log, (out / "client.log").open("w") as client_log:
        try:
            server = subprocess.Popen([str(root / "target/debug/overmatch-server")], cwd=root,
                                      env=dict(env, SPIKE_SHOT_TRACE=str(out / "server")),
                                      stdout=server_log, stderr=subprocess.STDOUT)
            children.append(server)
            # ASSUMED server readiness allowance: 30 s; no client starts against a failed boot.
            ready_by = time.monotonic() + 30
            while "server: starting, listening" not in (out / "server.log").read_text():
                if server.poll() is not None or time.monotonic() > ready_by:
                    raise RuntimeError(f"server did not start: {out / 'server.log'}")
                time.sleep(0.1)
            client = subprocess.Popen([str(root / "target/debug/overmatch")], cwd=root,
                                      env=dict(env, OVERMATCH_HARNESS=str(harness),
                                               SPIKE_SHOT_TRACE=str(out / "client")),
                                      stdout=client_log, stderr=subprocess.STDOUT)
            children.append(client)
            status = client.wait(timeout=deadline)
            if status != 0:
                raise RuntimeError(f"client exited {status}: {out / 'client.log'}")
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    child.send_signal(signal.SIGINT)
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()
    log = (out / "client.log").read_text()
    slot = match(r"harness: input slot app_s=([\d.]+)", log)
    release = match(r"harness: released script_tick=0 client_tick=(\d+) app_s=([\d.]+)", log)
    fire = match(r"harness: first fire input script_tick=0 client_tick=(\d+) app_s=([\d.]+)", log)
    complete = match(r"simulation script complete, exiting ticks=(\d+) app_s=([\d.]+)", log)
    if release[1] != fire[1] or int(complete[1]) != ticks:
        raise RuntimeError("tick zero did not fire on release, or the script did not complete")
    authority_release = int(match(r"Combat \{[^\n]* at tick (\d+)",
                                  (out / "server.log").read_text())[1])
    rows = [json.loads(line) for line in (out / "server.server.jsonl").read_text().splitlines()]
    fires = [row for row in rows if row["k"] == "fire"]
    if not fires or any((row["t"] - authority_release) % (1 << 32) >= (1 << 31) for row in fires):
        raise RuntimeError("the authority emitted no fire, or emitted during the hold")
    measured = dict(input_slot_app_s=float(slot[1]), client_release_tick=int(release[1]),
                    release_app_s=float(release[2]), authority_release_tick=authority_release,
                    first_fire_script_tick=0, first_fire_input_tick=int(fire[1]),
                    first_fire_authority_tick=fires[0]["t"], server_fire_count=len(fires),
                    completion_ticks=int(complete[1]), completion_app_s=float(complete[2]),
                    client_exit_status=status)
    result = {"MEASURED": measured, "logs": str(out)}
    (out / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
