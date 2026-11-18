# Parked creep diagnosis — 2026-11-18

Subject: Overmatch's driving physics and environment simulation; game state only.
Lane `lane/park-creep`, bay 2, base `eb4177ab`. Diagnosis only; no production change or retained probe.

## Finding

The parked hull is being carried by a moving, loaded contact pile. The large travel is strongly dependent on the resolution of that coupled contact simulation: the same three parked states, physical data and stopped belts travel centimetres at the engine's default substeps and millimetres with smaller substeps. Gravity, sampled support/grip, contact normals and friction account for the hull's velocity changes; there is no missing propulsion impulse. Some rubble fails its rest test and keeps the contact island awake even when the hull's own rest interval has matured.

The restored wall is a different case: it holds full throttle, with moving belts, and climbs/pitches against the wall. Releasing drive makes it sleep and stop. Its previously reported forward travel is an integral of velocity in the changing hull frame, rather than a world-space position difference.

This isolates the source of the visible parked travel to loaded-contact integration resolution, not to a delayed bill on a moving belt. It does **not** establish a unique defective contact row, a universally sufficient shipping substep count, or a fix for every sleeping outcome. The finest tested third pose still moves slightly and stays awake. The distinctions below matter when choosing a fix.

## Reproduction and controls

All experimental dimensions, poses, durations, selection rules and the visibility threshold are **Fixture** inputs. All experiment outputs below are **MEASURED** unless explicitly marked **DERIVED**. Table unit conversions are **DERIVED**. Versions, source locations and configuration defaults are sourced facts, not new physical measurements.

Read `AGENTS.md`, the diagnosis skill, the glossary, relevant domain rules, the prior lane's slice-4 report, and issues #353, #347 and #312 with their comments, read only. Engine behavior was checked in `vendor/avian3d-0.8.0-overmatch/`. No agents, web searches, issue writes, external-repository writes, pushes, windows or sound.

Every measurement run used `OVERMATCH_UNATTENDED=1 OVERMATCH_MUTE=1`. Builds used `scripts/serial-cargo` and `--features dev_ui,bitprobe`. The full-app probe used the existing `frozen_rubble::{flat_world,fell_house}` helpers, shipped tank/house data, and compute/async/IO pools of 2/1/1 threads.

Recipe, recovered from the prior lane rather than from historical transform snapshots: house 40 m ahead; collapse for 10 s; wait up to 60 s for the pile's awake census to empty; drive toward its pieces' mean Z for up to 20 s; brake for 3 s using the transmission's own park-engagement speed; release both controls and measure 10 s. Lateral house shifts select the poses. The window starts immediately after braking, before its first parked step; the prior report omitted that first step from the displacement window, explaining the small differences in its reported totals.

The initial red command was:

```sh
OVERMATCH_UNATTENDED=1 OVERMATCH_MUTE=1 PARK_RED=1 \
  scripts/serial-cargo test --lib --features dev_ui,bitprobe \
  park_creep_probe -- --ignored --nocapture
```

It failed with `parked hull visibly creeps: 0.1012441 m`. The disposable detection threshold was 3 cm, the prior report's visible-drift judgment, not a class law or proposed regression target. A warm single-pose run took about 14.5 s; three baseline runs took 43.79 s. Their complete per-tick CSVs were byte-identical. Every deciding arm was repeated three times; repeated summaries and retained complete trace copies agree exactly. Additional one-run screening arms are identified separately below.

All arms start at the same parked state; intervention begins after braking. Smaller-substep runs scale active cached contact normal/tangent impulses by old-count/new-count and set `Time<Substeps>`'s delta to the upcoming substep before contact/spring preparation. An early screening switch omitted that synchronization and is discarded. The physical tick, physical parameters, commands, initial poses and velocities remain the same. No physical law is retuned in the substep comparison.

Raw logs and trace copies remain outside the repository under `/tmp/park-*`. Throwaway sources, installer/runner scripts and all probe registrations are deleted before commit. Its red threshold and exact trajectories are observations, so no permanent test pins them.

## Three parked poses

All rows in the next table were repeated three times. Travel is net world-space hull-root displacement over the parked window. Belts remain exactly at zero in every baseline and finer-resolution row.

| Arm | House shift, m | Travel, cm | Sleeps by window end? |
|---|---:|---:|---|
| Default, 6 substeps | +0.05 | 10.1244 | no |
| Default, 6 substeps | -0.05 | 8.6111 | no |
| Default, 6 substeps | -0.10 | 15.4790 | no |
| 48 substeps, synchronized clock/cache | +0.05 | 0.1913 | yes; then no movement |
| 48 substeps, synchronized clock/cache | -0.05 | 0.2242 | yes; then no movement |
| 48 substeps, synchronized clock/cache | -0.10 | 0.8772 | no |
| 192 substeps, synchronized clock/cache | -0.10 | 0.2871 | no |

