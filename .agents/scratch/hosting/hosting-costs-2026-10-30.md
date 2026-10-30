# Dedicated-server hosting costs: Edgegap vs AWS (and two contrast hosts)

Read date for every source below: **2026-10-30**, unless a source date is stated with it.
Labels: **SOURCE** = figure read from the cited primary source; **DERIVED** = computed here from
SOURCE figures and the repo facts; **UNCONFIRMED** = looked for and not found in a primary source.

## Inputs (from the repo, as given)

| Fact | Value | Label |
|---|---|---|
| Downstream per client at 30 tanks | ~235 kB/s | DERIVED (repo) |
| Egress of one 30-player match | 235 kB/s × 30 = **7.05 MB/s = 56 Mbit/s** | DERIVED |
| Egress per match-hour | 7.05 MB/s × 3600 s = **25.4 GB** (decimal GB; 23.6 GiB) | DERIVED |
| Egress per 20-minute match | **8.46 GB** | DERIVED |
| Egress at 1,000 match-hours a month | **25,380 GB ≈ 25.4 TB** | DERIVED |
| CPU per match | 1 vCPU; one EPYC 7713 vCPU saturates at 23–26 tanks | repo, MEASURED |

The repo's "~25 GB per match-hour" checks out: 25.4 GB.

**One capacity caveat comes before any price.** At 30 tanks the server is past what one Edgegap
EPYC 7713 vCPU carries, which is 23–26 tanks. Every row below still prices **one vCPU or core per
match**. On the newer cores (Zen 4/5 at 3.7+ GHz, Graviton 3/4), one core per match may hold 30
tanks, but nobody has measured it. If a match needs two, compute doubles, and the egress
conclusion below does not move.

## 1. Edgegap

