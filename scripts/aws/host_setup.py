"""Standing EC2 hosting resources. See host-setup.md for the operator and provider contracts."""
from __future__ import annotations

import argparse
from dataclasses import dataclass
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


class Refused(Exception):
    """An outside input the operator must repair before continuing."""


class AwsError(Refused):
    def __init__(self, operation: str, code: str):
        self.code = code
        super().__init__(f"AWS {operation} refused ({code}); check the overmatch sign-in and permissions")


def environment() -> dict[str, str]:
    # Credential sources belong to the named profile, never the parent shell.
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("AWS_", "WRANGLER_OUTPUT_FILE_"))
           and k not in ("CLOUDFLARE_GLOBAL_KEY", "CLOUDFLARE_ACCOUNT_ID")}
    # If profile cli_history is enabled, AWS fails before sending any API request: its history
    # directory cannot be created below the null device. Never let it persist a create-key response.
    return env | {"AWS_PAGER": "", "AWS_CLI_AUTO_PROMPT": "off",
                  "AWS_CLI_HISTORY_FILE": "/dev/null/host-setup-history-disabled",
                  "WRANGLER_WRITE_LOGS": "false", "WRANGLER_LOG_SANITIZE": "true",
                  "WRANGLER_LOG": "error", "WRANGLER_SEND_METRICS": "false", "CI": "true"}


def command(argv: list[str], *, data: str | None = None, cwd: Path = ROOT) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(argv, input=data, cwd=cwd, env=environment(),
                              capture_output=True, text=True, timeout=180)
    except FileNotFoundError as error:
        raise Refused(f"missing CLI: {Path(argv[0]).name}; install it and retry") from error
    except subprocess.TimeoutExpired as error:
        raise Refused(f"{Path(argv[0]).name} did not finish within the CLI timeout; retry") from error
    except OSError as error:
        raise Refused(f"cannot execute {Path(argv[0]).name}; check its installation") from error


def aws_argv(*args: str) -> list[str]:
    return ["aws", "--profile", "overmatch", "--region", REGION, "--output", "json", *args]


def aws_code(result: subprocess.CompletedProcess) -> str:
    if "host-setup-history-disabled" in result.stderr:
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
    result = command(aws_argv(*args))
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


@dataclass
class Standing:
    account: str
    subnet: str
    group: dict | None
    image: dict
    kind: str
    port: int
    vpc: str

    def policy(self, group: str) -> dict:
        text = (ROOT / "scripts/aws/host-policy.json").read_text()
        for key, value in {"ACCOUNT": self.account, "SUBNET": self.subnet, "GROUP": group,
                           "IMAGE_OWNER": self.image["OwnerId"], "INSTANCE_TYPE": self.kind}.items():
            text = text.replace(f"@{key}@", value)
        if re.search(r"@[A-Z_]+@", text):
            raise Refused("host-policy.json: unresolved placeholder; repair the template")
        policy = decode(text, "host-policy.json")
        # IAM's user-inline-policy interface limit excludes whitespace.
        if len(json.dumps(policy, separators=(",", ":"))) > 2048:
            raise Refused("host-policy.json exceeds IAM's user inline-policy limit; reduce the policy before applying")
        return policy

    def config(self, group: str) -> dict:
        return {"region": REGION, "instanceType": self.kind, "securityGroupId": group,
                "subnetId": self.subnet, "imageId": "resolve:ssm:" + PARAMETER}


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
    usable = sorted((s for s in subnets if s["State"] == "available"), key=lambda s: s["AvailabilityZone"])
    if not usable:
        raise Refused("no available default subnet; restore the default subnet before setup")
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
    return Standing(account, usable[0]["SubnetId"], groups[0] if groups else None, images[0], kind, port, vpc)


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
             "IpRanges": [{"CidrIp": str(ipaddress.IPv4Network(0))}]}]


def normalized(value):
    """EC2 describes empty rule fields and optional descriptions that authorize nothing."""
    if isinstance(value, dict):
        return {k: normalized(v) for k, v in value.items() if v != [] and k != "Description"}
    if isinstance(value, list):
        return sorted((normalized(v) for v in value), key=lambda v: json.dumps(v, sort_keys=True))
    return value


def put_secret(name: str, value: str) -> None:
    # stdout/stderr are captured even on failure: Wrangler diagnostics must never echo credentials.
    result = command(["wrangler", "secret", "put", name, "--config", "wrangler.jsonc"],
                     data=value + "\n", cwd=ROOT / "backend")
    if result.returncode:
        raise Refused(f"Worker secret {name} was not confirmed; check Wrangler login and retry")


def keys() -> list[str]:
    return [k["AccessKeyId"] for k in aws("iam", "list-access-keys", "--user-name", USER)["AccessKeyMetadata"]]


