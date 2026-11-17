# Drivetrain slice 2: second-review closure

Subject: Overmatch's multiplayer video-game driving physics, game state only.
Lane `lane/drive-crank`, bay 2. The design narrative lives in
`.agents/docs/design/track-model/transmission-design.md`; this report records review dispositions
and observations. No parameter was chosen to recover a historical output.

## Finding dispositions

| Finding | Disposition |
|---|---|
| Review P1: breached hold locks out the driver | Fixed in `transmission::driver`. Exact-rest transfer preserves static support only. If the mean starts sliding, an exit immediately admits side park; an along-ladder request can release it for propulsion, Brake/reverse command their side stops, and steering remains available. The overload proof checks all four authorities on both ladders; the 6,048-state proof checks authority as well as order changes. |
| Review P2: brake capacity | The owner chose rated-grade stopping/holding. `TransmissionSpec::brake_capacity_n` derives per-side sliding force from the sheet's mass and rated slope. See the data/readers section below. |
| Review P2: N-row certificate | `block/tests.rs` has an independent ADMM/Cholesky constrained-QP reference, including static-to-dynamic restart, one-way limits and the production five-row/three-speed dependency pattern. It compares unique speeds and regimes and independently checks momentum and feasibility. The polygon loop includes parallel normals and normals parallel to box bounds. |
| Review P2: identity test | Deleted the test equating summed belt momentum with its own delivered force. Its observations are recorded below. The supported handover proof uses stiff unequal reaction slopes and asserts that the differential coupling term is substantial relative to load. Unstaffed stored-crank discharge is covered by the shipped eventual-rest test, rather than asserted to fit an artificial static load budget. |
| Review P2: cost | `block::Solution::assignments_tried` counts every attempted assignment, across restarts. Probes print max/mean per tank-tick, including candidate-price solves. No wall-clock benchmark. An accepted limit face is retained across a restart; fixed-regime convexity makes speeds unique regardless of search seed, and breakaway reads physical slip. Cross-tick warm start omitted: nonunique impulse splits can change the impulse-derived `driving` verdict (`transmission::advance_rows`), which `DriverSense::drove` records for its next decision. Unique endpoint speeds alone therefore do not prove absence of chatter. |
| Review P3: panic precondition | The panic states that an optimal independent KKT basis must be resolvable above the rank threshold. Construction admission checks all row subsets and crank-limit faces against that threshold; the asset gate adds filename to the field/repair refusal. Exact dependencies remain valid. |
| Review P3: unclamped interior impulse | The general solver clamps accepted row impulses to their box before checking combination feasibility and returning them. |
| Review P3: stale sheet comment | Removed. Data now states the actual linked brake construction and the neutral-turn torque Unknown. |
| Review P3: restart loses active limit face | Fixed by retaining the entire accepted assignment during static-to-dynamic restart. |
| Lens code-branching P3: box-only alternative | Deleted. Empty-limit fixtures and production use the same bordered solver. |
| Lens brevity P3: duplicated elimination | Deleted the Gram-only elimination. The bordered routine handles zero multiplier coordinates and also owns conditioning admission. |
| Lens brevity P3: mean-hold fixture ceremony | One `mean_hold_drivetrains` fixture iterator supplies the three supported-handover proofs. |
| Lens brevity P3: repeated drag explanation | The no-reversal/back-drive invariant lives on `bounded_drag` only. |
| Lens brevity pre-existing P3: repeated sheet provenance | Deleted the repeated comments; sourced field labels own provenance. |
| Lens testing P3: unstaffed sleep deadline | Shipped-plugin proof asserts eventual supported park and sleep with no fixture deadline. |
| Lens testing pre-existing P3: released-pivot deadline | Arrest bound derives from differential momentum and available steering impulse; monotonicity and non-reversal remain asserted. |

All review claims were checked against source. The optional cross-tick optimization is declined
for the feedback reason above; no reported correctness finding is dismissed. No backlog row
names the changed files. The orchestrator owns the independent six-lens landing review.

## Brake data and readers

