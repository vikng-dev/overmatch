#!/usr/bin/env python3
"""A parked tank's server cost on a rubble pile against flat ground (the game's tank driving and
environment destruction; game state only). Stdlib only.

    /usr/bin/lockf -k -t 2400 /tmp/overmatch-cargo.lock \\
        python3 scripts/perf/pile_park.py --out target/pile-park [--bin target/release]

Holding the build lock keeps every `scripts/serial-cargo` compile off the machine for the run;
before each condition the runner also waits for any other `rustc` to finish (at most
--quiet-wait-s) and records what it saw. It holds the machine awake (`caffeinate -ims -w`) while
it runs. No process takes a screen or makes a sound: the server is headless, the one client
GPU-less (`OVERMATCH_WINDOW=headless`, `OVERMATCH_UNATTENDED=1`, `OVERMATCH_MUTE=1`).

Conditions (default `pile flat rubble rubble flat pile`: each twice, as a palindrome, so a drift
linear in time cancels from each condition's mean). Each runs a fresh local server and one
scripted client (`net::harness`), whose tank is the only one:
  pile    spawns --runup-m short of the map's `house_0001` on the heading of
          `scripts/perf/ram.json`, holds full throttle for --throttle-ticks, brakes to a stop and
          then has no demand (`Drive::Park`): it rams the house and parks in its rubble;
  flat    spawns on flat ground (--flat-pose, a patch of 20 cm relief over 40 m on the shipped
          heightmap) and never drives (`Drive::Idle`);
  rubble  the flat run while the recorder fells `house_0001` at --fell-at-s: that house's rubble
          with no tank on it, which splits a pile run's excess between the pile and the tank.
The house's ground and the flat patch are read off the shipped heightmap, so a map change moves
both: the spawn y is the ground's highest point within 5 m plus 2 m, as `spawn_pos` resolves it.

The recorder is the scheduled-collapse trace (`OVERMATCH_LOADTEST`, src/net/loadtest.rs): every
fixed tick from combat admission, with its stage and phase wall times, Avian's step counters and
the production work counts. Outside a rubble run its one event names a construction far from both
spawns, at a time the run never reaches, so it fells nothing. The window is the trace's ticks
whose `elapsed_s` lies in [lead + --settle-s, lead + --settle-s + --park-s], where lead is the
throttle's end (the harness's 128-tick lead-in plus --throttle-ticks, DERIVED at 64 Hz) and
--settle-s covers the braking; every condition uses the same window. `tick_us` includes the
contact census leaves the trace adds (`contact_census_us`, `sweep_census_us`); `net_us` subtracts
them. The collector runs outside `tick_us`.

Gates per run (a failed one marks it INVALID in summary.json and leaves the pooled tables):
the server listened, the client was released and both were alive at the window's close; the
window holds at least 95 % of its ticks, each with one tank; over the settle span before the
window, while the tank is awake, a pile run's belt overlaps loose pieces (`work.belt_overlaps` > 0
on average) and a flat or rubble run's overlaps none. Whether a sleeping tank does belt work is
no part of the witness. Evidence is refused, never patched, naming the file and line: a stats
record that does not parse (only the kill's torn final line, with no line end, is skipped), a
record without its trace, a trace tick lacking a quantity the run reads, a failed process census,
or a settle span with no tick.

Products in --out: <run>.server.log, <run>.client.log, <run>.window.jsonl (the window's tick
rows), <run>.harness.ron, summary.json (per run and pooled per condition) and summary.txt.
Numbers from a run whose manifest shows other compiles or a loaded machine are not timings.
"""

import argparse
import json
import math
import os
from pathlib import Path
import signal
import socket
import statistics
import struct
import subprocess
import sys
import time
import zlib

