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
A scheduled run saves every tick's stages and work counts; --replication-trace adds replication
envelope size histograms, from a server started with REPLICATION_TRACE (--print-server-env).
Full-speed ram: --ram scripts/perf/ram.json (local or hosted).
--server-env OVERMATCH_KEY=VALUE (repeatable) sets one lever on the server only: on the local
server in match and ram runs, or in --print-server-env's lines for a deployment. Those runs
pass no inherited OVERMATCH_ variable to any process; the pairs are saved as server-env.json.
Without --schedule or --ram, the tankless HE diagnostic remains available.
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
# The server log directive whose send_server_packets lines replication_windows parses.
REPLICATION_TRACE = "lightyear_replication::server=trace"
# The levers this runner sets on its own server and clients; --server-env may not replace one.
RUNNER_KEYS = frozenset({
    "OVERMATCH_UNATTENDED", "OVERMATCH_WINDOW", "OVERMATCH_MUTE", "OVERMATCH_LOADTEST",
    "OVERMATCH_RAM", "OVERMATCH_BOT", "OVERMATCH_SERVER_STATS_S", "OVERMATCH_SERVER_BIND",
    "OVERMATCH_SERVER", "OVERMATCH_HARNESS",
})


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


class ServerLogs:
    """One polling/backoff/watchdog contract for every hosted recipe."""
    def __init__(self, path, deployment):
        self.path, self.deployment = path, deployment
        self.next_poll, self.poll_s = 0.0, HOSTED_LOG_POLL_S
        self.local_version = None

    def snapshot(self, deadline):
        if self.deployment:
            now = time.monotonic()
            if now < self.next_poll or now >= deadline:
                return None
            try:
                subprocess.run([str(ROOT / "scripts/edgegap/session.sh"), "logs", self.deployment, str(self.path)],
                               check=True, timeout=min(LOG_FETCH_TIMEOUT_S, deadline - now), stdout=subprocess.DEVNULL)
            except subprocess.CalledProcessError as error:
                if error.returncode != LOGS_RATE_LIMITED:
                    raise
                self.poll_s *= 2
                self.next_poll = time.monotonic() + self.poll_s
                print(f"Hosted logs rate limited (HTTP 429); retry in {self.poll_s:g} s if within the run deadline.", file=sys.stderr, flush=True)
                return None
            self.poll_s = HOSTED_LOG_POLL_S
            self.next_poll = time.monotonic() + self.poll_s
        if not self.path.exists():
            return ""
        if not self.deployment:
            stat = self.path.stat()
            version = stat.st_size, stat.st_mtime_ns
            if version == self.local_version:
                return None
            self.local_version = version
        return self.path.read_text()


def admit_server(snapshot, children, deadline, rehearsal):
    """Verify the real server allocation before admitting any clients."""
    while True:
        if any(child.poll() is not None for child in children) or time.monotonic() >= deadline:
            raise RuntimeError("no verified server startup line before admission deadline")
        text = snapshot(deadline)
        if text is not None and "server: host available_parallelism=" in text:
            host = allocation(text, rehearsal)
            print(host, flush=True)
            return host
        time.sleep(min(POLL_S, max(0, deadline - time.monotonic())))


