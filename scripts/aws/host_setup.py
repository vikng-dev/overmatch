"""Standing EC2 hosting resources. See host-setup.md for the operator and provider contracts."""
from __future__ import annotations

import argparse
from dataclasses import dataclass
from enum import Enum
import ipaddress
import json
import os
from pathlib import Path
import re
import resource
import shutil
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
REGION = "ap-southeast-1"
USER = "overmatch-host"
POLICY = "overmatch-host"
TAGS = {"Project": "overmatch", "Purpose": "host"}
PARAMETER = "/aws/service/canonical/ubuntu/server/26.04/stable/current/amd64/hvm/ebs-gp3/ami-id"
SECRETS = ("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_HOST_CONFIG")
CLOUDFLARE_AUTH = ("CLOUDFLARE_API_TOKEN", "CLOUDFLARE_API_KEY", "CLOUDFLARE_EMAIL")


class Refused(Exception):
    """An outside input the operator must repair before continuing."""


class AwsError(Refused):
    def __init__(self, operation: str, code: str):
        self.code = code
        super().__init__(f"AWS {operation} refused ({code}); check the overmatch sign-in and permissions")


def environment() -> dict[str, str]:
    return {k: v for k, v in os.environ.items()
            if not k.startswith(("AWS_", "WRANGLER_", "CLOUDFLARE_", "CF_"))}


def aws_environment() -> dict[str, str]:
    # Credential sources belong to the named profile, never the parent shell.
    # If profile cli_history is enabled, AWS fails before sending any API request: its history
    # directory cannot be created below the null device. Never let it persist a create-key response.
    return environment() | {"AWS_PAGER": "", "AWS_CLI_AUTO_PROMPT": "off",
                            "AWS_CLI_HISTORY_FILE": "/dev/null/host-setup-history-disabled"}


def wrangler_environment() -> dict[str, str]:
    # Wrangler's native names only; backend/wrangler.jsonc owns account selection.
    auth = {name: os.environ[name] for name in CLOUDFLARE_AUTH if name in os.environ}
    for name, value in auth.items():
        if not value.strip():
            raise Refused(f"{name} is empty; export a non-empty value or unset all Cloudflare variables for saved Wrangler login")
    if "CLOUDFLARE_API_TOKEN" in auth:
        if "CLOUDFLARE_API_KEY" in auth or "CLOUDFLARE_EMAIL" in auth:
            raise Refused("ambiguous Wrangler authentication; export CLOUDFLARE_API_TOKEN alone or unset it "
                          "and export CLOUDFLARE_API_KEY and CLOUDFLARE_EMAIL")
    elif auth or any(k.startswith(("CLOUDFLARE_", "CF_")) for k in os.environ):
        for name in ("CLOUDFLARE_API_KEY", "CLOUDFLARE_EMAIL"):
            if name not in auth:
                raise Refused(f"missing {name}; export CLOUDFLARE_API_KEY and CLOUDFLARE_EMAIL "
                              "using Wrangler's native names (CLOUDFLARE_GLOBAL_KEY is not mapped), "
                              "or export CLOUDFLARE_API_TOKEN alone")
    return environment() | auth | {
        "WRANGLER_WRITE_LOGS": "false", "WRANGLER_LOG_SANITIZE": "true",
        "WRANGLER_LOG": "log", "WRANGLER_SEND_METRICS": "false", "CI": "true"}


def command(argv: list[str], *, env: dict[str, str], data: str | None = None,
            secret: bool = False, cwd: Path = ROOT) -> subprocess.CompletedProcess:
    try:
        result = subprocess.run(argv, input=data, cwd=cwd, env=env,
                                capture_output=True, text=True, timeout=180)
    except FileNotFoundError as error:
        raise Refused(f"missing CLI: {Path(argv[0]).name}; install it and retry") from error
    except subprocess.TimeoutExpired as error:
        raise Refused(f"{Path(argv[0]).name} did not finish within the CLI timeout; retry") from error
    except OSError as error:
        raise Refused(f"cannot execute {Path(argv[0]).name}; check its installation") from error
    # Even a child that echoes auth or a secret on its stdin cannot return those values in diagnostics.
    hidden = {os.environ[name] for name in (*CLOUDFLARE_AUTH, "CLOUDFLARE_GLOBAL_KEY",
                                          "CF_API_TOKEN", "CF_API_KEY", "CF_EMAIL")
              if os.environ.get(name)}
    if secret and data and data.rstrip("\n"):
        hidden.add(data.rstrip("\n"))
    for value in sorted(hidden, key=len, reverse=True):
        result.stdout = result.stdout.replace(value, "<REDACTED>")
        result.stderr = result.stderr.replace(value, "<REDACTED>")
    return result