ROOT = Path(__file__).resolve().parents[2]
ADDRESS = "127.0.0.1:5888"
# Harness facts (src/net/harness.rs): the fixed rate, and `Drive::Park`'s lead-in before throttle.
TICK_HZ = 64
LEAD_TICKS = 128
# spawn_pos (src/terrain_grid.rs): the ground's highest point within this half-side, plus clearance.
SPAWN_HALF_M = 5.0
SPAWN_CLEARANCE_M = 2.0
# Resolution: a window must hold this share of its ticks to count.
WINDOW_SHARE = 0.95
# The recorder's never-fired event: a construction far from both spawns, after any run's end.
IDLE_RECORDER = {"tanks": 1, "events": [{"at_s": 1.0e6, "instances": ["house_0037"]}]}
CENSUS_LEAVES = ("contact_census_us", "sweep_census_us")
COUNTS = ("awake", "avian.pairs", "avian.touching_pairs", "avian.points", "avian.constraints",
          "avian.colors", "avian.broad_phase_candidates", "work.contact_points",
          "work.belt_near_colliders", "work.belt_candidates", "work.belt_slab_checks",
          "work.belt_slab_aabb_passes", "work.belt_overlaps", "work.belt_dynamic_contacts",
          "work.belt_dispatches", "work.belt_partition_faces", "work.belt_partition_cells",
          "work.belt_partition_patches", "work.belt_terrain_samples", "sweep_casts")
TIMES = ("tick_us", "net_us", "physics_us", "sleep_us", "tick_cpu_us", "belt_contacts_us",
         "contacts_us", "sweep_us", "contact_census_us", "phases_us.driving_us",
         "phases_us.physics_step_us", "phases_us.post_physics_us", "phases_us.fixed_update_us",
         "phases_us.fixed_gameplay_us", "avian.narrow_phase_us", "avian.broad_phase_us",
         "avian.prepare_constraints_us", "avian.solve_constraints_us", "avian.warm_start_us",
         "avian.relax_velocities_us", "avian.integrate_velocities_us",
         "avian.integrate_positions_us", "avian.collider_tree_update_us", "avian.finalize_us")
# Every quantity a trace tick must carry: the producer serializes each one, zero included.
READ = tuple(dict.fromkeys(("elapsed_s", "tanks", "work.belt_overlaps") + COUNTS + CENSUS_LEAVES
                           + tuple(k for k in TIMES if k != "net_us")))


def heightmap():
    """The shipped heightmap's metres and its sample-centre mapping (level.json, 16-bit grey)."""
    level = json.loads((ROOT / "assets/maps/kalinovo/level.json").read_text())
    terrain = level["terrain"]["heightmap"]
    assert terrain["bit_depth"] == 16 and terrain["channels"] == "grayscale", terrain
    data = (ROOT / "assets/maps/kalinovo" / terrain["asset"]).read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "the heightmap is a PNG"
    pos, idat, (width, height) = 8, b"", (0, 0)
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            width, height, depth, colour, _, _, interlace = struct.unpack(">IIBBBBB", body)
            assert (depth, colour, interlace) == (16, 0, 0), "16-bit grey, not interlaced"
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    raw, stride = zlib.decompress(idat), 2 * width
    rows, prev = [], bytearray(stride)
    for y in range(height):
        start = y * (stride + 1)
        kind, line = raw[start], bytearray(raw[start + 1:start + 1 + stride])
        for i in range(stride):
            a = line[i - 2] if i >= 2 else 0
            b = prev[i]
            c = prev[i - 2] if i >= 2 else 0
            if kind == 1:
                line[i] = (line[i] + a) & 255
            elif kind == 2:
                line[i] = (line[i] + b) & 255
            elif kind == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif kind == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(line)
        prev = line
    decode = terrain["height_decode"]
    first = terrain["sample_centers_xz"]["pixel_0_0"]
    last = terrain["sample_centers_xz"][f"pixel_{width - 1}_{height - 1}"]

    def at(x, z):
        fx = (x - first[0]) / (last[0] - first[0]) * (width - 1)
        fz = (z - first[1]) / (last[1] - first[1]) * (height - 1)
        i, j = int(fx), int(fz)
        tx, tz = fx - i, fz - j

        def h(ii, jj):
            v = rows[jj][2 * ii] << 8 | rows[jj][2 * ii + 1]
            return v / 65535.0 * decode["scale_m"] + decode["offset_m"]
        top = h(i, j) * (1 - tx) + h(i + 1, j) * tx
        bottom = h(i, j + 1) * (1 - tx) + h(i + 1, j + 1) * tx
        return top * (1 - tz) + bottom * tz
    return level, at


