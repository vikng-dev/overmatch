# Swept CCD: controlled collapse experiment

Subject: Overmatch's environment destruction model, game state only.

Follow-up: the [same-binary live rehearsal](swept-ccd-live-collapse.md) closes the live causal
question: the arms do not measurably separate beyond CCD time. The unresolved attribution below
describes this controlled experiment's evidence at the time; its landing-blocker conclusion is
superseded by that live comparison.

## Decision at this stage

**Stop at diagnosis, the (c) path; the cause and its size remain unknown.** Neither (a)'s causal
claim nor (b)'s engine fix is supported. In the controlled six-house scene the gate removes sweep
work, but changes no time-of-impact correction, piece motion, contact record, structural result
or rest transition. The proposed accidental damping does not occur in this scene. Keep the lane's
existing gate provisionally; add no engine change or compensation. The earlier live-run rest
association remains unexplained and remains a landing blocker. These runs do not establish that
the original later rest is either more honest behaviour or an attributable cost for #379.

The [earlier live investigation](swept-ccd-collapse-diagnosis.md) remains an honest unpaired
observation. This experiment changes only gate participation in one binary; it does not replay
that live server's complete pre-contact state. There is no evidence here identifying a different
cause, or establishing whether a future fix would be small or large.

## What the engine does

In `vendor/avian3d-0.8.0-overmatch/src/dynamics/ccd/mod.rs:925–944`, a fraction below one sets the
impact flag, multiplies `delta_position` by the fraction, and interpolates `delta_rotation` from
identity to that fraction. **It truncates translation and rotation, without changing linear or
angular velocity.** The velocity fields are never assigned by that correction. In
`dynamics/solver/solver_body/plugin.rs:347–363`, writeback applies those deltas, including the
center-of-mass rotation correction, and independently copies the solver's velocities to the body.

At `ccd/mod.rs:948–962`, an already-existing impact pair can also receive an increased speculative
distance so the next discrete step detects its contact. Thus an effect on later energy could
arise through changed subsequent contacts; it is not an immediate CCD velocity damping impulse.
MEASURED in both arms: every recorded linear and angular velocity immediately before and after
CCD correction is identical, including all impact corrections.

The body query defaults an absent `SweptCcd` (`ccd/mod.rs:653`); the piece spawn bundle
(`src/construction/breakage.rs:3106–3107`) adds no override. `SweptCcd::new` and
`CcdFilter::DEFAULT` (`ccd/mod.rs:370–375,426–428`) select static and kinematic targets. Other
awake dynamic pieces therefore meet through ordinary contacts, not swept CCD; frozen pieces,
which have become static, can be swept.

## Scene, controls and determinism

MEASURED: two independent processes per arm, one instrumented edit-profile test binary built on
`1ae2212c`, with `dev_ui,bitprobe`. Both runtime arms used the same binary SHA-256:
`da2eb5a46c66b100983b48f3679df0fe2de4d1641e065189fce0b653e3fbc5e1`.
A temporary switch read once disabled only the per-collider gate. The body gate, supported-shape
filter, contact laws, CCD impact correction and all game laws were identical.

The ignored probe reused `construction::breakage::tests::composed`, directly mounted the shipped
crumble installer, and used the shipped manifest and heightfield. It spawned only
`house_0007`–`house_0012`, at their admitted poses and terrain heights, in manifest order. It
removed their supports with the same `felling_lines`/`fell` path as `net::loadtest`, after three
fixed setup updates. It then stepped **1,280 fixed ticks at 64 Hz** (DERIVED: 20 simulation seconds).
`TimeUpdateStrategy::FixedTimesteps(1)` advanced exactly one fixed loop per update. No tanks,
clients, earlier workload events, other constructions, view loading or wall-clock event dispatch
participated. The fixture's compute pool had one thread in every process; it does not establish
parallel-server replay determinism or hosted performance. The clockless fixture keeps ledger
and piece-birth metadata at zero; scene rows carry the explicit stepped tick, and lifetime ages
use observed spawns rather than that metadata.

MEASURED determinism: all **1,280** simulation records matched between the two processes of each
arm, after excluding only timer fields and normalizing unordered diagnostic lists. This includes
piece identities, every pose/velocity/energy sample, CCD selections and results, collider ownership,
first-contact samples, the ledger and all rest transitions. Canonical SHA-256:

| Trace | Both independent processes |
|---|---|
| Gate on | `ef1136ea2711d95fb762a7ef7864625d3aeb36acae3258854fd1236bbce61719` |
| Gate off | `b1b2ccb65ad0a052242cc63038c1d1b52c785404e41a8697f6c9972c26fed0ab` |