def run_match(args, out, recipe):
    """Reuse the online runner's scripted GPU-less links and session.sh log snapshots."""
    import socket
    children, logs = [], []
    local = args.target == "local"
    address = "127.0.0.1:5888" if local else args.target
    # Inherited stress/trace levers would silently add a second workload.
    env = {k: v for k, v in os.environ.items() if not k.startswith(("OVERMATCH_", "SPIKE_"))}
    env.update(OVERMATCH_UNATTENDED="1", OVERMATCH_WINDOW="headless", OVERMATCH_MUTE="1", NO_COLOR="1")
    env["OVERMATCH_LOADTEST"] = json.dumps(recipe, separators=(",", ":"))
    if args.replication_trace:
        env["RUST_LOG"] = env.get("RUST_LOG", "info") + "," + REPLICATION_TRACE
    serverlog = out / "server.log"
    (out / "rows.json").unlink(missing_ok=True)
    client_count = args.clients or args.tanks
    # DERIVED: the script outlives both admission and observation at the server's tick rate.
    ticks = math.ceil((args.duration + 2 * args.startup_grace + 60) / tick_seconds())
    script = out / "load.harness.ron"
    script.write_text(f"(ticks: {ticks}, drive: Arc, aim: Sweep, fire_tick: {ticks})\n")
    def spawn(binary, logpath, extra):
        log = logpath.open("w")
        logs.append(log)
        child = subprocess.Popen([str(args.bin / binary)], cwd=ROOT, env=dict(env, **extra), stdout=log, stderr=subprocess.STDOUT)
        children.append(child)
        return child
    snapshot = ServerLogs(serverlog, None if local else args.deployment).snapshot
    start = time.monotonic()
    try:
        if local:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
                sock.bind(("127.0.0.1", 5888))
            spawn("overmatch-server", serverlog, dict(OVERMATCH_SERVER_BIND="127.0.0.1", OVERMATCH_SERVER_STATS_S="1", OVERMATCH_BOT=str(args.tanks-client_count), **dict(args.server_env)))
        verified_host = admit_server(snapshot, children, start + args.startup_grace, args.rehearsal)
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
                    trace = match_tick_summary(rows, recipe)
                    if args.replication_trace:
                        trace["replication_windows"] = replication_windows(text)
                    (out / "tick-attribution.json").write_text(json.dumps(trace, indent=2) + "\n")
                    print(f"MEASURED match-tick-attribution: {len(trace['ticks'])} ticks, {len(trace['over_budget_ticks'])} over budget; {out / 'tick-attribution.json'}")
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


def ram_recipe(path):
    """Operator route choices; the server derives speed and run-up from its admitted tank data."""
    value = json.loads(path.read_text())
    keys = {"instance", "heading_deg", "runup_margin", "minimum_speed_fraction", "approach_s", "deadline_s", "settle_s"}
    if not isinstance(value, dict) or set(value) != keys or not isinstance(value["instance"], str) or not value["instance"]:
        raise ValueError(f"{path}: expected ram recipe fields {sorted(keys)} and a nonempty instance")
    for key in keys - {"instance"}:
        v = value[key]
        if isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v):
            raise ValueError(f"{path}: {key} must be finite")
    if value["runup_margin"] < 1 or not 0 < value["minimum_speed_fraction"] <= 1 or any(value[k] <= 0 for k in ("approach_s", "deadline_s", "settle_s")) or value["settle_s"] >= value["deadline_s"]:
        raise ValueError(f"{path}: invalid ram margin, speed fraction or timing domain")
    return value


def tick_seconds():
    """Read the fixed-step owner's duration; refuse if its interface changes."""
    source = (ROOT / "src/net/server.rs").read_text()
    match = re.search(r"const TICK: Duration = Duration::from_nanos\(([\d_]+)\)", source)
    if not match:
        raise RuntimeError("fixed timestep declaration changed: update tick_seconds against its owner")
    return int(match[1].replace("_", "")) / 1e9


# Inclusive or outside-tick measurements, not independent leaf stages.
RAM_NON_ADDITIVE = {"tick_us", "physics_us", "sleep_us", "collector_us", "phases_us"}
# Resolution: one nanosecond permits floating conversion/summation noise, not a missing stage.
RAM_ROUNDING_US = 0.001


def ram_parts(tick):
    """Add every reported phase and leaf once; physics/sleep detail is nested, send is outside."""
    parts = {name.removesuffix("_us"): us for name, us in tick["phases_us"].items()}
    for name, us in tick.items():
        if name.endswith("_us") and name not in RAM_NON_ADDITIVE:
            name = name.removesuffix("_us")
            if name in parts or name == "unattributed":
                raise ValueError(f"duplicate phase/leaf timing {name}")
            parts[name] = us
    parts["physics_step"] -= tick["sleep_us"]
    if parts["physics_step"] < -RAM_ROUNDING_US:
        raise ValueError("sleep exceeds its containing physics_step phase")
    parts["sleep"] = tick["sleep_us"]
    remainder = tick["tick_us"] - sum(parts.values())
    if not math.isfinite(remainder) or remainder < -RAM_ROUNDING_US:
        raise ValueError("stage measurements exceed their containing tick")
    # Only sub-nanosecond conversion noise may be clamped; genuine gaps remain visible.
    parts["unattributed"] = max(0.0, remainder)
    return parts


