#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""One measured Overmatch dedicated server on AWS EC2 in Singapore, loaded by a bot battle and real
headless clients from a second machine; one cost table and two timing tables; teardown with proof.

    scripts/aws/harness.py run --tag v0.7.0-alpha.19 --server-type c7a.large \\
        --client-type c7a.large --bots 26 --clients 4 --window-s 60
    scripts/aws/harness.py teardown

RUN, in order (progress on stderr, the tables on stdout):
  1. Preflight, creating nothing: the settings; the AWS sign-in; the release's two `.sha256` files by
     explicit tag (a bad tag stops here; pre-releases have no `latest`); at that tag, its timing
     tastes (`assets/net_timing.ron`; a release before the tick stamp stops here) and its battle mode
     (`assets/modes/battle.ron`), for a window no shorter than the view and uplink windows and
     windows that fit the first combat; Canonical's Ubuntu 26.04
     amd64 image from its public SSM parameter, checked to be the public AWS-listed image of that
     name (the release needs glibc 2.43, so Amazon Linux 2023 cannot run it); both machine types
     (x86_64, not burstable) and their vCPUs, beside every instance already running, against the
     region's quota; that no harness resource is left; one zone that offers both types.
  2. Two security groups: server (UDP PORT from the client group only, SSH from the operator's
     address) and client (SSH from the operator's address).
  3. Both machines, booting `machine.sh` as user-data (its header lists the boot steps), launched
     with shutdown behaviour `terminate` and powering themselves off at the run's cap. A machine
     that boots without a network is rebooted once (REACH_TIMEOUT_S).
  4. Cold start per machine: launch call to kernel start to each boot step to server listening.
  5. Clients, one at a time, each started once the one before holds its input slot: a client
     without one 20 s after launch exits (src/net/harness.rs, WATCHDOG_SLACK), and a cold client
     loading beside others is slower. The first client's connection opens combat.
  6. WINDOWS windows. The server prints one `server_stats` line per window (its
     OVERMATCH_SERVER_STATS_S is the window length), so each line's percentiles are the window's
     own. A window counts once it starts WARMUP_S after the last slot, ends inside the first combat,
     and has every client connected at its close. A 1 Hz sampler on each machine adds machine busy
     and steal, and each client's CPU and resident memory, read over the same window.
  7. The tables, then teardown (always, a failure included), then the proof.

THE TIMING TABLES read the two measured records of net timing slice 0, which print without a lever
of their own: each client's view clock (src/net/view_clock.rs), inside its once-a-second
`client_jitter` line, and each server link's input-gap census (src/net/input_gap.rs), inside the
window's `server_stats` line. Each table's legend says what every column counts and over what span.
Each counted window's `server_stats` line and every `client_jitter` line is parsed once into what the
tables read; a field that is absent or of another kind is refused with its path, and a window its
client lines cover too little of is refused (COVERAGE).

THE RECIPE. "The same battle" is the same recipe, never the same outcome: release tag, bot count,
client count, client script (`machine.sh` CLIENT_SCRIPT) and both machine types. Contact solve order
follows entity allocation, which client join timing moves, so knockouts and rounds differ between
runs: read the spread over the windows, never one window.

TEARDOWN is by tag. Every resource carries Project=overmatch, Purpose=harness and Run=<run id>. A run
terminates its own machines, deletes its own groups, then proves region-wide that nothing tagged
Purpose=harness is left: instances not terminated, volumes, network interfaces, security groups, key
pairs. `teardown` does the same for every harness resource; its user is an operator whose run was
interrupted, since a run refuses to start while any is left. A machine whose operator vanished still
powers off at its cap and is terminated.

THE RESULTS CHANNEL is SSH as `ubuntu` with a key generated locally per run, its public half in the
user-data: no EC2 key pair, IAM role or standing resource to create, tag and delete. SSH is open
only to the operator's current public address.

STATE: $TMPDIR/overmatch-aws-harness/<run id>/, directory 700, files 600: the private key, the ids,
the raw journals and samples, `results.json`. Never in the repo. Everything printed passes `scrub`.

AWS: only `aws --profile overmatch --region ap-southeast-1`, CLOUDFLARE_GLOBAL_KEY removed from its
environment. An expired sign-in stops the run before anything is created.

Exit status: 0 measured, nothing left; 1 a failure, its cause printed (after teardown once anything
exists); 2 a malformed setting; 3 harness resources may be left (run `teardown`).
"""

from __future__ import annotations

import argparse
import ipaddress
import json
import math
import os
import re
import secrets
import shlex
import signal
import statistics
import subprocess
import sys
import time
import traceback
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT / "scripts" / "perf"))
from online_cost import ANSI, MalformedRecord, fmt, jitter_object, stats_object  # noqa: E402

PROFILE = "overmatch"
REGION = "ap-southeast-1"
RELEASES = "https://github.com/vikng-dev/overmatch/releases/download"
SOURCES = "https://raw.githubusercontent.com/vikng-dev/overmatch"
# The timing tastes' one owner (src/net/timing.rs `Timing::shipped`), compiled into the release.
NET_TIMING = "assets/net_timing.ron"
# The battle mode's one owner (src/net/battle_mode.rs), compiled into the release: its combat length.
BATTLE_MODE = "assets/modes/battle.ron"
TRIPLE = "x86_64-unknown-linux-gnu"
# The server's UDP port: src/net/server.rs PORT, fixed, no lever.
PORT = 5888
AMI_PARAMETER = "/aws/service/canonical/ubuntu/server/26.04/stable/current/amd64/hvm/ebs-gp3/ami-id"
AMI_NAME = re.compile(r"ubuntu/images/hvm-ssd-gp3/ubuntu-[a-z]+-26\.04-amd64-server-[0-9.]+")
# The account's Running On-Demand Standard (A, C, D, H, I, M, R, T, Z) instance quota here, vCPUs.
QUOTA_VCPUS = 5
STANDARD_FAMILIES = frozenset("acdhimrtz")
TAGS = {"Project": "overmatch", "Purpose": "harness"}
# Resolution: the fewest windows a spread is read from.
WINDOWS = 8
# Resolution: after the last client's slot, so its tank's spawn and the joins sit outside every window.
WARMUP_S = 30
# Resolution: launch to listening or ready, ten times the ~30 s this boot takes on c7a (MEASURED by
# this harness).
BOOT_TIMEOUT_S = 300
# Resolution: launch (or reboot) to the first SSH answer, six times the ~19 s it takes (MEASURED). Past
# it the machine booted without a network: the image's cloud-init (26.2) ran its local stage before the
# network card's driver was up ("The instance must have at least one eligible NIC" on the console), so
# it fetched neither the network setup nor the user-data. That is the race canonical/cloud-init issue
# 6697 reports, fixed upstream after 26.2; its recovery is a reboot, so such a machine is rebooted once.
# MEASURED with cloud-init 26.2: c7a.large lost the race in 1 boot of 11, c7a.medium in 4 of 4 (its
# reboot included), so no 1-vCPU c7a can serve here.
REACH_TIMEOUT_S = 120
# Resolution: start to input slot, per client; a client exits at 20 s without one, so this only
# bounds a client that is still starting.
SLOT_TIMEOUT_S = 60
# Resolution: the cap's allowance after the last window, for collection and teardown.
COLLECT_S = 300
# The script's longest run (u32): a client plays until teardown ends its machine.
CLIENT_TICKS = 2**32 - 1
# Resolution: round trips for the clock offset; the shortest bounds the error.
ECHOES = 8
# Resolution: a sampled interval must span this share of its window to stand for it.
COVERAGE = 0.8

SIGN_IN = re.compile(
    r"token has expired|sso session .*expired|error loading sso token|refresh failed|"
    r"security token included in the request is expired|unable to locate credentials",
    re.IGNORECASE,
)
SCRUB = (
    (re.compile(r"arn:aws[\w-]*:[^\s\"',)]*"), "<arn>"),
    (re.compile(r"\b(?:i|sg|sgr|vol|eni|ami|subnet|vpc|r|key|snap|igw|rtb|acl|eipalloc)-[0-9a-f]{8,17}\b"), "<id>"),
    (re.compile(r"\b(?:ip|ec2)-\d{1,3}-\d{1,3}-\d{1,3}-\d{1,3}\b"), "<host>"),
    (re.compile(r"(?<![\d.])(?:\d{1,3}\.){3}\d{1,3}(?![\d.])"), "<ip>"),
    (re.compile(r"(?<![\w:])[0-9a-fA-F]{1,4}(?::[0-9a-fA-F]{0,4}){4,7}(?![\w:])"), "<ip>"),
    (re.compile(r"(?<!\d)\d{12}(?!\d)"), "<account>"),
)
JOURNAL_LINE = re.compile(r"^\[\s*(\d+\.\d+)\]\s")
BATTLE = re.compile(r"server: Battle (\d+) (\w+)")
SETTINGS_MARK = "# @settings\n"


class Refused(Exception):
    """One failure with one cause the operator reads: an AWS error, a bad tag, a checksum mismatch,
    an expired sign-in, or a machine or the release not doing its part."""


def scrub(text: str) -> str:
    for pattern, mask in SCRUB:
        text = pattern.sub(mask, text)
    return text


# The running run's log file, beside stderr; empty outside a run.
LOG: list[Path] = []


def say(message: str) -> None:
    """One progress line on stderr and in the run's log. A log write that fails (a full disk) is
    reported on stderr and ends the log, so logging never stops a teardown."""
    line = scrub(message)
    print(line, file=sys.stderr, flush=True)
    for path in list(LOG):
        try:
            with path.open("a") as log:
                log.write(line + "\n")
        except OSError as error:
            LOG.remove(path)
            print(f"run log not written from here on: {error.strerror or error}", file=sys.stderr, flush=True)


def write_private(path: Path, text: str) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as out:
        out.write(text)


def aws(*args: str) -> dict:
    env = {key: value for key, value in os.environ.items() if key != "CLOUDFLARE_GLOBAL_KEY"}
    env["AWS_PAGER"] = ""
    argv = ["aws", "--profile", PROFILE, "--region", REGION, "--output", "json", *args]
    try:
        run = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=180)
    except OSError as error:
        raise Refused(f"{' '.join(argv)}: {error}; install the AWS CLI and make `aws` executable on PATH") from error
    except subprocess.TimeoutExpired as error:
        raise Refused(f"aws {' '.join(args[:2])}: no answer in 180 s") from error
    if run.returncode != 0:
        lines = run.stderr.strip().splitlines()
        detail = lines[-1] if lines else f"exit {run.returncode}"
        if SIGN_IN.search(run.stderr):
            raise Refused(f"the AWS sign-in is missing or expired ({detail}); run "
                          f"`aws sso login --profile {PROFILE}` and run again")
        raise Refused(f"aws {' '.join(args[:2])}: {detail}")
    return json.loads(run.stdout) if run.stdout.strip() else {}


def fetch(url: str, what: str) -> str:
    run = subprocess.run(["curl", "-fsSL", "--max-time", "60", url], capture_output=True, text=True)
    if run.returncode != 0:
        raise Refused(f"cannot fetch {what} ({run.stderr.strip() or f'curl exit {run.returncode}'})")
    return run.stdout


def tag_list(run_id: str, role: str) -> list[dict[str, str]]:
    values = TAGS | {"Run": run_id, "Role": role, "Name": f"harness-{role}-{run_id}"}
    return [{"Key": key, "Value": value} for key, value in values.items()]


def tag_specs(run_id: str, role: str, *resource_types: str) -> str:
    return json.dumps([{"ResourceType": kind, "Tags": tag_list(run_id, role)} for kind in resource_types])


def harness_filters(run_id: str | None) -> list[str]:
    filters = [f"Name=tag:{key},Values={value}" for key, value in TAGS.items()]
    return filters + ([f"Name=tag:Run,Values={run_id}"] if run_id else [])


def live_instances(*filters: str) -> list[dict]:
    states = "Name=instance-state-name,Values=pending,running,stopping,stopped,shutting-down"
    reservations = aws("ec2", "describe-instances", "--filters", states, *filters)["Reservations"]
    return [instance for reservation in reservations for instance in reservation["Instances"]]


# ------------------------------------------------------------------------------------- settings


@dataclass(frozen=True)
class Settings:
    tag: str
    server_type: str
    client_type: str
    bots: int
    clients: int
    window_s: int


def release_tag(value: str) -> str:
    if not re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?", value):
        raise argparse.ArgumentTypeError(f"'{value}' is not a release tag like v0.7.0-alpha.18")
    return value


def machine_type(value: str) -> str:
    if not re.fullmatch(r"[a-z][a-z0-9-]*\.[a-z0-9]+", value):
        raise argparse.ArgumentTypeError(f"'{value}' is not an EC2 instance type")
    if value[0] not in STANDARD_FAMILIES:
        raise argparse.ArgumentTypeError(f"'{value}' is outside the standard families the quota covers")
    return value


def count(least: int):
    def parse(value: str) -> int:
        if not value.isdigit() or int(value) < least:
            raise argparse.ArgumentTypeError(f"wants an integer of at least {least}, got '{value}'")
        return int(value)
    return parse


def combat_s(text: str, where: str) -> float:
    """The first combat's length, `combat_s: <seconds>` in the battle mode's data."""
    found = re.search(r"\bcombat_s:\s*(\d+(?:\.\d+)?)", text)
    if not found:
        raise Refused(f"{where} has no combat_s")
    return float(found.group(1))


def cap_min(s: Settings) -> int:
    """Minutes from boot to the machine's own power-off: everything the run can take, in order."""
    span = BOOT_TIMEOUT_S + s.clients * SLOT_TIMEOUT_S + WARMUP_S + (WINDOWS + 2) * s.window_s + COLLECT_S
    return math.ceil(span / 60)


@dataclass(frozen=True)
class NetTiming:
    """The release's timing tastes, as its `Timing` reads them: seconds."""
    view_window_s: float
    uplink_window_s: float
    stale_floor_s: float
    stale_ceil_s: float


def net_timing(text: str, where: str) -> NetTiming:
    """Each field's number from `<field>: (<number>, <label>)`; the release itself refuses to start on
    a nonpositive or reversed one."""
    values = {}
    for name in NetTiming.__dataclass_fields__:
        found = re.search(rf"\b{name}:\s*\(\s*(\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)\s*,", text)
        if not found:
            raise Refused(f"{where} has no {name}")
        values[name] = float(found.group(1))
    return NetTiming(**values)


# ------------------------------------------------------------------------------------ preflight


@dataclass(frozen=True)
class Plan:
    image: str
    root_device: str
    vpc: str
    subnet: str
    zone: str
    operator_cidr: str
    archives: dict[str, tuple[str, str]]  # role: (url, checksum line)
    timing: NetTiming
    combat_s: float


def archive(tag: str, role: str) -> tuple[str, str]:
    """The role's archive URL and its `sha256sum` line, read from the release (release.yml names)."""
    name = f"overmatch-server-{tag}-{TRIPLE}.tar.gz" if role == "server" else f"overmatch-{tag}-{TRIPLE}.tar.gz"
    checksum = f"{name}.sha256" if role == "server" else f"overmatch-{tag}-{TRIPLE}.sha256"
    line = fetch(f"{RELEASES}/{tag}/{checksum}", f"{checksum} from release {tag}: is the tag published?").strip()
    found = re.fullmatch(r"([0-9a-f]{64}) [ *](\S+)", line)
    if not found or found.group(2) != name:
        raise Refused(f"release {tag}: {checksum} is not one sha256sum line for {name}")
    return f"{RELEASES}/{tag}/{name}", f"{found.group(1)}  {name}"


def tagged_source(tag: str, path: str, absent: str) -> tuple[str, str]:
    """A repository file at the release's own tag, and its name for messages; `absent` asks why the
    tag might lack it."""
    where = f"{path} at {tag}"
    return fetch(f"{SOURCES}/{tag}/{path}", f"{where}: {absent}"), where


def preflight(s: Settings) -> Plan:
    aws("sts", "get-caller-identity")
    archives = {role: archive(s.tag, role) for role in ("server", "client")}
    timing = net_timing(*tagged_source(s.tag, NET_TIMING, "does the release carry the tick stamp and "
                                       "input-gap records?"))
    combat = combat_s(*tagged_source(s.tag, BATTLE_MODE, "does the tag carry the battle mode?"))
    longest = max(timing.view_window_s, timing.uplink_window_s)
    if s.window_s < longest:
        raise Refused(f"a {s.window_s} s window is shorter than the release's {longest:g} s timing windows "
                      f"({NET_TIMING} at {s.tag}): its timing histograms would reach before the window")
    if WARMUP_S + (WINDOWS + 1) * s.window_s > combat:
        raise Refused(f"{WINDOWS} windows of {s.window_s} s do not fit one {combat:.0f} s combat "
                      f"({BATTLE_MODE} at {s.tag})")

    image_id = aws("ssm", "get-parameter", "--name", AMI_PARAMETER)["Parameter"]["Value"]
    images = aws("ec2", "describe-images", "--owners", "amazon", "--image-ids", image_id)["Images"]
    if len(images) != 1:
        raise Refused(f"{AMI_PARAMETER} names no public AWS-listed image")
    image = images[0]
    if not (AMI_NAME.fullmatch(image["Name"]) and image["Architecture"] == "x86_64"
            and image["State"] == "available" and image["Public"]):
        raise Refused(f"{AMI_PARAMETER} names {image['Name']} ({image['Architecture']}, {image['State']}), "
                      "not an available public Ubuntu 26.04 amd64 server image")

    types = aws("ec2", "describe-instance-types", "--instance-types",
                *sorted({s.server_type, s.client_type}))["InstanceTypes"]
    vcpus = {}
    for kind in types:
        name = kind["InstanceType"]
        if "x86_64" not in kind["ProcessorInfo"]["SupportedArchitectures"]:
            raise Refused(f"{name} is not x86_64; the release is {TRIPLE}")
        if kind["BurstablePerformanceSupported"]:
            raise Refused(f"{name} is burstable: it would measure its CPU credits, not the server")
        vcpus[name] = kind["VCpuInfo"]["DefaultVCpus"]
    live = [i for i in live_instances() if i["State"]["Name"] != "stopped" and i["InstanceType"][0] in STANDARD_FAMILIES]
    used = sum(i["CpuOptions"]["CoreCount"] * i["CpuOptions"]["ThreadsPerCore"] for i in live)
    wanted = vcpus[s.server_type] + vcpus[s.client_type]
    if used + wanted > QUOTA_VCPUS:
        raise Refused(f"the run needs {wanted} vCPUs beside {used} running, over the region's {QUOTA_VCPUS}")

    left = proof()
    if any(left[kind] for kind in HARNESS_KINDS):
        raise Refused(f"harness resources from an earlier run are left ({proof_line(left)}); "
                      "run `scripts/aws/harness.py teardown`")

    vpcs = aws("ec2", "describe-vpcs", "--filters", "Name=is-default,Values=true")["Vpcs"]
    if len(vpcs) != 1:
        raise Refused(f"{REGION} has {len(vpcs)} default VPCs, not one")
    vpc = vpcs[0]["VpcId"]
    subnets = aws("ec2", "describe-subnets", "--filters", f"Name=vpc-id,Values={vpc}",
                  "Name=default-for-az,Values=true")["Subnets"]
    offers = aws("ec2", "describe-instance-type-offerings", "--location-type", "availability-zone",
                 "--filters", f"Name=instance-type,Values={s.server_type},{s.client_type}")["InstanceTypeOfferings"]
    zones = {zone for zone in {o["Location"] for o in offers}
             if {o["InstanceType"] for o in offers if o["Location"] == zone} == {s.server_type, s.client_type}}
    usable = sorted((sn["AvailabilityZone"], sn["SubnetId"]) for sn in subnets if sn["AvailabilityZone"] in zones)
    if not usable:
        raise Refused(f"no zone in {REGION} offers both {s.server_type} and {s.client_type}")
    zone, subnet = usable[0]

    address = fetch("https://checkip.amazonaws.com", "the operator's public address").strip()
    try:
        operator = ipaddress.IPv4Address(address)
    except ValueError as error:
        raise Refused("the operator's public address is not one IPv4 address") from error
    return Plan(image["ImageId"], image["RootDeviceName"], vpc, subnet, zone, f"{operator}/32", archives, timing,
                combat)


# ------------------------------------------------------------------------------------- machines


# ssh's status for its own failure (no connection, no key yet); any other is the remote command's.
SSH_FAILED = 255


@dataclass
class Machine:
    role: str
    state_dir: Path
    instance_id: str
    launched_at: float  # operator clock, just before the launch call
    public_ip: str = ""
    private_ip: str = ""
    boot_at: float = 0.0  # operator clock at the machine's kernel start
    boot_error: float = 0.0  # half the shortest offset round trip
    phases: dict[str, float] = field(default_factory=dict)
    rebooted: bool = False  # after a boot without a network (REACH_TIMEOUT_S)

    def argv(self) -> list[str]:
        return ["ssh", "-i", str(self.state_dir / "key"), "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes",
                "-o", "StrictHostKeyChecking=accept-new", "-o", f"UserKnownHostsFile={self.state_dir / 'known_hosts'}",
                "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=15", "-o", "LogLevel=ERROR",
                f"ubuntu@{self.public_ip}", "sudo", "-n", "/usr/local/bin/om-harness"]

    def run(self, *command: str) -> subprocess.CompletedProcess:
        try:
            return subprocess.run([*self.argv(), *map(shlex.quote, command)], capture_output=True, text=True,
                                  timeout=120)
        except subprocess.TimeoutExpired as error:
            raise Refused(f"{self.role} machine: `{command[0]}` over SSH gave no answer in 120 s") from error

    def ssh(self, *command: str) -> str:
        done = self.run(*command)
        if done.returncode != 0:
            raise Refused(f"{self.role} machine: `{command[0]}` over SSH failed: {done.stderr.strip()[-300:]}")
        return done.stdout

    def tail(self, unit: str) -> str:
        return "\n".join(self.ssh("journal", unit).splitlines()[-25:])


@dataclass
class BootState:
    phases: dict[str, float]
    failed: str | None
    unit: str | None
    listening: float | None


def boot_state(text: str) -> BootState:
    phases, failed, unit, listening = {}, None, None, None
    for words in map(str.split, text.splitlines()):
        match words:
            case ["failed", step, _]:
                failed = step
            case ["unit", state]:
                unit = state
            case ["listening", at]:
                listening = float(at)
            case [step, at]:
                phases[step] = float(at)
            case _:
                raise Refused(f"unreadable boot state line {' '.join(words)!r}")
    return BootState(phases, failed, unit, listening)


FAILED_STEP = {
    "guard": "scheduling its own power-off failed",
    "self": "installing om-harness failed",
    "key": "installing the SSH key failed",
    "packages": "installing libasound2t64 failed",
    "download": "downloading the release archive failed",
    "verify": "checksum mismatch: the archive does not match the release's .sha256",
    "unpack": "unpacking the archive failed",
    "start": "starting the server unit failed",
}


def create_groups(run_id: str, plan: Plan) -> dict[str, str]:
    groups = {}
    for role in ("client", "server"):
        groups[role] = aws("ec2", "create-security-group", "--group-name", f"harness-{role}-{run_id}",
                           "--description", f"Overmatch measurement harness, {role}", "--vpc-id", plan.vpc,
                           "--tag-specifications", tag_specs(run_id, role, "security-group"))["GroupId"]
    ssh_rule = {"IpProtocol": "tcp", "FromPort": 22, "ToPort": 22, "IpRanges": [{"CidrIp": plan.operator_cidr}]}
    game_rule = {"IpProtocol": "udp", "FromPort": PORT, "ToPort": PORT, "UserIdGroupPairs": [{"GroupId": groups["client"]}]}
    aws("ec2", "authorize-security-group-ingress", "--group-id", groups["client"], "--ip-permissions", json.dumps([ssh_rule]))
    aws("ec2", "authorize-security-group-ingress", "--group-id", groups["server"],
        "--ip-permissions", json.dumps([ssh_rule, game_rule]))
    return groups


def user_data(role: str, s: Settings, plan: Plan, public_key: str) -> str:
    url, checksum = plan.archives[role]
    values = {"ROLE": role, "CAP_MIN": cap_min(s), "PUBKEY": public_key, "ARCHIVE_URL": url, "ARCHIVE_SHA256": checksum}
    if role == "server":
        values |= {"BOTS": s.bots, "STATS_S": s.window_s}
    script = (HERE / "machine.sh").read_text()
    assert script.count(SETTINGS_MARK) == 1, "machine.sh holds exactly one settings mark"
    block = "".join(f"{key}={shlex.quote(str(value))}\n" for key, value in values.items())
    return script.replace(SETTINGS_MARK, block, 1)


def launch(role: str, kind: str, s: Settings, plan: Plan, groups: dict[str, str], run_id: str,
           state: Path, public_key: str) -> Machine:
    data = state / f"{role}.user-data"
    write_private(data, user_data(role, s, plan, public_key))
    launched_at = time.time()
    out = aws("ec2", "run-instances", "--image-id", plan.image, "--instance-type", kind, "--count", "1",
              "--instance-initiated-shutdown-behavior", "terminate",
              "--metadata-options", "HttpTokens=required,HttpEndpoint=enabled,HttpPutResponseHopLimit=1",
              "--block-device-mappings", json.dumps([{"DeviceName": plan.root_device,
                                                      "Ebs": {"DeleteOnTermination": True, "VolumeType": "gp3"}}]),
              "--network-interfaces", json.dumps([{"DeviceIndex": 0, "SubnetId": plan.subnet, "Groups": [groups[role]],
                                                   "AssociatePublicIpAddress": True, "DeleteOnTermination": True}]),
              "--user-data", f"file://{data}",
              "--tag-specifications", tag_specs(run_id, role, "instance", "volume", "network-interface"))
    say(f"{role}: launched {kind} in {plan.zone}")
    return Machine(role, state, out["Instances"][0]["InstanceId"], launched_at)


def wait_running(machines: list[Machine]) -> None:
    deadline = time.time() + BOOT_TIMEOUT_S
    while True:
        found = aws("ec2", "describe-instances", "--instance-ids", *(m.instance_id for m in machines))
        instances = {i["InstanceId"]: i for r in found["Reservations"] for i in r["Instances"]}
        for machine in machines:
            instance = instances[machine.instance_id]
            if instance["State"]["Name"] in ("shutting-down", "terminated", "stopping", "stopped"):
                reason = instance.get("StateReason", {}).get("Message", "no reason given")
                raise Refused(f"{machine.role} machine is {instance['State']['Name']}: {reason}")
            machine.public_ip = instance.get("PublicIpAddress", "")
            machine.private_ip = instance.get("PrivateIpAddress", "")
        if all(instances[m.instance_id]["State"]["Name"] == "running" and m.public_ip for m in machines):
            return
        if time.time() > deadline:
            raise Refused(f"the machines were not running {BOOT_TIMEOUT_S} s after launch")
        time.sleep(2)


def wait_booted(machines: list[Machine]) -> None:
    """Until the server listens and the client machine is ready, or a boot step fails."""
    deadline = time.time() + BOOT_TIMEOUT_S
    waiting = list(machines)
    last: dict[str, str] = {}
    reach_by = {m.role: m.launched_at + REACH_TIMEOUT_S for m in machines}
    while waiting:
        for machine in list(waiting):
            done = machine.run("state")
            unreachable = done.returncode == SSH_FAILED
            if unreachable and machine.role not in last and time.time() > reach_by[machine.role]:
                if machine.rebooted:
                    raise Refused(f"{machine.role} machine never answered SSH, after its launch or after a reboot: "
                                  "two boots without a network (see REACH_TIMEOUT_S); run again")
                aws("ec2", "reboot-instances", "--instance-ids", machine.instance_id)
                machine.rebooted, reach_by[machine.role] = True, time.time() + REACH_TIMEOUT_S
                say(f"{machine.role}: no SSH {REACH_TIMEOUT_S} s after launch, a boot without a network; rebooted it")
                continue
            if unreachable:  # sshd or the key is not up yet: the boot is still under way
                continue
            if done.returncode != 0:
                raise Refused(f"{machine.role} machine: `state` failed: {done.stderr.strip()[-300:]}")
            state = boot_state(done.stdout)
            machine.phases = state.phases
            if state.failed:
                raise Refused(f"{machine.role} machine: boot step `{state.failed}`: {FAILED_STEP[state.failed]}")
            latest = max(state.phases, key=state.phases.get, default="ssh")
            if last.get(machine.role) != latest:
                say(f"{machine.role}: {latest}")
                last[machine.role] = latest
            if machine.role == "client" and "ready" in state.phases:
                waiting.remove(machine)
            elif machine.role == "server" and state.listening is not None:
                machine.phases["listening"] = state.listening
                waiting.remove(machine)
            elif machine.role == "server" and "started" in state.phases and state.unit in ("failed", "inactive"):
                raise Refused(f"the server unit is {state.unit}:\n{machine.tail('overmatch-server')}")
        if waiting and time.time() > deadline:
            raise Refused(f"{', '.join(m.role for m in waiting)} not ready {BOOT_TIMEOUT_S} s after launch "
                          f"(last step: {', '.join(last.get(m.role, 'no SSH') for m in waiting)})")
        if waiting:
            time.sleep(2)


def measure_boot_clock(machine: Machine) -> None:
    """The operator-clock time of the machine's kernel start, from the shortest of ECHOES round trips
    that each read the machine's boot clock; the first, which carries the SSH handshake, never wins."""
    echo = subprocess.Popen([*machine.argv(), "uptime-echo"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True)
    best = None
    for _ in range(ECHOES):
        sent = time.time()
        echo.stdin.write("\n")
        echo.stdin.flush()
        reading = echo.stdout.readline()
        got = time.time()
        if not reading.strip():
            raise Refused(f"{machine.role} machine: the clock echo ended early")
        if best is None or got - sent < best[0]:
            best = (got - sent, (sent + got) / 2 - float(reading))
    echo.stdin.close()
    echo.wait(timeout=60)
    machine.boot_error, machine.boot_at = best[0] / 2, best[1]


def start_clients(client: Machine, server: Machine, clients: int) -> None:
    for k in map(str, range(1, clients + 1)):
        client.ssh("start-client", k, f"{server.private_ip}:{PORT}", str(CLIENT_TICKS))
        deadline = time.time() + SLOT_TIMEOUT_S
        while True:
            state = dict(line.split(maxsplit=1) for line in client.ssh("client-state", k).splitlines())
            if "slot" in state:
                break
            if state.get("unit") not in ("active", "activating"):
                raise Refused(f"client {k} ended without an input slot:\n{client.tail(f'om-client-{k}')}")
            if time.time() > deadline:
                raise Refused(f"client {k} had no input slot {SLOT_TIMEOUT_S} s after its start")
            time.sleep(1)
        say(f"client {k}: input slot")


# -------------------------------------------------------------------------------------- reading


class Fields:
    """A record's JSON object, read once at its edge: a field that is absent or of another kind is
    refused with its dotted path, never met later as a KeyError."""

    def __init__(self, value: dict, where: str):
        self.value, self.where = value, where

    def get(self, path: str, *kinds: type):
        value = self.value
        for key in path.split("."):
            if not isinstance(value, dict) or key not in value:
                raise Refused(f"{self.where}: no `{path}`")
            value = value[key]
        if isinstance(value, bool) or not isinstance(value, kinds):
            raise Refused(f"{self.where}: `{path}` is {json.dumps(value)[:60]}, not "
                          f"{' or '.join(kind.__name__ for kind in kinds)}")
        return value

    def num(self, path: str) -> float:
        value = float(self.get(path, int, float))
        if not math.isfinite(value):  # Python's JSON reads NaN and Infinity; the release prints neither
            raise Refused(f"{self.where}: `{path}` is {value}, not a finite number")
        return value

    def maybe_num(self, path: str) -> float | None:
        return None if self.get(path, int, float, type(None)) is None else self.num(path)

    def count(self, path: str) -> int:
        return self.get(path, int)

    def at(self, path: str) -> Fields:
        return Fields(self.get(path, dict), f"{self.where} `{path}`")

    def each(self, path: str) -> list[Fields]:
        items = self.get(path, list)
        if not all(isinstance(item, dict) for item in items):
            raise Refused(f"{self.where}: `{path}` holds a value that is not an object")
        return [Fields(item, f"{self.where} `{path}[{i}]`") for i, item in enumerate(items)]


@dataclass(frozen=True)
class Histogram:
    """A timing record's samples (src/net/timing.rs `histogram`): nearest-rank p50, p95 and max,
    and counts in tick-wide bins, the last open-ended."""
    n: int
    p50_ms: float
    p95_ms: float
    max_ms: float
    bin_ms: float
    bins: tuple[int, ...]

    def first_from(self, bound_ms: float) -> int | None:
        """The first bin wholly at or above bound_ms; None when the open-ended last bin straddles it."""
        first = math.ceil(bound_ms / self.bin_ms)
        return first if first < len(self.bins) else None


def histogram(fields: Fields, path: str) -> Histogram | None:
    """None: the record held no sample (null, as src/net/timing.rs prints an empty one). Refused
    unless it is n > 0 samples in nonnegative counts over bins of positive width."""
    if fields.get(path, dict, type(None)) is None:
        return None
    h = fields.at(path)
    n, bin_ms, bins = h.count("n"), h.num("bin_ms"), h.get("bins", list)
    counts = all(isinstance(b, int) and not isinstance(b, bool) and b >= 0 for b in bins)
    if not (n > 0 and bin_ms > 0 and counts and sum(bins) == n):
        raise Refused(f"{h.where}: n {n}, bin_ms {bin_ms:g} and bins {bins} are not n > 0 samples counted "
                      "in bins of positive width")
    return Histogram(n, h.num("p50"), h.num("p95"), h.num("max"), bin_ms, tuple(bins))


@dataclass(frozen=True)
class Arrival:
    """One connected link's input-gap record at a stats line (src/net/input_gap.rs)."""
    state: str  # live, stale or awaiting_first_arrival
    gaps: Histogram | None  # over the uplink window before the line; None: no arrival in it
    threshold_ms: float  # the would-be stale threshold S at the line
    stale_arrivals: int  # this and declarations: since the previous line
    declarations: int


@dataclass(frozen=True)
class Inputs:
    link: str
    arrival: Arrival


def inputs(row: Fields) -> Inputs:
    arrival = Arrival(
        row.get("arrival.state", str), histogram(row, "arrival.gap_ms"), row.num("arrival.stale_threshold_ms"),
        row.count("arrival.stale_arrivals"), row.count("arrival.stale_declarations"))
    return Inputs(row.get("link", str), arrival)


@dataclass(frozen=True)
class Stats:
    """What the tables read of a counted window's `server_stats` line (src/net/server_stats.rs). A
    field such a line leaves null (no tick, no send, no platform reading) is refused."""
    tanks: int
    projectiles: int
    tick_ms: tuple[float, float, float, float]  # p50, p95, p99, max
    overruns: int
    discarded_ms: float
    cpu_pct: float
    rss_mb: float
    send_p95_us: float
    link_kb_per_s: tuple[float, ...]
    inputs: tuple[Inputs, ...]


def stats(row: Fields) -> Stats:
    """Refused unless `inputs` holds one connected entry for each connected `send_clients` link, as
    the server builds both from one census: a missing one would drop its link from the gaps table."""
    sent = row.each("send_clients")
    census = tuple(map(inputs, row.each("inputs")))
    links = sorted(link.get("link", str) for link in sent)
    connected = sorted(entry.link for entry in census)
    if len(set(links)) != len(links) or connected != links:
        raise Refused(f"{row.where}: `inputs` holds connected entries for {connected}, not one for each "
                      f"`send_clients` link {links}")
    return Stats(row.count("tanks"), row.count("projectiles"),
                 tuple(row.num(f"tick_us.{p}") / 1e3 for p in ("p50", "p95", "p99", "max")),
                 row.count("overruns"), row.num("wall.discarded_ms"), row.num("cpu_pct"), row.num("rss_mb"),
                 row.num("send_us.p95"), tuple(link.num("bytes_per_s") / 1e3 for link in sent), census)


@dataclass
class ServerLog:
    combat_open: float | None
    combat_end: float | None
    stats: list[tuple[float, Fields]]  # (kernel time of the line, its object)


def server_log(text: str) -> ServerLog:
    combat_open = combat_end = None
    lines = []
    for line in text.splitlines():
        stamp = JOURNAL_LINE.match(line)
        if not stamp:
            continue
        at = float(stamp.group(1))
        clean = ANSI.sub("", line)
        battle = BATTLE.search(clean)
        if battle and battle.group(2) == "Combat" and combat_open is None:
            combat_open = at
        elif battle and battle.group(2) == "Results" and combat_open is not None and combat_end is None:
            combat_end = at
        try:
            row = stats_object(clean)
        except MalformedRecord as error:
            raise Refused(f"the server journal: {error}") from error
        if row is not None:
            lines.append((at, Fields(row, f"the server journal: the server_stats record at {at:.1f} s")))
    return ServerLog(combat_open, combat_end, lines)


@dataclass
class Window:
    start: float  # server kernel time
    end: float
    row: Stats


def pick_windows(log: ServerLog, clients: int, measure_from: float) -> tuple[list[Window], dict[str, int]]:
    """The first WINDOWS stats lines that count, and why each earlier or later one did not."""
    chosen, skipped = [], {}
    for at, row in log.stats:
        start = at - row.num("period_s")
        reason = None
        if start < measure_from:
            reason = "before the warmup ended"
        elif log.combat_end is not None and at > log.combat_end:
            reason = "after combat"
        elif row.count("clients") != clients or len(row.get("send_clients", list)) != clients:
            reason = f"not {clients} clients connected"
        if reason:
            skipped[reason] = skipped.get(reason, 0) + 1
        elif len(chosen) < WINDOWS:
            chosen.append(Window(start, at, stats(row)))
    return chosen, skipped


# The `view_clock.completion` counts, each since the previous line (src/net/view_clock.rs).
COMPLETION = ("completed_ticks", "late_ticks", "evicted_incomplete", "completed_without_stamp", "unmapped_stamps",
              "receipt_overflow")


@dataclass(frozen=True)
class Jitter:
    """What the tables read of one `client_jitter` line (src/net/frontier.rs), its `view_clock`
    included (src/net/view_clock.rs)."""
    at: float  # the client machine's kernel time, at the end of the line's period
    period_s: float
    held_ms: float  # of the period's frames, those whose target exhausted the completed tick buffer
    delay_ms: float  # applied delta above the latest completed tick's absence bound
    intervals: Histogram | None  # stamp intervals in the view window before the line; None: under two stamps
    target_minus_applied_ms: float | None  # None: no completed stamp pair yet
    target_above_x_ms: float | None
    completion: tuple[int, ...]  # COMPLETION
    worst_lag_ms: float
    cuts: int


def jitter(text: str, client: int) -> list[Jitter]:
    """Client k's `client_jitter` lines; one that does not parse or lacks a field is refused, never
    skipped."""
    out = []
    for line in text.splitlines():
        stamp = JOURNAL_LINE.match(line)
        if not stamp:
            continue
        try:
            found = jitter_object(line)
        except MalformedRecord as error:
            raise Refused(f"client {client}: {error}") from error
        if found is None:
            continue
        row = Fields(found, f"client {client}: a client_jitter record")
        view = row.at("view_clock")
        out.append(Jitter(float(stamp.group(1)), row.num("period_s"), row.num("holds.ms"),
                          view.num("applied_above_latest_x_ms"), histogram(view, "stamp_interval_ms"),
                          view.maybe_num("target_minus_applied_ms"), view.maybe_num("target_above_latest_x_ms"),
                          tuple(view.count(f"completion.{name}") for name in COMPLETION),
                          view.num("completion.worst_lag_ms"), view.count("cuts")))
    return out


def client_lines(records: list[Jitter], start: float, end: float, client: int) -> list[Jitter]:
    """Client k's lines over one window of its machine's clock, covering COVERAGE of it; less
    coverage is refused, so a release without the line reads as no reading."""
    inside = [r for r in records if start < r.at <= end]
    covered = sum(r.period_s for r in inside)
    if covered < COVERAGE * (end - start):
        raise Refused(f"client {client}'s client_jitter lines cover {covered:.0f} s of a {end - start:.0f} s "
                      "window: no client reading (does this release print client_jitter?)")
    return inside


@dataclass
class Sample:
    at: float  # the machine's kernel time
    units: dict[str, tuple[int | None, int | None]]  # unit: (cpu usec, rss kB)
    cpu: list[int]  # /proc/stat cpu: user nice system idle iowait irq softirq steal …


def samples(text: str) -> list[Sample]:
    out = []
    # Complete lines only: the sampler may be mid-line when the file is read.
    for line in text.split("\n")[:-1]:
        words = line.split()
        at, rest = float(words[0]), words[1:]
        split = rest.index("cpu")
        units = {}
        for word in rest[:split]:
            unit, values = word.split("=")
            cpu, rss = values.split(",")
            units[unit] = (None if cpu == "-" else int(cpu), None if rss == "-" else int(rss))
        out.append(Sample(at, units, [int(v) for v in rest[split + 1:]]))
    return out


@dataclass
class Interval:
    first: Sample
    last: Sample
    inside: list[Sample]

    def busy_pct(self) -> float:
        """Share of all the machine's CPUs not idle (idle + iowait), and of them stolen."""
        delta = [b - a for a, b in zip(self.first.cpu[:8], self.last.cpu[:8])]
        return 100.0 * (1 - (delta[3] + delta[4]) / sum(delta))

    def steal_pct(self) -> float:
        delta = [b - a for a, b in zip(self.first.cpu[:8], self.last.cpu[:8])]
        return 100.0 * delta[7] / sum(delta)

    def cpu_pct(self, unit: str) -> float:
        """The unit's CPU over the interval, % of one core."""
        first, last = self.first.units[unit][0], self.last.units[unit][0]
        if first is None or last is None:
            raise Refused(f"{unit} has no CPU sample at an edge of a measured window")
        return (last - first) / (self.last.at - self.first.at) / 1e4

    def rss_mb(self, unit: str) -> float:
        values = [s.units[unit][1] for s in self.inside if s.units.get(unit, (None, None))[1] is not None]
        if not values:
            raise Refused(f"{unit} has no memory sample in a measured window")
        return statistics.median(values) / 1024


def interval(series: list[Sample], start: float, end: float) -> Interval:
    inside = [s for s in series if start <= s.at <= end]
    if len(inside) < 2 or inside[-1].at - inside[0].at < COVERAGE * (end - start):
        raise Refused(f"the samples cover too little of a window ({len(inside)} samples)")
    return Interval(inside[0], inside[-1], inside)


COLUMNS = ["win", "at_s", "tanks", "proj", "p50_ms", "p95_ms", "p99_ms", "max_ms", "overrun", "drop_ms",
           "srv_cpu%", "srv_mb", "srv_busy%", "steal%", "send_p95_us", "link_kB/s", "link_max_kB/s",
           "stale_decl", "stale_arr", "held_ms", "cli_busy%", "cli_cpu%", "cli_cpu_max%", "cli_mb"]


def window_cells(index: int, window: Window, combat_open: float, server: Interval, client: Interval,
                 lines: list[list[Jitter]]) -> list[float | None]:
    row = window.row
    units = [f"om-client-{k}" for k in range(1, len(lines) + 1)]
    cpu = [client.cpu_pct(u) for u in units]
    return [index, window.start - combat_open, row.tanks, row.projectiles, *row.tick_ms, row.overruns,
            row.discarded_ms, row.cpu_pct, row.rss_mb, server.busy_pct(), server.steal_pct(), row.send_p95_us,
            statistics.median(row.link_kb_per_s), max(row.link_kb_per_s),
            sum(link.arrival.declarations for link in row.inputs),
            sum(link.arrival.stale_arrivals for link in row.inputs),
            max(sum(line.held_ms for line in own) for own in lines), client.busy_pct(), statistics.median(cpu),
            max(cpu), statistics.median(client.rss_mb(u) for u in units)]


def median(values: list[float]) -> float | None:
    return statistics.median(values) if values else None  # None: no sample


CLIENT_COLUMNS = ["cli", "win", "lines", "int_n", "int_p50", "int_p95", "int_max", "int_2T+", "tgt_n",
                  "tgt-applied_p50", "tgt-applied_min", "tgt-applied_max", "tgt-x_p50", "delta-x_ms", "cuts", "done", "late",
                  "late_max_ms", "evicted", "no_stamp", "unmapped", "overflow"]


def tiles(lines: list[Jitter], start: float, view_window_s: float) -> list[Histogram]:
    """The stamp-interval histograms of lines at least a view window apart, the first a view window
    after the window's start. Each holds the intervals of the view window before its line, so
    together they count each interval inside the window at most once."""
    out, edge = [], start + view_window_s
    for line in lines:
        if line.at >= edge:
            edge = line.at + view_window_s
            if line.intervals:
                out.append(line.intervals)
    return out


def client_cells(client: int, index: int, lines: list[Jitter], start: float,
                 timing: NetTiming) -> list[float | None]:
    taken = tiles(lines, start, timing.view_window_s)
    target = [line.target_minus_applied_ms for line in lines if line.target_minus_applied_ms is not None]
    totals = [sum(column) for column in zip(*(line.completion for line in lines))]
    return [client, index, len(lines), sum(h.n for h in taken), median([h.p50_ms for h in taken]),
            median([h.p95_ms for h in taken]), max((h.max_ms for h in taken), default=None),
            sum(sum(h.bins[2:]) for h in taken),  # bins 2 and up: two ticks or more
            len(target), median(target), min(target, default=None), max(target, default=None),
            median([line.target_above_x_ms for line in lines if line.target_above_x_ms is not None]),
            statistics.median(line.delay_ms for line in lines), sum(line.cuts for line in lines), totals[0], totals[1],
            max(line.worst_lag_ms for line in lines), *totals[2:]]


SERVER_COLUMNS = ["link", "win", "state", "gap_n", "gap_p50", "gap_p95", "gap_max", "gaps>=floor", "gaps>=ceil",
                  "S_ms", "stale_arr", "stale_decl"]


def server_cells(link: str, index: int, arrival: Arrival, timing: NetTiming) -> list[float | str | None]:
    gaps = arrival.gaps
    if gaps is None:
        census = [0, None, None, None, 0, 0]
    else:
        firsts = [gaps.first_from(bound_s * 1e3) for bound_s in (timing.stale_floor_s, timing.stale_ceil_s)]
        census = [gaps.n, gaps.p50_ms, gaps.p95_ms, gaps.max_ms,
                  *(None if first is None else sum(gaps.bins[first:]) for first in firsts)]
    return [link, index, arrival.state, *census, arrival.threshold_ms, arrival.stale_arrivals, arrival.declarations]


def bin_edge(sample: Histogram | None, bound_s: float) -> str:
    """Where the bins counted against bound_s begin, for the legend."""
    if sample is None:
        return "none recorded"
    first = sample.first_from(bound_s * 1e3)
    if first is None:
        return f"unknown: the open-ended bin from {(len(sample.bins) - 1) * sample.bin_ms:g} ms straddles it"
    return f"from {first * sample.bin_ms:g} ms"


def cell(value: float | str | None) -> str:
    return "-" if value is None else value if isinstance(value, str) else fmt(value)


def table(header: list[str], rows: list[list], labels: int) -> tuple[str, dict]:
    """The rows, then their median, min and max (DERIVED) over every row's number in each column
    after the first `labels`: the printed table and its `results.json` entry."""
    spread = {}
    for name, pick in (("median", statistics.median), ("min", min), ("max", max)):
        numbers = [[v for v in column if isinstance(v, (int, float))] for column in list(zip(*rows))[labels:]]
        spread[name] = [pick(column) if column else None for column in numbers]
    printed = [[cell(v) for v in row] for row in rows]
    printed += [[name] + [""] * (labels - 1) + [cell(v) for v in values] for name, values in spread.items()]
    widths = [max(len(c) for c in column) for column in zip(header, *printed)]
    text = "\n".join("  ".join(c.rjust(w) for c, w in zip(line, widths)) for line in [header, *printed])
    return text, {"columns": header, "rows": rows, "spread": spread}


def cold_start(machine: Machine) -> str:
    """Seconds since the launch call at each boot step: kernel start, then the boot's own marks. A
    rebooted machine's marks are of its second boot, so they are no cold-start sample."""
    steps = [("kernel", 0.0)] + sorted(machine.phases.items(), key=lambda item: item[1])
    marks = ", ".join(f"{name} {machine.boot_at + at - machine.launched_at:.1f}" for name, at in steps)
    kind = "boot after a reboot, not a cold-start sample" if machine.rebooted else "cold start"
    return f"{machine.role} {kind}, s since the launch call (±{machine.boot_error:.2f}): {marks}"


def report(s: Settings, server: Machine, client: Machine, host: dict[str, str], log: ServerLog,
           windows: list[Window], server_samples: list[Sample], client_samples: list[Sample],
           client_journals: list[str], timing: NetTiming) -> tuple[str, dict]:
    """The printed report, scrubbed, and its numbers for `results.json`."""
    # Server kernel time to client kernel time, through the operator's clock.
    to_client = server.boot_at - client.boot_at
    records = [jitter(journal, k) for k, journal in enumerate(client_journals, 1)]
    spans = [(i, w, w.start + to_client, w.end + to_client) for i, w in enumerate(windows, 1)]
    inside = [[client_lines(own, start, end, k) for k, own in enumerate(records, 1)] for _, _, start, end in spans]
    main, main_json = table(COLUMNS, [window_cells(i, w, log.combat_open, interval(server_samples, w.start, w.end),
                                                   interval(client_samples, start, end), inside[i - 1])
                                      for i, w, start, end in spans], 2)
    views, views_json = table(CLIENT_COLUMNS, [client_cells(k, i, inside[i - 1][k - 1], start, timing)
                                               for k in range(1, s.clients + 1) for i, _, start, _ in spans], 2)
    # Each connected link at each line, links in first-seen order.
    arrivals = {(link.link, i): link.arrival for i, w in enumerate(windows, 1) for link in w.row.inputs}
    links = list(dict.fromkeys(link for link, _ in arrivals))
    gaps, gaps_json = table(SERVER_COLUMNS, [server_cells(link, i, arrivals[link, i], timing) for link in links
                                             for i in range(1, len(windows) + 1) if (link, i) in arrivals], 3)
    # Every gap histogram has the same tick-wide bins.
    sample = next((a.gaps for a in arrivals.values() if a.gaps), None)
    edges = ", ".join(bin_edge(sample, bound_s) for bound_s in (timing.stale_floor_s, timing.stale_ceil_s))
    lines = [
        f"recipe: release {s.tag}; server {s.server_type} ({host['cpus']} CPUs, {host['model']}); "
        f"client machine {s.client_type}; {s.bots} bots + {s.clients} clients; "
        f"{len(windows)} windows of {s.window_s} s from {WARMUP_S} s after the last slot",
        f"net timing ({NET_TIMING} at {s.tag}): view window {timing.view_window_s:g} s, uplink window "
        f"{timing.uplink_window_s:g} s, stale floor {timing.stale_floor_s * 1e3:g} ms, ceiling "
        f"{timing.stale_ceil_s * 1e3:g} ms",
        cold_start(server),
        cold_start(client),
        "",
        main,
        "",
        "Window cells are MEASURED: one server_stats line each (tick percentiles over the window's own ticks, "
        "*_ms; srv_cpu% of one core; link_* the per-link payload kB/s the server queued, IP/UDP headers "
        "excluded; stale_decl and stale_arr sum each connected link's stale declarations and stale arrivals "
        "since the preceding stats line), the clients' client_jitter lines "
        "(held_ms: the largest per-client sum of frames held at the newest completed tick), and the 1 Hz samplers "
        "(*_busy% and steal% of the whole machine; cli_cpu% of one core per client; *_mb resident). "
        "median/min/max rows are DERIVED across the windows. at_s: window start, s after combat opened.",
        "",
        "View clock, per client and window (src/net/view_clock.rs, in the client_jitter lines):",
        views,
        "",
        "lines: the window's client_jitter lines. int_*: intervals between the server frame-start stamps of "
        f"consecutively completed stamped ticks, MEASURED, from lines at least the {timing.view_window_s:g} s view "
        "window apart, whose histograms count each interval inside the window at most once: int_n their count, "
        "int_max the largest, int_2T+ those of two ticks or more (a tick that never completed, or a long server "
        "frame); int_p50 and int_p95 the median of those histograms' nearest-rank p50 and p95 (DERIVED). "
        "tgt-applied_*: the delay target minus the paced delta, with the clock "
        "offset cancelled (negative: the paced delta is above target), DERIVED by the client at each of "
        "the tgt_n lines that had one; median, min and max over them. tgt-x_p50: the target above the newest "
        "sampled tick's lateness (DERIVED), median over the lines. delta-x_ms: applied delta above the newest completed tick's lateness, "
        "MEASURED, median over the lines. cuts: typed continuity jumps summed over the lines. "
        "done, late, evicted, no_stamp, unmapped, overflow: whole-tick "
        "completion counts summed over the lines, MEASURED: ticks completed; completed in a later frame than "
        "their stamp arrived (late_max_ms the longest such wait); stamps whose tick never completed within the "
        "receipt horizon; completions without a retained stamp; stamps without a checkpoint mapping; stamps a "
        "full receipt queue dropped. median/min/max rows: DERIVED over every client and window.",
        "",
        "Input gaps, per server link and window (src/net/input_gap.rs, the server_stats line's inputs[].arrival):",
        gaps,
        "",
        "gap_*: input arrival gaps (the silence before each arrival, at most one tick charged per empty server "
        f"receive), MEASURED over the {timing.uplink_window_s:g} s uplink window before the window's line only: "
        "gap_n their count, p50, p95, max nearest rank; gaps>=floor and gaps>=ceil those in the tick-wide bins "
        f"wholly at or above the floor and the ceiling ({edges}). "
        "S_ms: the would-be stale threshold, clamp(largest gap + one tick, floor, ceiling), DERIVED by the server "
        "at the line. stale_arr: arrivals after a silence above S; stale_decl: silences that crossed S; both "
        "MEASURED over the whole window. state: the link's at the line. median/min/max rows: DERIVED over every "
        "link and window.",
    ]
    results = {"settings": s.__dict__, "host": host, "net_timing": timing.__dict__, "windows": main_json,
               "view_clock": views_json, "input_gaps": gaps_json,
               "cold_start": {m.role: {"launch_to_kernel_s": m.boot_at - m.launched_at, "error_s": m.boot_error,
                                       "phases_s": m.phases, "rebooted": m.rebooted} for m in (server, client)}}
    return scrub("\n".join(lines)), results


# ------------------------------------------------------------------------------------------ run


def measure(s: Settings, plan: Plan, run_id: str, state: Path) -> None:
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "overmatch-harness",
                    "-f", str(state / "key")], check=True, capture_output=True)
    (state / "key.pub").chmod(0o600)
    public_key = (state / "key.pub").read_text().strip()
    write_private(state / "known_hosts", "")  # ssh appends the machines' addresses here
    groups = create_groups(run_id, plan)
    server = launch("server", s.server_type, s, plan, groups, run_id, state, public_key)
    client = launch("client", s.client_type, s, plan, groups, run_id, state, public_key)
    machines = [server, client]
    write_private(state / "ids.json", json.dumps({"groups": groups, "instances": {m.role: m.instance_id for m in machines}}))
    wait_running(machines)
    wait_booted(machines)
    for machine in machines:
        measure_boot_clock(machine)
        say(cold_start(machine))
    host = dict(line.split(maxsplit=1) for line in server.ssh("host").splitlines())

    start_clients(client, server, s.clients)
    # The last slot on the server's clock, late by at most one poll and a round trip.
    measure_from = time.time() - server.boot_at
    server.ssh("sample", "overmatch-server")
    client.ssh("sample", *(f"om-client-{k}" for k in range(1, s.clients + 1)))

    deadline = None
    while True:
        log = server_log(server.ssh("journal", "overmatch-server"))
        if log.combat_open is None:
            raise Refused("combat did not open although every client holds its slot")
        deadline = deadline or server.boot_at + log.combat_open + plan.combat_s + s.window_s
        windows, skipped = pick_windows(log, s.clients, max(measure_from, log.combat_open) + WARMUP_S)
        if len(windows) == WINDOWS:
            break
        if time.time() > deadline:
            raise Refused(f"only {len(windows)} of {WINDOWS} windows counted by the end of combat; skipped: {skipped}")
        say(f"windows: {len(windows)} of {WINDOWS}")
        time.sleep(min(30, s.window_s / 2))

    write_private(state / "server.journal", server.ssh("journal", "overmatch-server"))
    client_journals = [client.ssh("journal", f"om-client-{k}") for k in range(1, s.clients + 1)]
    for k, journal in enumerate(client_journals, 1):
        write_private(state / f"client-{k}.journal", journal)
    server_samples, client_samples = server.ssh("samples"), client.ssh("samples")
    write_private(state / "server.samples", server_samples)
    write_private(state / "client.samples", client_samples)
    text, results = report(s, server, client, host, log, windows, samples(server_samples), samples(client_samples),
                           client_journals, plan.timing)
    print(text, flush=True)
    write_private(state / "report.txt", text + "\n")
    write_private(state / "results.json", json.dumps(results, indent=1))


