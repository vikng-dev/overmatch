# Rubble settlement: slice 0

Subject: Overmatch's environment destruction and driving physics; game state only.
Lane `lane/settle-probe`, base `ceac5e06734dc356d7340aeeef80855506196d77`.
Only this report is retained.

**Control: FAIL. Arm A: PASS. Arm B: PASS**, under the requested binary gate.
**These passes do not establish parked support on a driveable settled pile:** both
experimental arms stop the tank at the front of the pile and never reach the
control's parking position. The side-hit experiment also demonstrates a one-tick
wall in B. **Recommendation: build slice 1 changed**, with a new settled-target
sweep interface and finite-mass first contact as prerequisites, rather than treating
this binary pass as sufficient evidence to replace freeze/thaw.

All experimental positions, dimensions, durations and selection rules are **Fixture**
inputs. The parked gate is Fixture travel below 3 cm over 10 s **and** the hull's
island asleep at the end. All observations and counts below are **MEASURED**, with
three repetitions per arm/pose unless specified. Rates, percentages, differences,
unit conversions and accelerations from velocity differences are **DERIVED** from
those measurements. Configuration values and source statements are sourced facts.
No hosted cost was measured.

## Method and validity

Read `AGENTS.md`, the supplied design in the main checkout (read only), the
park-creep diagnosis on this branch, and the requested modules. Engine behavior
was checked in `vendor/avian3d-0.8.0-overmatch/`, including the rest hook, body-type
observers, contact graph, solver bodies, collider attachment and collider placement.
No agents, web search, GitHub writes, windows, sound or pushes.

The disposable full-app test reused `frozen_rubble::{flat_world,fell_house}` and the
shipped tank, house and material data. Global compute/async/IO pools were initialized
to 2/1/1 before boot. Every run used `OVERMATCH_UNATTENDED=1 OVERMATCH_MUTE=1`.
Builds used `scripts/serial-cargo test --lib --features dev_ui,bitprobe`, under its
`edit` profile. Physics retained the shipped substep configuration.

The diagnosis's recipe was rebuilt: house 40 m ahead, collapse for 10 s, wait up to
60 s for no awake loose pieces, drive toward the loose pieces' mean Z for up to
20 s, brake for 3 s using `transmission::park_engage_speed`, release both controls,
then observe 10 s. The three lateral house shifts were +0.05, -0.05 and -0.10 m.
The displacement starts immediately after braking, before the first parked tick.
A supplemental 12 s reverse drive follows the parked measurement.

The rebuilt red loop failed at `0.1012441 m`, awake, matching the diagnosis's first
pose exactly. The other two controls also reproduce its reported travel. The final
primary series contains 27 runs: three arms, three poses, three repetitions. For
each arm/pose, **every recorded state column across the entire run, including the
reverse extension, is identical across repetitions**; wall-clock costs differ.

A separate nine-run control audit reads shipped `Stages` freeze/thaw counters.
Its state traces match the primary controls exactly, including the parked and
reverse windows. Those audit runs supply control churn, not the cost table.
Earlier A/control screens also repeated deterministically. An incomplete B screen
hit a disposable bookkeeping bug after a split retired a piece; it is excluded.
The corrected probe drops retired owners before judging retained rests. A corrected
B screen ran alongside the side-hit test; its timing is excluded too.

## What the disposable arms did

Control retains shipped freeze/thaw. In **both A and B**, early returns at the entry
of both `debris::freeze::freeze` and `thaw`, selected by the test arm, disabled all
shipped freezing and thawing before the house was spawned. Their observers and
empty index remained installed. No already-frozen population was imported from a
parked snapshot.

A runs after physics Writeback and construction SplitSet, before Publish, for the
whole collapse/drive/brake/park run. A dynamic `Piece` passing the installed `RestTest`
this tick, with no rest timer, settles if a touching `ContactGraph` pair that generates
constraints joins it to a static rigid body. A fixed-set snapshot makes this one
layer per tick. It sets `RigidBody::Static` and zero linear/angular velocity, without
writing pose. It records support colliders and bodies. Only removal/reattachment of
a support collider or its body ceasing to be static releases it, with dependency
closure in the same commit. Retired pieces leave the probe's index.