The last second of the third pose at 48 substeps moves 1.253 mm; its last five seconds move 5.273 mm. Thus a claim that every residual motion or sleep failure is eliminated would be false. All three finer-resolution displacements are below the original red loop's 3 cm threshold. On the first pose the hull's centre of mass travels 9.411 cm at default resolution; the root's travel is not merely a rotation-origin artifact.

For the first pose, repeated three times at each resolution:

| Substeps | Travel, cm | Sleeping ticks in the parked window |
|---:|---:|---:|
| 6 | 10.1244 | 0 |
| 12 | 3.6827 | 251 |
| 24 | 0.8492 | 539 |
| 48 | 0.1913 | 609 |

At fixed physical data, the last two halvings of the substep reduce travel by factors **DERIVED 4.34 and 4.44**. These are convergence observations with sleeping transitions, not a universal error-order theorem. They make the default result unsuitable as a physical settlement measurement.

The first pose run for 60 parked seconds, three times, is first observed asleep after 886 fixed steps (**DERIVED 13.84375 s**), reaches 13.1768 cm net root travel and remains still to the end. It is finite settling in this fixture, not indefinite propulsion. Two complete long traces and all three outcome summaries agree; the first raw trace copy was overwritten by another measurement process and is not used.

## What moves the hull

The probe sums every completed-step `ContactTransfers::solver_groups()` point involving the hull, with its body sign, splitting the impulse into normal, belt-face tangent and other tangent. Separately it reads the sampled support normal and sampled tangent force; the latter already includes rolling resistance. It records position, rotation rate, velocity, belt speed, rest timer, island rest census and the substep velocities after velocity integration, warm start, biased contacts and Relax.

First pose, default resolution: means over the parked window, in world-space kN. Contact impulses are converted to mean force using the physical tick. This is a force budget, not five independent actuator commands.

| Path | X | Y | Z |
|---|---:|---:|---:|
| All solver normals | -15.535 | 404.881 | -65.467 |
| Belt-face solver tangents | -36.892 | 11.196 | 76.155 |
| Other solver tangents | 1.101 | 5.213 | 8.850 |
| Sampled support normal | 0 | 137.725 | 0 |
| Sampled grip and rolling | 51.394 | 0 | -19.629 |
| Gravity | 0 | -559.170 | 0 |

The ground field is flat: horizontal normal components come from the pile/contact geometry. Preloaded ground grip opposes those reactions even with stopped belts. Removing sampled horizontal forces leaves 8.2521 cm of travel; it is not a sufficient cause of the creep. Zero belt speed means zero belt mechanical work; stored elastic support/grip and gravitational potential can still change.

For every tick after the first, `Δv = ΣJ/m + (F_sampled/m + g)·dt` closes to a maximum absolute component residual of 4.55e-7 m/s. `contact_transfer` is the ledger of already-applied impulses, not a second force writer (`src/contact_transfer.rs`, `add_substep`/`add_restitution`). There is no unaccounted hull impulse in this run.

**DERIVED magnitude:** the large reactions nearly cancel, leaving mean component imbalances of order 0.1 kN. On the measured 57,000 kg fixture, a 100 N residual gives `a = F/m = 0.00175 m/s²`, and from rest `a·(10 s)²/2 = 8.77 cm`. The actual force varies and the hull enters with motion, so this is an order-of-magnitude calculation, not a trajectory prediction. Across shifts +0.05/-0.05/-0.10 m, the measured mean net-force vectors are `(68.6, -154.6, -90.2)`, `(-43.9, 47.7, 4.7)` and `(94.3, -61.9, 21.4)` N. Applying `|F_mean|·T²/(2m)` gives **DERIVED characteristic travels 16.8 / 5.7 / 10.1 cm**, the same order as the measured 10.1 / 8.6 / 15.5 cm. The corresponding maximum per-component velocity-ledger residuals are **MEASURED 4.55e-7 / 3.15e-7 / 1.94e-7 m/s**. Integrating the measured pre-position substep velocities gives centre-of-mass displacement approximately `(61.18, -69.84, -15.36) mm`, matching the independent centre-of-mass position change to rounding. The root additionally moves as the body rotates about its centre of mass.