# ------------------------------------------------------------------------------------- teardown

HARNESS_KINDS = ("instances", "volumes", "network interfaces", "security groups", "key pairs")


def proof() -> dict[str, int]:
    """Region-wide counts of what is tagged for this harness, and for reference of everything live."""
    tagged = harness_filters(None)
    return {
        "instances": len(live_instances(*tagged)),
        "volumes": len(aws("ec2", "describe-volumes", "--filters", *tagged)["Volumes"]),
        "network interfaces": len(aws("ec2", "describe-network-interfaces", "--filters", *tagged)["NetworkInterfaces"]),
        "security groups": len(aws("ec2", "describe-security-groups", "--filters", *tagged)["SecurityGroups"]),
        "key pairs": len(aws("ec2", "describe-key-pairs", "--filters", *tagged)["KeyPairs"]),
        "live instances, any tag": len(live_instances()),
        "volumes, any tag": len(aws("ec2", "describe-volumes")["Volumes"]),
    }


def proof_line(counts: dict[str, int]) -> str:
    return ", ".join(f"{kind} {n}" for kind, n in counts.items())


SIGNALS = (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)


def teardown(run_id: str | None) -> bool:
    """Terminate, then delete, the run's harness resources (every run's without an id), then prove
    region-wide that no harness resource is left. True when none is. Signals wait until it returns."""
    previous = {signum: signal.signal(signum, signal.SIG_IGN) for signum in SIGNALS}
    try:
        return remove_and_prove(run_id)
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)


