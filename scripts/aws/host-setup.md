# EC2 host setup

`scripts/aws/host-setup` owns the standing resources for the Overmatch multiplayer game's EC2
provider. It creates no server. The hosting design lives in
`.agents/scratch/aws-hosting-design-2026-11-17.md` in the owner's main checkout; this file owns the
command and provider interfaces, authorization evidence, and operational limits.

Run from the repository root, using the signed-in `overmatch` AWS profile and the existing backend's
Wrangler login. Setup reads the AWS account identity and refuses unless its runtime account name is
`Overmatch` (case insensitive). Identifiers stay in memory. No account identifier is a tracked input.
AWS CLI history must be disabled for this profile: the command deliberately makes a history-enabled
CLI fail before it can send a request. No configuration or credential file is opened by the script.

```sh
npm ci --prefix backend --ignore-scripts --no-audit --no-fund
export PATH="$PWD/backend/node_modules/.bin:$PATH"
scripts/aws/host-setup plan
scripts/aws/host-setup apply
scripts/aws/host-setup verify
```

- `plan`: local only. Lists the desired standing parts and secret names; no CLI or cloud calls.
- `apply`: reconciles the `overmatch-host` security group in the default VPC, IAM user of the same
  name at path `/overmatch/`, and its inline `overmatch-host` policy. Requires both ownership tags
  `Project=overmatch`, `Purpose=host` before adopting existing resources. Refuses extra user policies,
  group memberships, a permissions boundary or console access. Replaces drifted ingress with the
  server's single UDP port, parsed from `src/net/server.rs`; no SSH. Default outbound access stays
  available for release downloads and Worker reports. Reads the production type and vCPU allocation
  from `backend/production-server.json`, and refuses a mismatch with EC2. Uses the first available
  default subnet in availability-zone order. Writes the discovered provider configuration to the
  existing Worker. Creates and forwards a key only when the user has none; an unchanged rerun keeps
  its key. A missing Worker is refused before AWS creation. No boot package is installed or bundled.
- `verify`: creates nothing. Before setup, simulates the template against an existing default
  security group as a stand-in; after setup, uses `iam simulate-principal-policy` on the installed
  user. Tests every required launch resource separately, since EC2 requires authorization for all of
  them. Also performs concrete-image and SSM-image launch dry-runs with the operator profile. It
  explicitly labels those as operator proofs, never as execution with the provider's key.

For subsequent key replacement or complete removal:

```sh
scripts/aws/host-setup rotate-key
scripts/aws/host-setup teardown
```

`rotate-key` creates one replacement key, forwards both credential fields, then revokes the old
key. On any caught failure after creation, it reconciles key metadata and revokes the new key,
including when the create response was lost. A revocation failure is an explicit emergency refusal.
The two `secret put` operations are separate remote writes: failure can leave a mismatched Worker
pair even though the new key has been revoked. Rerun `rotate-key` to restore the pair; do not enable
the provider until that completes. Existing keys are retained until both puts succeed. Two existing
keys require operator reconciliation before another can be created.

`teardown` refuses until the provider's tagged instances, volumes, interfaces and key pairs have
been drained; it never terminates a match as a side effect of setup removal. It revokes user keys,
deletes its Worker secrets, inline policy, user and security group, then proves that its secret
names, IAM user and both-tagged EC2 resources are absent. Terminated EC2 tombstones do not count as
live resources. Interrupted removal is restartable; unrelated resources and Worker secrets survive.
Neither rotation nor teardown depends on the image parameter, subnet or production type remaining
available.

The credential path is AWS stdout -> anonymous subprocess pipe -> in-memory forwarder -> Wrangler
stdin, with shell tracing disabled. Neither credential field is placed in a file, log, environment
variable, or Wrangler argument. Child output is captured even on failure. Wrangler disk logs and
metrics are disabled, log sanitization is forced, inherited output-file settings are removed, and
core dumps are disabled. The Wrangler version must match `backend/package.json`; its pinned source
provides `WRANGLER_WRITE_LOGS=false`. AWS CLI's `AWS_CLI_HISTORY_FILE` points below the null device so
history cannot initialize if a profile enables it. Parent AWS credential overrides are removed.

## Provider contract

The next provider slice reads these exact secret bindings from the existing backend deployment
(`backend/wrangler.jsonc`). They are installed now; adding their typed Worker reads belongs to that
slice, without changing the current Edgegap provider or its required secrets here.

| Secret | Shape |
|---|---|
| `AWS_ACCESS_KEY_ID` | IAM access-key identifier |
| `AWS_SECRET_ACCESS_KEY` | IAM secret access key |
| `AWS_HOST_CONFIG` | JSON object with `region`, `instanceType`, `subnetId`, `securityGroupId`, `imageId` |

`imageId` is `resolve:ssm:` followed by
`/aws/service/canonical/ubuntu/server/26.04/stable/current/amd64/hvm/ebs-gp3/ami-id`.
The parameter is public; its ARN has an **empty account field**, verified by a live read. Setup
reads that public parameter and its image to discover Canonical's owner for the policy, rather
than tracking an account identifier. The provider must request the two ownership tags on each
instance, volume and network interface. Additional launch tags, such as version, are permitted.
It must use the configured subnet and security group. It needs no group/subnet discovery actions.

## Policy and evidence