def wrangler(*args: str, data: str | None = None, secret: bool = False) -> subprocess.CompletedProcess:
    env = wrangler_environment()
    result = command(["wrangler", *args], env=env, data=data, secret=secret, cwd=ROOT / "backend")
    if result.returncode:
        if "CLOUDFLARE_API_TOKEN" in env:
            fix = "check CLOUDFLARE_API_TOKEN; export a valid token with access to the configured account"
        elif "CLOUDFLARE_API_KEY" in env:
            fix = "check CLOUDFLARE_API_KEY and CLOUDFLARE_EMAIL; export the configured account's valid key/email pair"
        else:
            fix = "run wrangler login with access to the configured account, or export CLOUDFLARE_API_TOKEN"
        raise Refused(f"Wrangler {' '.join(args[:2])} failed; {fix}; check the existing backend deployment and retry")
    return result


def aws_argv(*args: str) -> list[str]:
    return ["aws", "--profile", "overmatch", "--region", REGION, "--output", "json", *args]


def aws_code(result: subprocess.CompletedProcess) -> str:
    if "/dev/null" in result.stderr and ("FileExistsError" in result.stderr or "NotADirectoryError" in result.stderr):
        return "CliHistoryEnabled"
    match = re.search(r"An error occurred \(([A-Za-z0-9.]+)\)", result.stderr)
    return match[1] if match else "SignInOrCliFailure"


def decode(text: str, source: str) -> dict:
    try:
        value = json.loads(text)
    except ValueError as error:
        raise Refused(f"{source} returned invalid JSON; retry") from error
    if not isinstance(value, dict):
        raise Refused(f"{source} returned the wrong JSON shape; retry")
    return value


def aws(*args: str) -> dict:
    result = command(aws_argv(*args), env=aws_environment())
    if result.returncode:
        if aws_code(result) == "CliHistoryEnabled":
            raise Refused("AWS CLI history is enabled; disable cli_history in the overmatch profile before retrying")
        raise AwsError(" ".join(args[:2]), aws_code(result))
    return decode(result.stdout, "AWS") if result.stdout.strip() else {}


def optional(*args: str) -> dict | None:
    try:
        return aws(*args)
    except AwsError as error:
        if error.code == "NoSuchEntity":
            return None
        raise


def tag_list() -> list[dict]:
    return [{"Key": key, "Value": value} for key, value in TAGS.items()]


def owned(resource: dict) -> bool:
    return TAGS.items() <= {t["Key"]: t["Value"] for t in resource.get("Tags", [])}.items()


def game_settings() -> tuple[str, int]:
    data = decode((ROOT / "backend/production-server.json").read_text(), "production-server.json")
    kind = data["instance_type"]
    if not isinstance(kind, str) or not re.fullmatch(r"[a-z][a-z0-9]*\.[a-z0-9]+", kind):
        raise Refused("production-server.json: invalid instance_type; declare the production EC2 type")
    ports = re.findall(r"^const PORT: u16 = ([0-9]+);$", (ROOT / "src/net/server.rs").read_text(), re.M)
    if len(ports) != 1 or not 0 < int(ports[0]) < 65536:
        raise Refused("src/net/server.rs: cannot read the server's single PORT; update the setup reader")
    return kind, int(ports[0])


def bind_template(value, bindings: dict):
    if isinstance(value, dict):
        return {k: bind_template(v, bindings) for k, v in value.items()}
    if isinstance(value, list):
        return [bind_template(v, bindings) for v in value]
    if isinstance(value, str):
        if value in bindings:
            return bindings[value]
        for key, replacement in bindings.items():
            if key in value:
                if not isinstance(replacement, str):
                    raise Refused("host-policy.json: non-string placeholder embedded in a string; repair the template")
                value = value.replace(key, replacement)
        if "@" in value:
            raise Refused("host-policy.json: unresolved placeholder; repair the template")
    return value