def remove_and_prove(run_id: str | None) -> bool:
    filters = harness_filters(run_id)
    try:
        ids = [i["InstanceId"] for i in live_instances(*filters)]
        if ids:
            aws("ec2", "terminate-instances", "--instance-ids", *ids)
            deadline = time.time() + 600
            while live_instances(f"Name=instance-id,Values={','.join(ids)}"):
                if time.time() > deadline:
                    raise Refused("the machines were not terminated 600 s after the request")
                time.sleep(3)
        say(f"teardown: {len(ids)} machines terminated")
        groups = aws("ec2", "describe-security-groups", "--filters", *filters)["SecurityGroups"]
        # The server group's game rule names the client group, so the server group goes first.
        groups.sort(key=lambda g: {t["Key"]: t["Value"] for t in g.get("Tags", [])}.get("Role") != "server")
        for group in groups:
            deadline = time.time() + 180
            while True:
                try:
                    aws("ec2", "delete-security-group", "--group-id", group["GroupId"])
                    break
                except Refused as error:  # a terminated machine's interface can hold it for a while
                    if "DependencyViolation" not in str(error) or time.time() > deadline:
                        raise
                    time.sleep(5)
        say(f"teardown: {len(groups)} security groups deleted")
    except Refused as error:
        say(f"teardown: {error}")
    deadline = time.time() + 180
    while True:
        try:
            counts = proof()
        except Refused as error:
            say(f"teardown: no proof ({error}); HARNESS RESOURCES MAY BE LEFT: run scripts/aws/harness.py teardown")
            return False
        clean = not any(counts[kind] for kind in HARNESS_KINDS)
        if clean or time.time() > deadline:  # a terminated machine's volume can take a while to go
            break
        time.sleep(5)
    say(f"proof, region-wide, tagged Purpose=harness: {proof_line(counts)}")
    if not clean:
        say("HARNESS RESOURCES ARE LEFT: run scripts/aws/harness.py teardown")
    return clean


