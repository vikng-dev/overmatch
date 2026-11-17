# EC2 host setup

`scripts/aws/host-setup` owns the standing resources for the Overmatch multiplayer game's EC2
provider. It creates no server. The hosting design lives in
`.agents/scratch/aws-hosting-design-2026-11-17.md` in the owner's main checkout; this guide owns the
operator commands, provider interface and authorization limits.

## Operator sequence

**First, the owner sets a billing alert.** A stolen Worker key can incur continuing charges that
this policy cannot bound. The alert is the control for the open costs below, not a spending cap or
automatic shutdown. The owner keeps the key-in-Worker design with this limitation.

Use the signed-in `overmatch` AWS profile. Setup checks the runtime account name is `Overmatch`
(case insensitive). Identifiers stay in memory; none are tracked inputs or printed. AWS CLI history
must be disabled in that profile: setup refuses before sending requests if history is enabled.
The script opens no credential or AWS configuration file. Parent AWS and Cloudflare credential
and environment overrides are stripped from every child. Wrangler uses its existing saved OAuth
login; no parent Cloudflare API token is needed.

Now, after review, the orchestrator runs from the repository root:

```sh
scripts/aws/host-setup plan
scripts/aws/host-setup apply
scripts/aws/host-setup verify
```

- `plan` is local only; no CLI or cloud calls.
- `apply` creates/reconciles only the security group, keyless IAM user and inline policy, all named
  `overmatch-host`. It requires ownership tags `Project=overmatch`, `Purpose=host` before adoption,
  plus IAM path `/overmatch/`. Extra policies, groups, boundaries or console access are refused.
  Policy JSON, substitutions, inline-policy size and AWS custom-policy simulations are checked
  before the first mutation. The parsed template is retained while the new group identifier is
  bound. Ingress drift is replaced by the single UDP port owned by `src/net/server.rs`, from the
  IPv4 internet, with no SSH. Default outbound access remains for downloads and Worker reports.
  No Wrangler, access key, Worker secret, deployment, instance or boot package is created.
- `verify` creates nothing and needs no provider key or Wrangler. It checks installed policy and
  ingress/egress drift, resource ownership and IAM permissions, and that at most one active key
  exists. It simulates the **installed user's** policy. Missing standing parts require `apply`.
  It states the remaining provider-key proof explicitly.

The production instance type and vCPU allocation belong to `backend/production-server.json`;
setup checks them against EC2 on both apply and verify. All available default subnets whose zones
actually offer that type are selected, in zone order. The policy allows precisely those subnets
and the host security group; the provider receives the whole list for capacity fallback.

Later, **only when the provider slice reads the secrets on an existing proving Worker**, run:

```sh
npm ci --prefix backend --ignore-scripts --no-audit --no-fund
export PATH="$PWD/backend/node_modules/.bin:$PATH"
scripts/aws/host-setup issue-key --env aws-proving
scripts/aws/host-setup verify
```

`issue-key` refuses any existing user key. It first checks the target Worker exists and the standing
resources pass verification, writes discovered network configuration, then creates one IAM key
and forwards its credential pair to the named Worker. There is no target default. `aws-proving`
selects the `env.aws-proving` config entry in `backend/wrangler.jsonc`, naming
`overmatch-backend-aws-proving`. This is a config entry, not a deployment pipeline: the provider
slice supplies its own bindings and deploys it before issuing a key. The production deployment
continues using the top-level config. `--env production` explicitly selects that top-level Worker
(Wrangler receives an empty `--env`). Never issue a production key before its provider consumes it.

For replacement and removal, explicitly name the deployment holding the credential:

```sh
scripts/aws/host-setup rotate-key --env aws-proving
scripts/aws/host-setup teardown --env aws-proving
```

Use `--env production` instead only when production is the key holder. This user has one credential
pair for one active provider deployment; it is not a way to operate both Workers concurrently.
Moving to production requires draining the proving provider and removing its key/secrets before
issuing a production key. Rotation requires exactly one old key and temporarily creates a second.
Old keys stay valid until both credential writes succeed. After both writes, the new key is never
revoked by failure cleanup. Failed old-key deletion returns `OldKeyDeletionFailed`, names only its
creation date, and instructs the operator to remove that dated key in IAM and rerun `verify`.
Another rotation is refused until only one key remains.

