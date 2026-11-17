#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""One measured Overmatch dedicated server on AWS EC2 in Singapore, loaded by a bot battle and real
headless clients from a second machine; one table; teardown with proof.

    scripts/aws/harness.py run --tag v0.7.0-alpha.18 --server-type c7a.large \\
        --client-type c7a.large --bots 26 --clients 4 --window-s 60
    scripts/aws/harness.py teardown

RUN, in order (progress on stderr, the table on stdout):
  1. Preflight, creating nothing: the settings; the AWS sign-in; the release's two `.sha256` files by
     explicit tag (a bad tag stops here; pre-releases have no `latest`); Canonical's Ubuntu 26.04
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
  7. The table, then teardown (always, a failure included), then the proof.

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
from online_cost import ANSI, MalformedStats, fmt, stats_object  # noqa: E402

PROFILE = "overmatch"
REGION = "ap-southeast-1"
RELEASES = "https://github.com/vikng-dev/overmatch/releases/download"
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
# The client's once-a-second stream record, `client_jitter {json}` (src/net/frontier.rs).
JITTER_RECORD = re.compile(r"\bclient_jitter (?=\{)")
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


def combat_s() -> float:
    """The first combat's length, from the mode data the release ships."""
    path = ROOT / "assets" / "modes" / "battle.ron"
    found = re.search(r"\bcombat_s:\s*([0-9.]+)", path.read_text())
    if not found:
        raise Refused(f"{path}: no combat_s")
    return float(found.group(1))


def cap_min(s: Settings) -> int:
    """Minutes from boot to the machine's own power-off: everything the run can take, in order."""
    span = BOOT_TIMEOUT_S + s.clients * SLOT_TIMEOUT_S + WARMUP_S + (WINDOWS + 2) * s.window_s + COLLECT_S
    return math.ceil(span / 60)


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


def archive(tag: str, role: str) -> tuple[str, str]:
    """The role's archive URL and its `sha256sum` line, read from the release (release.yml names)."""
    name = f"overmatch-server-{tag}-{TRIPLE}.tar.gz" if role == "server" else f"overmatch-{tag}-{TRIPLE}.tar.gz"
    checksum = f"{name}.sha256" if role == "server" else f"overmatch-{tag}-{TRIPLE}.sha256"
    line = fetch(f"{RELEASES}/{tag}/{checksum}", f"{checksum} from release {tag}: is the tag published?").strip()
    found = re.fullmatch(r"([0-9a-f]{64}) [ *](\S+)", line)
    if not found or found.group(2) != name:
        raise Refused(f"release {tag}: {checksum} is not one sha256sum line for {name}")
    return f"{RELEASES}/{tag}/{name}", f"{found.group(1)}  {name}"


def preflight(s: Settings) -> Plan:
    aws("sts", "get-caller-identity")
    if WARMUP_S + (WINDOWS + 1) * s.window_s > combat_s():
        raise Refused(f"{WINDOWS} windows of {s.window_s} s do not fit one {combat_s():.0f} s combat")
    archives = {role: archive(s.tag, role) for role in ("server", "client")}

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
    return Plan(image["ImageId"], image["RootDeviceName"], vpc, subnet, zone, f"{operator}/32", archives)


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


@dataclass
class ServerLog:
    combat_open: float | None
    combat_end: float | None
    stats: list[tuple[float, dict]]  # (kernel time of the line, its object)


def server_log(text: str) -> ServerLog:
    combat_open = combat_end = None
    stats = []
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
        except MalformedStats as error:
            raise Refused(f"the server journal: {error}") from error
        if row is not None:
            stats.append((at, row))
    return ServerLog(combat_open, combat_end, stats)


@dataclass
class Window:
    start: float  # server kernel time
    end: float
    row: dict


def pick_windows(log: ServerLog, clients: int, measure_from: float) -> tuple[list[Window], dict[str, int]]:
    """The first WINDOWS stats lines that count, and why each earlier or later one did not."""
    chosen, skipped = [], {}
    for at, row in log.stats:
        start = at - row["period_s"]
        reason = None
        if start < measure_from:
            reason = "before the warmup ended"
        elif log.combat_end is not None and at > log.combat_end:
            reason = "after combat"
        elif row["clients"] != clients or len(row["send_clients"]) != clients:
            reason = f"not {clients} clients connected"
        if reason:
            skipped[reason] = skipped.get(reason, 0) + 1
        elif len(chosen) < WINDOWS:
            chosen.append(Window(start, at, row))
    return chosen, skipped


@dataclass
class Jitter:
    at: float  # the client machine's kernel time, at the end of the record's period
    period_s: float
    starved_ms: float  # of the period's frames, those whose presented cursor had passed the newest tick


def jitter(text: str, client: int) -> list[Jitter]:
    """Client k's `client_jitter` records; one that does not parse is refused, never skipped."""
    out = []
    for line in text.splitlines():
        stamp, record = JOURNAL_LINE.match(line), JITTER_RECORD.search(line)
        if stamp and record:
            try:
                row, _ = json.JSONDecoder().raw_decode(line, record.end())
                out.append(Jitter(float(stamp.group(1)), float(row["period_s"]), float(row["starved"]["ms"])))
            except (ValueError, KeyError, TypeError) as error:
                raise Refused(f"client {client}: a client_jitter record does not parse "
                              f"({type(error).__name__}: {error}): {line.strip()[:160]}") from error
    return out