@dataclass
class Standing:
    account: str
    subnets: list[str]
    group: dict | None
    image: dict
    kind: str
    port: int
    vpc: str
    root_size: int
    template: dict

    def policy(self, group: str) -> dict:
        prefix = f"arn:aws:ec2:{REGION}:{self.account}:"
        policy = bind_template(self.template, {
            "@ACCOUNT@": self.account, "@REGION@": REGION,
            "@NETWORK_ARNS@": [prefix + "security-group/" + group,
                                 *[prefix + "subnet/" + subnet for subnet in self.subnets]],
            "@IMAGE_OWNER@": self.image["OwnerId"], "@INSTANCE_TYPE@": self.kind,
            "@ROOT_SIZE@": self.root_size, "@PROJECT@": TAGS["Project"],
            "@PURPOSE@": TAGS["Purpose"], "@PARAMETER@": PARAMETER})
        # IAM's user-inline-policy interface limit excludes whitespace.
        if len(json.dumps(policy, separators=(",", ":"))) > 2048:
            raise Refused("host-policy.json exceeds IAM's user inline-policy limit; reduce the policy before applying")
        if policy.get("Version") != "2012-10-17" or not isinstance(policy.get("Statement"), list):
            raise Refused("host-policy.json: expected IAM Statement list; repair the template")
        return policy

    def config(self, group: str) -> dict:
        return {"securityGroupId": group, "subnetIds": self.subnets}


def identity_account() -> str:
    identity = aws("sts", "get-caller-identity")
    info = aws("account", "get-account-information")
    if info["AccountName"].lower() != "overmatch":
        raise Refused("wrong AWS account: the overmatch profile must select the Overmatch account")
    account = identity["Account"]
    if not re.fullmatch(r"[0-9]{12}", account):
        raise Refused("AWS identity has no valid account; repair the overmatch profile")
    return account


def discover(account: str) -> Standing:
    template = decode((ROOT / "scripts/aws/host-policy.json").read_text(), "host-policy.json")
    kind, port = game_settings()
    types = aws("ec2", "describe-instance-types", "--instance-types", kind)["InstanceTypes"]
    allocation = decode((ROOT / "backend/production-server.json").read_text(), "production-server.json")
    if len(types) != 1 or types[0]["VCpuInfo"]["DefaultVCpus"] != allocation["vcpu"]:
        raise Refused("production-server.json: instance_type does not match vcpu; repair the allocation")
    vpcs = aws("ec2", "describe-vpcs", "--filters", "Name=is-default,Values=true")["Vpcs"]
    if len(vpcs) != 1:
        raise Refused("default VPC missing or ambiguous; restore the default VPC before setup")
    vpc = vpcs[0]["VpcId"]
    subnets = aws("ec2", "describe-subnets", "--filters", f"Name=vpc-id,Values={vpc}",
                  "Name=default-for-az,Values=true")["Subnets"]
    offers = aws("ec2", "describe-instance-type-offerings", "--location-type", "availability-zone",
                 "--filters", f"Name=instance-type,Values={kind}")["InstanceTypeOfferings"]
    zones = {o["Location"] for o in offers if o["InstanceType"] == kind}
    usable = sorted((s for s in subnets if s["State"] == "available" and s["AvailabilityZone"] in zones),
                    key=lambda s: s["AvailabilityZone"])
    if not usable:
        raise Refused("no available default subnet offering the production instance type; restore a supported default subnet")
    parameter = aws("ssm", "get-parameter", "--name", PARAMETER)["Parameter"]
    if parameter["ARN"] != f"arn:aws:ssm:{REGION}::parameter{PARAMETER}":
        raise Refused("Ubuntu parameter is not the public regional parameter; refuse the image")
    images = aws("ec2", "describe-images", "--image-ids", parameter["Value"])["Images"]
    if len(images) != 1 or not (images[0]["Public"] and images[0]["State"] == "available"
            and images[0]["Architecture"] == "x86_64"
            and re.fullmatch(r"ubuntu/images/hvm-ssd-gp3/ubuntu-[a-z]+-26\.04-amd64-server-[0-9.]+", images[0]["Name"])):
        raise Refused("public Ubuntu parameter does not resolve to available Ubuntu 26.04 amd64; refuse the image")
    groups = aws("ec2", "describe-security-groups", "--filters", f"Name=vpc-id,Values={vpc}",
                 f"Name=group-name,Values={USER}")["SecurityGroups"]
    if len(groups) > 1 or (groups and not owned(groups[0])):
        raise Refused("host security-group name is occupied without the ownership tags; refuse to adopt it")
    roots = [b["Ebs"] for b in images[0]["BlockDeviceMappings"]
             if b["DeviceName"] == images[0]["RootDeviceName"]]
    if len(roots) != 1 or type(roots[0]["VolumeSize"]) is not int or roots[0]["VolumeSize"] <= 0:
        raise Refused("Ubuntu image has no single positive EBS root size; refuse the image")
    root_contract = decode((ROOT / "scripts/aws/host-image.json").read_text(), "host-image.json")
    if (images[0]["RootDeviceName"] != root_contract["rootDeviceName"]
            or roots[0]["VolumeSize"] != root_contract["rootSizeGiB"]):
        raise Refused("current Ubuntu root differs from host-image.json; review the image, update its measured root contract and rebuild the Worker")
    standing = Standing(account, [s["SubnetId"] for s in usable], groups[0] if groups else None,
                        images[0], kind, port, vpc, roots[0]["VolumeSize"], template)
    # Bind and validate all local/discovered inputs before the first cloud mutation.
    standing.policy("sg-" + "0" * 17)
    return standing