def interrupted(signum, _frame):
    raise Refused(f"interrupted by signal {signum}")


def cmd_run(s: Settings) -> int:
    plan = preflight(s)
    run_id = time.strftime("%Y%m%dt%H%M%Sz", time.gmtime()) + "-" + secrets.token_hex(2)
    base = Path(os.environ.get("TMPDIR") or "/tmp") / "overmatch-aws-harness"
    base.mkdir(mode=0o700, parents=True, exist_ok=True)
    state = base / run_id
    state.mkdir(mode=0o700)
    write_private(state / "run.log", "")
    LOG[:] = [state / "run.log"]
    say(f"run {run_id}: {s.server_type} server, {s.client_type} client machine, {s.bots} bots, "
        f"{s.clients} clients, {WINDOWS} windows of {s.window_s} s, cap {cap_min(s)} min; state in {state}")
    previous = {signum: signal.signal(signum, interrupted) for signum in SIGNALS}
    status = 1
    try:
        measure(s, plan, run_id, state)
        status = 0
    except Refused as error:
        say(f"FAILED: {error}")
    except Exception:
        say(f"FAILED, a harness bug:\n{traceback.format_exc()}")
    finally:
        clean = teardown(run_id)
        for signum, handler in previous.items():
            signal.signal(signum, handler)
        LOG.clear()
    return status if clean else 3


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    run = sub.add_parser("run", help="one measured run")
    run.add_argument("--tag", required=True, type=release_tag, help="the release, e.g. v0.7.0-alpha.18")
    run.add_argument("--server-type", required=True, type=machine_type)
    run.add_argument("--client-type", required=True, type=machine_type)
    run.add_argument("--bots", required=True, type=count(0))
    run.add_argument("--clients", required=True, type=count(1), help="combat opens only on a connected client")
    run.add_argument("--window-s", required=True, type=count(10), help="one stats window, seconds")
    sub.add_parser("teardown", help="remove every harness resource, then prove none is left")
    args = parser.parse_args(argv)
    try:
        if args.cmd == "teardown":
            return 0 if teardown(None) else 3
        return cmd_run(Settings(args.tag, args.server_type, args.client_type, args.bots, args.clients, args.window_s))
    except Refused as error:
        say(f"refused: {error}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