B adds a gravity-plus-contact wrench verdict before settling and rechecks every
settled piece each tick. Inputs include every signed `ContactTransfers::solver_groups`
point from a non-static body, implicit contacts, and retained sleeping presses.
Impulses are divided by the reading's own step length. Fixed-contact point positions,
normals and friction are retained across static–static pair removal. Friction comes
from each manifold's combined material coefficient; `substances::contact_friction`
and `PAIR_RULE` establish the same geometric-mean pair rule as the construction law.
A failed verdict releases the body that tick, **after that tick's solver**. A released
body cannot re-settle until a subsequent tick.

**Local stand-in, not an existing API call:** `parts::Assessment` cannot accept these
point rests. The disposable solver asks whether forces at the retained points can
balance the full force and moment while each force satisfies `normal >= 0` and
`|tangent| <= mu * normal`. Support-polygon tipping and frictional torque thus follow
from the points; no patch area or invented torsional strength is added. Projected
FISTA uses a conservative operator step, a Resolution cap of 2,048 iterations, and
`parts::LOAD_TOLERANCE` as the dimensionless equilibrium residual after scaling force
by weight and moment by weight times contact reach. It rejects a non-converged
answer rather than calling it held. This is a numerical feasibility stand-in; it
does not reproduce the authored-face elastic load distribution or its elliptic
friction approximation. Independent inline brackets passed for slide at mu W,
slide at mu(N+W), lift, and tipping at the support boundary.

B's completed-step contact reading deliberately exposes whether post-step release
alone suffices. No predictive settled-piece target adapter was invented for the
existing sweep. The production requirement that this leaves open is explicit in
the source check and recommendation below.

## Parked verdicts and fraction fixed

`f` is static loose pieces / current loose pieces: B's held pieces, A's kinematically
fixed pieces, and control's shipped frozen pieces. Retired parents are not in the
denominator. The three entries are **at brake / at park start / at window end**.
Travel is net world-space hull-root travel, not integrated speed or path length.

| House shift, m | Arm | Travel, cm | Island asleep at end | f: brake / park / end |
|---:|---|---:|:---:|---|
| +0.05 | Control | 10.1244 | no | 0/45 / 0/45 / 10/45 |
| +0.05 | A | 0 | yes | 42/42 / 42/42 / 42/42 |
| +0.05 | B | 0 | yes | 42/42 / 42/42 / 42/42 |
| -0.05 | Control | 8.6111 | no | 0/57 / 0/57 / 4/57 |
| -0.05 | A | 0 | yes | 41/41 / 41/41 / 41/41 |
| -0.05 | B | 0 | yes | 43/43 / 43/43 / 43/43 |
| -0.10 | Control | 15.4790 | no | 1/43 / 1/43 / 0/43 |
| -0.10 | A | 0 | yes | 36/36 / 36/36 / 36/36 |
| -0.10 | B | 0 | yes | 38/38 / 38/38 / 38/38 |

The final hull islands contain 36 / 53 / 41 bodies in control and one body in each
A/B pose. The A/B parked displacement is exactly zero at the recorded f32 poses.
Their final static fraction is 100%; control's is DERIVED 22.2% / 7.0% / 0%.

**Plateau needs its denominator.** Before driving, all arms have only seven loose
pieces; more of the construction becomes loose when driven into. A has that initial
static plateau after five completed collapse ticks, B after six, and control after
36. At these plateaus f is 7/7. These are not whole-house collapse completion times.
The final static populations in A/B reach their last-switch plateau at the following
completed tick counts from the collapse request; each remains unchanged through
braking, parking and reverse. There is no persistent final plateau in control's
loaded parked window.

| Shift, m | A final plateau, ticks | B final plateau, ticks |
|---:|---:|---:|
| +0.05 | 1,164 | 1,182 |
| -0.05 | 1,164 | 1,183 |
| -0.10 | 1,163 | 1,171 |

These final plateaus follow the drive impact, rather than proving that a newly
collapsed whole house becomes driveable in a few ticks.

## Churn and verdict work

Each drive cell is **settles/releases per second**, with total switch counts in
parentheses. Control uses actual shipped counters; a piece retiring is not counted
as a type switch. Drive lengths differ: control drives for 1,243 / 802 / 1,240 ticks,
whereas A/B reach the 1,280-tick cap in every pose. All three arms have **zero switches
during braking**. A/B also have **zero during parking and reverse**.