**DERIVED resolution scale:** the hull's normal load averages 404.9 kN across 217.1 recorded hull contact points, including inert points. A uniform-share scale is 1.86 kN/point. At the default substep `h = 1/(64·6) s`, that is 4.86 N·s; on a measured 59.14 kg piece, one such unbalanced contact changes speed by 0.082 m/s. A few contact projections have a speed scale comparable to the sourced rest threshold of 0.15 m/s. At 48 substeps that single-projection scale is 0.0103 m/s. This explains why small residuals of large load-bearing impulses can cause centimetres of slow hull travel and repeatedly spoil a light piece's rest interval. It is not an assertion that every point carries an equal load or that a whole projection remains uncancelled.

The engine code gives the numerical pathway: `solver/schedule.rs` orders velocity integration, warm start, biased contacts, position integration and Relax; `contact/mod.rs` solves normals point by point and friction only in Relax; `contact/normal_part.rs` uses different biased/relaxed rigid-contact equations and the game's spring equation in both passes for compliant runs. `integrator/mod.rs::integrate_positions` advances the centre of mass from the pre-Relax velocity. `solver_body/plugin.rs` then converts centre-of-mass motion to the root pose. A final-velocity reading alone cannot account for the travelled position.

The normal law itself has a relevant length scale. For a rigid contact with softness coefficients `(alpha, beta, gamma)`, a fixed-load equilibrium satisfies `alpha*m_eff*beta*overlap = gamma*F*h`. The sourced coefficients give `alpha*beta/gamma = h*omega²`, hence **DERIVED `overlap = F/(m_eff*omega²)`**. In the repeated baseline normal read, a contact has **MEASURED effective mass 43.538 kg**, a cached substep impulse corresponding to **MEASURED 71.03 kN**, and anchor-corrected overlap **MEASURED 20.01 mm**. The sourced dynamic-contact frequency gives **DERIVED omega² = 90,958 s^-2**, predicting **DERIVED 17.94 mm** of fixed-load softness. This is numerical contact compliance with a centimetre length scale, not a measurement of rubble material elasticity. Early depth debug lines that confused the constraint's anchor-offset `initial_separation` with the geometric gap are discarded; the final depth and speculative-contact arms use the anchor-corrected gap.

The evidence identifies coarse integration of the loaded normal/friction network. It does not identify friction's phase placement alone as the cause: adding friction before position integration helps one pose but fails the other two. It also does not identify artificial normal softness alone: removing it helps two poses but worsens the third. Those counterexamples prevent promoting either attractive single-row story to the diagnosis.

## The pile and sleeping

In the first default pose, contacted pieces move up to 23.81 cm in world space, with final mean displacement 11.37 cm across the pieces tracked from their first hull contact. The pile is not a stationary inclined support. At 48 substeps the first pose's corresponding maximum is 1.855 mm.

The hull's own rest gate never vetoes sleep in the first baseline window: `LastPass::at_rest` stays true; `commit_drivetrains` does not reset its timer. Its timer reaches 4.71875 s, exceeding the sourced 0.5 s delay. Its island averages 43.23 solver bodies, of which 8.22 have an immature rest timer; the minimum member timer is zero on 264 of 640 ticks and never exceeds 0.3125 s. Peak island linear speed is 1.155 m/s. Representative blockers include pieces of 59.14, 88.70 and 180.75 kg; they are not the hull's internal belt degrees of freedom.

In the finest third-pose run, persistent blockers include a 282.28 kg piece under the hull and a 527.70 kg piece farther through the connected pile. The hull remaining awake is therefore not evidence of a nonzero belt speed. Avian's `islands/sleeping.rs::update_sleeping_states` admits an island to sleep only when every member's own rest interval matures, using the installed rest test. `src/avian_ext/rest_test.rs` owns the game's velocity/kinetic-energy verdict. `wake_on_changed` applies its user-change checks to sleepers; this first parked window never sleeps, so repeated user-change wakes are not its explanation.

The relevant game wakes were checked: drive demand and a failed resting drivetrain trial (`track::sim::wake_driven_tanks`), non-resting drive/contact verdicts (`commit_drivetrains`), removed/changed supports (`debris`), and contact/impulse wakes in Avian. Disabling the tank rest gate, cold-starting the drivetrain's assignment seed, and removing restitution each leave the baseline trace unchanged. The freeze audit records 20 pieces frozen and 10 thawed in the first default window, including one reach-triggered island thaw; disabling new freezing gives zero freeze/thaw events and still 9.9730 cm travel. In the third default pose, disabling new freezing preserves the hull trace exactly at 15.4790 cm despite changing freeze/thaw counts from 1/2 to 0/1. Each audit/control was repeated three times. Freeze/thaw feedback is therefore not necessary for the coarse creep. At 48 substeps, disabling new freezing also preserves the third pose's hull trace exactly, including its awake outcome. The drivetrain warm start is a bounded-solve assignment seed (`track/block.rs`), not a physical hull impulse cache. There is no compliant held-belt row in Avian's contact graph to tune; the internal stopped-belt constraint does not itself translate the hull.