Official sources consulted for each condition and resource type:

- [EC2 Service Authorization Reference](https://docs.aws.amazon.com/service-authorization/latest/reference/list_amazonec2.html):
  `RunInstances` authorizes image, instance, security-group, subnet, network-interface and volume
  resources independently. `ec2:Owner` belongs to the image check. `ec2:InstanceType` and
  `ec2:InstanceMarketType` belong to the instance check. `aws:RequestTag/Project` and
  `aws:RequestTag/Purpose` belong to new instances, volumes and network interfaces. `CreateTags`
  supports `ec2:CreateAction`; `TerminateInstances` supports instance type and resource tags.
  `DescribeInstances` supports `ec2:Region`, with no resource-level scope. Regional resource ARNs
  restrict every other grant to Singapore. The exact security-group and subnet ARNs enforce the
  network restriction without relying on cross-resource condition context.
- [Tagging on creation](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/supported-iam-actions-tagging.html):
  the separate `CreateTags` grant is conditional on `ec2:CreateAction=RunInstances`; standalone
  retagging stays denied.
- [SSM Service Authorization Reference](https://docs.aws.amazon.com/service-authorization/latest/reference/list_awssystemsmanager.html):
  `GetParameters` supports a specific parameter ARN.
- [Native AMI parameter support](https://docs.aws.amazon.com/systems-manager/latest/userguide/parameter-store-ec2-aliases.html)
  explicitly says the caller of `run-instances` with `resolve:ssm:` must have `ssm:GetParameters`.
  The policy grants it only for the one public Canonical path above, with no listing or other SSM
  action. [AMI parameter permissions](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/using-systems-manager-parameter-to-find-AMI.html#systems-manager-permissions)
  also document this read for the launch wizard.
- [Public AMI launch syntax](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/finding-an-ami-parameter-store.html)
  documents `resolve:ssm:`.

The five allowed actions are `RunInstances`, `CreateTags`, `TerminateInstances`, `DescribeInstances`
and `ssm:GetParameters`. No IAM, S3, role passing, reboot, group management or standalone volume
creation is granted. Launch is On-Demand only, using Canonical images, the declared type and network,
and both request tags. Termination requires the declared type and **both** resource tags. Describe
necessarily includes untagged instances in Singapore; AWS cannot implement tagged-only Describe.

MEASURED on this lane with AWS's custom-policy simulator, without creating a user or security group:

| Case | Result |
|---|---|
| Every required launch resource | allowed separately |
| Create tags on instance, volume and interface during launch | allowed |
| Declared-type termination with both ownership tags | allowed |
| Describe in Singapore; read the one public parameter | allowed |
| Other type, Spot, other image owner, other security group | denied |
| Missing either request tag on any new resource | denied |
| Standalone tagging of instance, volume or interface | denied |
| Terminate missing either ownership tag, or with another type | denied |
| Other-region launch resources, termination and Describe | denied |
| Another parameter, IAM listing/creation/PassRole, S3 listing/write | denied |
| Security-group creation, standalone volume creation, reboot | denied |

MEASURED: both concrete-image and `resolve:ssm:` **operator** launch requests returned
`DryRunOperation`. These establish that AWS accepts the launch recipe and public parameter under
the operator's existing permissions. They do not test the future user's permissions. The simulator
results establish the template's intended authorization; neither mocks nor simulation are claims
that an actual instance launched.

SSM answer: **yes, grant `ssm:GetParameters`**, as AWS's native AMI parameter documentation requires
for the `run-instances` caller. Its note appears directly before the `resolve:ssm:` launch example
(although its last clause confusingly calls the failure a "parameter creation" failure). EC2's
authorization reference omits that dependency. The permitted dry-runs establish success with the
grant; they cannot independently demonstrate failure without it because the operator already has
SSM access. Removing it from a custom simulation cannot test EC2's internal resolution; that
empirical comparison requires a restricted principal, which cannot be created in this lane.

Design corrections found: its "three-action policy" actually lists four EC2 actions and one SSM
read; its `Purpose=match` becomes the brief's `Purpose=host`; Describe cannot be tag limited;
instance type was absent from `production-server.json` and is now declared there; the network
identifiers need a runtime provider handoff, supplied by `AWS_HOST_CONFIG`; audio bundling is
superseded by the owner's audio-free release. The current-image lookup is a provider contract, not
an IAM guarantee: the policy permits Canonical-owned images, as the design requests.

Still unverified without provisioning: installed-principal simulation, actual execution under its
key, key creation/rotation and Cloudflare secret writes, actual tagged/untagged termination, and
teardown against real resources. The orchestrator runs apply and then verify after review. The
future provider's live tests must cover actual lifecycle behavior.

Local validation:

```sh
python3 -B scripts/aws/test_host_setup.py
shellcheck scripts/aws/host-setup
python3 scripts/aws/test_harness.py
```

Tests execute the shipped shell/Python entry point with fake AWS and Wrangler on PATH. They prove
argument refusal before any CLI call, account/ownership refusal, retries, ingress correction from
the server's port owner, provider-key preservation, revocation on a lost response or failed handoff,
secrecy even with a CLI that echoes secrets, policy-negative verification, and drained teardown.
The fakes intentionally write credential logs if logging guards are removed, so the secrecy test
catches that regression too. No Rust changes or cargo checks are involved.