def spawn_y(ground, x, z):
    steps = [SPAWN_HALF_M * (k / 10 - 1) for k in range(21)]
    return max(ground(x + dx, z + dz) for dx in steps for dz in steps) + SPAWN_CLEARANCE_M


def poses(args):
    """(pos, rot) per condition: the pile run-up on the ram recipe's heading, else flat ground."""
    level, ground = heightmap()
    house = next(i for i in level["instances"] if i["id"] == "house_0001")
    heading = math.radians(json.loads((ROOT / "scripts/perf/ram.json").read_text())["heading_deg"])
    # Quat::from_rotation_y(heading) * -Z, as `net::ram` plans its approach.
    dx, dz = -math.sin(heading), -math.cos(heading)
    hx, _, hz = house["translation"]
    x, z = hx - dx * args.runup_m, hz - dz * args.runup_m
    rot = (0.0, math.sin(heading / 2), 0.0, math.cos(heading / 2))
    fx, fz = args.flat_pose
    flat_pose = ((fx, spawn_y(ground, fx, fz), fz), (0.0, 0.0, 0.0, 1.0))
    return {"pile": ((x, spawn_y(ground, x, z), z), rot), "flat": flat_pose, "rubble": flat_pose}


class Refused(Exception):
    """A run's own evidence is unusable; the message names what and where."""


def rustc_running():
    """pgrep exits 0 on a match and 1 on none; anything else is a census that did not happen."""
    census = subprocess.run(["pgrep", "-f", "^[^ ]*rustc( |$)"], capture_output=True, text=True)
    if census.returncode not in (0, 1):
        raise Refused(f"the process census failed (pgrep exit {census.returncode}: "
                      f"{census.stderr.strip()})")
    return census.returncode == 0


def wait_quiet(limit_s):
    """Seconds waited for other compiles to finish; None if they outlasted the limit."""
    start = time.monotonic()
    while rustc_running():
        if time.monotonic() - start > limit_s:
            return None
        time.sleep(5)
    return time.monotonic() - start


def port_free():
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        try:
            sock.bind(("127.0.0.1", 5888))
        except OSError:
            return False
    return True


def alive(proc):
    return proc.poll() is None


def traces(path):
    """(where, trace) per stats record. The kill can cut the log's final line short: a record
    there with no line end is torn and skipped; any other record that does not parse, or carries
    no trace, refuses the run."""
    text = path.read_text(errors="replace")
    lines = text.splitlines()
    for number, line in enumerate(lines, 1):
        if line.startswith("server_stats "):
            where = f"{path}:{number}"
            try:
                record = json.loads(line[len("server_stats "):])
            except json.JSONDecodeError as error:
                if number == len(lines) and not text.endswith("\n"):
                    continue
                raise Refused(f"{where}: malformed server_stats record ({error})")
            load = record.get("loadtest")
            if not isinstance(load, dict) or not isinstance(load.get("ticks"), list) \
                    or "awake_bodies" not in load:
                raise Refused(f"{where}: the record carries no trace with ticks and awake_bodies")
            yield where, load


def admit(row, where):
    """A trace tick's quantities, with `net_us`; a tick lacking one the run reads refuses it."""
    cells = flat(row)
    missing = [k for k in READ if k not in cells]
    if missing:
        raise Refused(f"{where}: a trace tick lacks {missing}")
    cells["net_us"] = cells["tick_us"] - sum(cells[k] for k in CENSUS_LEAVES)
    return cells


def flat(row, prefix=""):
    out = {}
    for key, value in row.items():
        if isinstance(value, dict):
            out.update(flat(value, f"{prefix}{key}."))
        elif isinstance(value, (int, float)) and not isinstance(value, bool):
            out[prefix + key] = float(value)
    return out


def percentile(values, q):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, max(0, math.ceil(q * len(ordered)) - 1))]


def describe(rows):
    out = {"ticks": len(rows)}
    if not rows:
        return out
    for key in COUNTS + TIMES:
        values = [r[key] for r in rows]
        out[key] = {"mean": statistics.fmean(values)}
        if key in TIMES:
            out[key].update(p50=percentile(values, 0.5), p95=percentile(values, 0.95),
                            p99=percentile(values, 0.99), max=max(values))
    return out


