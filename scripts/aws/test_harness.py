"""Offline checks of the AWS harness's cost guard, isolation, teardown and tables: the real harness and
machine.sh, with `aws`, `ssh`, `ssh-keygen` and `curl` replaced by fixtures on PATH."""
import contextlib
import dataclasses
import base64
import shlex
import importlib.util
import io
import json
import math
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("harness", HERE / "harness.py")
harness = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(harness)

# A fake EC2: one JSON state file, every call logged. Fixture ids, addresses and the account are the
# shapes the scrubber must hide.
FAKE_AWS = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
argv = sys.argv[1:]
with open(os.environ["CALLS"], "a") as log:
    log.write(json.dumps({"argv": argv, "cloudflare": "CLOUDFLARE_GLOBAL_KEY" in os.environ}) + "\n")
path = Path(os.environ["FAKE_EC2"])
state = json.loads(path.read_text())
service, op, rest = argv[6], argv[7], argv[8:]

def opt(name):
    return rest[rest.index(name) + 1] if name in rest else None

def opts(name):
    if name not in rest:
        return []
    out = []
    for word in rest[rest.index(name) + 1:]:
        if word.startswith("--"):
            break
        out.append(word)
    return out

def filters():
    return {f.split(",Values=")[0].removeprefix("Name="): f.split(",Values=")[1].split(",") for f in opts("--filters")}

def matches(resource, flt):
    tags = {t["Key"]: t["Value"] for t in resource.get("Tags", [])}
    return all(tags.get(n[4:]) in v for n, v in flt.items() if n.startswith("tag:"))

def fail(message, code=254):
    sys.stderr.write(message + "\n")
    sys.exit(code)

def new_id(prefix):
    state["n"] += 1
    return f"{prefix}-{state['n']:017x}"

def tags_for(kind):
    return next((s["Tags"] for s in json.loads(opt("--tag-specifications") or "[]") if s["ResourceType"] == kind), [])

LISTS = {"describe-volumes": ("volumes", "Volumes"), "describe-network-interfaces": ("interfaces", "NetworkInterfaces"),
         "describe-key-pairs": ("key_pairs", "KeyPairs"), "describe-security-groups": ("groups", "SecurityGroups")}
VCPUS = {"c7a.medium": 1, "c7a.large": 2, "c7a.xlarge": 4, "t3.small": 2}
out = {}
if op == "get-caller-identity":
    if os.environ.get("SCENARIO") == "expired":
        fail("Error when retrieving token from sso: Token has expired and refresh failed "
             "(arn:aws:sts::123456789012:assumed-role/fixture)", 255)
    out = {"Account": "123456789012", "Arn": "arn:aws:sts::123456789012:assumed-role/fixture/fixture"}
elif op == "get-parameter":
    out = {"Parameter": {"Value": "ami-0123456789abcdef0"}}
elif op == "describe-images":
    out = {"Images": [{"ImageId": "ami-0123456789abcdef0", "Architecture": "x86_64", "State": "available",
                       "Public": True, "RootDeviceName": "/dev/sda1",
                       "Name": "ubuntu/images/hvm-ssd-gp3/ubuntu-resolute-26.04-amd64-server-20261104.1"}]}
elif op == "describe-instance-types":
    out = {"InstanceTypes": [{"InstanceType": n, "VCpuInfo": {"DefaultVCpus": VCPUS[n]},
                              "ProcessorInfo": {"SupportedArchitectures": ["x86_64"]},
                              "BurstablePerformanceSupported": n.startswith("t")} for n in opts("--instance-types")]}
elif op == "describe-instances":
    flt, ids = filters(), opts("--instance-ids")
    found = [i for i in state["instances"] if matches(i, flt) and (not ids or i["InstanceId"] in ids)
             and i["State"]["Name"] in flt.get("instance-state-name", [i["State"]["Name"]])
             and i["InstanceId"] in flt.get("instance-id", [i["InstanceId"]])]
    out = {"Reservations": [{"Instances": found}]}
elif op == "describe-vpcs":
    out = {"Vpcs": [{"VpcId": "vpc-0aaaaaaaaaaaaaaaa"}]}
elif op == "describe-subnets":
    out = {"Subnets": [{"SubnetId": "subnet-0bbbbbbbbbbbbbbb1", "AvailabilityZone": "ap-southeast-1b"},
                       {"SubnetId": "subnet-0bbbbbbbbbbbbbbb2", "AvailabilityZone": "ap-southeast-1a"}]}
elif op == "describe-instance-type-offerings":
    out = {"InstanceTypeOfferings": [{"InstanceType": t, "Location": z} for t in filters()["instance-type"]
                                     for z in ("ap-southeast-1a", "ap-southeast-1b")]}
elif op in LISTS:
    out = {LISTS[op][1]: [r for r in state[LISTS[op][0]] if matches(r, filters())]}
elif op == "create-security-group":
    group = {"GroupId": new_id("sg"), "Tags": tags_for("security-group"), "IpPermissions": []}
    state["groups"].append(group)
    out = {"GroupId": group["GroupId"]}
elif op == "authorize-security-group-ingress":
    next(g for g in state["groups"] if g["GroupId"] == opt("--group-id"))["IpPermissions"] += json.loads(opt("--ip-permissions"))
elif op == "run-instances":
    iid = new_id("i")
    n = state["n"]
    kind = opt("--instance-type")
    state["instances"].append({"InstanceId": iid, "InstanceType": kind, "State": {"Name": "running"},
                               "Tags": tags_for("instance"), "PublicIpAddress": f"192.0.2.{n}",
                               "PrivateIpAddress": f"10.0.0.{n}", "CpuOptions": {"CoreCount": VCPUS[kind], "ThreadsPerCore": 1},
                               "Groups": json.loads(opt("--network-interfaces"))[0]["Groups"]})
    state["volumes"].append({"VolumeId": new_id("vol"), "Instance": iid, "Tags": tags_for("volume")})
    state["interfaces"].append({"NetworkInterfaceId": new_id("eni"), "Instance": iid, "Tags": tags_for("network-interface")})
    out = {"Instances": [{"InstanceId": iid}]}
elif op == "reboot-instances":
    for i in state["instances"]:
        if i["InstanceId"] in opts("--instance-ids"):
            i["Reboots"] = i.get("Reboots", 0) + 1
elif op == "terminate-instances":
    ids = opts("--instance-ids")
    for i in state["instances"]:
        if i["InstanceId"] in ids:
            i["State"] = {"Name": "terminated"}
    state["volumes"] = [v for v in state["volumes"] if v.get("Instance") not in ids]
    state["interfaces"] = [e for e in state["interfaces"] if e.get("Instance") not in ids]
elif op == "delete-security-group":
    gid = opt("--group-id")
    held = any(gid in i.get("Groups", []) and i["State"]["Name"] != "terminated" for i in state["instances"])
    named = any(pair["GroupId"] == gid for g in state["groups"] for p in g["IpPermissions"] for pair in p.get("UserIdGroupPairs", []))
    if held or named:
        fail(f"An error occurred (DependencyViolation) when calling the DeleteSecurityGroup operation: resource {gid} has a dependent object")
    state["groups"] = [g for g in state["groups"] if g["GroupId"] != gid]