def finite(value, field):
    try:
        if not isinstance(value, bool) and isinstance(value, (int, float)) and math.isfinite(value):
            return
    except OverflowError:
        pass
    raise ValueError(f"invalid {field}: expected a finite number")


def read_tick_samples(batches, tanks, ticks=None, sends=None):
    ticks = [] if ticks is None else ticks
    sends = [] if sends is None else sends
    for batch in batches:
        for tick in batch["ticks"]:
            if tick["tick"] != len(ticks) + 1:
                raise RuntimeError("missing, duplicated or reordered instrument tick")
            if tick["tanks"] != tanks:
                raise RuntimeError("tick population differs from requested tanks")
            required = {"tick_us", "physics_us", "sleep_us", "intake_us", "structure_us", "refit_us", "publish_us", "contacts_us", "sweep_us", "crumble_us", "collector_us", "dt_s", "phases_us"}
            if missing := required - tick.keys():
                raise ValueError(f"missing tick timing fields: {sorted(missing)}")
            for field in {name for name in tick if name.endswith("_us") and name != "phases_us"} | {"dt_s"}:
                finite(tick[field], f"ticks.{field}")
                if tick[field] < 0:
                    raise ValueError(f"invalid {field}")
            if tick["dt_s"] <= 0 or tick["sleep_us"] + tick["contacts_us"] > tick["physics_us"]:
                raise ValueError("invalid timestep or nested sleep bracket")
            phases = tick["phases_us"]
            if not isinstance(phases, dict) or not phases or any(not name.endswith("_us") or name in RAM_NON_ADDITIVE | {"unattributed_us"} for name in phases):
                raise ValueError("invalid phases_us: expected named phase timings")
            for name, us in phases.items():
                finite(us, f"phases_us.{name}")
                if us < 0:
                    raise ValueError(f"invalid phases_us.{name}")
            parts = ram_parts(tick)
            if ticks and parts.keys() != ram_parts(ticks[0]).keys():
                raise ValueError("instrument timing fields changed during capture")
            ticks.append(tick)
        for send in batch["sends"]:
            if not isinstance(send["tick"], int) or send["tick"] <= 0 or send["tick"] > len(ticks) or sends and send["tick"] <= sends[-1]["tick"]:
                raise ValueError("invalid or reordered send tick")
            finite(send["us"], "sends.us")
            if send["us"] < 0:
                raise ValueError("invalid send duration")
            sends.append(send)
    return ticks, sends


def ram_rows(text, recipe):
    rows, ticks, sends = [], [], []
    last = None
    for number, line in enumerate(text.splitlines(), 1):
        if not line.startswith("server_stats "):
            continue
        try:
            row = json.loads(line.removeprefix("server_stats "))
            ram = row["ram"]
            if ram["recipe"] != recipe:
                raise RuntimeError("server ram recipe differs from requested recipe")
            ticks, sends = read_tick_samples([ram], 1, ticks, sends)
            plan, impact = ram["plan"], ram["impact"]
            if plan is not None:
                for field in ("runup_m", "governed_m_s", "minimum_m_s"):
                    finite(plan[field], f"plan.{field}")
                    if plan[field] < 0:
                        raise ValueError(f"invalid plan.{field}: expected nonnegative distance or speed")
                for field, size in (("spawn", 3), ("rotation", 4)):
                    if not isinstance(plan[field], list) or len(plan[field]) != size:
                        raise ValueError(f"invalid plan.{field}: expected {size} coordinates")
                    for coordinate in plan[field]:
                        finite(coordinate, f"plan.{field}")
            elif ticks or impact is not None or ram["done"] is not None:
                raise ValueError("ram samples require an admitted plan")
            if impact is not None:
                if isinstance(impact["tick"], bool) or not isinstance(impact["tick"], int) or not 0 < impact["tick"] <= len(ticks):
                    raise ValueError("invalid impact.tick: expected a captured tick")
                for field in ("speed_m_s", "normal_speed_m_s"):
                    finite(impact[field], f"impact.{field}")
            rows.append(row)
            last = ram
        except (ValueError, KeyError, TypeError) as error:
            raise RuntimeError(f"server log line {number}: malformed ram stats: {error}") from error
    return rows, ticks, sends, last


