"""Offline checks of the AWS harness's cost guard, isolation, teardown and table: the real harness and
machine.sh, with `aws`, `ssh`, `ssh-keygen` and `curl` replaced by fixtures on PATH."""
import contextlib
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
cmd, args = words[0], words[1:]
if cmd == "state" and os.environ.get("STATE_FAILS") == role:
    sys.stderr.write("Failed to open the journal: Input/output error\n")
    sys.exit(1)
if cmd == "state":
    lines = ["userdata 10.0", "guard 10.1", "self 10.2", "key 10.3", "packages 20.0", "downloaded 22.0"]
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
    links = [f"{k}v1" for k in range(clients)]
    at = window
    while at < 1500:
        row = {"period_s": window, "clients": clients, "tanks": 30, "projectiles": 3, "overruns": 2,
               "tick_us": {"p50": 7000.0, "p95": 9000.0, "p99": 11000.0, "max": 20000.0, "total": 1.0},
               "wall": {"discarded_ms": 0.0}, "cpu_pct": 80.0, "rss_mb": 220.0, "send_us": {"p95": 500.0},
               "send_clients": [{"link": l, "bytes": 1, "bytes_per_s": 180000.0, "max_packets": 3} for l in links],
               "inputs": [{"link": l, "counts": {"tick_exact": 630, "held": 10}} for l in links]}
        record = json.dumps(row)
        if os.environ.get("MALFORMED_STATS") and at > 4 * window:
            record = record[:40]
        print(f"[{at:12.6f}] fixture overmatch-server[7]: server_stats {record}")
        at += window
elif cmd == "journal":
    print("[  101.000000] fixture overmatch[9]: client: controlled entity 4v2 — input slot")
    # NO_JITTER: a release that prints no client_jitter record.
    for t in range(102, 102 if os.environ.get("NO_JITTER") else 1500):
        print(f"[{t:12.6f}] fixture overmatch[9]: INFO overmatch::net::frontier: client_jitter "
              + json.dumps({"period_s": 1.0, "starved": {"events": 1, "ms": 2.5, "longest_ms": 2.5}}))
'''

FAKE_KEYGEN = r'''#!/usr/bin/env python3
import sys
from pathlib import Path
key = Path(sys.argv[sys.argv.index("-f") + 1])
key.write_text("fixture private key\n")
key.with_name(key.name + ".pub").write_text("ssh-ed25519 AAAAfixture overmatch-harness\n")
'''

FAKE_CURL = r'''#!/usr/bin/env python3
import re, sys
url = sys.argv[-1]
if "checkip" in url:
    print("192.0.2.99")
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
elif name == "curl":
    Path(args[args.index("-o") + 1]).write_bytes(b"fixture archive")
elif name == "sha256sum":
    sys.stdin.read()
elif name == "dpkg-query":
    sys.exit(1)
elif name == "journalctl":
    if os.environ.get("JOURNAL_FAILS"):
        sys.stderr.write("Failed to open the journal: Input/output error\n")
        sys.exit(1)
    print(os.environ.get("JOURNAL", ""), end="")
elif name == "systemctl" and args[:1] == ["is-active"]:
    print("active")