def user_state() -> dict | None:
    result = optional("iam", "get-user", "--user-name", USER)
    if result is None:
        return None
    user = result["User"]
    if not owned(user) or user["Path"] != "/overmatch/" or "PermissionsBoundary" in user:
        raise Refused("host IAM user has foreign ownership, path or permissions boundary; refuse to adopt it")
    attached = aws("iam", "list-attached-user-policies", "--user-name", USER)["AttachedPolicies"]
    groups = aws("iam", "list-groups-for-user", "--user-name", USER)["Groups"]
    policies = aws("iam", "list-user-policies", "--user-name", USER)["PolicyNames"]
    login = optional("iam", "get-login-profile", "--user-name", USER)
    if attached or groups or set(policies) - {POLICY} or login:
        raise Refused("host IAM user has extra permissions or console access; remove them before setup")
    return user


def ingress(port: int) -> list[dict]:
    return [{"IpProtocol": "udp", "FromPort": port, "ToPort": port,
             "IpRanges": [{"CidrIp": str(ipaddress.IPv4Network((0, 0)))}]}]


def normalized(value):
    """EC2 describes empty rule fields and optional descriptions that authorize nothing."""
    if isinstance(value, dict):
        return {k: normalized(v) for k, v in value.items() if v != [] and k != "Description"}
    if isinstance(value, list):
        return sorted((normalized(v) for v in value), key=lambda v: json.dumps(v, sort_keys=True))
    return value


def egress() -> list[dict]:
    return [{"IpProtocol": "-1", "IpRanges": [{"CidrIp": str(ipaddress.IPv4Network((0, 0)))}]}]


def worker_args(target: str) -> list[str]:
    # Production is Wrangler's top-level environment; empty --env selects it explicitly.
    return ["--config", "wrangler.jsonc", "--env", "" if target == "production" else target]


def put_secret(name: str, value: str, target: str) -> None:
    # stdout/stderr are captured even on failure: Wrangler diagnostics must never echo credentials.
    wrangler("secret", "put", name, *worker_args(target), data=value + "\n", secret=True)


def keys() -> list[dict]:
    metadata = aws("iam", "list-access-keys", "--user-name", USER)["AccessKeyMetadata"]
    for key in metadata:
        if not isinstance(key["AccessKeyId"], str) or not re.fullmatch(
                r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:.]+(?:Z|[+-][0-9]{2}:[0-9]{2})", key["CreateDate"]):
            raise Refused("IAM key metadata is malformed; reconcile keys in IAM before retrying")
    return metadata


def check_key_count(metadata: list[dict], *, rotate: bool) -> None:
    if (rotate and len(metadata) != 1) or (not rotate and metadata):
        raise Refused("rotate-key requires exactly one existing key; issue-key requires none; reconcile keys in IAM")