def starved_ms(records: list[Jitter], start: float, end: float, client: int) -> float:
    """Client k's starved time over one window of its machine's clock, from records that cover
    COVERAGE of it; less coverage is refused, so a release without the record reads as no reading."""
    inside = [r for r in records if start < r.at <= end]
    covered = sum(r.period_s for r in inside)
    if covered < COVERAGE * (end - start):
        raise Refused(f"client {client}'s client_jitter records cover {covered:.0f} s of a {end - start:.0f} s "
                      "window: no starvation reading (does this release print client_jitter?)")
    return sum(r.starved_ms for r in inside)


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
           "held%", "starved_ms", "cli_busy%", "cli_cpu%", "cli_cpu_max%", "cli_mb"]


def window_cells(index: int, window: Window, combat_open: float, server: Interval, client: Interval,
                 to_client: float, starving: list[list[Jitter]]) -> list[float]:
    row = window.row
    tick = row["tick_us"]
    links = [link["bytes_per_s"] / 1e3 for link in row["send_clients"]]
    counts = [entry["counts"] for entry in row["inputs"]]
    inputs = sum(c["held"] + c["tick_exact"] for c in counts)
    units = [f"om-client-{k}" for k in range(1, len(starving) + 1)]
    cpu = [client.cpu_pct(u) for u in units]
    start, end = window.start + to_client, window.end + to_client
    worst_starved = max(starved_ms(records, start, end, k) for k, records in enumerate(starving, 1))
    return [index, window.start - combat_open, row["tanks"], row["projectiles"], tick["p50"] / 1e3,
            tick["p95"] / 1e3, tick["p99"] / 1e3, tick["max"] / 1e3, row["overruns"], row["wall"]["discarded_ms"],
            row["cpu_pct"], row["rss_mb"], server.busy_pct(), server.steal_pct(), row["send_us"]["p95"],
            statistics.median(links), max(links), 100.0 * sum(c["held"] for c in counts) / inputs if inputs else 0.0,
            worst_starved, client.busy_pct(), statistics.median(cpu), max(cpu),
            statistics.median(client.rss_mb(u) for u in units)]


def table(header: list[str], rows: list[list[str]]) -> str:
    widths = [max(len(cell) for cell in column) for column in zip(header, *rows)]
    return "\n".join("  ".join(c.rjust(w) for c, w in zip(line, widths)) for line in [header, *rows])


def cold_start(machine: Machine) -> str:
    """Seconds since the launch call at each boot step: kernel start, then the boot's own marks. A
    rebooted machine's marks are of its second boot, so they are no cold-start sample."""
    steps = [("kernel", 0.0)] + sorted(machine.phases.items(), key=lambda item: item[1])
    marks = ", ".join(f"{name} {machine.boot_at + at - machine.launched_at:.1f}" for name, at in steps)
    kind = "boot after a reboot, not a cold-start sample" if machine.rebooted else "cold start"
    return f"{machine.role} {kind}, s since the launch call (±{machine.boot_error:.2f}): {marks}"


def report(s: Settings, server: Machine, client: Machine, host: dict[str, str], log: ServerLog,
           windows: list[Window], server_samples: list[Sample], client_samples: list[Sample],
           client_journals: list[str]) -> tuple[str, dict]:
    """The printed report, scrubbed, and its numbers for `results.json`."""
    # Server kernel time to client kernel time, through the operator's clock.
    to_client = server.boot_at - client.boot_at
    starving = [jitter(journal, k) for k, journal in enumerate(client_journals, 1)]
    rows = [window_cells(i, w, log.combat_open, interval(server_samples, w.start, w.end),
                         interval(client_samples, w.start + to_client, w.end + to_client), to_client, starving)
            for i, w in enumerate(windows, 1)]
    spread = {name: [statistics.median(col), min(col), max(col)] for name, col in zip(COLUMNS[2:], zip(*(r[2:] for r in rows)))}
    lines = [
        f"recipe: release {s.tag}; server {s.server_type} ({host['cpus']} CPUs, {host['model']}); "
        f"client machine {s.client_type}; {s.bots} bots + {s.clients} clients; "
        f"{len(windows)} windows of {s.window_s} s from {WARMUP_S} s after the last slot",
        cold_start(server),
        cold_start(client),
        "",
        table(COLUMNS, [[fmt(v) for v in r] for r in rows]
              + [[label, ""] + [fmt(spread[name][i]) for name in COLUMNS[2:]]
                 for i, label in enumerate(("median", "min", "max"))]),
        "",
        "Window cells are MEASURED: one server_stats line each (tick percentiles over the window's own ticks, "
        "*_ms; srv_cpu% of one core; link_* the per-link payload kB/s the server queued, IP/UDP headers "
        "excluded; held% of input reads with no input for their tick), the clients' client_jitter lines "
        "(starved_ms: the most any client's presented cursor ran past its newest tick), and the 1 Hz samplers "
        "(*_busy% and steal% of the whole machine; cli_cpu% of one core per client; *_mb resident). "
        "median/min/max rows are DERIVED across the windows. at_s: window start, s after combat opened.",
    ]
    results = {"settings": s.__dict__, "host": host, "columns": COLUMNS, "rows": rows, "spread": spread,
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
        deadline = deadline or server.boot_at + log.combat_open + combat_s() + s.window_s
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
                           client_journals)
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