| Shift, m | Control drive | A drive | B drive | Control park, settles/releases per s |
|---:|---|---|---|---|
| +0.05 | 0.206/0.566 (4/11) | 2.10/0.30 (42/6) | 3.70/1.90 (74/38) | 2.0/1.0 (20/10) |
| -0.05 | 0.638/1.197 (8/15) | 2.05/0.30 (41/6) | 4.20/2.35 (84/47) | 0.4/0 (4/0) |
| -0.10 | 0.155/0.465 (3/9) | 1.80/0.30 (36/6) | 2.35/0.75 (47/15) | 0.1/0.2 (1/2) |

During the collapse phase A settles seven with no release; B settles eight and
releases one; control freezes seven with no thaw. B has transient driving churn,
concentrated around the impact and ending by its plateau above. It has no persistent
parked flip-flop in this fixture. Its failed verdicts include friction demand and
moment/support-geometry failures; this result does not establish a generally quiet
verdict under a tank actually on top of the pile.

A and control perform no added hold verdicts. B's mean/max evaluations per drive tick
are 27.619/42, 28.221/43 and 25.348/38. During braking and parking they are exactly
42, 43 and 38 each tick. The stand-in re-evaluates unchanged gravity-only states;
it does **not** implement the design's changed-input cache. Diagnostic force-only
and unlimited-friction re-solves and output happen outside timed updates.

## Local tick cost

Whole `App::update` elapsed time, exactly one fixed tick per update, excluding CSV
observation and diagnostic re-solves. Thus these include headless app overhead and
all simulated work, not just the contact solver. The table pools three runs per cell;
p99 uses nearest rank. Park has 1,920 samples per row. Awake-pile drive samples are
drive ticks whose post-step loose-piece awake census is nonzero. Sample populations
are different outcomes, **not matched workloads**: A/B rapidly eliminate almost all
awake-pile ticks, leaving expensive split/impact ticks in that subset.

| Shift, m | Arm | Awake drive samples | Awake drive mean / p99, ms | Park mean / p99, ms |
|---:|---|---:|---:|---:|
| +0.05 | Control | 2,178 | 7.545 / 9.793 | 8.177 / 10.814 |
| +0.05 | A | 15 | 12.435 / 27.857 | 1.056 / 1.161 |
| +0.05 | B | 66 | 10.325 / 26.373 | 4.240 / 4.622 |
| -0.05 | Control | 855 | 8.935 / 14.815 | 9.173 / 10.826 |
| -0.05 | A | 15 | 12.029 / 26.042 | 1.082 / 1.261 |
| -0.05 | B | 69 | 11.085 / 27.790 | 4.794 / 5.875 |
| -0.10 | Control | 2,169 | 7.365 / 9.527 | 7.154 / 8.774 |
| -0.10 | A | 12 | 12.491 / 24.293 | 1.057 / 1.258 |
| -0.10 | B | 33 | 10.227 / 27.029 | 4.210 / 4.954 |

The A/B awake subset's p99 is effectively an extreme sample, not a stable estimate
of sustained pile throughput. B's serial local feasibility solver starts from zero,
scans the transfer stream per piece and checks every settled piece every tick.
These costs price this disposable implementation on the local machine's edit
profile with 2/1/1 pools. They do not price an optimized ownership-parallel verdict,
a changed-load cache, a two-core hosted allocation, or a pile driven over successfully.

## Driving, side hit and dip

All arms enter the first impact at 7.132231 m/s. The largest per-tick forward velocity
loss in the drive occurs at that impact, recorded tick 1,158. DERIVED deceleration
below is `(v_before.z - v_after.z) * 64`, including the whole tick's effects, not a
claim about an instantaneous physical acceleration.

| Shift, m | Control peak, m/s² | A peak, m/s² | B peak, m/s² | A / B parked root Z, m |
|---:|---:|---:|---:|---:|
| +0.05 | 570.145 | 481.269 | 481.269 | 33.872776 / 33.906662 |
| -0.05 | 605.862 | 470.706 | 470.706 | 33.876842 / 33.903893 |
| -0.10 | 535.124 | 482.190 | 482.190 | 33.871426 / 33.874687 |