Removing the swept/gated selection lists makes **both arms' complete simulation traces identical**:
`57b551fd104d54b5c9383ba987fb3ab8da825e499532b090b93e3dabc130dbd6`.
The TOI fractions, requested/removed travel, impact collider identities and before/after CCD
velocities remain in that comparison. No five-run fallback was needed.

Before this scene, two level-bed six-house runs were also identical for all 1,280 ticks, but had
zero fast-body visits, sweeps or truncations. They could not test the mechanism and are excluded
from the A/B evidence below. One earlier invocation failed at app setup before producing any
simulation record; its crumble installer was then called directly, as the existing fixture does.

All four map runs held `scripts/build-lock`; MEASURED admitted one-minute loads, in gate-on then
gate-off order: **3.061 / 2.907 / 2.637 / 2.819**, all at or below four. No compile overlapped the
runs. The runner used `OVERMATCH_UNATTENDED=1`; the headless test creates no window.

## Evidence

MEASURED, **two runs per arm**; all non-timing results below were identical within each arm.
Distances/angles are sums derived from recorded per-step displacements and rotations; sleep
seconds are derived from recorded ticks at 64 Hz. These observations are not test obligations.

| Observation | Gate off | Gate on |
|---|---:|---:|
| CCD-fast body/step visits | 7,371 | 7,371 |
| Swept collider/step visits | 201,458 | 104,488 |
| Gated collider/step visits | 0 | 96,970 |
| TOI truncations | 88 | 88 |
| Translation removed, m | 2.107119 | 2.107119 |
| Rotation removed, rad | 2.722764 | 2.722764 |
| Total center-of-mass path over all piece lifetimes, m | 302.229042 | 302.229042 |
| Total rotation over all piece lifetimes, rad | 229.507559 | 229.507559 |
| Pieces created / final survivors / retired parents | 357 / 266 / 91 | 357 / 266 / 91 |
| Final-sleep age p50 / p90 / worst, s | 3.40625 / 5.703125 / 5.859375 | 3.40625 / 5.703125 / 5.859375 |
| Final-freeze age p50, s | 3.984375 | 3.984375 |
| Wakes | 2 | 2 |
| Awake piece/ticks | 56,501 | 56,501 |
| Active touching pair/ticks | 1,202,205 | 1,202,205 |
| Active solver point/ticks | 1,981,886 | 1,981,886 |
| Peak awake / touching pairs / solver points | 255 / 5,461 / 9,482 | 255 / 5,461 / 9,482 |

Final-sleep ages describe surviving pieces' last completed awake episodes, measured from each
piece's first recorded spawn. The raw record preserves earlier episodes, all retired parents and
all freezes. MEASURED: the last survivor slept at scene tick **376** and froze at **377** in both
arms; all survivors were resting for the remainder of the capture. Parent travel through its
last physics step is included before the game's split pass retires it. Rotation uses quaternion
relative-angle accumulation, with identical quaternions contributing zero.

MEASURED first contacts: **9,289** first-observed collider pairs, **16,925** retained points,
identical point-for-point in both arms. Separation is minus retained point penetration; normal
impulse is the solver's recorded N s. DERIVED nearest-rank separation p10 / p50 / p90:
**−15.370 / +10.775 / +21.562 mm**. First-contact records are keyed by persistent collider pair,
not every later recontact or collider rehome; they do not measure final post-solver surface gaps.

MEASURED local stage totals over all recorded ticks, seconds, with both runs shown. These include
temporary instrumentation, use one compute thread and the edit profile, and are not hosted prices.
The solver column sums preparation, warm start, biased solve, relaxation and impulse storage.

| Arm/run | Narrow phase | Solver | CCD |
|---|---:|---:|---:|
| Gate off 1 | 4.828433 | 1.577688 | 0.199133 |
| Gate off 2 | 4.920088 | 1.625475 | 0.211673 |
| Gate on 1 | 4.858343 | 1.577099 | 0.129793 |
| Gate on 2 | 4.819693 | 1.576539 | 0.122136 |

## First difference and mechanism test

MEASURED: selection first differs at **scene tick 2**, on `house_0007` (construction ordinal
**641**), body bits **8589934023**, starting with collider bits **4294956122**, source chunk **9**.
That body has **902** chunks: the gate skips **701** and sweeps **201**, while the ungated arm
sweeps all **902**. Both return fraction **1**, with no impact. These identifiers describe this
fixture record; no game logic or gate uses them.