## Plain rigid box

The control keeps the fixture's authored mass, inertia and centre of mass and replaces its colliders with one ordinary rigid cuboid spanning the two run colliders' bounding box. Its footprint is **DERIVED 3.814 × 5.864 m** from the loaded geometry; the original run's friction is **MEASURED 0.6**, with its geometric-mean pair rule. Tank force/drivetrain advancement is disabled; the logical tank ownership is retained so unrelated full-app consumers remain valid. No `BeltFace` or compliant run is attached to the box collider.

The box still moves and does not sleep in three identical runs. Total travel is 27.09 cm, but much of it is the replacement shape's initial overlap correction and should not be compared directly with the tank's total. Its final five seconds travel 2.410 cm. With 48 substeps, its final five seconds travel 0.0254 cm, while the connected pile still keeps it awake. Thus tank-specific belt/suspension behavior is not necessary for slow loaded-pile motion. The filled cuboid changes the contact network; this control does not prove identical trajectories or contact loads.

## Other interventions

Every row below is three identical runs of the first pose unless stated otherwise. These are causal comparisons at its parked state, not proposed shipping switches.

| Arm | Travel, cm | Sleeps? |
|---|---:|---|
| Default | 10.1244 | no |
| Remove sampled horizontal forces | 8.2521 | no |
| Cold drivetrain assignment seed | 10.1244 | no |
| Disable tank rest gate | 10.1244 | no |
| Remove restitution | 10.1244 | no |
| Double contact friction coefficient | 8.6594 | no |
| Remove contact warm starting | 20.6005 | no |
| Remove speculative rigid contacts | 11.8081 | no |
| Disable new debris freezing | 9.9730 | no |
| Sixteen contact sweeps per biased/Relax stage | 3.9632 | yes, after further settling |
| Lock rubble rotation | 7.8247 | no |
| Disable body-pair contact reduction | 10.0605 | no |
| Hold the pile fixed in world space | 9.0669, including intervention transient | yes; final five seconds have zero travel |

Disabling body-pair reduction increases the mean prepared contact-point count from 1,680.24 to 3,383.35 and the mean manifold count from 587.70 to 954.91, while travel changes only from 10.1244 to 10.0605 cm. These counts were repeated three times. This is not a universal contact-count scaling law; it rules out a simple “fewer points caused this motion” explanation in this pose.

The fixed-pile arm is not a material model or a fix. Changing a loaded dynamic recipient to a static one changes its mobility and causes a transient; the first pose even briefly moves its belts. Only its subsequent stationary outcome is used to show that the tank can rest on the same fixed pile geometry. In the other two poses the repeated fixed-pile totals are 1.0300 cm and 0.1040 cm respectively; both sleep and have zero final-five-second travel. Thus removing pile motion stops sustained creep in all three poses, with different intervention transients.

Across the three poses, adding biased-pass friction gives 2.2257 / 9.7501 / 7.0031 cm, sleeping only in the first. Replacing the pile's normal rows with rigid velocity constraints without positional correction gives 2.0551 / 5.2064 / 19.4177 cm, sleeping only in the first two. Both sets were repeated three times. Neither is a sufficient general explanation or fix.

One-run screens, not used as deciding proofs: doubled default contact frequency leaves 10.54 cm; hard normal correction leaves 13.56 cm in the first pose and worsens the third; removing compliant runs is not a remedy; biased friction restricted to hull contacts or restricted to pile contacts also fails to remove the first pose's travel. These arms change the result and are insufficient to select a physical law.

## Wall: driven pitch, not a parked belt

The restored `rigid_wall_bed` is the prior branch's two belt-lane walls, clearing the hull nose. Reproduce its initial four seconds at full throttle, then measure the next four. Each wall row was repeated three times and is deterministic.

| Arm | Integrated hull-frame forward velocity, cm | World root displacement, cm | Sleep |
|---|---:|---:|---|
| Continue full throttle | 4.7725 | 28.9585 | no |
| Release throttle | 0.1963 | 4.4590, then zero | yes; 208 of 256 ticks |
| Add biased-pass friction, continue throttle | 4.6958 | 29.3614 | no |

The baseline belts run near 0.656 m/s. The wall carries a mean forward normal reaction of 244.6 kN and upward tangential reaction of 135.6 kN; sampled ground traction pushes forward at 133.1 kN. It climbs: centre of mass rises 23.54 cm, while moving forward only 1.870 cm in world Z. The root moves forward 14.958 cm and the hull rotates by 0.14936 rad.