- The sheet authors `rated_slope_deg = 35` as **MEASURED**: Spielberger, *Der Panzerkampfwagen
  Tiger und seine Abarten*, 6th ed. 1998, pp. 190–192, ascending and descending climb rating.
- Generic law: per-side sliding capacity `mass * g * sin(rated_slope) / 2` (**DERIVED**), following
  the owner decision and AMCP 706-355 sections 3-2.1 and 8-28's stop/hold-on-rated-grade rule.
- `brake_static_factor = 1.2` is **MEASURED as a design assumption**, DARCOM-P 706-358 (1976),
  section 14-8, pp. 14-17–14-18: factors 0.50 service and 0.60 static. It is not an Argus lining
  measurement. The old inferred 1.5 and inferred 20-degree sizing are absent.
- Parking links the same two side discs, stated by `mean_hold_brake_count` in data: Tigerfibel
  D 656/27 p. 41's hand-lever halt and Spielberger pp. 51–55's actuation/construction. The mean
  row derives its rating from that count and the per-side capacity. No independent force remains.
- Neutral-turn gearbox main-shaft brake torque is **Unknown**. The British MI summary of
  November 1943 gives its clutch-drag-cancellation purpose; OLVAR manual D 656/21+ Pt 2 or
  measured torque/reduction would settle the missing rating. No gearbox torque law is invented.
- **DERIVED** for 57,000 kg and g=9.81 m/s²: 160.36 kN per side sliding, 192.44 kN static;
  linked mean 320.73/384.87 kN. Ideal level brake-only deceleration is 5.627 m/s² (0.574 g).
- Rejected sizing: ground-grip capacity is circular because ground material is not brake
  construction; 20-degree holding underspecifies the sourced rating. Peak brake power does not
  make a force capacity energy-impossible: total energy per stop is fixed by initial speed.
- Mass is supplied explicitly through nominal and physical transmission adapters. Every force
  reader uses the derived owner: runtime brakes/ceiling, bot `TrackGear::braking_deceleration`, HUD
  transmission params, sandbox, spawn/crank seed, bake, running-gear admission, bitprobe startup,
  contact/slope scorecards, tests and the Blender sheet generator. The clutch-and-brake fixture carries its own Fixture slope.
  Authored `brake_force` and `mean_hold_force` fields are removed, without defaults or fallbacks.

## Observations

All probe readings below are **MEASURED observations**, never acceptance targets. Before means
the second-review run retained in the prior report/logs; after means one complete final run of
each shipped probe. A fresh execution can arrive at a different terrain contact or rubble pose.
The large metrics JSON is deleted; raw logs remain outside git under `/tmp`.

The deleted momentum-identity witness's prior replay observed 217.0 kN starting reaction,
339.79 kN unconstrained delivered mean force and a 288 kN then-static rating (**MEASURED**).
The independently summed equation gave an extra 122.8 kN from unequal affine ground slopes
and a 339.9 kN total (**DERIVED**). These values describe that replay, not an obligation on the
new brake data. Starting load alone never establishes that full coupled demand fits capacity.

One complete sweep produced 55 turn cases and 16 long pivot cases. **MEASURED:** 64 cases
reproduce their round-2 held yaw, crank rpm and release travel exactly. The seven cases that
moved, plus both unchanged along-slope 25-degree releases, are listed here. Release travel is
planar net displacement over five simulated seconds.

| Case | Held yaw before → after (rad/s) | Release travel before → after (m) |
|---|---:|---:|
| pivot slope25 uphill | -0.083504148 → -0.083504148 | 0.023485437 → 0.023485437 |
| pivot slope25 downhill | -0.124657965 → -0.124657965 | 0.042793903 → 0.042793903 |
| pivot slope25 across | -0.006260733 → -0.006260733 | 4.313269437 → 0.032994319 |
| pivot slope35 uphill | 0.000030875 → -0.000133356 | 0.029722382 → 0.000747446 |
| pivot slope35 downhill | 0.000526777 → 0.001878037 | 0.001402680 → 0.026710626 |
| pivot slope35 across | 0.000146556 → 0.000558021 | 0.001019659 → 0.000959396 |
| flat g3 lifted tight | -0.105808742 → -0.105808334 | 0.000347954 → 0.000362697 |
| flat g5 lifted tight | -0.105808277 → -0.105807929 | 0.000328684 → 0.000362697 |
| pieces neutral pivot | -0.092398019 → -0.092983525 | 0.017468046 → 0.016872228 |