def run(args, name, condition, pose, out):
    """One condition's record and window rows; refused evidence marks the record INVALID."""
    record = {"run": name, "condition": condition, "pose": pose}
    try:
        return measure(args, name, condition, pose, out, record)
    except Refused as refusal:
        record["invalid"] = str(refusal)
        return record, []


def measure(args, name, condition, pose, out, record):
    drive = f"Park(throttle_ticks: {args.throttle_ticks})" if condition == "pile" else "Idle"
    (pos, rot) = pose
    lead_s = (LEAD_TICKS + args.throttle_ticks) / TICK_HZ
    start_s = lead_s + args.settle_s
    end_s = start_s + args.park_s
    script = out / f"{name}.harness.ron"
    script.write_text(
        f"(ticks: {int((end_s + 60) * TICK_HZ)}, drive: {drive}, "
        f"spawn_pose: (pos: ({pos[0]:.3f}, {pos[1]:.3f}, {pos[2]:.3f}), "
        f"rot: ({rot[0]:.6f}, {rot[1]:.6f}, {rot[2]:.6f}, {rot[3]:.6f})))\n")
    base = {k: v for k, v in os.environ.items() if not k.startswith(("OVERMATCH_", "SPIKE_"))}
    base.update(BEVY_ASSET_ROOT=str(ROOT), OVERMATCH_UNATTENDED="1", OVERMATCH_MUTE="1",
                OVERMATCH_HARNESS=str(script), NO_COLOR="1")
    recorder = IDLE_RECORDER
    if condition == "rubble":
        recorder = {"tanks": 1, "events": [{"at_s": args.fell_at_s, "instances": ["house_0001"]}]}
    server_env = dict(base, OVERMATCH_SERVER_BIND="127.0.0.1", OVERMATCH_SERVER_STATS_S="1",
                      OVERMATCH_LOADTEST=json.dumps(recorder, separators=(",", ":")))
    client_env = dict(base, OVERMATCH_WINDOW="headless", OVERMATCH_SERVER=ADDRESS)
    record.update(drive=drive, recorder=recorder, window_s=[start_s, end_s],
                  loadavg_before=os.getloadavg())
    waited = wait_quiet(args.quiet_wait_s)
    record["quiet_wait_s"] = waited
    if waited is None:
        record["invalid"] = "another compile outlasted --quiet-wait-s"
        return record, []
    if not port_free():
        record["invalid"] = "UDP 5888 is already bound"
        return record, []
    server_log, client_log = out / f"{name}.server.log", out / f"{name}.client.log"
    with open(server_log, "w") as slog, open(client_log, "w") as clog:
        server = subprocess.Popen([str(args.bin / "overmatch-server")], env=server_env, stdout=slog,
                                  stderr=subprocess.STDOUT, start_new_session=True)
        client = None
        try:
            deadline = time.monotonic() + 60
            while "listening" not in server_log.read_text(errors="replace"):
                if not alive(server) or time.monotonic() > deadline:
                    record["invalid"] = "the server never listened"
                    return record, []
                time.sleep(0.2)
            client = subprocess.Popen([str(args.bin / "overmatch")], env=client_env, stdout=clog,
                                      stderr=subprocess.STDOUT, start_new_session=True)
            deadline = time.monotonic() + 120
            while "harness: released" not in client_log.read_text(errors="replace"):
                if not alive(client) or time.monotonic() > deadline:
                    record["invalid"] = "the client was never released"
                    return record, []
                time.sleep(0.2)
            # The trace's wall clock starts at combat admission, which the client's release
            # follows; the window's completeness is gated on the rows below, not on this wait.
            # Reading the growing log here would load the machine under measurement.
            close = time.monotonic() + end_s + 3
            compiles = 0
            while time.monotonic() < close:
                if not (alive(server) and alive(client)):
                    record["invalid"] = "a process exited before the window closed"
                    return record, []
                compiles += rustc_running()
                time.sleep(1)
            record["seconds_with_rustc_running"] = compiles
        finally:
            for proc in (client, server):
                if proc is not None and alive(proc):
                    os.killpg(proc.pid, signal.SIGKILL)
                    proc.wait()
    rows, flats, awake_bodies, settle_overlaps = [], [], [], []
    for where, load in traces(server_log):
        in_window = False
        for row in load["ticks"]:
            cells = admit(row, where)
            if lead_s <= cells["elapsed_s"] < start_s:
                settle_overlaps.append(cells["work.belt_overlaps"])
            elif start_s <= cells["elapsed_s"] <= end_s:
                rows.append(row)
                flats.append(cells)
                in_window = True
        if in_window:
            awake_bodies.append(load["awake_bodies"])
    with open(out / f"{name}.window.jsonl", "w") as handle:
        for row in rows:
            handle.write(json.dumps(row) + "\n")
    record["loadavg_after"] = os.getloadavg()
    record["awake_bodies_per_line"] = awake_bodies
    record["window"] = describe(flats)
    expected = args.park_s * TICK_HZ
    if not settle_overlaps:
        raise Refused(f"{server_log}: the settle span [{lead_s}, {start_s}) s holds no tick")
    # The settle span's tank is awake, so its belt work witnesses what it rests on.
    belt = statistics.fmean(settle_overlaps)
    record["settle_belt_overlaps_mean"] = belt
    if len(flats) < WINDOW_SHARE * expected:
        record["invalid"] = f"the window holds {len(flats)} of {expected} ticks"
    elif any(r["tanks"] != 1 for r in flats):
        record["invalid"] = "a window tick holds other than one tank"
    elif condition == "pile" and belt <= 0:
        record["invalid"] = "the tank does not rest on loose pieces"
    elif condition != "pile" and belt > 0:
        record["invalid"] = "the flat tank touches loose pieces"
    return record, flats