def install_key(target: str, *, rotate: bool) -> None:
    old = keys()
    check_key_count(old, rotate=rotate)
    old_ids = {k["AccessKeyId"] for k in old}
    created = None
    published = False
    try:
        # One anonymous pipe, no shell, file, environment variable or command-line credential.
        # Only the forwarder reads this pipe, and only Wrangler's stdin receives each field.
        try:
            producer = subprocess.Popen(aws_argv("iam", "create-access-key", "--user-name", USER),
                                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=aws_environment())
        except OSError as error:
            raise Refused("cannot start AWS key creation; check the AWS CLI") from error
        try:
            try:
                payload, _ = producer.communicate(timeout=180)
            except subprocess.TimeoutExpired as error:
                raise Refused("AWS key creation timed out; reconcile its metadata before retrying") from error
            if producer.returncode:
                raise Refused("AWS key creation was not confirmed; check IAM permissions and retry")
            key = decode(payload.decode(), "AWS key creation")["AccessKey"]
            if isinstance(key["AccessKeyId"], str) and key["AccessKeyId"]:
                created = key["AccessKeyId"]
            if not all(isinstance(key[n], str) and key[n] for n in ("AccessKeyId", "SecretAccessKey")):
                raise Refused("AWS key creation returned incomplete credentials; retry")
            put_secret(SECRETS[0], key["AccessKeyId"], target)
            put_secret(SECRETS[1], key["SecretAccessKey"], target)
            published = True
        finally:
            if producer.poll() is None:
                producer.kill()
                producer.wait()
        for stale in old:
            try:
                aws("iam", "delete-access-key", "--user-name", USER, "--access-key-id", stale["AccessKeyId"])
            except Refused as error:
                raise Refused(f"OldKeyDeletionFailed: Worker has the new key; remove the overmatch-host key "
                              f"created {stale['CreateDate']} in IAM, then rerun "
                              f"`scripts/aws/host-setup verify`; rotate-key --env {target} requires one key") from error
    except BaseException:
        if published:
            raise
        # A lost create response can still have created a key. Reconcile metadata, never secrets.
        try:
            unpublished = {created} if created is not None else {k["AccessKeyId"] for k in keys()} - old_ids
            for key_id in unpublished:
                aws("iam", "delete-access-key", "--user-name", USER, "--access-key-id", key_id)
        except Refused as error:
            raise Refused("new key cleanup was not confirmed; revoke the host user's new key in IAM immediately") from error
        raise
    print("Worker credential pair installed; previous keys revoked")


def require_wrangler() -> None:
    if not shutil.which("wrangler"):
        raise Refused("missing CLI: wrangler; add backend/node_modules/.bin to PATH after installing backend dependencies")
    expected = decode((ROOT / "backend/package.json").read_text(), "backend/package.json")["devDependencies"]["wrangler"]
    version = wrangler("--version")
    if re.search(r"(?<![0-9.])" + re.escape(expected) + r"(?![0-9.])", version.stdout) is None:
        raise Refused("Wrangler version does not match backend/package.json; use the backend's installed CLI")


def secret_names(target: str) -> set[str]:
    result = wrangler("secret", "list", *worker_args(target))
    try:
        names = json.loads(result.stdout)
    except ValueError as error:
        raise Refused("Wrangler secret list returned invalid JSON; retry with the pinned CLI") from error
    if not isinstance(names, list) or not all(isinstance(n, dict) and isinstance(n.get("name"), str) for n in names):
        raise Refused("Wrangler secret list has the wrong shape; update the setup reader")
    return {n["name"] for n in names}


def apply(standing: Standing) -> None:
    # AWS also checks IAM syntax before any mutation, using a group identifier of the final length.
    simulate(standing, "sg-" + "0" * 17, None)
    user = user_state()
    if standing.group is None:
        result = aws("ec2", "create-security-group", "--group-name", USER, "--description", "Overmatch game hosts",
                     "--vpc-id", standing.vpc, "--tag-specifications",
                     json.dumps([{"ResourceType": "security-group", "Tags": tag_list()}]))
        group_id = result["GroupId"]
        permissions = []
    else:
        group_id, permissions = standing.group["GroupId"], standing.group["IpPermissions"]
    expected = ingress(standing.port)
    # Replace drift, including broad or SSH ingress, rather than accumulating exceptions.
    if normalized(permissions) != normalized(expected):
        if permissions:
            aws("ec2", "revoke-security-group-ingress", "--group-id", group_id,
                "--ip-permissions", json.dumps(permissions))
        aws("ec2", "authorize-security-group-ingress", "--group-id", group_id,
            "--ip-permissions", json.dumps(expected))
    if user is None:
        aws("iam", "create-user", "--user-name", USER, "--path", "/overmatch/", "--tags", json.dumps(tag_list()))
    policy = standing.policy(group_id)
    installed = optional("iam", "get-user-policy", "--user-name", USER, "--policy-name", POLICY)
    if installed is None or installed["PolicyDocument"] != policy:
        aws("iam", "put-user-policy", "--user-name", USER, "--policy-name", POLICY,
            "--policy-document", json.dumps(policy))
    print("Keyless standing host resources reconciled; run verify before issuing a provider key")