Before both credential writes are confirmed, failure revokes the unpublished key, including a
lost create response reconciled through key metadata. Failure of that cleanup is an emergency
refusal. Separate Worker writes can leave a mismatched pair after an unconfirmed handoff: rerun
`issue-key` if no key remains, or `rotate-key` if the old key remains, using the same target. Do not
enable the provider until the handoff succeeds. No secret can be recovered from IAM metadata.

`teardown` refuses while tagged instances, volumes, interfaces or key pairs remain; it never
terminates a match implicitly. It revokes keys first, deletes the selected Worker's hosting secrets,
inline policy, user and security group, then proves absence. Unrelated secrets survive. A keyless
setup can be removed with the same command using an existing target Worker and saved Wrangler login.
Interrupted removal is restartable; image availability and the production allocation are irrelevant.

## Credential handling

AWS stdout -> anonymous subprocess pipe -> in-memory forwarder -> Wrangler stdin. Shell tracing,
core dumps, Wrangler disk logs and metrics are disabled. Child output is captured even on failure;
credential fields are never printed or passed in Wrangler argv, files or environment variables.
Key **identifiers**, unlike secret values, appear in AWS `delete-access-key` argv during revocation
and can be observed by local process inspection. The script's own output names neither field.

`WRANGLER_LOG=log` preserves version and secret-list stdout in pinned Wrangler; those outputs are
captured and parsed. `WRANGLER_WRITE_LOGS=false` prevents disk logs. The version must match
`backend/package.json`. `AWS_CLI_HISTORY_FILE` points below the null device so a history-enabled
CLI fails before it can persist key creation responses. Parent AWS, Cloudflare and Wrangler
settings cannot override these guards.

## Provider contract and policy

| Secret | Shape |
|---|---|
| `AWS_ACCESS_KEY_ID` | IAM access-key identifier |
| `AWS_SECRET_ACCESS_KEY` | IAM secret access key |
| `AWS_HOST_CONFIG` | JSON with `securityGroupId` and `subnetIds` (ordered list) only |

Region, instance type, image parameter and tags remain repo-owned facts, not duplicated network
configuration. The provider launches On-Demand with default tenancy, IMDSv2 (`HttpTokens=required`),
the declared production type, a selected subnet and the host group. Every instance, volume and
network interface needs both ownership tags. Additional launch tags are allowed. It must explicitly
send gp3, `VolumeSize` equal to the current image root size, `Iops=3000`, `Throughput=125` and
`DeleteOnTermination=true` for its root block-device mapping.

MEASURED on this lane: the public Ubuntu 26.04 amd64 image reports an **8 GiB gp3 root** through
`DescribeImages.BlockDeviceMappings`, matched to `RootDeviceName`. DERIVED: the policy's per-volume
size cap is that reported root size, the minimum root needed to boot this image. It is discovered
again on apply/verify, not a second authored disk size. The provider slice must confirm its
audio-free release fits that disk. The policy requires gp3 and caps every volume at that size,
**3000 IOPS and 125 MiB/s**, the documented gp3 included baselines. No `IfExists` weakens these
requirements: absent IAM context keys fail closed. Explicit launch fields avoid relying on EC2
populating context from omitted defaults; only the provider-key dry-run can prove that context.

The five allowed actions are `ec2:RunInstances`, `ec2:CreateTags`, `ec2:TerminateInstances`,
`ec2:DescribeInstances` and `ssm:GetParameters`. Launch permits Canonical-owned images; choosing the
current image belongs to the provider. SSM read is restricted to the public Canonical path owned
by `PARAMETER` in `host_setup.py`. Public parameter ARNs have an empty account field, documented by
AWS and confirmed by the lane's read; execution of EC2's parameter resolution under the provider
key is still unproven. Native `resolve:ssm:` launch requires `ssm:GetParameters`.