**MEASURED:** every 25-degree release ends with both belts exactly zero and park present for
305 of 320 release ticks. Across-slope changed from zero parked ticks and final belts
`[-1.697334409, -1.697334409]` m/s to 305 parked ticks and zero belts. All three held yaw/rpm
readings are unchanged; across releases at heading 1.721987605 rad after 12.318467140 m hold
travel. Its stronger stop now accepts the differential handover demand rather than entering an
un-arrestable slide at the old rating.

The 35-degree cases remain **grip-limited**, not evidence of a rated-grade brake stop. Base-ground
mu is 0.574 (Unknown data). Including full rolling gives about 0.618 total resistance
per normal load, below `tan(35 degrees)=0.700` (**DERIVED**), so soil contact cannot carry it.
They travel 368.34/369.32/369.62 m during hold (**MEASURED**) and finish near the finite probe's
edge solids (hull centres z=371.15, z=-372.09, x=-373.59 m **MEASURED**, within the 375 m half
extent). `world::boundary::spawn` installs those ordinary edge colliders. Their small release
travel must not be read as stopping on the open slope. No brake enlargement can supply missing
ground grip; no material or contact parameter was tuned. The cold-slope law proof passes below
both moving capacities on every shipped ground, and moves above total available resistance.

### Level service stop

**MEASURED**, same 2.183719158 m/s starting speed: stop travel **0.691332281 → 0.410641074 m**;
peak-distance tick **44 → 26**; park latch tick **45 → 28**. Rock-back **5.110681 → 8.016706 mm**;
peak reverse speed **149.183426 → 233.407974 mm/s**. The equivalent constant deceleration
`v_start^2/(2*stop_travel)` is **3.448869 → 5.806323 m/s², DERIVED from those MEASURED readings**.
The ideal device-only capacity is not a target for the composed hull/contact/slew response.

### Slab pull and rubble release

**MEASURED** slab end speed **3.368829727 → 2.498057842 m/s**. In/Slip/Out ticks
**117/52/87 → 0/256/0**. Slip left-belt alternation **0.099798590 → 0.242600963 m/s**;
slip whole-hull delivered-force alternation **232.096436 → 296.927979 kN**. New In/Out amplitudes
are absent because those regimes have no samples; zero would misstate the observation.

The trace explains the different path: the old run begins in F1 and buys F2/F3/F4/F5/F8 at ticks
20/61/101/159/249; this run selects F8 on its first requested tick and pays no further cuts
(**MEASURED**). `Driver::choose` admits the entire ladder to an unchosen positive-speed reading,
while a nonpositive reading uses the authored launch range. An initial F8 with no cut is therefore
in that existing rolling-selection class (**DERIVED from the branch**). The new brake changes
the settled arrival; the first recorded hull speed is 0.0012 m/s. Neither the old gear sequence
nor the old alternation is an obligation, and the gear policy is unchanged. Delayed hard-contact
reaction still belongs to the contact slice; these observations do not claim it solved.

| Rubble park | Travel before → after (m), MEASURED | Sleeping/parked ticks after | Moving-belt ticks after |
|---|---:|---:|---:|
| flat | 0.000179421 → 0.000179421 | 1280/1280 | 0 |
| rammed | 0.020582624 → 0.021932388 | 1280/1280 | 0 |
| house | 0.001984692 → 0.014056585 | 1280/1280 | 0 |
| yard | 0.005523772 → 0.025241429 | 1280/1280 | 0 |

All rubble observation windows contain 1,280 ticks. Arrivals differ with the brake construction;
these are complete executions, not identical parked-pose comparisons.

### Assignment counts

**MEASURED deterministic counts**, including candidate-price solves, static-to-dynamic restarts,
fixture arrival and sleeping ticks (zero work). The turn aggregate covers the 71 case bodies
and brake control that emit count records. Means below weight each record by its tank-ticks.
There is no measured round-2 counter baseline and no wall-clock cost claim.