def provision_key(standing: Standing, target: str, *, rotate: bool) -> None:
    require_wrangler()
    # Refuse missing deployments and standing-resource drift before key creation.
    secret_names(target)
    verify(standing)
    check_key_count(keys(), rotate=rotate)
    put_secret(SECRETS[2], json.dumps(standing.config(standing.group["GroupId"])), target)
    install_key(target, rotate=rotate)


def simulate(standing: Standing, group: str, principal: dict | None) -> None:
    policy = standing.policy(group)
    prefix = f"arn:aws:ec2:{REGION}:{standing.account}:"
    # Synthetic resources test authorization without requiring a live machine.
    resources = [f"arn:aws:ec2:{REGION}::image/{standing.image['ImageId']}",
                 prefix + "subnet/" + standing.subnets[0], prefix + "security-group/" + group,
                 *[prefix + name + "/" + name for name in ("instance", "volume", "network-interface")]]
    context = {"ec2:Region": REGION, "ec2:Owner": standing.image["OwnerId"],
               "ec2:InstanceType": standing.kind, "ec2:InstanceMarketType": "on-demand",
               "aws:RequestTag/Project": TAGS["Project"], "aws:RequestTag/Purpose": TAGS["Purpose"],
               "aws:ResourceTag/Project": TAGS["Project"], "aws:ResourceTag/Purpose": TAGS["Purpose"],
               "ec2:CreateAction": "RunInstances", "ec2:Tenancy": "default",
               "ec2:MetadataHttpTokens": "required", "ec2:VolumeType": "gp3",
               "ec2:VolumeSize": str(standing.root_size), "ec2:VolumeIops": "3000",
               "ec2:VolumeThroughput": "125"}
    other_kind = "m7a.large" if standing.kind != "m7a.large" else "c7a.large"
    base = (["iam", "simulate-principal-policy", "--policy-source-arn", principal["Arn"]] if principal else
            ["iam", "simulate-custom-policy", "--policy-input-list", json.dumps(policy)])

    def check(label: str, action: str, resource: str, allowed: bool, changes: dict | None = None) -> None:
        values = context | (changes if changes is not None else {})
        entries = [{"ContextKeyName": k, "ContextKeyValues": [v], "ContextKeyType": "numeric" if k in ("ec2:VolumeSize", "ec2:VolumeIops", "ec2:VolumeThroughput") else "string"}
                   for k, v in values.items() if v is not None]
        reply = aws(*base, "--action-names", action, "--resource-arns", resource,
                    "--context-entries", json.dumps(entries))["EvaluationResults"]
        if len(reply) != 1:
            raise Refused("IAM simulator returned the wrong result count; no authorization evidence")
        decision = Decision.parse(reply[0].get("EvalDecision"))
        if (decision is Decision.ALLOWED) != allowed:
            raise Refused(f"policy verification failed: {label}; repair the host policy before use")
        print(f"simulation {'allowed' if allowed else 'denied'}: {label}")

    for resource in resources:
        check("launch " + resource.split(":")[-1].split("/")[0], "ec2:RunInstances", resource, True)
    for subnet in standing.subnets[1:]:
        check("launch offering subnet", "ec2:RunInstances", prefix + "subnet/" + subnet, True)
    check("different subnet", "ec2:RunInstances", prefix + "subnet/foreign", False)
    for field, wrong in [("ec2:Tenancy", "dedicated"), ("ec2:MetadataHttpTokens", "optional")]:
        check(field, "ec2:RunInstances", resources[3], False, {field: wrong})
        check("missing " + field, "ec2:RunInstances", resources[3], False, {field: None})
    for field, wrong in [("ec2:VolumeType", "io2"), ("ec2:VolumeSize", str(standing.root_size + 1)),
                         ("ec2:VolumeIops", "3001"), ("ec2:VolumeThroughput", "126")]:
        check(field, "ec2:RunInstances", resources[4], False, {field: wrong})
        check("missing " + field, "ec2:RunInstances", resources[4], False, {field: None})
    for kind, resource in zip(("instance", "volume", "network-interface"), resources[3:]):
        check("launch tags on " + kind, "ec2:CreateTags", resource, True)
        check("standalone retag " + kind, "ec2:CreateTags", resource, False, {"ec2:CreateAction": None})
        for key in TAGS:
            check("launch missing " + key + " on " + kind, "ec2:RunInstances", resource, False,
                  {"aws:RequestTag/" + key: None})
    check("different instance type", "ec2:RunInstances", resources[3], False, {"ec2:InstanceType": other_kind})
    check("Spot launch", "ec2:RunInstances", resources[3], False, {"ec2:InstanceMarketType": "spot"})
    check("foreign image", "ec2:RunInstances", resources[0], False, {"ec2:Owner": "amazon"})
    check("different security group", "ec2:RunInstances", prefix + "security-group/foreign", False)
    check("tagged host termination", "ec2:TerminateInstances", resources[3], True)
    for key in TAGS:
        check("termination missing " + key, "ec2:TerminateInstances", resources[3], False,
              {"aws:ResourceTag/" + key: None})
    check("different type termination", "ec2:TerminateInstances", resources[3], True, {"ec2:InstanceType": other_kind})
    check("regional describe", "ec2:DescribeInstances", "*", True)
    for action, resource in [("ec2:RunInstances", r) for r in resources] + [
            ("ec2:TerminateInstances", resources[3]), ("ec2:DescribeInstances", "*")]:
        check("other-region " + action, action, resource.replace(REGION, "ap-southeast-2"), False,
              {"ec2:Region": "ap-southeast-2"})
    check("one Ubuntu parameter", "ssm:GetParameters", f"arn:aws:ssm:{REGION}::parameter{PARAMETER}", True)
    check("other SSM parameter", "ssm:GetParameters", f"arn:aws:ssm:{REGION}::parameter/foreign", False)
    for action in ("iam:ListUsers", "iam:CreateUser", "iam:PassRole", "s3:ListAllMyBuckets", "s3:PutObject",
                   "ec2:CreateSecurityGroup", "ec2:CreateVolume", "ec2:RebootInstances"):
        check(action, action, "*", False)