# Match ticks add these to the shared ram schema; ram ticks carry none of them.
MATCH_TICK_KEYS = ("elapsed_s", "events_fired", "work")


def tick_attribution(tick):
    stages = ram_parts(tick)
    return dict(tick=tick["tick"], tick_us=tick["tick_us"], parts_us=stages,
                attributed_pct=100 * (1 - stages["unattributed"] / tick["tick_us"]),
                **{key: tick[key] for key in MATCH_TICK_KEYS if key in tick})


def replication_windows(text):
    """Pinned Lightyear send_server_packets trace: plaintext envelopes, before transport retries.

    The histogram preserves oversized single-entity mutations for source-based byte attribution;
    it does not mislabel replication bytes as encrypted sends or assume poses fill every mutation.
    """
    windows, channels = [], {}
    seen = False
    for number, line in enumerate(text.splitlines(), 1):
        line = ANSI.sub("", line)
        if "send_server_packets: sending " in line:
            match = re.search(r"send_server_packets: sending (\d+) bytes on channel_idx=(\d+) to ", line)
            if match is None:
                raise RuntimeError(f"server log line {number}: malformed replication size trace")
            size, channel = map(int, match.groups())
            histogram = channels.setdefault(str(channel), collections.Counter())
            histogram[size] += 1
            seen = True
        if "server_stats " in line:
            row = json.loads(line.split("server_stats ", 1)[1])
            sequence = row["loadtest"]["sequence"]
            if sequence != len(windows) + 1:
                raise RuntimeError("missing, duplicated or reordered replication window")
            windows.append(dict(sequence=sequence, channels={
                channel: dict(messages=sum(histogram.values()),
                              bytes=sum(size*count for size, count in histogram.items()),
                              sizes=dict(sorted(histogram.items())))
                for channel, histogram in sorted(channels.items())}))
            channels = {}
    if not seen:
        raise RuntimeError("replication byte trace missing: start the server with --print-server-env --replication-trace")
    # An unfinished trailing stats period is outside the complete-window report.
    return windows


def match_tick_summary(rows, recipe):
    """Per-tick wave attribution; the ram and match share schema checks and additive accounting."""
    batches = [r["loadtest"] for r in rows]
    # A server built before the match tick trace omits both fields.
    if any("ticks" not in batch or "sends" not in batch for batch in batches):
        raise RuntimeError("match tick trace missing: rebuild the instrumented server")
    ticks, sends = read_tick_samples(batches, recipe["tanks"])
    if not ticks:
        raise RuntimeError("match tick trace empty: no tick ran after admission")
    if any(t["dt_s"] != ticks[0]["dt_s"] for t in ticks):
        raise RuntimeError("match fixed timestep changed during capture")
    return dict(ticks=ticks, sends=sends,
                worst_tick_attribution=tick_attribution(max(ticks, key=lambda t: t["tick_us"])),
                over_budget_ticks=[tick_attribution(t) for t in ticks if t["tick_us"] > t["dt_s"]*1e6])