'''
MACHINE_COMMANDS = ("install", "shutdown", "chown", "dpkg-query", "apt-get", "curl", "sha256sum", "tar",
                    "systemctl", "journalctl")
# Where each machine path machine.sh names lands under a test's root.
MACHINE_PATHS = {"/var/lib/om-harness": "state", "/opt/overmatch": "opt", "/usr/local/bin/om-harness": "om-harness",
                 "/home/ubuntu": "home", "/etc/systemd/system": "units", "/proc/uptime": "uptime"}

SETTINGS = ["--tag", "v0.7.0-alpha.18", "--server-type", "c7a.large", "--client-type", "c7a.large",
            "--bots", "26", "--clients", "4", "--window-s", "10"]
FOREIGN = {"InstanceId": "i-0f0f0f0f0f0f0f0f0", "InstanceType": "c7a.medium", "State": {"Name": "running"},
           "Tags": [{"Key": "Name", "Value": "om-worker"}], "CpuOptions": {"CoreCount": 1, "ThreadsPerCore": 1}}
WRITES = {"create-security-group", "authorize-security-group-ingress", "run-instances", "reboot-instances",
           "terminate-instances", "delete-security-group"}
LEAKS = re.compile(r"\b(?:i|sg|vol|eni|ami|subnet|vpc)-[0-9a-f]{8,}|192\.0\.2\.\d+|10\.0\.0\.\d+|123456789012|arn:aws")


class Fixture:
    """One temporary world: fake commands on PATH, a fake EC2 state, and a run's captured output."""

    def __init__(self, cleanup, seed: dict | None = None, **env: str):
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
        (self.root / "tmp").mkdir()
        self.env = {"PATH": f"{binary}:{os.environ['PATH']}", "FAKE_EC2": str(self.ec2), "TMPDIR": str(self.root / "tmp"),
                    "CALLS": str(self.root / "calls.jsonl"), "SSH_CALLS": str(self.root / "ssh.jsonl"),
                    "FIXTURE_CLIENTS": "4", "FIXTURE_WINDOW_S": "10", "CLOUDFLARE_GLOBAL_KEY": "fixture"} | env

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


class MeasuredRun(unittest.TestCase):
    """One whole run against the fakes, beside a foreign instance it must leave alone."""

    @classmethod
    def setUpClass(cls):
        cls.fixture = Fixture(cls.addClassCleanup, seed={"instances": [FOREIGN]})
        cls.status = cls.fixture.main("run", *SETTINGS)

    def test_a_run_prints_its_windows_and_leaves_nothing_but_what_it_found(self):
        self.assertEqual(self.status, 0, self.fixture.stderr)
        data_rows = [line.split() for line in self.fixture.stdout.splitlines() if re.match(r"\s*\d+\s", line)]
        self.assertEqual([row[0] for row in data_rows], [str(i) for i in range(1, harness.WINDOWS + 1)])
        # Each client reports 2.5 ms starved per 1 s line, so a window sums its own seconds' lines,
        # read on the client machine's clock.
        window_s = float(option(SETTINGS, "--window-s"))
        starved = harness.COLUMNS.index("starved_ms")
        self.assertEqual({float(row[starved]) for row in data_rows}, {2.5 * window_s})
        self.assertIn("proof, region-wide, tagged Purpose=harness: instances 0, volumes 0, network interfaces 0, "
                      "security groups 0, key pairs 0", self.fixture.stderr)
        world = self.fixture.world()
        self.assertEqual([i["InstanceId"] for i in world["instances"] if i["State"]["Name"] != "terminated"],
                         [FOREIGN["InstanceId"]])
        self.assertEqual((world["groups"], world["volumes"], world["interfaces"]), ([], [], []))

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
    def test_a_failure_names_its_cause_and_still_tears_down(self):
        cases = [({"BOOT_FAIL": "verify"}, "checksum mismatch"),
                 ({"NETLESS": "client:2"}, "two boots without a network"),
                 ({"STATE_FAILS": "server"}, "`state` failed: Failed to open the journal"),
                 ({"MALFORMED_STATS": "1"}, "a server_stats record does not parse"),
                 ({"NO_JITTER": "1"}, "client 1's client_jitter records cover 0 s")]
        for env, cause in cases:
            with self.subTest(cause), mock.patch.object(harness, "REACH_TIMEOUT_S", 0):
                fixture = Fixture(self.addCleanup, **env)
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

        # The shortest window that no longer fits: the combat's length from the mode data it owns,
        # against the warmup and the windows the harness waits for after it.
        overlong = math.floor((harness.combat_s() - harness.WARMUP_S) / (harness.WINDOWS + 1)) + 1
        leftover = {"groups": [{"GroupId": "sg-0e0e0e0e0e0e0e0e0", "IpPermissions": [],
                                "Tags": [{"Key": k, "Value": v} for k, v in harness.TAGS.items()]}]}
        cases = [
            ("expired sign-in", SETTINGS, {"SCENARIO": "expired"}, None, 1, "sign-in"),
            ("unpublished tag", setting("--tag", "v9.9.9"), {}, None, 1, "is the tag published"),
            ("over the quota", setting("--server-type", "c7a.xlarge"), {}, None, 1, "vCPUs"),
            ("burstable type", setting("--client-type", "t3.small"), {}, None, 1, "burstable"),
            ("an earlier run's leftovers", SETTINGS, {}, leftover, 1, "earlier run"),
            ("windows past combat", setting("--window-s", str(overlong)), {}, None, 1, "do not fit"),
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
        plan = harness.Plan("ami-0123456789abcdef0", "/dev/sda1", "vpc", "subnet", "zone", "192.0.2.99/32", archives)
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
    def test_a_window_counts_only_inside_combat_after_the_warmup_with_every_client(self):
        def line(at, clients=4):
            return at, {"period_s": 10.0, "clients": clients, "send_clients": [{}] * clients}

        # Seven 10 s lines against a limit of three: the fourth line that counts (ending at 150) is
        # past the limit, neither chosen nor skipped.
        stats = [line(100), line(110, clients=3), line(120), line(130), line(140), line(150), line(170)]
        log = harness.ServerLog(combat_open=50.0, combat_end=165.0, stats=stats)
        with mock.patch.object(harness, "WINDOWS", 3):
            chosen, skipped = harness.pick_windows(log, clients=4, measure_from=95.0)
        self.assertEqual([(w.start, w.end) for w in chosen], [(110.0, 120.0), (120.0, 130.0), (130.0, 140.0)])
        self.assertEqual(skipped, {"before the warmup ended": 1, "not 4 clients connected": 1, "after combat": 1})

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