class Decision(Enum):
    ALLOWED = "allowed"
    IMPLICIT_DENY = "implicitDeny"
    EXPLICIT_DENY = "explicitDeny"

    @classmethod
    def parse(cls, value) -> Decision:
        try:
            return cls(value)
        except (ValueError, TypeError) as error:
            raise Refused("IAM simulator returned an unknown decision; no authorization evidence") from error


def verify(standing: Standing) -> None:
    principal = user_state()
    if principal is None or standing.group is None:
        raise Refused("keyless standing resources are absent; run apply before verify")
    group = standing.group["GroupId"]
    installed = optional("iam", "get-user-policy", "--user-name", USER, "--policy-name", POLICY)
    if installed is None or installed["PolicyDocument"] != standing.policy(group):
        raise Refused("installed host policy has drifted; run apply before verify")
    if normalized(standing.group["IpPermissions"]) != normalized(ingress(standing.port)):
        raise Refused("host security-group ingress has drifted; run apply before verify")
    if normalized(standing.group["IpPermissionsEgress"]) != normalized(egress()):
        raise Refused("host security-group egress has drifted; restore default outbound rules before verify")
    metadata = keys()
    if len(metadata) > 1 or any(k["Status"] != "Active" for k in metadata):
        raise Refused("host keys need reconciliation: at most one active key is allowed")
    print(f"Checking installed principal permissions without a key; AMI root cap {standing.root_size} GiB")
    simulate(standing, group, principal)
    print("Keyless verification passed. Only provider-key launch dry-runs can prove EC2's actual request "
          "context and public-parameter resolution; those belong to the provider slice.")


def tagged_resources(*, project_only: bool = False) -> list[tuple[str, list[dict]]]:
    tags = {"Project": TAGS["Project"]} if project_only else TAGS
    filters = [f"Name=tag:{k},Values={v}" for k, v in tags.items()]
    reservations = aws("ec2", "describe-instances", "--filters", *filters)["Reservations"]
    active = [i for r in reservations for i in r["Instances"] if i["State"]["Name"] != "terminated"]
    result = [("instances", active)]
    for op, field in [("describe-volumes", "Volumes"), ("describe-network-interfaces", "NetworkInterfaces"),
                      ("describe-security-groups", "SecurityGroups"), ("describe-key-pairs", "KeyPairs")]:
        result.append((field, aws("ec2", op, "--filters", *filters)[field]))
    return result