def rotate_key() -> None:
    old = keys()
    if len(old) >= 2:
        raise Refused("host user already has the IAM key limit; reconcile its keys before rotating")
    created = None
    try:
        # One anonymous pipe, no shell, file, environment variable or command-line credential.
        # Only the forwarder reads this pipe, and only Wrangler's stdin receives each field.
        try:
            producer = subprocess.Popen(aws_argv("iam", "create-access-key", "--user-name", USER),
                                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=environment())
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
            put_secret(SECRETS[0], key["AccessKeyId"])
            put_secret(SECRETS[1], key["SecretAccessKey"])
        finally:
            if producer.poll() is None:
                producer.kill()
                producer.wait()
        for key_id in old:
            aws("iam", "delete-access-key", "--user-name", USER, "--access-key-id", key_id)
    except BaseException:
        # A lost create response can still have created a key. Reconcile metadata, never secrets.
        try:
            unpublished = {created} if created is not None else set(keys()) - set(old)
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
    version = command(["wrangler", "--version"])
    if version.returncode or re.search(r"(?<![0-9.])" + re.escape(expected) + r"(?![0-9.])", version.stdout) is None:
        raise Refused("Wrangler version does not match backend/package.json; use the backend's installed CLI")


def secret_names() -> set[str]:
    result = command(["wrangler", "secret", "list", "--config", "wrangler.jsonc"], cwd=ROOT / "backend")
    if result.returncode:
        raise Refused("cannot list Worker secret names; check Wrangler login and the existing backend deployment")
    names = json.loads(result.stdout)
    if not isinstance(names, list) or not all(isinstance(n, dict) and isinstance(n.get("name"), str) for n in names):
        raise Refused("Wrangler secret list has the wrong shape; update the setup reader")
    return {n["name"] for n in names}


def apply(standing: Standing) -> None:
    require_wrangler()
    # Listing refuses a missing Worker instead of letting `secret put` silently create one.
    names = secret_names()
    user = user_state()
    existing_keys = keys() if user else []
    if len(existing_keys) > 1 or (existing_keys and not set(SECRETS[:2]) <= names):
        raise Refused("host credentials need reconciliation; run rotate-key before applying again")
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
    put_secret(SECRETS[2], json.dumps(standing.config(group_id)))
    if not keys():
        rotate_key()
    print("Standing host resources reconciled; run verify before enabling the provider")


def simulate(standing: Standing, group: str, principal: dict | None) -> None:
    policy = standing.policy(group)
    prefix = f"arn:aws:ec2:{REGION}:{standing.account}:"
    # Synthetic resources test authorization without requiring a live machine.
    resources = [f"arn:aws:ec2:{REGION}::image/{standing.image['ImageId']}",
                 prefix + "subnet/" + standing.subnet, prefix + "security-group/" + group,
                 *[prefix + name + "/" + name for name in ("instance", "volume", "network-interface")]]
    context = {"ec2:Region": REGION, "ec2:Owner": standing.image["OwnerId"],
               "ec2:InstanceType": standing.kind, "ec2:InstanceMarketType": "on-demand",
               "aws:RequestTag/Project": TAGS["Project"], "aws:RequestTag/Purpose": TAGS["Purpose"],
               "aws:ResourceTag/Project": TAGS["Project"], "aws:ResourceTag/Purpose": TAGS["Purpose"],
               "ec2:CreateAction": "RunInstances"}
    other_kind = "m7a.large" if standing.kind != "m7a.large" else "c7a.large"
    base = (["iam", "simulate-principal-policy", "--policy-source-arn", principal["Arn"]] if principal else
            ["iam", "simulate-custom-policy", "--policy-input-list", json.dumps(policy)])

    def check(label: str, action: str, resource: str, allowed: bool, changes: dict | None = None) -> None:
        values = context | (changes if changes is not None else {})
        entries = [{"ContextKeyName": k, "ContextKeyValues": [v], "ContextKeyType": "string"}
                   for k, v in values.items() if v is not None]
        reply = aws(*base, "--action-names", action, "--resource-arns", resource,
                    "--context-entries", json.dumps(entries))["EvaluationResults"]
        if len(reply) != 1 or (reply[0]["EvalDecision"] == "allowed") != allowed:
            raise Refused(f"policy verification failed: {label}; repair the host policy before use")
        print(f"simulation {'allowed' if allowed else 'denied'}: {label}")

    for resource in resources:
        check("launch " + resource.split(":")[-1].split("/")[0], "ec2:RunInstances", resource, True)
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
    check("different type termination", "ec2:TerminateInstances", resources[3], False, {"ec2:InstanceType": other_kind})
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