**DERIVED geometry check:** the shipped sim GLB's `Center_Of_Mass` marker is **MEASURED 0.881226 m** above its hull origin (parsed from `tiger_1.sim.glb`; `tank/template.rs` uses that root position). `0.881226·sin(0.14936) = 13.113 cm`; adding the measured 1.870 cm centre-of-mass advance predicts about 14.983 cm of root forward motion, matching the measured 14.958 cm to about 0.3 mm. The changing origin and pitch explain the apparent forward motion against a fixed wall. Raising the centre of mass costs **DERIVED about 132 kJ** of gravitational potential, supplied by continued driving. There is no parked no-input wall creep in this reproduction after release/settling.

## Missing truth, merit and recommendation

The missing bound is **solver Resolution**: the upstream default of six substeps does not resolve this heavily loaded, heterogeneous contact network to a player-quiet result before sleep. A solver's numerical softness and finite contact projections are not rubble material measurements. No per-vehicle exception, brake constant or tank sleep rule is justified by this evidence. The wall additionally needs the correct experimental state and coordinate/velocity distinction, not a parked-hold fix.

**Recommend increasing contact integration resolution at one named game-side Avian seam**, with the measured 48-substep arm as the reference for eliminating visible creep. Establish the cheapest resolution/work partition against this loop before choosing its shipping cost; 48 is evidence, not a new authored material property or a guaranteed all-sleep value. A future change must also exercise moving belts, moving rubble and the two-core server allocation. Nothing is implemented here.

| Candidate | Merit | Cost and risk |
|---|---|---|
| Smaller physics substeps, game-side setting | Directly suppresses visible travel across all three matched poses; preserves physical data | **DERIVED 8× inner-substep calls at 48 vs 6**, not necessarily 8× whole-tick cost. More drivetrain/contact/integration work while awake; changed moving-contact behavior. Parked cost can fall when bodies sleep earlier. Hosted throughput is unmeasured. |
| Local/adaptive substepping of loaded contact groups | Same resolution problem with potentially less unrelated work | Larger implementation; requires coherent ownership and ordered cross-group effects, not a tank-specific heuristic. Not prototyped or priced. |
| More contact sweeps at the same timestep | Helps first pose | Sixteen sweeps still give visible first-pose travel; **DERIVED 16× contact solves**. Weaker measured payoff than smaller substeps. |
| Change friction phase, friction magnitude or normal softness | Some individual poses improve | Fails counterposes; changes the contact law or its discretization without a general remedy. Not recommended from this evidence. |
| Looser sleeping, immobilized/rotation-locked rubble or a parked-hull freeze | Can conceal motion or simplify the network | Hides or changes active support state; rotation locking still creeps. Reject as fixes. |
| New rolling/contact-moment material law | Could represent a real missing material property in another investigation | No necessity demonstrated here: locking rotation does not remove creep. A physical parameter needs a source and a generic law, not a number fitted to this trace. |

## Believability and limits

A heavy body loading loose pieces can settle. This lane has no external empirical reference that establishes how much this particular rubble should settle or for how long; no web search was permitted. I therefore do not call the coarse 13 cm/14 s result realistic merely because the simulation produces it. The reference used for judgment is the same initial state and physical data at finer integration, plus the prior report's camera-based visibility estimate. A player can accept a brief visible settling response after driving onto loose rubble; prolonged centimetre drift with stopped controls affects aiming and reads as loss of control. Millimetre residuals are below that report's noticeable accumulated-drift estimate, and remain observations rather than regression targets.

Unresolved: the exact individual row/geometry condition responsible for the finest third pose's immature timers; a production error estimator or minimum sufficient resolution across all piles; hosted awake-body cost; real-rubble settlement calibration; and the effect of a resolution change on #347's moving-belt/piece reversals. The experiments do not justify replacing the drivetrain, a new per-tank branch, or declaring every sleep failure fixed.

## Retained change and verification

Only this report is retained. All disposable arms, registrations, counters and engine changes are removed; no probe test is retained. Verification on the restored source:

- `git diff --exit-code -- src vendor`: passed; retained Rust and vendored solver are identical to the lane base.
- `cargo fmt --all --check`: passed.
- `headless_test::track_scorecard::belt_friction`: 1 passed, its on-demand measurement probe ignored.
- `tests::avian_island_sleep`: 21 passed.
- Temporary source/runner files under `/tmp` were deleted using literal absolute paths; raw logs and CSV data remain.
- CI clippy was not needed under this lane's explicit report-only exception: no code remains in the diff. No push or merge.