else:
    fail(f"fake aws: unhandled {service} {op}", 99)
path.write_text(json.dumps(state))
print(json.dumps(out))
'''

# A fake machine behind SSH: the role comes from the instance the address belongs to.
FAKE_SSH = r'''#!/usr/bin/env python3
import json, os, shlex, sys
from pathlib import Path
argv = sys.argv[1:]
host = next(a for a in argv if a.startswith("ubuntu@"))
words = shlex.split(" ".join(argv[argv.index(host) + 1:]))
words = words[words.index("/usr/local/bin/om-harness") + 1:]
state = json.loads(Path(os.environ["FAKE_EC2"]).read_text())
instance = next(i for i in state["instances"] if i.get("PublicIpAddress") == host.split("@")[1])
role = {t["Key"]: t["Value"] for t in instance["Tags"]}["Role"]
with open(os.environ["SSH_CALLS"], "a") as log:
    log.write(json.dumps({"role": role, "words": words}) + "\n")
# NETLESS=<role>:<n>: that machine's first n boots come up without a network.
netless_role, _, netless_boots = os.environ.get("NETLESS", "").partition(":")
if netless_role == role and instance.get("Reboots", 0) < int(netless_boots):
    sys.stderr.write("ssh: connect to host port 22: Operation timed out\n")
    sys.exit(255)
clients, window = int(os.environ["FIXTURE_CLIENTS"]), float(os.environ["FIXTURE_WINDOW_S"])
rows = json.loads(Path(os.environ["ROWS"]).read_text())
cmd, args = words[0], words[1:]
if cmd == "state" and os.environ.get("STATE_FAILS") == role:
    sys.stderr.write("Failed to open the journal: Input/output error\n")
    sys.exit(1)
if cmd == "state":
    lines = ["userdata 10.0", "guard 10.1", "self 10.2", "key 10.3"]
    lines += (["packages 20.0"] if role == "client" else []) + ["downloaded 22.0"]
    if role == "server" and os.environ.get("BOOT_FAIL") == "verify":
        lines.append("failed verify 22.3")
    else:
        lines += ["verified 22.3", "unpacked 23.0"] + (["started 23.1"] if role == "server" else []) + ["ready 23.2"]
        lines += ["unit active", "listening 24.0"] if role == "server" else []
    print("\n".join(lines))
elif cmd == "host":
    print("cpus 2\nmodel Fixture CPU")
elif cmd == "uptime-echo":
    for _ in sys.stdin:
        print("100.00", flush=True)
elif cmd == "client-state":
    print("unit active\nslot 101.0")
elif cmd == "samples":
    units = ["overmatch-server"] if role == "server" else [f"om-client-{k}" for k in range(1, clients + 1)]
    for t in range(2000):
        cells = " ".join(f"{u}={t * 250000},204800" for u in units)
        print(f"{t}.00 {cells} cpu  {t * 60} 0 {t * 40} {t * 100} 0 0 0 {t} 0 0")
elif cmd == "journal" and args[0] == "overmatch-server":
    print("[   24.000000] fixture overmatch-server[7]: server: starting, listening on 0.0.0.0:5888")
    print("[   30.000000] fixture overmatch-server[7]: server: client connected: 10.0.0.2:40000 (link 9v1)")
    print("[   30.500000] fixture overmatch-server[7]: server: Battle 1 Combat { ends_at: SimTick(1) } at tick 1952")
    at = window
    while at < 1500:
        record = json.dumps(rows["server"])
        if os.environ.get("MALFORMED_STATS") and at > 4 * window:
            record = record[:40]
        print(f"[{at:12.6f}] fixture overmatch-server[7]: server_stats {record}")
        at += window
elif cmd == "journal":
    print("[  101.000000] fixture overmatch[9]: client: controlled entity 4v2 — input slot")
    # NO_JITTER: a release that prints no client_jitter record.
    for t in range(102, 102 if os.environ.get("NO_JITTER") else 1500):
        print(f"[{t:12.6f}] fixture overmatch[9]: INFO overmatch::net::frontier: client_jitter "
              + json.dumps(rows["jitter"]))
'''

FAKE_KEYGEN = r'''#!/usr/bin/env python3
import sys
from pathlib import Path
key = Path(sys.argv[sys.argv.index("-f") + 1])
key.write_text("fixture private key\n")
key.with_name(key.name + ".pub").write_text("ssh-ed25519 AAAAfixture overmatch-harness\n")
'''

FAKE_CURL = r'''#!/usr/bin/env python3
import os, re, sys
from pathlib import Path
url = sys.argv[-1]
if "checkip" in url:
    print("192.0.2.99")
    sys.exit(0)
# Each repository file read at a tag, served from the file its variable names; NO_<variable>: a tag
# without it.
RAW = {"assets/net_timing.ron": "NET_TIMING", "assets/modes/battle.ron": "BATTLE_MODE"}
if url.startswith("https://raw.githubusercontent.com/"):
    served = next(name for path, name in RAW.items() if url.endswith("/" + path))
    if os.environ.get("NO_" + served):
        sys.stderr.write("curl: (22) The requested URL returned error: 404\n")
        sys.exit(22)
    print(Path(os.environ[served]).read_text(), end="")
    sys.exit(0)
tag, name = re.search(r"/download/([^/]+)/([^/]+)$", url).groups()
if tag == "v9.9.9":
    sys.stderr.write("curl: (22) The requested URL returned error: 404\n")
    sys.exit(22)
archive = name.removesuffix(".sha256")
print(f"{'a' * 64}  {archive if archive.endswith('.tar.gz') else archive + '.tar.gz'}")
'''

# The commands machine.sh calls, for running its own entrypoint here: every call logged in order;
# `install` and `curl` do their filesystem part; `journalctl` prints JOURNAL, or fails on JOURNAL_FAILS.
FAKE_MACHINE_COMMAND = r'''#!/usr/bin/env python3
import json, os, shutil, sys
from pathlib import Path
name, args = Path(sys.argv[0]).name, sys.argv[1:]
with open(os.environ["MACHINE_CALLS"], "a") as log:
    log.write(json.dumps([name, *args]) + "\n")
if os.environ.get("MACHINE_FAIL") == name:
    sys.exit(1)
if name == "install":
    operands, directory, words = [], False, iter(args)
    for word in words:
        if word == "-d":
            directory = True
        elif word in ("-m", "-o", "-g"):
            next(words)
        else:
            operands.append(word)
    if directory:
        for path in operands:
            Path(path).mkdir(parents=True, exist_ok=True)
    else:
        shutil.copy(*operands)
elif name == "curl" and "-o" in args:
    Path(args[args.index("-o") + 1]).write_bytes(b"fixture archive")
elif name == "sha256sum":
    line = sys.stdin.read().strip()
    _, filename = line.split(None, 1)
    if not Path(filename.lstrip("*")).is_file():
        sys.exit(1)
elif name == "dpkg-query":
    sys.exit(1)