def verify(standing: Standing) -> None:
    principal = user_state()
    if principal:
        if standing.group is None:
            raise Refused("host user exists without its security group; run apply before verify")
        group = standing.group["GroupId"]
        print("Checking installed principal permissions (not using or retrieving its key)")
    else:
        # Pre-creation proof binds the template to an existing group solely for an operator dry-run.
        groups = aws("ec2", "describe-security-groups", "--filters", f"Name=vpc-id,Values={standing.vpc}",
                     "Name=group-name,Values=default")["SecurityGroups"]
        if len(groups) != 1:
            raise Refused("no unique default security group for the pre-creation dry-run")
        group = groups[0]["GroupId"]
        print("Host user absent: checking template with a borrowed group; installed permissions remain unproven")
    simulate(standing, group, principal)
    specifications = [{"ResourceType": kind, "Tags": tag_list()}
                      for kind in ("instance", "volume", "network-interface")]
    for image, label in [(standing.image["ImageId"], "concrete image"), ("resolve:ssm:" + PARAMETER, "SSM image")]:
        result = command(aws_argv("ec2", "run-instances", "--image-id", image, "--instance-type", standing.kind,
                                 "--subnet-id", standing.subnet, "--security-group-ids", group,
                                 "--tag-specifications", json.dumps(specifications),
                                 "--instance-initiated-shutdown-behavior", "terminate", "--dry-run"))
        if result.returncode == 0 or aws_code(result) != "DryRunOperation":
            raise Refused(f"operator launch dry-run failed: {label} ({aws_code(result)}); no resource was created")
        print(f"operator dry-run passed: {label}; this does not test the provider key")


def tagged_resources() -> list[tuple[str, list[dict]]]:
    filters = [f"Name=tag:{k},Values={v}" for k, v in TAGS.items()]
    reservations = aws("ec2", "describe-instances", "--filters", *filters)["Reservations"]
    active = [i for r in reservations for i in r["Instances"] if i["State"]["Name"] != "terminated"]
    result = [("instances", active)]
    for op, field in [("describe-volumes", "Volumes"), ("describe-network-interfaces", "NetworkInterfaces"),
                      ("describe-security-groups", "SecurityGroups"), ("describe-key-pairs", "KeyPairs")]:
        result.append((field, aws("ec2", op, "--filters", *filters)[field]))
    return result


def teardown() -> None:
    require_wrangler()
    user = user_state()
    resources = tagged_resources()
    if any(items for name, items in resources if name != "SecurityGroups"):
        raise Refused("tagged host resources remain; stop and drain the provider before teardown")
    groups = next(items for name, items in resources if name == "SecurityGroups")
    if any(g["GroupName"] != USER for g in groups):
        raise Refused("unexpected tagged security group; reconcile ownership before teardown")
    if user:
        for key_id in keys():
            aws("iam", "delete-access-key", "--user-name", USER, "--access-key-id", key_id)
    # `secret delete` refuses on an absent secret. Listing names first makes teardown restartable.
    names = secret_names()
    for name in SECRETS:
        if name in names:
            result = command(["wrangler", "secret", "delete", name, "--config", "wrangler.jsonc"],
                             data="y\n", cwd=ROOT / "backend")
            if result.returncode:
                raise Refused(f"Worker secret {name} deletion not confirmed; retry teardown")
    if user:
        if POLICY in aws("iam", "list-user-policies", "--user-name", USER)["PolicyNames"]:
            aws("iam", "delete-user-policy", "--user-name", USER, "--policy-name", POLICY)
        aws("iam", "delete-user", "--user-name", USER)
    for group in groups:
        aws("ec2", "delete-security-group", "--group-id", group["GroupId"])
    remaining = tagged_resources()
    if (any(items for _, items in remaining)
            or optional("iam", "get-user", "--user-name", USER) is not None
            or set(SECRETS) & secret_names()):
        raise Refused("teardown proof failed: host resources remain; retry teardown")
    print("Teardown proof: tagged instances, volumes, interfaces, groups, key pairs and IAM user all absent")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("plan", "apply", "verify", "rotate-key", "teardown"))
    args = parser.parse_args(argv)
    try:
        if args.command == "plan":
            kind, port = game_settings()
            print(f"Plan only: IAM user {USER}, inline host-policy.json, security group {USER}; "
                  f"{REGION}, {kind}, UDP {port} from the IPv4 internet, no SSH")
            print("Existing default VPC/subnet and public Ubuntu 26.04 parameter; no instance, package, role or key pair")
            print("Apply writes Worker secrets: " + ", ".join(SECRETS) + "; an existing key is kept")
            return 0
        account = identity_account()
        if args.command == "apply":
            apply(discover(account))
        elif args.command == "verify":
            verify(discover(account))
        elif args.command == "rotate-key":
            require_wrangler()
            if user_state() is None:
                raise Refused("host IAM user is absent; run apply before rotating its key")
            secret_names()
            rotate_key()
        else:
            teardown()
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