The lower A/B first-impact deceleration is **not** evidence that they ride over the
pile. After continuing full throttle to the drive cap their forward speeds have
magnitudes below 0.000072 m/s. Controls reach the pile centre and park at root Z
39.943092 / 40.634342 / 39.992016 m. A/B stop around Z 33.9 m. Their loose-piece
counts and collapse histories also differ from control. The experiment measures
an obstruction, not a firm bank successfully crossed.

In the supplemental reverse run, A backs away 24.644 / 24.654 / 24.644 m and B
24.637 / 24.646 / 24.644 m in the Fixture 12 s; final backward speeds are about
2.18 m/s. Nothing traps the tank against reverse. It did not first climb onto the
pile, so this is backing away from its front, **not a validated descent off its top**.

**Side hit: one-tick wall, three identical repetitions.** A physics-only supplemental
fixture mounts the shipped physics extensions and contact-transfer reader. A Fixture
0.4 m cube uses the shipped Brick density and friction (resulting mass 118.0800 kg).
It settles before a Fixture 100 kg, 0.4 m mover arrives laterally at 3 m/s. Its dynamic
twin starts from the same settled pose, switched dynamic before the approach, with
settling disabled for that supplemental twin. Neither fixture installs shipped
freeze/thaw. Both receive the same mover and ground setup.

At contact tick 24, B's piece is static during the solve, then released by the verdict
in the same tick. The mover takes -313.2887 N·s in X and leaves at -0.132887 m/s;
the released piece still has zero velocity. Next tick the piece settles again while
the mover recedes. The twin takes -169.9252 N·s, leaves at +1.300748 m/s, and the
piece moves at +1.337148 m/s. Thus release after the completed-step transfer does not
make first contact finite-mass. The impulse was already resolved as a wall. This
is stronger evidence than the whole-pile deceleration comparison, whose geometry
and breakup histories differ.

**Dip observations, three identical repetitions per row.** The probe marks switches
of pieces with a current direct hull contact, before switching deletes that pair.
These are the contact witnesses for switches under/against the hull; indirect
support changes without a direct hull contact are not separately attributed.
Every recorded event below switches one piece. Positive values mean downward hull
root motion, negative values upward. Same-tick motion happened in physics **before**
the post-step switch; next-tick motion is the immediate following observation.
These are temporal observations, not isolated causal switch displacement.

| Shift, m | Arm | Recorded tick | Same-tick / following-tick downward motion, mm |
|---:|---|---:|---:|
| +0.05 | A | 1,160 | 5.033 / 4.799 |
| -0.05 | A | 1,159 | 5.518 / 5.195 |
| -0.05 | A | 1,160 | 5.195 / 4.702 |
| -0.10 | A | 1,159 | 6.568 / 5.852 |
| -0.10 | A | 1,160 | 5.852 / 5.090 |
| +0.05 | B | 1,168 | -5.094 / -3.306 |
| -0.05 | B | 1,168 | 1.265 / 1.112 |
| -0.10 | B | 1,160 | 5.649 / 5.103 |
| -0.10 | B | 1,161 | 5.103 / 4.374 |

There are no braking/parking switches or associated dips in A/B. No pose is written
by their switch operation. The observations do not separate collider re-derivation,
loss of warm start, changing contact softness, or ordinary impact seating.

## Source check of the design's premises

- **Correct:** shipped freezing works on whole sleeping piece-only islands and
  excludes islands containing a tank. The request to disable both freeze and thaw
  matters; suppressing only new freezes would not empty a pre-existing frozen set.
- **Correct:** the installed rest gate has no requirement for a mature timer when
  called directly; its hook can serve A/B. Static–static broad-phase pairs disappear.
  `Insert<RigidBody>` removes a body's old contact pairs in
  `collision/narrow_phase/mod.rs::remove_body_on`; it also reattaches colliders in
  `collider_hierarchy/plugin.rs::on_collider_body_changed`. Retaining witnesses and
  handling retirement are necessary, not optional cache details.
