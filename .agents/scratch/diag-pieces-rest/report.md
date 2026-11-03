Subject: the video game's environment destruction and damage model, game state only.

**Cause: residual angular motion and contact wakes hold whole islands awake; awake native pieces repeatedly check full construction weight. High confidence locally; the owner's incident remains unattributed.**

**Client panic:** the same-opening client hit `assert_input_buffer_ranges` while the server was full-speed. Its rising sync margin and panic are retained. The spread client exited normally. The owner's `ClientEntityInUse` means a request reached an existing connection; why it retried remains unknown. No server panic/exit occurred.

The owner log confirms DERIVED median empty-server speed .332 from MEASURED marker timestamps. The incident was **not reproduced**. MEASURED same-opening run: nine fire attempts, 35 pieces, full speed; normal round reset eventually cleared them. MEASURED spread run: twelve authoritative shots, initial APHE then eleven HE, only seven pieces; after DERIVED 634 s without a client, all slept. Neither run released a building-scale pile. HE's collapse threshold and the owner's dominant host cost remain unknown; the larger native probe required MEASURED four diagnostic 16 MJ construction inputs.

MEASURED Singapore EPYC, release `0.7.0-alpha.10`, one-core quota, pools 1/1/1. Counts are window medians; times are medians of period p50s. Shot windows DERIVED from adjacent log timestamps. Speed DERIVED Hz/64; shot 1 is the initial round, later shots HE.

| Stage | Awake/asleep | Tick / physics / send ms | Speed | RSS MB |
|---|---:|---:|---:|---:|
| Quiet tank | 0/0 | 3.87 / 0.48 / 0.34 | 1.00 | 204.1 |
| Shot 1 | 0/0 | 3.84 / 0.48 / 0.33 | 1.00 | 204.3 |
| Shot 2 | 0/0 | 3.74 / 0.49 / 0.33 | 1.00 | 204.5 |
| Shot 3 | 0/0 | 3.86 / 0.49 / 0.36 | 1.00 | 204.5 |
| Shot 4 | 2.5/2.5 | 4.25 / 0.57 / 0.34 | 0.91 | 210.6 |
| Shot 5 | 2/5 | 5.41 / 0.67 / 0.35 | 1.00 | 210.6 |
| Shot 6 | 2/5 | 5.23 / 0.69 / 0.34 | 1.00 | 210.6 |
| Shot 7 | 2/5 | 5.44 / 0.71 / 0.39 | 1.00 | 210.6 |
| Shot 8 | 1/6 | 4.86 / 0.66 / 0.35 | 1.00 | 210.6 |
| Shot 9 | 1/6 | 4.83 / 0.62 / 0.36 | 1.00 | 210.6 |
| Shot 10 | 1/6 | 4.82 / 0.64 / 0.35 | 1.00 | 210.6 |
| Shot 11 | 1/6 | 4.89 / 0.63 / 0.38 | 1.00 | 210.6 |
| Shot 12 | 1/6 | 4.93 / 0.65 / 0.36 | 1.00 | 210.6 |
| Empty +1 min | 1/6 | 3.60 / 0.48 / 0.21 | 1.00 | 210.6 |
| Empty +10 min | 0/7 | 2.60 / 0.36 / 0.18 | 1.00 | 210.6 |

DERIVED hosted empty-server non-physics cost is 2.23 ms versus .36 ms physics; no slow hosted episode was available to classify.

MEASURED Mac M4, `edit` profile, minimal flat-slab composition using production construction/release geometry. Times below: last one-second window before intervention and final window. They do not price the hosted release.

| Local probe | Awake/asleep | Update / physics ms | Weight calls / simulated s |
|---|---:|---:|---:|
| Native, before | 140/40 | 41.17 / 3.11 | 9,017 |
| Native, held asleep | 0/180 | .96 / .34 | 0 |
| Dense fixture, before | 934/66 | 7.70 / 7.41 | 0 |
| Dense fixture, held asleep | 0/1,000 | 1.10 / .85 | 0 |
| Sparse fixture, before (window medians) | 5/15 | .325 / .149 | 0 |
| Sparse fixture, held asleep (window medians) | 0/20 | .204 / .035 | 0 |

At the native pre-intervention snapshot, MEASURED 113/140 awake pieces had passed the default 0.5 s timer; nine exceeded 0.15 rad/s, none 0.15 m/s. Three islands held them, with no non-piece dynamic member. The dense fixture's largest island held MEASURED 930 bodies. Its spawn hulls are disjoint (DERIVED bounds versus lattice pitch); no tank, deposits or construction is needed.

Avian's new-contact path can wake an entirely sleeping/static pair. Merging/waking resets the whole island's timers: MEASURED native pre-intervention window, 160 contact-wake requests reset 1,900 body timers; force/game/pose/velocity wake counters were zero. **Proof:** forced island sleep alone failed; additionally suppressing contact wakes with no awake non-static body kept both same piles asleep for DERIVED 50 simulated seconds. Zeroing sleeping velocity for contact prediction failed; larger thresholds also failed. The guard alone's effect on automatic settling and wake-on-impact integration remain unmeasured.

Awake contacts, even native pieces on terrain, reach `apply_deposits` → `run_laws` → `fracture::weigh`: DERIVED about 35 ms/update locally. Sleeping contacts retain their press without starting deposits. The proving run confirms zero recurring weight calls; residual narrow-phase/census/rest-cache work remains.

Smallest candidates: correct the generic frozen-contact wake boundary (library-rest semantics; resting pieces stay still), then cache/reuse construction graph work (same player behaviour). Neither alone is priced for hosted real time. Coarser construction resolution gives fewer, larger visible pieces. A timed freeze/removal would add a piece-specific rule; the forced-sleep probe is not a shipping proposal.

All code changes reverted; patches and analysis in `evidence/`. Large logs stay in `target/diag-pieces-rest/`, listed by `evidence/summary.json`. Both deployments stopped; live profile count MEASURED zero. Validation: formatting, CI clippy, and MEASURED 48 required guard/bake tests passed.