| Recorded run | Max assignments/tank-tick | Mean assignments/tank-tick | Tank-ticks |
|---|---:|---:|---:|
| Turn/pivot cases + brake | 492 | 40.013534 | 146,516 |
| Slab pull | 237 | 184.784375 | 320 |
| Rubble arrivals + park | 329 | 33.204672 | 8,604 |
| pivot slope25 uphill | 39 | 7.891927 | 4,608 |
| pivot slope25 downhill | 39 | 7.663411 | 4,608 |
| pivot slope25 across | 39 | 7.784722 | 4,608 |
| rammed | 248 | 74.569822 | 2,757 |

The largest recorded tank-tick is flat F5 tight at 492 assignments; the largest rubble tick is
house arrival at 329. Every static restart keeps its accepted combination face; no assignment
is cached across ticks.

## Verification

The full library suite ran **once after the merge**: **2,843 passed, two failed, 50 ignored**.
Both failures were stale expectations exposed by the brake change, and were then repaired and
rerun directly:

- The loose-piece wake test required at least one coasting tick after release. Stronger brakes
  legitimately produce zero. Wake, forward reaction, external-push wake and sustained sleep
  remain asserted; coasting duration is a MEASURED observation. Its settling helpers now assert
  eventual rest without fixture deadlines.
- The cold slope test omitted rolling resistance when requiring persistent downhill movement.
  On sand it observed rest at 31.35 degrees, above grip alone but inside grip plus rolling.
  The witness now chooses above the full resistance threshold: the lower of
  `atan(mu + rolling)` and `atan(rolling) + asin((static_brake/weight)/sqrt(1+rolling^2))`
  (DERIVED from the existing force balance). No physical parameter or production law changed.

After those test corrections, the direct wake and slope reruns each pass. Final targeted checks:
**375 track tests, 18 protocol tests, three doc-citation tests and 54 spec tests pass**. CI clippy
(`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`) and
`cargo fmt --all --check` pass. The 71 turn/pivot cases, slab pull and four-case rubble park probe
all pass. `scripts/serial-cargo check --no-default-features --bin overmatch-server` passes on the
final source. All test/probe runs used `OVERMATCH_UNATTENDED=1` and their existing headless mode.

Merged `main` at `f81ab128`, bringing timing slice 1, the audio-free server and AWS scripts.
`PROTOCOL_REV = 105`; normalized own-wire-type pin **`0x322da02509ae4264`**. The protocol tests
confirm the combined wire graph. The full suite was not repeated after the two expectation and
probe-only telemetry corrections; production physics did not change after that full run.

## Additional findings and slice 3

- Finite arithmetic alone does not admit a construction: independent rows must also be resolvable.
  An extreme high-rpm/low-belt-inertia fixture now exercises the conditioning refusal.
- A sliding hold exit must actually order the replacement brakes, including with no operator;
  clearing its latch alone leaves an unstaffed released construction coasting.
- A spring-closed unstaffed clutch can discharge stored crank energy into a hold. A no-gravity
  fixture does not thereby prove that its full coupled demand fits static capacity.
- Assignment telemetry must be cleared before a probe tick, then read after it. Deciding from
  sleep status afterwards would omit the final solve of a tick entering sleep. Sleeping ticks
  now contribute zero without losing that transition's actual work. Only diagnostic data resets.
- A second numerical rank floor can misclassify tiny independent rows as exact dependencies.
  Construction admission uses direct minors of the three-speed row matrix, then the runtime
  bordered rank check. Fixtures at angle scales down to 1e-20 are refused; exact dependencies
  remain admitted. The impulse limit is a speed-coordinate column and cannot resolve a physical
  null impulse, so dependent sets need no extra conditioning decision.
- Slice 3 relocates this same row integration to substeps, separating tick-owned Driver orders
  from physical advancement. Contact feedback and joint hull/contact integration remain slice 4.
  Counts in this report price assignment work; they do not establish server wall-clock capacity.