- **The resting law exists, but not the proposed API.** `parts.rs` forms rigid parts
  from authored chunks and interfaces from authored joints. `bondsolve::joint_edge`
  reads nonzero face area, material compliance and polygon/disk geometry. Its face
  verdict groups patches, judges pressure against their corner hull, and uses an
  elliptic sliding/twisting limit. This is not a public six-coordinate arbitrary
  point-contact feasibility entry. Passing zero-area fake joints would not be the
  stated point law. Production needs a pure point-rest/wrench entry with explicit
  numerical status and reusable geometry/workspace; it must not mutate the shared
  authored `Structure` or invent contact patch area.
- **The sweep needs more than a filter widening.** Both its `Owners` and contact hook
  select `Without<Piece>`. `sweep::Geometry` addresses authored chunks and cuts;
  `resolve` compares a Hertz peak plus projected drive against a *directional
  translation* min-cut. It does not accept a settled body's live support points or
  decide a full support moment. The source test
  `compressed_sliding_reads_only_existing_gravity_preload` explicitly preserves
  gravity preload rather than adding the imposed press to friction preload. Thus
  that API is not already the requested mu(N+W) live loaded hold verdict. The
  unloaded resting-board bracket is valid but does not establish the loaded case.
  Production needs a settled-target adapter that supplies the live wrench/support
  law, occupied geometry and finite inertia to pre-contact release and exchange,
  plus release closure through newly freed sources. Post-step rejudgement cannot
  substitute for it, as the side-hit numbers show.
- **The claimed discarded-energy bound does not follow from `at_rest`.** Its OR of
  separate linear/angular velocity bounds and the energy bound permits a body that
  passes the separate test to have rotational energy above the quoted linear-only
  limit. No settling-energy limit was measured here.
- **Timer observations do not price f.** The diagnosis's zero-timer statistic does
  not count pieces that simultaneously pass rest and have fixed support, nor their
  propagation bottom-up. The measured f here supplies that count for this fixture,
  but reaches 100% with the tank stopped outside the pile. It does not establish the
  loaded-pile fraction assumed by the design's cost forecast. A new downward load
  also raises friction capacity only insofar as its moment remains supportable;
  added compression is not a general proof against tipping.

## Recommendation and remaining gate

**Build slice 1 changed.** Preserve the generic material/support direction, but make
its pre-contact part a first-class interface rather than a filter edit to the
standing-construction sweep. A reusable full-wrench point-rest verdict and a
settled-target sweep/exchange must release a side-hit piece, and its dependency
chain, **before the static solve can consume the mover's momentum**. Rejudgement
from actual transfers remains necessary after the solve. Keep one source of pair
friction and explicit numerical failure handling. Cache unchanged inputs and price
that implementation separately; this prototype's serial cold solve is not a cost
promise.

Before landing the whole replacement, repeat this disposable gate with a tank that
actually reaches and parks inside the settled pile, then traverses off it, and with
side-hit dynamic twins. The present literal gate admits a stopped-at-the-front false
positive for the intended behavior. Do not loosen the hold verdict, add a rest delay,
or tune friction to these numbers. This experiment does not establish that the law
is too strict under parked solver noise; no loaded parked B case was reached.
Hosted throughput, indirect-support dip attribution, actual ride-over/descent, and
pre-contact release-chain equivalence remain unmeasured. No production change is
justified by treating those gaps as passed.

## Retained artifact and verification

Raw primary CSVs: `/tmp/settle-{control,A,B}-{0.05,-0.05,-0.1}-{0,1,2}.csv`.
Primary logs: `/tmp/settle-final-{arm}-{shift}-{run}.log`.
Control audit CSVs/logs use `audit0..2`; summaries are `/tmp/settle-summary.json` and
`/tmp/settle-control-churn.json`. The side-hit/bracket output is in
`/tmp/settle-B-build2.log`. These are local evidence artifacts, not committed inputs.

All disposable systems, registrations, arm switches, local solver, bracket/side-hit
tests and instrumentation are removed. Temporary source and runner files were
removed with literal absolute paths. `target/` is preserved. `git diff --exit-code
-- src vendor` passes; only this Markdown report is retained. Final checks on the restored source:

- `cargo fmt --all --check`: passed.
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`: passed.
- Headless `scripts/serial-cargo test --lib --features dev_ui,bitprobe debris::freeze::tests`: 13 passed, none failed.
- `git diff --exit-code -- src vendor` and `git diff --check`: passed.

No production code, experiment source or retained test is committed. No merge or push.