def ram_summary(text, recipe):
    rows, ticks, sends, last = ram_rows(text, recipe)
    if last is None:
        return None
    plan, impact = last["plan"], last["impact"]
    if impact and impact["speed_m_s"] < plan["minimum_m_s"]:
        raise RuntimeError(f"ram did not reach full-speed admission: impact={impact}, plan={plan}")
    if last["done"] is None:
        return None
    if last["done"] not in ("rest", "deadline"):
        raise RuntimeError(f"ram observation invalidated: {last['done']}")
    if not impact:
        raise RuntimeError("completed ram has no measured impact")
    window = [t for t in ticks if t["tick"] >= impact["tick"]]
    if not window:
        raise RuntimeError("ram has no measured contact window")
    dt = window[0]["dt_s"]
    if any(t["dt_s"] != dt for t in ticks):
        raise RuntimeError("ram fixed timestep changed during capture")
    total = sum(t["tick_us"] for t in window)
    breakdowns = [ram_parts(t) for t in window]
    parts = {k: sum(p[k] for p in breakdowns) for k in breakdowns[0]}
    if total <= 0:
        raise RuntimeError("ram has no positive measured tick time")
    ordered = sorted(t["tick_us"] for t in window)
    send_us = sum(s["us"] for s in sends if window[0]["tick"] <= s["tick"] <= window[-1]["tick"])
    worst = max(window, key=lambda t: t["tick_us"])
    return dict(recipe=recipe, plan=plan, impact=impact, end=last["done"],
                first_tick=window[0]["tick"], last_tick=window[-1]["tick"], window_s=len(window)*dt,
                worst_tick=worst, p99_us=ordered[math.ceil(0.99*len(ordered))-1],
                worst_tick_attribution=tick_attribution(worst),
                worst_capture_tick_attribution=tick_attribution(max(ticks, key=lambda t: t["tick_us"])),
                # Include approach ticks too: the attribution gate applies to the whole capture.
                over_budget_ticks=[tick_attribution(t) for t in ticks if t["tick_us"] > dt*1e6],
                peak_awake=max(t["awake"] for t in window), final_awake=window[-1]["awake"],
                overruns=sum(t["tick_us"] > dt*1e6 for t in window),
                collector_mean_us=sum(t["collector_us"] for t in window)/len(window),
                collector_max_us=max(t["collector_us"] for t in window),
                tick_mean_us=total/len(window), stage_us=parts,
                share_pct={k: 100*v/total for k,v in parts.items()},
                send_us=send_us, send_pct_of_tick_time=100*send_us/total, ticks=ticks, rows=rows)


def report_ram(result):
    compact = {k: v for k, v in result.items() if k not in ("ticks", "rows")}
    print("MEASURED ram (shared physics; send outside fixed ticks): " + json.dumps(compact), flush=True)