def table(pooled):
    names = list(pooled)
    lines = ["quantity " + " ".join(f"{n:>14}" for n in names)]
    for key in COUNTS:
        lines.append(f"{key:40}" + " ".join(
            f"{pooled[n].get(key, {}).get('mean', float('nan')):14.2f}" for n in names))
    for key in TIMES:
        for stat in ("mean", "p50", "p95", "p99"):
            lines.append(f"{key + ' ' + stat:40}" + " ".join(
                f"{pooled[n].get(key, {}).get(stat, float('nan')):14.1f}" for n in names))
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0],
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--bin", type=Path, default=ROOT / "target/release")
    parser.add_argument("conditions", nargs="*",
                        default=["pile", "flat", "rubble", "rubble", "flat", "pile"])
    parser.add_argument("--runup-m", type=float, default=30.0)
    parser.add_argument("--throttle-ticks", type=int, default=640)
    parser.add_argument("--flat-pose", type=float, nargs=2, default=(680.0, -520.0),
                        metavar=("X", "Z"))
    parser.add_argument("--fell-at-s", type=float, default=1.0)
    parser.add_argument("--settle-s", type=float, default=10.0)
    parser.add_argument("--park-s", type=float, default=30.0)
    parser.add_argument("--quiet-wait-s", type=float, default=2400.0)
    args = parser.parse_args()
    if any(c not in ("pile", "flat", "rubble") for c in args.conditions):
        parser.error("conditions are pile, flat and rubble")
    args.bin = args.bin.resolve()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    caffeinate = subprocess.Popen(["caffeinate", "-ims", "-w", str(os.getpid())])
    head = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True,
                          text=True).stdout.strip()
    spawn = poses(args)
    manifest = {"head": head, "bin": str(args.bin),
                "args": {k: str(v) for k, v in vars(args).items()},
                "poses": spawn, "runs": []}
    pooled = {}
    try:
        for n, condition in enumerate(args.conditions, 1):
            name = f"r{n:02d}-{condition}"
            print(f"== {name}", file=sys.stderr, flush=True)
            record, rows = run(args, name, condition, spawn[condition], out)
            manifest["runs"].append(record)
            print(f"   {record.get('invalid', 'valid')}", file=sys.stderr, flush=True)
            if "invalid" not in record:
                pooled.setdefault(condition, []).extend(rows)
    finally:
        caffeinate.terminate()
    manifest["pooled"] = {c: describe(rows) for c, rows in pooled.items()}
    (out / "summary.json").write_text(json.dumps(manifest, indent=1) + "\n")
    text = table(manifest["pooled"]) if manifest["pooled"] else "no valid run"
    (out / "summary.txt").write_text(text + "\n")
    print(text)
    return 0 if all("invalid" not in r for r in manifest["runs"]) else 1


if __name__ == "__main__":
    sys.exit(main())