def remove_credentials(target: str, user_exists: bool) -> None:
    if user_exists:
        for key in keys():
            aws("iam", "delete-access-key", "--user-name", USER, "--access-key-id", key["AccessKeyId"])
    # Listing names first makes interrupted secret removal restartable.
    names = secret_names(target)
    for name in SECRETS:
        if name in names:
            wrangler("secret", "delete", name, *worker_args(target), data="y\n")


def revoke_key(target: str) -> None:
    require_wrangler()
    user = user_state()
    resources = tagged_resources(project_only=True)
    if any(items for name, items in resources if name != "SecurityGroups"):
        raise Refused("tagged project resources remain across environments; stop and drain before revoke-key")
    remove_credentials(target, user is not None)
    if (user is not None and keys()) or set(SECRETS) & secret_names(target):
        raise Refused("key revocation proof failed; retry revoke-key with the same environment")
    print("Revocation proof: no host keys or selected Worker hosting secrets; standing user, policy and security group retained")


def teardown(target: str) -> None:
    require_wrangler()
    user = user_state()
    resources = tagged_resources()
    if any(items for name, items in resources if name != "SecurityGroups"):
        raise Refused("tagged host resources remain; stop and drain the provider before teardown")
    groups = next(items for name, items in resources if name == "SecurityGroups")
    if any(g["GroupName"] != USER for g in groups):
        raise Refused("unexpected tagged security group; reconcile ownership before teardown")
    remove_credentials(target, user is not None)
    if user:
        if POLICY in aws("iam", "list-user-policies", "--user-name", USER)["PolicyNames"]:
            aws("iam", "delete-user-policy", "--user-name", USER, "--policy-name", POLICY)
        aws("iam", "delete-user", "--user-name", USER)
    for group in groups:
        aws("ec2", "delete-security-group", "--group-id", group["GroupId"])
    remaining = tagged_resources()
    if (any(items for _, items in remaining)
            or optional("iam", "get-user", "--user-name", USER) is not None
            or set(SECRETS) & secret_names(target)):
        raise Refused("teardown proof failed: host resources remain; retry teardown")
    print("Teardown proof: tagged instances, volumes, interfaces, groups, key pairs and IAM user all absent")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("plan", "apply", "verify", "issue-key", "rotate-key", "revoke-key", "teardown"):
        sub = commands.add_parser(name)
        if name in ("issue-key", "rotate-key", "revoke-key", "teardown"):
            sub.add_argument("--env", required=True, choices=("production", "aws-proving"))
    args = parser.parse_args(argv)
    try:
        if args.command == "plan":
            kind, port = game_settings()
            print(f"Plan only: IAM user {USER}, inline host-policy.json, security group {USER}; "
                  f"{REGION}, {kind}, UDP {port} from the IPv4 internet, no SSH")
            print("Existing default VPC/subnet and public Ubuntu 26.04 parameter; no instance, package, role or key pair")
            print("Apply is keyless; issue-key --env TARGET is deferred until a deployed provider reads the secrets")
            return 0
        account = identity_account()
        if args.command == "apply":
            apply(discover(account))
        elif args.command == "verify":
            verify(discover(account))
        elif args.command in ("issue-key", "rotate-key"):
            provision_key(discover(account), args.env, rotate=args.command == "rotate-key")
        elif args.command == "revoke-key":
            revoke_key(args.env)
        else:
            teardown(args.env)
        return 0
    except Refused as error:
        print(f"host-setup refused: {error}", file=sys.stderr)
    except (KeyError, TypeError, ValueError, AttributeError, IndexError):
        print("host-setup refused: malformed configuration or CLI response; check the documented input shapes", file=sys.stderr)
    except OSError:
        print("host-setup refused: local file or CLI unavailable; check dependencies and checkout files", file=sys.stderr)
    except KeyboardInterrupt:
        print("host-setup refused: interrupted; rerun the command to reconcile its resources", file=sys.stderr)
    return 1


if __name__ == "__main__":
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    sys.exit(main())