elif name == "journalctl":
    if os.environ.get("JOURNAL_FAILS"):
        sys.stderr.write("Failed to open the journal: Input/output error\n")
        sys.exit(1)
    print(os.environ.get("JOURNAL", ""), end="")
elif name == "systemctl" and args[:1] == ["show"]:
    if os.environ.get("UPDATER_QUERY_FAILS"):
        sys.exit(1)
    print("LoadState=" + os.environ.get("UPDATER_LOAD", "masked"))
    print("ActiveState=" + os.environ.get("UPDATER_STATE", "inactive"))
elif name == "systemctl" and args[:1] == ["is-enabled"]:
    print("masked")
    sys.exit(1)
elif name == "systemctl" and args[:1] == ["is-active"]:
    print("active")
'''
MACHINE_COMMANDS = ("install", "shutdown", "chown", "dpkg-query", "apt-get", "curl", "sha256sum", "tar",
                    "systemctl", "journalctl", "useradd")
# Where each machine path machine.sh names lands under a test's root.
MACHINE_PATHS = {"/var/lib/om-harness": "state", "/opt/overmatch": "opt", "/usr/local/bin/om-harness": "om-harness",
                 "/etc/overmatch-report-header": "report-header", "/etc/overmatch-host.env": "host.env",
                 "/home/ubuntu": "home", "/etc/systemd/system": "units", "/proc/uptime": "uptime"}

# The timing tastes' and the battle mode's owners, which the fake release serves at every tag.
OWNER, MODE = harness.ROOT / harness.NET_TIMING, harness.ROOT / harness.BATTLE_MODE
TIMING = harness.net_timing(OWNER.read_text(), str(OWNER))
COMBAT_S = harness.combat_s(MODE.read_text(), str(MODE))
# Fixture: three of the release's longest timing windows, so each holds at least two disjoint
# view-window histograms, or the longest window its first combat admits.
WINDOW_S = min(3 * math.ceil(max(TIMING.view_window_s, TIMING.uplink_window_s)),
               int((COMBAT_S - harness.WARMUP_S) // (harness.WINDOWS + 1)))
SETTINGS = ["--tag", "v0.7.0-alpha.19", "--server-type", "c7a.large", "--client-type", "c7a.large",
            "--bots", "26", "--clients", "4", "--window-s", str(WINDOW_S)]
# Fixture: the 64 Hz tick's width, the bins' width in both records.
BIN_MS = 15.625


def wholly_at_or_above(bins: list[int] | tuple[int, ...], bound_s: float) -> int | None:
    """The samples in bins whose lower edge is at or above the bound; None when the bound lies inside
    the open-ended last bin, which holds samples on both sides of it."""
    if (len(bins) - 1) * BIN_MS < bound_s * 1e3:
        return None
    return sum(count for i, count in enumerate(bins) if i * BIN_MS >= bound_s * 1e3)


def fixture_histogram(bins: list[int], p50: float, p95: float, top: float) -> dict:
    """A timing record's histogram in the shape src/net/timing.rs prints: 16 tick bins and an
    open-ended one."""
    padded = bins + [0] * (17 - len(bins))
    return {"n": sum(padded), "min": 0.0, "p50": p50, "p95": p95, "max": top, "bin_ms": BIN_MS, "bins": padded}


# Fixture: a live link with a few long gaps, its S at the clamp of the longest.
ARRIVAL = {"state": "live", "silence_ms": 0.0, "stale_threshold_ms": 250.0, "stale_declarations": 1,
           "stale_arrivals": 2, "gap_ms": fixture_histogram([600, 30, 5, 0, 0, 0, 2, 3] + [0] * 8 + [1], 0.0, 15.625, 260.0)}


def stats_row(clients: int, period_s: float, arrival: dict | None = ARRIVAL) -> dict:
    """A server_stats object in src/net/server_stats.rs's shape; arrival None: a release without the
    input-gap census."""
    links = [f"{k}v1" for k in range(clients)]
    return {"period_s": period_s, "clients": clients, "tanks": 30, "projectiles": 3, "overruns": 2,
            "tick_us": {"p50": 7000.0, "p95": 9000.0, "p99": 11000.0, "max": 20000.0, "total": 1.0},
            "wall": {"discarded_ms": 0.0}, "cpu_pct": 80.0, "rss_mb": 220.0, "send_us": {"p95": 500.0},
            "send_clients": [{"link": l, "bytes": 1, "bytes_per_s": 180000.0, "max_packets": 3} for l in links],
            "inputs": [{"link": l} | ({"arrival": arrival} if arrival is not None else {})
                       for l in links]}


# Fixture: one client_jitter object (src/net/frontier.rs), its view clock completing every tick of a
# 64 Hz second.
JITTER = {"period_s": 1.0, "starved": {"events": 1, "ms": 2.5, "longest_ms": 2.5},
          "min_delay_ms": {"applied": 60.0, "target": 55.0}, "rtt_ms": 9.0,
          "view_clock": {"delta_star_ms": -1400.0, "today_stamp_lag_ms": -1390.0, "target_minus_today_ms": -10.0,
                         "target_above_latest_x_ms": 20.0,
                         "stamp_interval_ms": fixture_histogram([327, 310, 2], 15.6, 18.2, 40.0),
                         "completion": {"completed_ticks": 64, "sampled_ticks": 64, "late_ticks": 0,
                                        "worst_lag_frames": 0, "worst_lag_ms": 0.0, "evicted_incomplete": 0,
                                        "completed_without_stamp": 0, "unmapped_stamps": 0, "receipt_overflow": 0},
                         "pending_ticks": 1}}
FOREIGN = {"InstanceId": "i-0f0f0f0f0f0f0f0f0", "InstanceType": "c7a.medium", "State": {"Name": "running"},
           "Tags": [{"Key": "Name", "Value": "om-worker"}], "CpuOptions": {"CoreCount": 1, "ThreadsPerCore": 1}}
WRITES = {"create-security-group", "authorize-security-group-ingress", "run-instances", "reboot-instances",
           "terminate-instances", "delete-security-group"}
LEAKS = re.compile(r"\b(?:i|sg|vol|eni|ami|subnet|vpc)-[0-9a-f]{8,}|192\.0\.2\.\d+|10\.0\.0\.\d+|123456789012|arn:aws")


class Fixture:
    """One temporary world: fake commands on PATH, a fake EC2 state, the lines the fake machines
    print, and a run's captured output."""

    def __init__(self, cleanup, seed: dict | None = None, server: dict | None = None, client: dict = JITTER,
                 **env: str):
        temp = tempfile.TemporaryDirectory()
        cleanup(temp.cleanup)
        self.root = Path(temp.name)
        binary = self.root / "bin"
        binary.mkdir()
        for name, text in {"aws": FAKE_AWS, "ssh": FAKE_SSH, "ssh-keygen": FAKE_KEYGEN, "curl": FAKE_CURL}.items():
            (binary / name).write_text(text)
            (binary / name).chmod(0o755)
        self.ec2 = self.root / "ec2.json"
        world = {"n": 0, "instances": [], "volumes": [], "interfaces": [], "key_pairs": [], "groups": []}
        self.ec2.write_text(json.dumps(world | (seed or {})))
        clients, window_s = int(option(SETTINGS, "--clients")), float(option(SETTINGS, "--window-s"))
        rows = self.root / "rows.json"
        rows.write_text(json.dumps({"server": server or stats_row(clients, window_s), "jitter": client}))
        (self.root / "tmp").mkdir()
        self.env = {"PATH": f"{binary}:{os.environ['PATH']}", "FAKE_EC2": str(self.ec2), "TMPDIR": str(self.root / "tmp"),
                    "CALLS": str(self.root / "calls.jsonl"), "SSH_CALLS": str(self.root / "ssh.jsonl"),
                    "ROWS": str(rows), "NET_TIMING": str(OWNER), "BATTLE_MODE": str(MODE),
                    "FIXTURE_CLIENTS": str(clients),
                    "FIXTURE_WINDOW_S": str(window_s), "CLOUDFLARE_GLOBAL_KEY": "fixture"} | env

    def main(self, *argv: str) -> int:
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.dict(os.environ, self.env), mock.patch.object(harness.time, "sleep", lambda _s: None), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                status = harness.main(list(argv))
            except SystemExit as exit_:
                status = exit_.code
        self.stdout, self.stderr = out.getvalue(), err.getvalue()
        return status

    def calls(self) -> list[dict]:
        path = self.root / "calls.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def ops(self) -> list[str]:
        return [call["argv"][7] for call in self.calls()]

    def world(self) -> dict:
        return json.loads(self.ec2.read_text())

    def results(self) -> dict:
        return json.loads(next((self.root / "tmp").glob("overmatch-aws-harness/*/results.json")).read_text())


class MachineScript:
    """A generated machine.sh run through its own entrypoint: its machine paths moved under a
    temporary root, the commands it calls replaced by logging fakes, its control flow untouched."""

    def __init__(self, cleanup, user_data: str):
        temp = tempfile.TemporaryDirectory()
        cleanup(temp.cleanup)
        self.root = Path(temp.name)
        binary = self.root / "bin"
        binary.mkdir()
        for name in MACHINE_COMMANDS:
            (binary / name).write_text(FAKE_MACHINE_COMMAND)
            (binary / name).chmod(0o755)
        (self.root / "units").mkdir()
        (self.root / "uptime").write_text("12.50 3.00\n")
        for path, under in MACHINE_PATHS.items():
            user_data = user_data.replace(path, str(self.root / under))
        self.boot_script = self.root / "user-data"
        self.boot_script.write_text(user_data)
        self.installed = self.root / MACHINE_PATHS["/usr/local/bin/om-harness"]
        self.env = {"PATH": f"{binary}:{os.environ['PATH']}", "MACHINE_CALLS": str(self.root / "calls.jsonl")}

    def boot(self) -> subprocess.CompletedProcess:
        """cloud-init's call: the user-data as root, no arguments."""
        return subprocess.run(["bash", str(self.boot_script)], env=self.env, capture_output=True, text=True, timeout=60)

    def command(self, *argv: str, **env: str) -> subprocess.CompletedProcess:
        """The operator's call over SSH: the installed copy with a command."""
        return subprocess.run(["bash", str(self.installed), *argv], env=self.env | env, capture_output=True,
                              text=True, timeout=60)

    def calls(self) -> list[list[str]]:
        return [json.loads(line) for line in (self.root / "calls.jsonl").read_text().splitlines()]

    def phases(self) -> list[str]:
        return [line.split()[0] for line in (self.root / "state" / "phases").read_text().splitlines()]


def leftovers() -> dict:
    """An earlier run's machine and group, beside a foreign machine and group."""
    harness_tags = [{"Key": k, "Value": v} for k, v in harness.TAGS.items()] + [{"Key": "Run", "Value": "earlier"}]
    return {
        "instances": [FOREIGN, {**FOREIGN, "InstanceId": "i-0d0d0d0d0d0d0d0d0", "Tags": harness_tags, "Groups": ["sg-0d0d0d0d0d0d0d0d0"]}],
        "groups": [{"GroupId": "sg-0d0d0d0d0d0d0d0d0", "Tags": harness_tags, "IpPermissions": []},
                   {"GroupId": "sg-0c0c0c0c0c0c0c0c0", "Tags": [{"Key": "Name", "Value": "om-worker"}], "IpPermissions": []}],
    }


def tags(resource: dict) -> dict[str, str]:
    return {t["Key"]: t["Value"] for t in resource.get("Tags", [])}


def option(argv: list[str], name: str) -> str:
    return argv[argv.index(name) + 1]


class HostedBoot(unittest.TestCase):
    """The provider and harness share this entrypoint; the hosted role must keep its lifetime guards."""

    def machine(self):
        archive_name = "overmatch-server-v0.8.0-fixture-x86_64-unknown-linux-gnu.tar.gz"
        values = {"ROLE": "hosted", "CAP_MIN": 1440, "ARCHIVE_URL": "https://release.example/" + archive_name,
                  "ARCHIVE_SHA256": "a" * 64 + "  " + archive_name, "HOSTED_API": "https://worker.example",
                  "HOSTED_SERVER_ID": "fixture-server", "HOSTED_REPORT_SECRET": "fixture-report",
                  "HOSTED_ENV_B64": base64.b64encode(b'OVERMATCH_SERVER_ID="fixture-server"\n').decode()}
        script = (HERE / "machine.sh").read_text().replace(harness.SETTINGS_MARK,
                 "".join(f"{k}={shlex.quote(str(v))}\n" for k, v in values.items()))
        return MachineScript(self.addCleanup, script)

    def test_hosted_boot_installs_no_package_or_ssh_and_powers_off_on_every_service_exit(self):
        machine = self.machine()
        result = machine.boot()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = machine.calls()
        self.assertFalse(any(c[0] in ("dpkg-query", "apt-get", "chown") for c in calls))
        self.assertLess(next(i for i, c in enumerate(calls) if c[0] == "shutdown"),
                        next(i for i, c in enumerate(calls) if c[0] == "curl"))
        unit = (machine.root / "units/overmatch-server.service").read_text()
        self.assertIn("ExecStopPost=+/usr/bin/systemctl --no-block poweroff", unit)
        self.assertIn("EnvironmentFile=", unit)
        self.assertIn("User=overmatch", unit)
        self.assertIn("NoNewPrivileges=yes", unit)
        self.assertIn(["useradd", "--system", "--no-create-home", "--shell", "/usr/sbin/nologin", "overmatch"], calls)
        self.assertNotIn("sudo", unit)
        mask = next(i for i, c in enumerate(calls) if c[:3] == ["systemctl", "mask", "--now"])
        start = calls.index(["systemctl", "start", "overmatch-server"])
        self.assertLess(mask, start)
        self.assertTrue({"apt-daily.timer", "apt-daily-upgrade.timer", "unattended-upgrades.service", "snapd.service", "snapd.socket"} <= set(calls[mask]))
        verify = next(c for c in calls if c[0] == "sha256sum")
        self.assertEqual(verify[1:], ["--check", "--strict", "--quiet", "-"])
        self.assertFalse((machine.root / "state/server.tar.gz").exists())
        self.assertEqual((machine.root / "host.env").stat().st_mode & 0o777, 0o600)
        self.assertEqual((machine.root / "report-header").stat().st_mode & 0o777, 0o600)

    def test_every_unconfirmed_updater_state_refuses_play_reports_boot_updates_and_powers_off(self):
        cases = [{'UPDATER_STATE': active} for active in
                 ('active', 'activating', 'deactivating', 'reloading', '', 'future-state')]
        cases.append({'UPDATER_QUERY_FAILS': '1'})
        for values in cases:
            with self.subTest(values=values):
                machine = self.machine()
                machine.env.update(values)
                self.assertNotEqual(machine.boot().returncode, 0)
                self.assertNotIn(["systemctl", "start", "overmatch-server"], machine.calls())
                self.assertIn(["systemctl", "--no-block", "poweroff"], machine.calls())
                report = next(c for c in machine.calls() if c[0] == 'curl' and '--data' in c)
                self.assertEqual(json.loads(report[report.index('--data') + 1]), {'reason': 'boot-updates'})

    def test_failed_or_absent_updaters_allow_play(self):
        for values in ({'UPDATER_STATE': 'failed'}, {'UPDATER_LOAD': 'not-found', 'UPDATER_STATE': 'inactive'}):
            machine = self.machine()
            machine.env.update(values)
            self.assertEqual(machine.boot().returncode, 0)

    def test_failed_checksum_reports_the_step_and_powers_off_without_starting_a_service(self):
        machine = self.machine()
        machine.env["MACHINE_FAIL"] = "sha256sum"
        result = machine.boot()
        self.assertNotEqual(result.returncode, 0)
        calls = machine.calls()
        reports = [c for c in calls if c[0] == "curl" and "--data" in c]
        self.assertEqual(len(reports), 1)
        self.assertEqual(json.loads(reports[0][reports[0].index("--data") + 1]), {"reason": "boot-verify"})
        self.assertIn(["systemctl", "--no-block", "poweroff"], calls)
        self.assertNotIn(["systemctl", "start", "overmatch-server"], calls)


class MeasuredRun(unittest.TestCase):
    """One whole run against the fakes, beside a foreign instance it must leave alone."""

    @classmethod
    def setUpClass(cls):
        cls.fixture = Fixture(cls.addClassCleanup, seed={"instances": [FOREIGN]})
        cls.status = cls.fixture.main("run", *SETTINGS)

    def test_a_run_prints_its_windows_and_leaves_nothing_but_what_it_found(self):
        self.assertEqual(self.status, 0, self.fixture.stderr)
        cost = self.fixture.stdout.split("View clock")[0]
        data_rows = [line.split() for line in cost.splitlines() if re.match(r"\s*\d+\s", line)]
        self.assertEqual([row[0] for row in data_rows], [str(i) for i in range(1, harness.WINDOWS + 1)])
        # Each client reports 2.5 ms starved per 1 s line, so a window sums its own seconds' lines,
        # read on the client machine's clock.
        window_s = float(option(SETTINGS, "--window-s"))
        starved = harness.COLUMNS.index("starved_ms")
        self.assertEqual({float(row[starved]) for row in data_rows}, {2.5 * window_s})
        clients = int(option(SETTINGS, "--clients"))
        for column, field in (("stale_decl", "stale_declarations"), ("stale_arr", "stale_arrivals")):
            self.assertEqual({int(row[harness.COLUMNS.index(column)]) for row in data_rows},
                             {clients * ARRIVAL[field]})
        self.assertIn("proof, region-wide, tagged Purpose=harness: instances 0, volumes 0, network interfaces 0, "
                      "security groups 0, key pairs 0", self.fixture.stderr)
        world = self.fixture.world()
        self.assertEqual([i["InstanceId"] for i in world["instances"] if i["State"]["Name"] != "terminated"],
                         [FOREIGN["InstanceId"]])
        self.assertEqual((world["groups"], world["volumes"], world["interfaces"]), ([], [], []))

    def test_each_client_and_window_counts_its_own_view_clock_lines_once(self):
        # Completion counts are per-line deltas, so a window's count is the sum of its own lines; the
        # stamp-interval histograms each hold the view window before their line, so the window counts
        # one histogram per disjoint view window that fits in it, never one per 1 s line.
        view = self.fixture.results()["view_clock"]
        column = {name: i for i, name in enumerate(view["columns"])}
        clients, window_s = int(option(SETTINGS, "--clients")), float(option(SETTINGS, "--window-s"))
        self.assertEqual(sorted((row[0], row[1]) for row in view["rows"]),
                         [(k, i) for k in range(1, clients + 1) for i in range(1, harness.WINDOWS + 1)])
        per_line = JITTER["view_clock"]
        # Fixture journals emit on whole seconds: disjoint histograms need ceil(view_window_s)
        # seconds between lines, even when the owned window is fractional.
        fits = math.floor(window_s / max(1, math.ceil(TIMING.view_window_s)))
        for row in view["rows"]:
            lines = row[column["lines"]]
            self.assertEqual(row[column["done"]], per_line["completion"]["completed_ticks"] * lines)
            tiles, rest = divmod(row[column["int_n"]], per_line["stamp_interval_ms"]["n"])
            self.assertEqual(rest, 0)
            self.assertIn(tiles, range(max(fits - 1, 0), fits + 1))
            self.assertEqual(row[column["tgt_n"]], lines)

    def test_each_server_link_and_window_reads_that_windows_own_census(self):
        # The stale counts and the gaps against the release's bounds are the window's own line's,
        # never summed across lines; every connected link has a row in every window.
        gaps = self.fixture.results()["input_gaps"]
        column = {name: i for i, name in enumerate(gaps["columns"])}
        self.assertEqual(len(gaps["rows"]), int(option(SETTINGS, "--clients")) * harness.WINDOWS)
        bins = ARRIVAL["gap_ms"]["bins"]
        for row in gaps["rows"]:
            self.assertEqual(row[column["stale_arr"]], ARRIVAL["stale_arrivals"])
            self.assertEqual(row[column["S_ms"]], ARRIVAL["stale_threshold_ms"])
            self.assertEqual(row[column["gap_n"]], ARRIVAL["gap_ms"]["n"])
            self.assertEqual(row[column["gaps>=floor"]], wholly_at_or_above(bins, TIMING.stale_floor_s))
            self.assertEqual(row[column["gaps>=ceil"]], wholly_at_or_above(bins, TIMING.stale_ceil_s))

    def test_every_launch_carries_its_cost_guard(self):
        # A machine the operator loses must still delete itself: terminate on its own power-off, a
        # power-off its own boot schedules before it fetches anything, at a cap that outlives the
        # measurement, and a root volume that dies with it.
        launches = [c["argv"][8:] for c in self.fixture.calls() if c["argv"][7] == "run-instances"]
        self.assertEqual(len(launches), 2)
        window_s = int(option(SETTINGS, "--window-s"))
        for argv in launches:
            self.assertEqual(option(argv, "--instance-initiated-shutdown-behavior"), "terminate")
            self.assertIn("HttpTokens=required", option(argv, "--metadata-options"))
            self.assertTrue(all(m["Ebs"]["DeleteOnTermination"] for m in json.loads(option(argv, "--block-device-mappings"))))
            for spec in json.loads(option(argv, "--tag-specifications")):
                self.assertLessEqual(harness.TAGS.items(), tags(spec).items(), spec["ResourceType"])
            self.assertEqual({s["ResourceType"] for s in json.loads(option(argv, "--tag-specifications"))},
                             {"instance", "volume", "network-interface"})
            machine = MachineScript(self.addCleanup, Path(option(argv, "--user-data").removeprefix("file://")).read_text())
            booted = machine.boot()
            self.assertEqual(booted.returncode, 0, booted.stderr)
            self.assertEqual(machine.phases()[-1], "ready")
            calls = machine.calls()
            guard = next(i for i, call in enumerate(calls) if call[0] == "shutdown")
            fetch = next(i for i, call in enumerate(calls) if call[0] == "curl")
            self.assertLess(guard, fetch)
            self.assertEqual(calls[guard][1], "-h")
            cap = int(calls[guard][2].removeprefix("+"))
            self.assertGreater(cap * 60, harness.WARMUP_S + harness.WINDOWS * window_s)

    def test_server_boot_needs_no_package_manager_but_client_keeps_its_audio_runtime(self):
        # The server host must reach systemd without consulting a package manager or mirror.
        launches = [c["argv"][8:] for c in self.fixture.calls() if c["argv"][7] == "run-instances"]
        for argv in launches:
            user_data = Path(option(argv, "--user-data").removeprefix("file://")).read_text()
            machine = MachineScript(self.addCleanup, user_data)
            booted = machine.boot()
            self.assertEqual(booted.returncode, 0, booted.stderr)
            packages = [call for call in machine.calls() if call[0] in ("dpkg-query", "apt-get")]
            if "ROLE=server" in user_data:
                self.assertEqual(packages, [])
                self.assertNotIn("packages", machine.phases())
                self.assertIn(["systemctl", "start", "overmatch-server"], machine.calls())
            else:
                self.assertTrue(any(call[0] == "apt-get" and "libasound2t64" in call for call in packages))
                self.assertIn("packages", machine.phases())

    def test_the_game_port_is_open_only_to_the_client_group_and_ssh_only_to_the_operator(self):
        groups = {}
        for call in self.fixture.calls():
            if call["argv"][7] == "authorize-security-group-ingress":
                groups[option(call["argv"], "--group-id")] = json.loads(option(call["argv"], "--ip-permissions"))
        created = [c for c in self.fixture.calls() if c["argv"][7] == "create-security-group"]
        self.assertEqual(len(created), 2)
        server, client = (g for g, rules in sorted(groups.items(), key=lambda item: -len(item[1])))
        self.assertEqual([r for r in groups[server] if r["IpProtocol"] == "udp"],
                         [{"IpProtocol": "udp", "FromPort": harness.PORT, "ToPort": harness.PORT,
                           "UserIdGroupPairs": [{"GroupId": client}]}])
        for rules in groups.values():
            for rule in rules:
                if rule["IpProtocol"] == "tcp":
                    self.assertEqual((rule["FromPort"], rule["IpRanges"]), (22, [{"CidrIp": "192.0.2.99/32"}]))
        self.assertEqual({r["IpProtocol"] for rules in groups.values() for r in rules}, {"tcp", "udp"})

    def test_every_aws_call_is_the_overmatch_profile_in_singapore_without_the_cloudflare_key(self):
        for call in self.fixture.calls():
            self.assertEqual(call["argv"][:4], ["--profile", "overmatch", "--region", "ap-southeast-1"])
            self.assertFalse(call["cloudflare"])

    def test_nothing_printed_carries_an_id_an_address_or_the_account(self):
        self.assertIsNone(LEAKS.search(self.fixture.stdout + self.fixture.stderr))


class Failures(unittest.TestCase):
    def test_an_unavailable_aws_executable_is_refused_with_the_command_cause_and_remedy(self):
        # Preflight must tell the operator how to repair an OS failure before launching anything.
        for error in (FileNotFoundError("fixture missing executable"), PermissionError("fixture permission denied")):
            with self.subTest(error), mock.patch.object(harness.subprocess, "run", side_effect=error):
                with self.assertRaisesRegex(harness.Refused, "aws .*sts get-caller-identity.*fixture.*install the AWS CLI"):
                    harness.aws("sts", "get-caller-identity")

    def test_a_failure_names_its_cause_and_still_tears_down(self):
        clients, window_s = int(option(SETTINGS, "--clients")), float(option(SETTINGS, "--window-s"))
        no_view_clock = {key: value for key, value in JITTER.items() if key != "view_clock"}
        unbinned = ARRIVAL | {"gap_ms": ARRIVAL["gap_ms"] | {"n": ARRIVAL["gap_ms"]["n"] + 1}}
        row = stats_row(clients, window_s)
        # A link without its census entry, or with two, would leave the gaps table short or doubled.
        census = "`inputs` holds connected entries for"
        cases = [({"env": {"BOOT_FAIL": "verify"}}, "checksum mismatch"),
                 ({"env": {"NETLESS": "client:2"}}, "two boots without a network"),
                 ({"env": {"STATE_FAILS": "server"}}, "`state` failed: Failed to open the journal"),
                 ({"env": {"MALFORMED_STATS": "1"}}, "a server_stats record does not parse"),
                 ({"env": {"NO_JITTER": "1"}}, "client 1's client_jitter lines cover 0 s"),
                 ({"client": no_view_clock}, "client 1: a client_jitter record: no `view_clock`"),
                 ({"server": stats_row(clients, window_s, arrival=None)}, "no `arrival.state`"),
                 ({"server": stats_row(clients, window_s, arrival=unbinned)}, "are not n > 0 samples"),
                 ({"server": row | {"inputs": []}}, census),
                 ({"server": row | {"inputs": row["inputs"] + row["inputs"][:1]}}, census)]
        for case, cause in cases:
            with self.subTest(cause), mock.patch.object(harness, "REACH_TIMEOUT_S", 0):
                fixture = Fixture(self.addCleanup, server=case.get("server"), client=case.get("client", JITTER),
                                  **case.get("env", {}))
                self.assertEqual(fixture.main("run", *SETTINGS), 1)
                self.assertIn(cause, fixture.stderr)
                world = fixture.world()
                self.assertEqual([i for i in world["instances"] if i["State"]["Name"] != "terminated"], [])
                self.assertEqual((world["groups"], world["volumes"]), ([], []))
                self.assertIn("security groups 0", fixture.stderr)

    def test_a_boot_without_a_network_is_rebooted_once_and_left_out_of_the_cold_start_samples(self):
        with mock.patch.object(harness, "REACH_TIMEOUT_S", 0):
            fixture = Fixture(self.addCleanup, NETLESS="client:1")
            self.assertEqual(fixture.main("run", *SETTINGS), 0, fixture.stderr)
        rebooted = [option(c["argv"], "--instance-ids") for c in fixture.calls() if c["argv"][7] == "reboot-instances"]
        client = next(i["InstanceId"] for i in fixture.world()["instances"] if tags(i)["Role"] == "client")
        self.assertEqual(rebooted, [client])
        self.assertIn("client boot after a reboot, not a cold-start sample", fixture.stdout)
        self.assertIn("server cold start", fixture.stdout)

    def test_refusals_come_before_anything_is_created(self):
        def setting(name, value):
            argv = list(SETTINGS)
            argv[argv.index(name) + 1] = value
            return argv

        leftover = {"groups": [{"GroupId": "sg-0e0e0e0e0e0e0e0e0", "IpPermissions": [],
                                "Tags": [{"Key": k, "Value": v} for k, v in harness.TAGS.items()]}]}
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        # Fixture: a release whose uplink window outlasts the run's window by a second.
        longer = Path(temp.name) / "net_timing.ron"
        longer.write_text(re.sub(r"(uplink_window_s:\s*\()[^,]+", rf"\g<1>{WINDOW_S + 1}.0", OWNER.read_text()))
        # Fixture: a release whose first combat is a second shorter than the warmup and the windows the
        # harness waits for after it; the local mode data still admits them, so the tag's copy is read.
        shorter = Path(temp.name) / "battle.ron"
        need_s = harness.WARMUP_S + (harness.WINDOWS + 1) * WINDOW_S
        shorter.write_text(re.sub(r"(\bcombat_s:\s*)[0-9.]+", rf"\g<1>{need_s - 1}.0", MODE.read_text()))
        cases = [
            ("expired sign-in", SETTINGS, {"SCENARIO": "expired"}, None, 1, "sign-in"),
            ("unpublished tag", setting("--tag", "v9.9.9"), {}, None, 1, "is the tag published"),
            ("a tag before the timing records", SETTINGS, {"NO_NET_TIMING": "1"}, None, 1,
             "does the release carry the tick stamp"),
            ("timing windows past the run's window", SETTINGS, {"NET_TIMING": str(longer)}, None, 1,
             "shorter than the release's"),
            ("a tag without the battle mode", SETTINGS, {"NO_BATTLE_MODE": "1"}, None, 1,
             "does the tag carry the battle mode"),
            ("windows past the tag's combat", SETTINGS, {"BATTLE_MODE": str(shorter)}, None, 1, "do not fit"),
            ("over the quota", setting("--server-type", "c7a.xlarge"), {}, None, 1, "vCPUs"),
            ("burstable type", setting("--client-type", "t3.small"), {}, None, 1, "burstable"),
            ("an earlier run's leftovers", SETTINGS, {}, leftover, 1, "earlier run"),
            ("no client to open combat", setting("--clients", "0"), {}, None, 2, "at least 1"),
        ]
        for name, argv, env, seed, status, cause in cases:
            with self.subTest(name):
                fixture = Fixture(self.addCleanup, seed=seed, **env)
                self.assertEqual(fixture.main("run", *argv), status, fixture.stderr)
                self.assertIn(cause, fixture.stderr)
                self.assertEqual(WRITES & set(fixture.ops()), set())
                self.assertIsNone(LEAKS.search(fixture.stdout + fixture.stderr))

    def test_teardown_removes_every_harness_resource_and_nothing_else(self):
        fixture = Fixture(self.addCleanup, seed=leftovers())
        self.assertEqual(fixture.main("teardown"), 0, fixture.stderr)
        world = fixture.world()
        self.assertEqual({i["InstanceId"]: i["State"]["Name"] for i in world["instances"]},
                         {FOREIGN["InstanceId"]: "running", "i-0d0d0d0d0d0d0d0d0": "terminated"})
        self.assertEqual([g["GroupId"] for g in world["groups"]], ["sg-0c0c0c0c0c0c0c0c0"])

    def test_a_run_log_that_cannot_be_written_never_stops_teardown(self):
        # Teardown reports its progress through the log; a full disk there must still leave no group
        # behind and still print the proof.
        fixture = Fixture(self.addCleanup, seed=leftovers())
        with mock.patch.object(harness, "LOG", [fixture.root]):  # a directory refuses the write, as a full disk does
            self.assertEqual(fixture.main("teardown"), 0, fixture.stderr)
        self.assertIn("run log not written from here on", fixture.stderr)
        self.assertEqual([g["GroupId"] for g in fixture.world()["groups"]], ["sg-0c0c0c0c0c0c0c0c0"])
        self.assertIn("proof, region-wide, tagged Purpose=harness: instances 0", fixture.stderr)


class MachineCommands(unittest.TestCase):
    """The installed machine.sh's `state` on a booted server: the harness's wait reads its listening
    line, and treats any failure other than ssh's own as a refusal."""

    @classmethod
    def setUpClass(cls):
        values = dict(zip(SETTINGS[::2], SETTINGS[1::2]))
        settings = harness.Settings(values["--tag"], values["--server-type"], values["--client-type"],
                                    int(values["--bots"]), int(values["--clients"]), int(values["--window-s"]))
        archives = {role: (f"https://example.invalid/{role}.tar.gz", f"{'a' * 64}  {role}.tar.gz")
                    for role in ("server", "client")}
        plan = harness.Plan("ami-0123456789abcdef0", "/dev/sda1", "vpc", "subnet", "zone", "192.0.2.99/32", archives,
                            TIMING, COMBAT_S)
        cls.machine = MachineScript(cls.addClassCleanup, harness.user_data("server", settings, plan, "ssh-ed25519 AAAAfixture"))
        booted = cls.machine.boot()
        assert booted.returncode == 0, booted.stderr

    def test_the_listening_line_is_read_and_a_journal_without_it_is_no_line(self):
        found = self.machine.command("state", JOURNAL="[   24.000000] fixture overmatch-server[7]: server: "
                                                      "starting, listening on 0.0.0.0:5888\n")
        self.assertIn("listening 24.000000", found.stdout.splitlines())
        absent = self.machine.command("state", JOURNAL="[    3.000000] fixture overmatch-server[7]: loading\n")
        self.assertEqual(absent.returncode, 0, absent.stderr)
        self.assertFalse([line for line in absent.stdout.splitlines() if line.startswith("listening")])

    def test_a_journal_that_cannot_be_read_fails_state_with_its_error(self):
        failed = self.machine.command("state", JOURNAL_FAILS="1")
        self.assertNotIn(failed.returncode, (0, harness.SSH_FAILED))
        self.assertIn("Failed to open the journal", failed.stderr)


class Reading(unittest.TestCase):
    def test_input_rows_join_by_link_and_require_the_current_arrival_fields(self):
        # A reordered census must keep each link's counters; the retired counts object cannot
        # substitute for a missing arrival field in a release's stats line.
        row = stats_row(2, 30.0)
        row["inputs"].reverse()
        row["inputs"][0]["arrival"] = ARRIVAL | {"stale_declarations": 7, "stale_arrivals": 3}
        parsed = harness.stats(harness.Fields(row, "fixture"))
        self.assertEqual({entry.link: (entry.arrival.declarations, entry.arrival.stale_arrivals)
                          for entry in parsed.inputs}, {"1v1": (7, 3), "0v1": (1, 2)})
        for field in ("state", "gap_ms", "stale_threshold_ms", "stale_declarations", "stale_arrivals"):
            broken = stats_row(1, 30.0)
            broken["inputs"][0]["arrival"] = {k: v for k, v in ARRIVAL.items() if k != field}
            broken["inputs"][0]["counts"] = {"held": 10, "tick_exact": 630}
            with self.subTest(field), self.assertRaisesRegex(harness.Refused, rf"no `arrival\.{field}`"):
                harness.stats(harness.Fields(broken, "fixture"))
        broken = stats_row(1, 30.0)
        del broken["inputs"][0]["link"]
        with self.assertRaisesRegex(harness.Refused, "no `link`"):
            harness.stats(harness.Fields(broken, "fixture"))

    def test_a_window_counts_only_inside_combat_after_the_warmup_with_every_client(self):
        def line(at, clients=4):
            return at, harness.Fields(stats_row(clients, 10.0), "fixture")

        # Seven 10 s lines against a limit of three: the fourth line that counts (ending at 150) is
        # past the limit, neither chosen nor skipped.
        stats = [line(100), line(110, clients=3), line(120), line(130), line(140), line(150), line(170)]
        log = harness.ServerLog(combat_open=50.0, combat_end=165.0, stats=stats)
        with mock.patch.object(harness, "WINDOWS", 3):
            chosen, skipped = harness.pick_windows(log, clients=4, measure_from=95.0)
        self.assertEqual([(w.start, w.end) for w in chosen], [(110.0, 120.0), (120.0, 130.0), (130.0, 140.0)])
        self.assertEqual(skipped, {"before the warmup ended": 1, "not 4 clients connected": 1, "after combat": 1})

    def test_a_gap_counts_above_a_bound_only_from_a_bin_wholly_at_or_above_it(self):
        # The floor and ceiling columns are read against the release's own bounds; a bin that
        # straddles a bound holds gaps on both sides, so counting it would report gaps above the
        # floor that may all lie below it, and an open-ended last bin across it leaves the count unknown.
        columns = [harness.SERVER_COLUMNS.index(name) for name in ("gaps>=floor", "gaps>=ceil")]
        bins = len(ARRIVAL["gap_ms"]["bins"])
        for i in range(bins):
            one = tuple(int(j == i) for j in range(bins))
            gaps = harness.Histogram(1, 0.0, 0.0, 0.0, BIN_MS, one)
            row = harness.server_cells("0v1", 1, harness.Arrival("live", gaps, 100.0, 0, 0), TIMING)
            for column, bound_s in zip(columns, (TIMING.stale_floor_s, TIMING.stale_ceil_s)):
                self.assertEqual(row[column], wholly_at_or_above(one, bound_s), (i, bound_s))

    def test_a_histogram_that_cannot_be_read_against_a_bound_is_refused_where_it_is_parsed(self):
        # The bound columns divide by the bin width and sum counts from a bin on; a zero width would
        # fail later as a harness bug, and a negative count or a sum other than n would report gaps
        # the record does not hold, so each is refused at the record, naming it.
        good = ARRIVAL["gap_ms"]
        broken = {"no bin width": good | {"bin_ms": 0.0}, "an endless bin width": good | {"bin_ms": math.inf},
                  "a negative count": good | {"bins": [good["bins"][0] + good["bins"][1] + 1, -1] + good["bins"][2:]},
                  "bins short of n": good | {"n": good["n"] + 1}, "no sample": good | {"n": 0, "bins": [0] * 17}}
        self.assertIsNotNone(harness.histogram(harness.Fields({"gap_ms": good}, "fixture"), "gap_ms"))
        for name, value in broken.items():
            with self.subTest(name), self.assertRaisesRegex(harness.Refused, "^fixture `gap_ms`"):
                harness.histogram(harness.Fields({"gap_ms": value}, "fixture"), "gap_ms")

    def test_a_view_window_histogram_is_taken_once_per_view_window(self):
        # Each line's histogram holds the view window before it; lines a second apart overlap, so
        # summing them would count each interval up to a view window's worth of times.
        hist = harness.Histogram(10, 15.6, 18.0, 20.0, BIN_MS, (5, 5) + (0,) * 15)
        line = harness.Jitter(0.0, 1.0, 0.0, 60.0, hist, None, None, (0,) * len(harness.COMPLETION), 0.0)
        # Fixture: ten lines per view window over three view windows.
        lines = [dataclasses.replace(line, at=k * TIMING.view_window_s / 10) for k in range(1, 31)]
        self.assertEqual(len(harness.tiles(lines, 0.0, TIMING.view_window_s)), 3)

    def test_integer_journal_timestamps_keep_fractional_view_windows_inside_and_disjoint(self):
        # A 1.5 s view window cannot be sampled every 1.5 lines on a whole-second journal clock.
        for width in (1.5, TIMING.view_window_s):
            hist = harness.Histogram(1, 0.0, 0.0, 0.0, BIN_MS, (1,) + (0,) * 16)
            lines = [harness.Jitter(float(k), 1.0, 0.0, 60.0, dataclasses.replace(hist), None, None,
                                   (0,) * len(harness.COMPLETION), 0.0)
                     for k in range(1, math.ceil(3 * width) + 2)]
            taken = harness.tiles(lines, 0.25, width)
            stamps = {id(line.intervals): line.at for line in lines}
            ends = [stamps[id(h)] for h in taken]
            self.assertTrue(ends)
            self.assertGreaterEqual(ends[0] - width, 0.25)
            self.assertTrue(all(b - width >= a for a, b in zip(ends, ends[1:])))
            self.assertLessEqual(ends[-1], lines[-1].at)

    def test_sampled_cpu_is_percent_of_one_core_and_busy_is_of_the_whole_machine(self):
        # 2.5 CPU-seconds over 10 s is a quarter core; 300 of 400 jiffies not idle is 75 % busy.
        text = ("0.00 a=0,1024 cpu  0 0 0 0 0 0 0 0 0 0\n"
                "10.00 a=2500000,3072 cpu  200 0 80 80 20 0 0 20 0 0\n")
        span = harness.interval(harness.samples(text), 0.0, 10.0)
        self.assertAlmostEqual(span.cpu_pct("a"), 25.0)
        self.assertAlmostEqual(span.busy_pct(), 75.0)
        self.assertAlmostEqual(span.steal_pct(), 5.0)
        self.assertAlmostEqual(span.rss_mb("a"), 2.0)

    def test_scrub_hides_every_kind_the_rules_name(self):
        text = ("arn:aws:iam::123456789012:user/x i-0123456789abcdef0 sg-0123abcd vol-0123456789abcdef0 "
                "ami-0123456789abcdef0 54.1.2.3 ip-172-31-4-5 ec2-54-1-2-3 2406:da18:abc:1::5 account 123456789012")
        self.assertIsNone(re.search(r"\d+\.\d+\.\d+\.\d+|\d{12}|arn:|-[0-9a-f]{8,}|da18|172-31", harness.scrub(text)))
        self.assertEqual(harness.scrub("v0.7.0-alpha.18 tick 1952 at 12:30:05"), "v0.7.0-alpha.18 tick 1952 at 12:30:05")


if __name__ == "__main__":
    unittest.main()