| Item | Figure | Source |
|---|---|---|
| Edge Cloud (on demand) | **$0.00115 per vCPU per minute** (= $0.069/vCPU-h); "Billed by the Minute, Only on Usage"; "0 active servers = $0"; sizes "0.25 to 4 vCPU" | [edgegap.com/pricing](https://edgegap.com/pricing) |
| Egress | **"$0.10/GB of monthly Egress"** | same |
| Minimum | $1 minimum in any month with at least one deployment | same |
| Free trial | $0, no card; up to 1.5 vCPU / 3 GB, one concurrent deployment, 60-min uptime cap | same; 1.5 vCPU / 3 GB also in [docs](https://docs.edgegap.com/unity) |
| Private Fleet (bare metal, reserved) | $280/host/month (12-month) or $350 month-to-month; "16 vCPU/host total (14.7 usable)"; **"Incl. 6 TB per host, per month"** egress, with overage "billed at the public cloud rate" (the page gives no figure for that rate) | [edgegap.com/pricing](https://edgegap.com/pricing) |
| Enterprise | "Up to 40% discount on monthly cloud spend" for qualified minimums | same |
| Memory | Included at a fixed 2:1 RAM:vCPU (0.25 vCPU → 512 MB); 1024 units = 1 vCPU | [docs: Apps and Versions](https://docs.edgegap.com/learn/orchestration/application-and-versions) |

Both scripted claims hold: **$0.069/vCPU-h and $0.10/GB are confirmed**.

**What an Edgegap "vCPU" is: UNCONFIRMED.** Neither the pricing page nor the docs say whether it is a
thread or a core. The docs say only "AMD/Intel CPUs with clock speed up to 3.7 GHz, varying by
location". Edgegap's marketing glossary ([edgegap.com/glossary/vcpu](https://edgegap.com/glossary/vcpu))
says that "on most cloud hardware" a vCPU is one hardware thread, but it does not say this of
Edgegap. The EPYC 7713 our test ran on is an SMT part (64 cores / 128 threads), so a vCPU there
*may* be a hyperthread.

## 2. AWS

### Price sources (all AWS-published)

- **EC2 On-Demand:** the feed behind [aws.amazon.com/ec2/pricing/on-demand](https://aws.amazon.com/ec2/pricing/on-demand/):
  `https://b0.p.awsstatic.com/pricing/2.0/meteredUnitMaps/ec2/USD/current/ec2-ondemand-without-sec-sel/<Location>/Linux/index.json`
  for *Asia Pacific (Singapore)* and *US East (N. Virginia)*. The feed's publication date is 2026-10-29T17:56Z.
- **EC2 Spot:** the feed behind [aws.amazon.com/ec2/spot/pricing](https://aws.amazon.com/ec2/spot/pricing/):
  `https://website.spot.ec2.aws.a2z.com/spot.json` (last-modified 2026-10-30 12:15 GMT). Spot
  prices move. These are a **snapshot**.
- **Savings Plans (Price List API):** `https://pricing.us-east-1.amazonaws.com/savingsPlan/v1.0/aws/AWSComputeSavingsPlan/20261029175641/{ap-southeast-1,us-east-1}/index.json`
- **Data transfer (Price List API):** `https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AWSDataTransfer/20260916132208/{region}/index.json`
- **GameLift (Price List API):** `https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AmazonGameLift/20261013153044/{region}/index.json`
- **Fargate (Price List API):** `https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AmazonECS/20261019224317/{region}/index.json`
- **Public IPv4 (Price List API):** `https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AmazonVPC/20261016224321/{region}/index.json`: **$0.005 per in-use public IPv4 address per hour**, in both regions.
- **Billing granularity:** EC2 Linux bills "by the hour or second (minimum of 60 seconds)" ([EC2 On-Demand page](https://aws.amazon.com/ec2/pricing/on-demand/)).

### EC2 families: vCPU type, prices ($/instance-hour, Linux)

The thread-per-core values come from [docs.aws.amazon.com/ec2/latest/instancetypes/co.html](https://docs.aws.amazon.com/ec2/latest/instancetypes/co.html).

| Family | CPU | vCPU = | Size | OD SG | OD USE1 | Spot SG | Spot USE1 | Compute SP 1yr NU SG / USE1 | Compute SP 3yr NU SG / USE1 |
|---|---|---|---|---|---|---|---|---|---|
| c7a | AMD EPYC 9R14 | **full core** (1 thread/core) | medium (1 vCPU) | 0.05917 | 0.05132 | 0.0210 | 0.0139 | 0.04491 / 0.03709 | 0.03065 / 0.02478 |
| c7i | Intel Sapphire Rapids | **hyperthread** (2 threads/core) | large (2 vCPU = 1 core) | 0.1029 | 0.08925 | 0.0519 | 0.0267 | 0.0781 / 0.0645 | 0.0533 / 0.04309 |
| c7g | Graviton3 (ARM) | **full core** | medium | 0.0417 | 0.0363 | 0.0169 | 0.0113 | 0.0316 / 0.0260 | 0.0216 / 0.0176 |
| c8g | Graviton4 (ARM) | **full core** | medium | 0.04582 | 0.03988 | 0.0202 | 0.0203 | 0.03478 / 0.02859 | 0.02373 / 0.01931 |
| c8a | AMD EPYC 9R45 | **full core** | medium | not in SG feed | 0.05389 | — | 0.0128 | — / 0.03894 | — / 0.02602 |
| c8i | Intel Granite Rapids | **hyperthread** | large (1 core) | 0.10805 | 0.09371 | 0.0486 | 0.0349 | 0.08201 / 0.06772 | 0.05597 / 0.04525 |

Larger sizes cost exactly in proportion to vCPU count. The 16xlarge, for example, is 64× the
medium price in On-Demand: c7a.16xlarge costs $3.78688 in SG and $3.28448 in USE1.

**Matches per instance (DERIVED, at one core per match):**
- **Full-core families (c7a, c8a, c7g, c8g):** one match per vCPU. A medium holds 1, a large 2, an
  xlarge 4 and a 16xlarge 64.
- **Hyperthread families (c7i, c8i):** one match per *two* vCPUs (a whole core). A large holds 1 and
  a 16xlarge 32. Running 64 on a 16xlarge, one per thread, would share each core with a second
  match; that contention is unmeasured.
- **Memory** is not the limit: 2 GiB per vCPU against the measured 230–370 MB.
- **Network** is not the limit either:
  - A medium's *baseline* bandwidth is 0.39 Gbit/s (c7a) to 0.52 Gbit/s (c7g/c8g/c8a) (SOURCE,
    co.html), against 0.056 Gbit/s per match.
  - A 16xlarge has 25–30 Gbit/s, against 64 × 0.056 = 3.6 Gbit/s.

**Savings Plans** are a 1- or 3-year $/hour commitment. For per-match on-demand servers, they pay
off only on the always-on floor of concurrency. 1,000 match-hours a month is 1.4 matches running
on average (DERIVED, 1000/730 h).

### Amazon GameLift Servers

- **Managed EC2 and container fleets** bill per instance-hour. In the Price List the instance-hour
  includes 50 GB EBS. **Since 2026-06-15, network bandwidth is free on generation-6-and-later
  instances, On-Demand and Spot**, in all GameLift regions except China. There is no enrollment.
  - Docs: "Network bandwidth is free for all instance types from generation 6 and later" ([pricing docs](https://docs.aws.amazon.com/gameliftservers/latest/developerguide/gamelift-intro-pricing.html)).
  - FAQ: "All traffic between your game servers and connected players in and out of AWS is included
    at no additional charge" ([FAQ](https://aws.amazon.com/gamelift/servers/faqs/)).
  - Announcement: [what's new, 2026-06-15](https://aws.amazon.com/about-aws/whats-new/2026/06/amazon-gamelift-servers-free-network-bandwidth/).
  - The Price List descriptions agree: e.g. "$0.15023 per GameLift On Demand Linux c7a.large Instance
    Hour with 50 GB EBS and free network bandwidth in Asia Pacific (Singapore)".
  - The benefit "applies only to eligible Amazon GameLift Servers game server workloads", not to
    EC2 or EKS (FAQ).
  - **No fair-use cap is stated on the pages read.**

**GameLift On-Demand vs EC2 On-Demand, same instance (DERIVED from the two price lists):**

| Instance | GameLift SG / USE1 ($/h) | EC2 SG / USE1 ($/h) | Premium |
|---|---|---|---|
| c7a.medium | 0.07922 / 0.068 | 0.05917 / 0.05132 | 1.34× / 1.33× |
| c7i.large | 0.132 / 0.114 | 0.1029 / 0.08925 | 1.28× / 1.28× |
| c7g.medium | 0.058 / 0.050 | 0.0417 / 0.0363 | 1.39× / 1.38× |
| c8g.medium | 0.063 / 0.055 | 0.04582 / 0.03988 | 1.37× / 1.38× |
| c8a.medium | — / 0.07152 | — / 0.05389 | — / 1.33× |

On compute alone, GameLift costs **about 1.3–1.4× EC2**. With its bandwidth free, it is far cheaper per match.

**GameLift Spot:** the FAQ gives "savings of between 50% and 85%" and says GameLift Spot "never
exceed[s] … GameLift On-Demand". It adds "two minutes of notification" before an interruption and
that "you will be charged the price set at the beginning of each hour".

The [instance-pricing page](https://aws.amazon.com/gamelift/servers/pricing/instance-pricing/)
renders its Spot table as `price.USD × 1.2` from a Spot price feed. I read that formula in the
page's HTML, and it is **not a stated pricing rule**. Hence the DERIVED row below: GameLift Spot ≈
EC2 Spot × 1.2.

**FleetIQ** (GameLift managing Spot in *your own* EC2/ASG):
- An add-on fee per instance-hour on top of the EC2 price. For example, SG c6g.medium costs $0.008
  of fee against $0.0392 for the EC2 On-Demand instance, and c5.large $0.020 against $0.098: about
  +20% of the On-Demand price (DERIVED).
- The Price List carries FleetIQ rates only for c4, c5, c5a, c6g, m4, m5, m5a, m6g, r4, r5, r5a and
  r6g. There are **none for c7/c8**.
- **Whether free bandwidth covers FleetIQ instances: UNCONFIRMED.** The FAQ excludes "general AWS, EC2".

**Anywhere** (GameLift orchestrating servers you host anywhere):
- $0.0012 per game session placed. The first 3,000 a month are $0.00, per the Price List line.
- $0.000011 per server-process connection minute. The first 500K minutes a month are $0.00.
- 1,000 match-hours a month is 3,000 sessions and 60,000 process-minutes, which falls **inside those
  free lines** (DERIVED).
- Whether those free lines expire after 12 months: UNCONFIRMED.
- Your own host's egress still applies.

**Integration cost:**
- The server SDK exists for **C++, C# and Go, with no Rust**.
- Alternatively, the "game server wrapper" deploys a server "without … integrat[ing] the server SDK".
- Managed AMIs are Amazon Linux 2023. Container fleets are Linux only.
- Source: [GameLift dev tools docs](https://docs.aws.amazon.com/gameliftservers/latest/developerguide/gamelift-supported.html).

**Fleet idle cost:**
- GameLift bills every running instance, whether or not it hosts a session.
- One always-on c7g.large (2 match slots) adds 0.108 × 730 = **$79/month in SG** (DERIVED) per location.

### ECS / Fargate

**Price:**
- x86 Fargate is $0.05056 per vCPU-h and $0.00553 per GB-h in SG; $0.04048 and $0.004445 in USE1.
- ARM Fargate is $0.04045 and $0.00442 in SG; $0.03238 and $0.00356 in USE1 (Price List).
- A 1-vCPU task needs at least 2 GB: "Min. 2 GB and Max. 8 GB". The page bills "per second with a
  1-minute minimum".
- Fargate Spot is "up to a 70% discount".
- Data transfer is at standard rates, plus public IPv4 per task.
- Source: [Fargate pricing](https://aws.amazon.com/fargate/pricing/).

**Feasibility:**
- UDP works on Fargate:
  - Through an NLB, since [2020-07-31](https://aws.amazon.com/about-aws/whats-new/2020/07/aws-fargate-for-amazon-ecs-now-supports-udp-load-balancing-with-network-load-balancer/) (platform version 1.4).
  - Or through a task's own public IPv4, which the pricing page confirms exists.
- The CPU model cannot be chosen beyond x86 or ARM, and whether a Fargate vCPU is a core or a
  thread is **UNCONFIRMED**.
- The task cold-start time is unmeasured.
- Fargate earns nothing over EC2 here: it costs about the same, and the egress is identical.

### Local Zones and Wavelength

**Local Zones:**
- The Local Zones under the Singapore parent are **Bangkok, Hanoi and Manila**. Delhi and Kolkata
  hang off Mumbai and Taipei off Tokyo; Perth and Auckland off Sydney
  ([locations](https://aws.amazon.com/about-aws/global-infrastructure/localzones/locations/)).
- Their instance menus are thin. From the On-Demand feeds for those Local Zone locations:
  - Bangkok: c5 only (c5.xlarge $0.274, i.e. $0.0685/vCPU-h).
  - Manila: c5 (c5.2xlarge $0.470).
  - Hanoi: c7i at the SG price (c7i.large $0.1029).
- Egress in Bangkok and Manila is **$0.12/GB, the same as Singapore** (Price List). They buy
  latency, not cost.
- GameLift in Local Zones supports only "C5d 2xlarge and R5d 2xlarge" (FAQ). Those are generation-5
  instances, so the gen-6+ free-bandwidth rule would **not** cover them (DERIVED; not stated outright).

**Wavelength: not relevant.**
- A carrier gateway "allows inbound traffic from a carrier network". There is "no inbound connection
  configuration from the internet to a Wavelength Zone" ([docs](https://docs.aws.amazon.com/wavelength/latest/developerguide/how-wavelengths-work.html)).
  PC players on home broadband therefore cannot connect in.
- The only Asian zones are KDDI's in Tokyo and Osaka ([locations](https://aws.amazon.com/wavelength/locations/)).

## 3. Egress: the price that decides it

**Data transfer out to the internet** (Price List; inbound is $0.000/GB):

| | First 10 TB | Next 40 TB | Next 100 TB | Over 150 TB |
|---|---|---|---|---|
| ap-southeast-1 (also Bangkok/Manila LZ) | $0.120/GB | $0.085 | $0.082 | $0.080 |
| us-east-1 | $0.090/GB | $0.085 | $0.070 | $0.050 |

The first 100 GB a month are free, "aggregated globally" across all services. Edgegap charges a
flat $0.10/GB. GameLift charges **$0** on gen-6+ instances.

**Per match-hour (DERIVED):**

| Provider | Egress | Compute | Egress / compute |
|---|---|---|---|
| Edgegap | 25.4 GB × $0.10 = **$2.54** | $0.069 | **37×** |
| AWS EC2, SG (c7g.medium) | 25.4 GB × $0.12 = **$3.05** | $0.042 | **73×** |
| AWS EC2, USE1 (c7g.medium) | 25.4 GB × $0.09 = **$2.28** | $0.036 | **63×** |

The repo's claim holds: **on Edgegap, plain EC2 and Fargate, egress is about 97–98% of the bill.**
The one AWS product where that flips is GameLift managed fleets on gen-6+ instances: there, egress
is $0.

## Cost table: one 30-player, 20-minute match, and 1,000 match-hours a month

Everything in this table is DERIVED from the SOURCE rates above, under these assumptions:
- One match per instance-core.
- 100% utilisation, so idle fleet capacity, boot time and between-match gaps are excluded. Real
  bills run higher, by the same additive amount for every option.
- EC2 and Fargate rows add $0.005/h public IPv4 per instance.
- The 20-minute match uses the first-tier egress price. The monthly figure uses the tiered price,
  less the 100 GB free.

| Option | Compute $/match-h (SG / USE1) | One 20-min match, SG: compute + egress = total | One 20-min match, USE1 | 1,000 match-h/month, SG | 1,000 match-h/month, USE1 |
|---|---|---|---|---|---|
| **Edgegap Edge Cloud, 1 vCPU (list)** | 0.0690 (one price, all locations) | $0.023 + $0.85 = **$0.87** | same | $69 + $2,538 = **$2,607** | same |
| EC2 On-Demand c7a.medium | 0.0592 / 0.0513 | $0.021 + $1.02 = **$1.04** | $0.019 + $0.76 = **$0.78** | $64 + $2,507 = **$2,571** | $56 + $2,200 = **$2,256** |
| EC2 On-Demand c7i.large (1 core) | 0.1029 / 0.0892 | $0.036 + $1.02 = **$1.05** | $0.031 + $0.76 = **$0.79** | $108 + $2,507 = **$2,615** | $94 + $2,200 = **$2,294** |
| EC2 On-Demand c7g.medium (ARM) | 0.0417 / 0.0363 | $0.016 + $1.02 = **$1.03** | $0.014 + $0.76 = **$0.78** | $47 + $2,507 = **$2,554** | $41 + $2,200 = **$2,241** |
| EC2 On-Demand c8g.medium (ARM) | 0.0458 / 0.0399 | $0.017 + $1.02 = **$1.03** | $0.015 + $0.76 = **$0.78** | $51 + $2,507 = **$2,558** | $45 + $2,200 = **$2,245** |
| EC2 On-Demand c8a.medium | — / 0.0539 | not offered in SG | $0.020 + $0.76 = **$0.78** | — | $59 + $2,200 = **$2,259** |
| EC2 Spot c7a.medium (snapshot) | 0.0210 / 0.0139 | $0.009 + $1.02 = **$1.02** | $0.006 + $0.76 = **$0.77** | $26 + $2,507 = **$2,533** | $19 + $2,200 = **$2,219** |
| EC2 Spot c7g.medium (snapshot) | 0.0169 / 0.0113 | $0.007 + $1.02 = **$1.02** | $0.005 + $0.76 = **$0.77** | $22 + $2,507 = **$2,529** | $16 + $2,200 = **$2,216** |
| EC2 Spot c7i.large (1 core, snapshot) | 0.0519 / 0.0267 | $0.019 + $1.02 = **$1.03** | $0.011 + $0.76 = **$0.77** | $57 + $2,507 = **$2,564** | $32 + $2,200 = **$2,232** |
| EC2 Compute SP 1yr no-upfront c7g.medium | 0.0316 / 0.0260 | $0.012 + $1.02 = **$1.03** | $0.010 + $0.76 = **$0.77** | $37 + $2,507 = **$2,544** | $31 + $2,200 = **$2,231** |
| EC2 Compute SP 3yr no-upfront c7g.medium | 0.0216 / 0.0176 | $0.009 + $1.02 = **$1.02** | $0.008 + $0.76 = **$0.77** | $27 + $2,507 = **$2,534** | $23 + $2,200 = **$2,223** |
| **GameLift On-Demand c7a.medium** | 0.0792 / 0.0680 | $0.026 + $0 = **$0.03** | $0.023 + $0 = **$0.02** | **$79** | **$68** |
| **GameLift On-Demand c7g.medium (ARM)** | 0.0580 / 0.0500 | $0.019 + $0 = **$0.02** | $0.017 + $0 = **$0.02** | **$58** | **$50** |
| **GameLift On-Demand c8g.medium (ARM)** | 0.0630 / 0.0550 | $0.021 + $0 = **$0.02** | $0.018 + $0 = **$0.02** | **$63** | **$55** |
| GameLift On-Demand c7i.large (1 core) | 0.1320 / 0.1140 | $0.044 + $0 = **$0.04** | $0.038 + $0 = **$0.04** | **$132** | **$114** |
| GameLift Spot c7g.medium (EC2 Spot × 1.2) | 0.0203 / 0.0136 | **$0.01** | **$0.005** | **$20** | **$14** |
| Fargate x86, 1 vCPU / 2 GB | 0.0616 / 0.0494 | $0.022 + $1.02 = **$1.04** | $0.018 + $0.76 = **$0.78** | $67 + $2,507 = **$2,574** | $54 + $2,200 = **$2,254** |
| Fargate ARM, 1 vCPU / 2 GB | 0.0493 / 0.0395 | $0.018 + $1.02 = **$1.03** | $0.015 + $0.76 = **$0.78** | $54 + $2,507 = **$2,561** | $44 + $2,200 = **$2,244** |

**Edgegap Private Fleet (DERIVED):** one host covers 14 concurrent 1-vCPU matches and 6 TB. Since
25.4 TB is about 4.2 hosts' allowance, **5 hosts at $280–350 cost $1,400–1,750 a month for 30 TB**.
The alternative, 1 host plus 19.4 TB of overage at the unstated "public cloud rate", costs
~$2,290/month if that rate is $0.10.

## 4. Contrast: bundled-traffic hosts (brief)

**Hetzner Cloud CCX (dedicated vCPU, AMD)**
- Source: [hetzner.com/cloud/general-purpose](https://www.hetzner.com/cloud/general-purpose/), with
  Singapore traffic also on [cloud-singapore](https://www.hetzner.com/cloud-singapore/).
- CCX13 (2 vCPU):

  | Location | Price | Included traffic | Extra traffic |
  |---|---|---|---|
  | EU (NBG1/HEL1) | €43.49/month (€21.7 per vCPU-month) | 20 TB | **€1.00/TB** |
  | Singapore (SIN1) | €54.49/month (€27.2 per vCPU-month) | 1 TB | **€7.40/TB** (the Singapore page says "$8.49 per TB") |

- Whether a "dedicated vCPU" is a core or a thread: UNCONFIRMED.
- The page marked every CCX plan's "Create" button "currently unavailable" on the read date.
- In Singapore, 1,000 match-hours a month would be roughly 2 × CCX13 plus 23.4 TB × €7.40, about
  **€282/month** (DERIVED).

**Hetzner dedicated AX**
- Source: [matrix-ax](https://www.hetzner.com/dedicated-rootserver/matrix-ax/) and the [traffic docs](https://docs.hetzner.com/robot/general/traffic/).
- AX42 (Ryzen 7 PRO 8700GE, 8 cores): €79–99/month (≈€10–12 per core); AX102 (16 cores): €159–259.
- Traffic: "Traffic usage is unlimited and free of charge" on a 1 Gbit/s port, which carries about
  17 concurrent matches (DERIVED).
- Only German and Finnish data centres are named. **No Asian location was found.**

**OVHcloud (Singapore)**
- Advance-1 (EPYC 4245P, 6 cores / 12 threads): S$239.99/month (≈S$40 per core), 1–5 Gbit/s
  public, "High bandwidth and unlimited traffic"
  ([SG page](https://www.ovhcloud.com/en-sg/bare-metal/dedicated-server-singapore/)).
- The same site's bare-metal page, however, says "unlimited and unmetered traffic is enabled by
  default, **except in our Asia-Pacific datacentres**"
  ([bare-metal](https://www.ovhcloud.com/en-sg/bare-metal/)).
- **Whether Singapore traffic is unmetered: UNCONFIRMED (the sources conflict).**
- The GAME-1 (Ryzen 7 9800X3D, 8 cores / 16 threads, S$419.99/month, 1 Gbit/s unmetered) is
  "Deployable in 5 regions on 2 continents, including the US"
  ([game](https://www.ovhcloud.com/en-sg/bare-metal/game/)). Its APAC availability is UNCONFIRMED.

## Caveats and unconfirmed items

1. **GameLift's free bandwidth is four months old (2026-06-15)** and is the whole conclusion. No
   fair-use cap or end date is stated on the pages read. Re-read the FAQ before committing. Two
   coverage questions are UNCONFIRMED:
   - Whether it covers FleetIQ instances.
   - Whether GameLift charges public IPv4 on managed fleets.
2. **Rust**: GameLift has no Rust server SDK. Integration is through the game server wrapper, or
   FFI to the C++ SDK or a Go sidecar. This is engineering cost that the table omits.
3. **100% utilisation is assumed everywhere.** For GameLift and EC2, add the always-on floor per
   location: ~$79/month for one c7g.large in SG. Edgegap bills nothing when idle.
4. **Spot prices are a 2026-10-30 snapshot** and move. The GameLift Spot ×1.2 factor is read from
   page code, not stated as a rule.
5. **Capacity is unmeasured off the 7713.** 30 tanks exceeds one 7713 vCPU (23–26). The
   one-core-per-match rows are optimistic for the 7713 and untested on c7a, c8a and Graviton. ARM
   needs an aarch64 rebuild.
6. **Units:** decimal GB throughout. If a provider bills GiB, its egress figures drop about 7%.
7. **Edgegap vCPU (thread vs core): UNCONFIRMED.** The Edgegap Private Fleet overage rate is
   unstated.
8. **Hetzner and OVH prices are in EUR/SGD, excluding VAT/GST,** and have not been converted.