def run_ram(args, out, recipe):
    """Same locked runner and hosted log plumbing, with one real full-throttle UDP client."""
    import socket
    local = args.target == "local"
    address = "127.0.0.1:5888" if local else args.target
    env = {k: v for k, v in os.environ.items() if not k.startswith(("OVERMATCH_", "SPIKE_"))}
    env.update(OVERMATCH_UNATTENDED="1", OVERMATCH_WINDOW="headless", OVERMATCH_MUTE="1",
               NO_COLOR="1", OVERMATCH_RAM=json.dumps(recipe, separators=(",", ":")),
               OVERMATCH_BOT="0", OVERMATCH_SERVER_STATS_S="1")
    dt = tick_seconds()
    # DERIVED: the client outlives approach, observation and both admission watchdog spans.
    drive_window_s = recipe["approach_s"] + recipe["deadline_s"]
    wall_limit = 2*args.startup_grace + drive_window_s
    ticks = math.ceil((wall_limit + 60)/dt)
    # DERIVED: accepted contact is at most approach_s after capture starts, so observation
    # ends by approach_s + deadline_s, while that much throttle starts later, at driving.
    throttle_ticks = math.ceil(drive_window_s/dt)
    if ticks > 2**32-1 or throttle_ticks > 2**32-129:
        raise ValueError("ram duration exceeds harness tick representation")
    script = out / "ram.harness.ron"
    script.write_text(f"(ticks: {ticks}, drive: Ram(throttle_ticks: {throttle_ticks}))\n")
    children, logs = [], []
    serverlog = out / "server.log"
    snapshot = ServerLogs(serverlog, None if local else args.deployment).snapshot
    def spawn(binary, path, extra):
        log = path.open("w")
        logs.append(log)
        child = subprocess.Popen([str(args.bin / binary)], cwd=ROOT, env=dict(env, **extra), stdout=log, stderr=subprocess.STDOUT)
        children.append(child)
        return child
    start = time.monotonic()
    try:
        if local:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
                sock.bind(("127.0.0.1", 5888))
            spawn("overmatch-server", serverlog, dict(OVERMATCH_SERVER_BIND="127.0.0.1", **dict(args.server_env)))
        host = admit_server(snapshot, children, start + args.startup_grace, local)
        spawn("overmatch", out / "client.log", dict(OVERMATCH_SERVER=address, OVERMATCH_HARNESS=str(script)))
        deadline = start + wall_limit
        while True:
            if any(c.poll() is not None for c in children):
                raise RuntimeError("ram server or client exited before completion")
            if time.monotonic() >= deadline:
                raise RuntimeError("ram incomplete by wall deadline (no contact, slow sim, or missing logs)")
            text = snapshot(deadline)
            if text is not None:
                if allocation(text, rehearsal=local) != host:
                    raise RuntimeError("server startup allocation changed during ram")
                result = ram_summary(text, recipe)
                if result:
                    log = (out / "client.log").read_text()
                    if log.count("client: connected (entity") != 1 or "harness: released" not in log:
                        raise RuntimeError("ram client needs one connection and synchronized combat release")
                    (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
                    report_ram(result)
                    print(f"COMPLETE: ram artifacts: {out}", flush=True)
                    return
            time.sleep(min(POLL_S, max(0, deadline-time.monotonic())))
    except BaseException:
        print(f"RAM RUN INVALID; logs: {out}", file=sys.stderr)
        raise
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.kill()
            child.wait()
        for log in logs:
            log.close()


def server_env(text):
    """One --server-env pair: an OVERMATCH_ lever this runner does not set, and a value."""
    key, sep, value = text.partition("=")
    if not sep or not value or not re.fullmatch(r"OVERMATCH_[A-Z0-9_]+", key):
        raise argparse.ArgumentTypeError("must be OVERMATCH_<NAME>=<non-empty value>")
    if key in RUNNER_KEYS:
        raise argparse.ArgumentTypeError(f"{key} is set by this runner")
    return key, value


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
               RUST_BACKTRACE="1")
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
    parser.add_argument("--ram", type=Path, help="full-speed ram recipe JSON; shared local/hosted runner")
    parser.add_argument("--schedule", type=Path, help="real-match mode: JSON {events: [{at_s, instances}]} for the game's construction workload")
    parser.add_argument("--target", default="local", help="local, or ip:port of a deployment the operator already started")
    parser.add_argument("--deployment", help="existing Edgegap deployment ID; only its logs are read")
    parser.add_argument("--tanks", type=count, help="total tanks, including bots and real links (owner workload: 30)")
    parser.add_argument("--clients", type=count, help="real window-less clients; remaining tanks are bots (default: all tanks)")
    parser.add_argument("--startup-grace", type=positive, default=90, help="admission and trailing-window watchdog seconds")
    parser.add_argument("--bin", type=Path, default=ROOT / "target/debug", help="binary directory (default target/debug; other directories must already be built)")
    parser.add_argument("--rehearsal", action="store_true", help="local plumbing only: print unpriced observations without claiming a two-vCPU allocation")
    parser.add_argument("--print-server-env", action="store_true", help="print the recipe/environment for the operator's deployment, then exit without a build or run")
    parser.add_argument("--replication-trace", action="store_true", help="require and save replication envelope size histograms (the server runs REPLICATION_TRACE)")
    parser.add_argument("--server-env", type=server_env, action="append", default=[], metavar="OVERMATCH_KEY=VALUE", help="repeatable: set one OVERMATCH_ lever on the server only (match and ram runs; saved as server-env.json)")
    parser.add_argument("--holding-lock", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.bin = args.bin.resolve()
    args.tanks = args.tanks if args.tanks is not None else (1 if args.ram else 30)
    if args.replication_trace and not args.schedule:
        parser.error("--replication-trace requires --schedule")
    if args.ram and args.tanks != 1:
        parser.error("--ram requires exactly one tank")
    if args.clients is not None and args.clients > args.tanks:
        parser.error("--clients cannot exceed --tanks")
    if args.rehearsal and args.target != "local":
        parser.error("--rehearsal is local only; hosted runs must verify two vCPU")
    if args.target != "local" and (not (args.schedule or args.ram) or not args.deployment):
        parser.error("remote runs require --schedule or --ram, and --deployment")
    if args.ram and (args.schedule or args.sample or args.clients is not None):
        parser.error("--ram is one client; incompatible with --schedule, --sample and --clients")
    ram = ram_recipe(args.ram) if args.ram else None
    recipe = recipe_for(args.schedule, args.tanks) if args.schedule else None
    if args.deployment and args.target == "local":
        parser.error("--deployment requires its explicit remote --target IP:PORT")
    if args.rehearsal and not (recipe or ram):
        parser.error("--rehearsal requires a real-match --schedule")
    if recipe and args.sample:
        parser.error("--sample belongs to the tankless diagnostic; match mode observes server windows")
    if args.server_env:
        keys = [key for key, _ in args.server_env]
        if len(set(keys)) != len(keys):
            parser.error("--server-env names a key twice")
        if not (recipe or ram):
            parser.error("--server-env requires --schedule or --ram")
        if args.target != "local" and not args.print_server_env:
            parser.error("--server-env cannot reach a running deployment; start it with --print-server-env's lines")
    if args.print_server_env:
        if ram:
            print("OVERMATCH_RAM=" + json.dumps(ram, separators=(",", ":")))
            print("OVERMATCH_BOT=0")
            print("OVERMATCH_SERVER_STATS_S=1")
            print("OVERMATCH_UNATTENDED=1")
            print("OVERMATCH_WINDOW=headless")
            print("OVERMATCH_MUTE=1")
            for key, value in args.server_env:
                print(f"{key}={value}")
            return 0
        if not recipe:
            parser.error("--print-server-env requires --schedule")
        print("OVERMATCH_LOADTEST=" + json.dumps(recipe, separators=(",", ":")))
        print(f"OVERMATCH_BOT={args.tanks - (args.clients or args.tanks)}")
        print("OVERMATCH_SERVER_STATS_S=1")
        if args.replication_trace:
            print("RUST_LOG=info," + REPLICATION_TRACE)
        for key, value in args.server_env:
            print(f"{key}={value}")
        return 0
    if recipe and args.duration <= recipe["events"][-1]["at_s"] + STATS_PERIOD_S:
        parser.error("--duration must leave a complete stats period after the last scheduled break")
    out = (args.out or ROOT / "target" / time.strftime("collapse-%Y%m%d-%H%M%S")).resolve()
    out.mkdir(parents=True, exist_ok=True)
    if args.sample and not Path("/usr/bin/sample").is_file():
        parser.error("--sample needs macOS /usr/bin/sample")
    if args.holding_lock:
        if args.server_env:
            (out / "server-env.json").write_text(json.dumps(dict(args.server_env), indent=2) + "\n")
            print("server env (server only): " + " ".join(f"{k}={v}" for k, v in args.server_env), flush=True)
        if ram:
            run_ram(args, out, ram)
        elif recipe:
            run_match(args, out, recipe)
        else:
            run(args, out)
        return 0
    env = os.environ.copy()
    env["OVERMATCH_CARGO_LOCK_WAIT_S"] = "10800"
    start = time.monotonic()
    build = [str(ROOT / "scripts/serial-cargo"), "build", "--locked"]
    if not (recipe or ram) or args.target == "local":
        build.extend(["--bin", "overmatch-server"])
    if recipe or ram:
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