Termination requires both resource tags, regardless of instance type, so allocation changes do not
strand older hosts. Describe includes all Singapore instances; AWS offers no tagged-only Describe.
CreateTags is permitted only as part of RunInstances, never for standalone retagging. No IAM, S3,
role passing, Spot, reboot, group management or standalone volume creation is granted.

IAM limits all user inline policies together to 2048 non-whitespace characters. Optional statement
IDs are omitted. Exact string conditions use `StringLike` with no wildcards, equivalent to equality;
the policy retains the current `2012-10-17` language version. Generated launch-resource ARNs
use an account wildcard: RunInstances creates these resources in the caller's account; the exact
subnet/group ARNs still bind the launch network to that account. The CreateTags resource wildcard
cannot authorize an independent tagging request; its creation-action condition and the launch
grants constrain it. The size check refuses before mutation if the complete subnet list cannot fit.

## Evidence and limits

MEASURED on this lane against AWS's custom-policy simulator: **60 checks passed, 15 allowed and
45 denied**, using an existing default security group as a stand-in, without creating resources.
MEASURED: three available offering subnets; the rendered policy uses 2043 non-whitespace characters.
These are template results, not an installed-user proof.

AWS simulation checks each required resource separately with explicit request context. It tests
positive launch/tag/termination/describe/parameter reads, every offered subnet, and refusals for
wrong/missing tags, another subnet/group/owner/type, Spot, dedicated tenancy, IMDSv1, non-gp3 or
oversized/overprovisioned volumes, missing constrained context, standalone tagging, foreign-region
resources and ungranted IAM/S3/EC2 actions. Only `allowed`, `implicitDeny`, `explicitDeny` are accepted
simulator decisions; unknown or malformed results are refused, never counted as evidence.

Simulation proves policy evaluation **for the supplied context**, not that EC2 supplies it or a
server can launch. `verify` runs installed-principal simulation only after apply; until the
orchestrator runs it, that installed proof remains unmeasured. The provider slice must dry-run both
concrete and `resolve:ssm:` image requests with its own key, including explicit gp3 baseline fields,
default tenancy, IMDSv2 and tags on all new resources, before creating a server. No `verify --with-key`
is offered: AWS CLI would require exposing credentials outside the existing Wrangler-stdin pipe.
The Worker that already holds the key owns that proof. Actual handoff, rotation, launch and removal
remain unmeasured against live resources on this lane.

**Open to a key thief:** volume count and `DeleteOnTermination` are not constrained, so repeated
launch/termination can leave billed gp3 volumes after revocation. User-data, egress and
API-termination protection are open; a thief can discard the shutdown guard, transfer data and
block provider/sweep termination. Neither the sweep nor key revocation guarantees shutdown or
volume deletion. Quotas are not a spend cap. The owner-set billing alert is the control for these
open costs; the owner must investigate and remove protected instances/volumes. No launch template
is introduced.

Official sources:

- [EC2 authorization](https://docs.aws.amazon.com/service-authorization/latest/reference/list_amazonec2.html): per-resource launch keys, tenancy, metadata, volume restrictions and tagging.
- [gp3 baselines](https://docs.aws.amazon.com/ebs/latest/userguide/general-purpose.html).
- [EBS launch fields](https://docs.aws.amazon.com/AWSEC2/latest/APIReference/API_EbsBlockDevice.html): optional performance fields and snapshot-derived size.
- [Public parameter ARN examples](https://docs.aws.amazon.com/systems-manager/latest/userguide/parameter-store-public-parameters-ami.html).
- [Native AMI parameter permissions](https://docs.aws.amazon.com/systems-manager/latest/userguide/parameter-store-ec2-aliases.html).
- [IAM string operators](https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_policies_elements_condition_operators.html): exact `StringLike` values have equality semantics.
- [Create-on-launch tagging](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/supported-iam-actions-tagging.html).

Local validation:

```sh
python3 -B scripts/aws/test_host_setup.py
python3 scripts/aws/test_harness.py
shellcheck scripts/aws/host-setup
```

Tests run the shipped entry point with recording AWS/Wrangler fakes, including the actual tags and
IAM path sent by the installer, and run the real installed pinned `wrangler --version` under the
script's child environment. No Rust changed; no cargo checks are needed.