There is **no first physical divergence**. Every later TOI result also matches. The extra
ungated collider sweeps never reduce the body's earliest TOI. Consequently the difference in
removed travel is **zero**, the difference in actual path and rotation is **zero**, and the energy
trajectories, contact samples, ledger entries, piece partitions and rest episodes are identical.
There is no earlier changed break or split that could mask an extra truncation.

Avian's correction can suppress motion without lowering instantaneous kinetic energy, but this
record supplies no additional suppression caused by thin siblings. It therefore cannot explain
the earlier live-run medians or justify an energy-damping story. The wider live server state and
merged-source differences remain uncontrolled in those old observations. A causal attribution
would require the live pre-contact state that actually produces a divergent TOI result; this
six-house isolated scene does not produce one. No second hypothesis was promoted into a fix.

## Cleanup, merge and verification

The runtime arm, recording API and ignored probe were deleted before the merge. No observation
was turned into a regression oracle, and no permanent probe has a current user. The shipping
source is exactly the prior lane source plus main's changes: no new physics rule, sibling
coupling, parameter, margin, gate policy or body-specific branch was added.

Merged `main` tip `37d624e4` into the lane, preserving its tank-template, server-path ratchet and
capture-clock changes. The merge was automatic: neither `OVERMATCH_PATCH.md` nor
`upstream/README.md` conflicted; both sides' existing inventory and CCD draft entries survived.

MEASURED on the merged shipping tree: `cargo fmt --all --check` passed; CI clippy
(`scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe --
-D warnings`) passed. All targeted Rust invocations used `scripts/serial-cargo test --lib --locked
--features dev_ui,bitprobe` (edit profile):

| Filter | Passed | Existing ignored |
|---|---:|---:|
| `tests::avian` | 110 | 0 |
| `headless_test::track_scorecard` | 16 | 8 |
| `construction::breakage` | 80 | 1 |
| `debris::` | 39 | 0 |
| `net::debris_stress` | 19 | 0 |
| `swept_ccd_probe -- --ignored` | 1 | 0 |
| `tank::template::` | 5 | 0 |
| `dev_capture::` | 42 | 0 |

MEASURED Python checks: `scripts/cost/test_analyze.py` passed all **8** tests and
`scripts/perf/test_trace_profile.py` all **9**, through `unittest discover`. Both staged and
unstaged whitespace checks passed. No full suite was run locally. The independent archive
reconstruction check also passed. No source changed after these gates.

## Fork census and retained evidence

MEASURED anew on the merged shipping source, using per-file `diff --minimal -u` against cached
published source archives (including fork-only source):

| Crate | Added | Removed | Total |
|---|---:|---:|---:|
| Avian 0.8.0 | 707 | 168 | 875 |
| Bevy PBR 0.20.1 | 27 | 20 | 47 |
| Lightyear netcode 0.31.0 | 140 | 19 | 159 |
| Lightyear Avian 0.31.0 | 0 | 0 | 0 |
| All forks | 874 | 207 | 1,081 |

Avian has **797** changed lines in upstream files plus **78** fork-only lines; its CCD file
remains **18 added + 4 removed = 22**. Archive hashes and the complete per-file census are in
[swept-ccd-controlled-fork-lines.json](swept-ccd-controlled-fork-lines.json).

The [compressed record](swept-ccd-controlled-collapse-record.json.gz) retains recoverable per-tick
states and CCD records for both arms, every run's timers and load admission, per-piece lifetime
summaries, first contacts, collider ownership and ledger deltas. Repeated unchanged states are
encoded as deltas, with reconstruction rules in its header. The duplicate processes refer to
their arm's identical simulation trace and retain independent timer arrays. It contains evidence,
not a retained experiment switch or executable probe. An independent decoder reconstructed both
arm traces and reproduced all three canonical hashes above. MEASURED archive size: **23,477,076
bytes**; SHA-256: `0a7bc9340373517e596c0e7ae91556f933879a19c4be08723d7f5ad799e24ddb`.

Temporary source and runner artifacts remain outside the tree in `/private/tmp/ccd5-map-probe.rs`,
`/private/tmp/ccd5-instrumentation.patch`, `/private/tmp/ccd5-run.py` and `/private/tmp/ccd5/`.
Gate log: `/private/tmp/ccd5-gates.log`. No reset, stash, push, web access, external comment,
sub-agent or target-directory deletion was used. Independent six-lens landing review remains
with the orchestrator; the live-run causal question remains open.
