# Drivetrain slice 2: final review closure

Subject: Overmatch's multiplayer video-game driving physics, game state only. The unstaffed
proof concerns the game's damage model: a Driver station is out of action, represented by game
state only. Lane `lane/drive-crank`, bay 2. The design narrative belongs to
`.agents/docs/design/track-model/transmission-design.md`; this report records checked findings
and observations for the owner's feel. No parameter was tuned to recover an old output.

## Checked findings

Every review/lens finding was checked against source. All actionable findings are addressed.
The review's slower-throttle-launch inference is rejected by matched traces; see the diagnosis.

| Finding | Disposition |
|---|---|
| P1: any positive hull reading opens the ladder | Fixed in `driver::choose`: rest uses `park_engage_speed()`. Above the launch range, a rolling selection also needs `speed * ratio / sprocket_radius >= launch_omega(command)`. No new number. The shared unchosen-launch path covers spawn and direction changes. Regression witnesses cover creep/rollback on both ladders and rolling readings below/above upper-gear clutch closure. Both new tests failed before the fix. |
| P2: unexplained zero coasting count | Traced the full throttle phase before and after the selector fix; see below. Restored the moving-belt assertion. The prior report's explanation involving release braking was wrong: the first count reads the pre-release belts. |
| P1/P2: wrong brake device | Deleted `mean_hold_brake_count`. The sheet authors the neutral-turn gearbox brake's independent `mean_hold_capacity_n = 192000`, labelled `Unknown`: missing shaft torque and reduction, settled by the OLVAR manual or measurements. Parking applies both side stops at their per-side rating and therefore resists differential motion. |
| P3: two hold exits | One retiring-hold rule keeps it only while there is static support to preserve: a stationary mean with differential motion retains the mean row; exact belt rest transfers to both side rows; a sliding mean admits side stops immediately. An along-ladder request can then release park to its one-way launch stop. |
| P3: provenance | The static/sliding ratio is `StandIn`, naming the missing lining fact and handbook example. The slope's `Measured` label describes only the slope. The rated-grade brake rule's citation is beside `TransmissionSpec::brake_capacity_n`. |
| P3: conditioning claim | The refusal names actual speed, radius and inertia fields. The check's comment states its unloaded-metric precondition and derives why increasing effective belt inertia can worsen rank separation. There is no authored upper bound on contact slopes, so this is explicitly not a certificate of all loaded metrics. Runtime still requires a resolvable optimal independent KKT basis. |
| P3: duplicated fixture facts | The drive fixture reads mass, slope, independent mean rating and static factor from the sheet, and gravity from `derive::G`. |
| P3: unbounded rest loops | Finite watchdogs derive arrest time from reflected crank, belt, hull and angular momentum divided by the lesser of side-brake and contact force; suspension damping and Avian sleep dwell supply settling time. A labelled fixture margin allows sequential settling. All touched rest loops use this owner, including the previously fixed-duration helpers. |
| Lens brevity: duplicated coupling geometry | `Coupling::jacobian` supplies admission and runtime clutch, steering and stop rows, preserving signed runtime ratios and positive admission magnitudes. |
| Lens testing: pinned case count | Replaced `6048` with coverage for every catalogue construction, its side stops and each supported mean-hold class. The authority/order invariants remain. |
| Lens error handling: late `TrackGear` check | `RigGeom::build` returns the admitted `TransmissionParams` with geometry; `TrackGear::from_construction` consumes them without revalidation. Geometry-only consumers explicitly discard the parameters. |
| Lens brevity: duplicated test setup | Opposing/along-ladder handover setup shares one named throttle; every assertion remains. |

The first post-change targeted run found two false fixture premises, investigated before changing
expectations: `pivot_release_hands_static_load_to_side_stops_from_exact_rest` and
`parked_static_load_survives_pivot_and_service_handover` selected their supposedly supported load
from the side rating alone. The independent mean brake can be weaker. Their load now derives from
the lesser of the side rating and half the mean rating, within that device's static interval.
The exact-rest transfer and zero-mean assertions are retained. No production rating changed.

A final read found that `TrackGear::bitprobe_startup` omitted the mean rating. Now that the
rating is independent, startup telemetry emits it whenever the construction has a mean brake.
This is diagnostic data only. The current-mechanics section of the design document also removes
the false side-disc-to-mean linkage and states the shared handover and free-selection rules.
The pre-telemetry full suite passed 2847 tests with zero failures
and 50 ignored; the required checks were repeated after this final source edit.

No lens-backlog row names the edited items. The orchestrator owns the independent six-lens landing
review. The existing general solver, independent ADMM/Cholesky reference, limit-face retention and
assignment counting remain; cross-tick warm starting stays absent because nonunique impulses feed
the Driver's `driving` observation. This change adds no switch, fallback or alternative solver.

## Wake-test diagnosis

**MEASURED before the selector fix**, command 1 for 32 ticks (half a simulated second): the loose-
piece wake fixture selects F8 on the first driven tick and stays in `Slip` for all 32 ticks.
At tick 0 its belts read `[0.008858673, 0.008858673]` m/s. At ticks 27–31 they read
`[0, 0]`, `[0.54429775, 0.54429775]`, `[0, 0]`, `[0.5782018, 0.5782018]`, then
`[0.034809906, 0.034809906]` m/s. The final hull reading is 0.12295915 m/s; forward travel is
0.057429716 m. The first release count therefore sees both belts below the Driver's 0.05 m/s rest
threshold, before any release braking. There are zero coasting counts. The alternating response is
an observation of the existing delayed loose-piece/contact coupling, not a new law or target.

**MEASURED after the fix:** F4 replaces F8, still `TakingUp`/`Slip` for all 32 throttle ticks.
Every throttle-phase belt and hull sample is identical to before, including final belts
`[0.034809906, 0.034809906]` m/s and 0.057429716 m forward travel. It is therefore wrong to infer a
slower throttle-phase launch from the zero count. The crank is still spooling: it ends at
199.49248 rad/s, below the 219.9115 rad/s take-up floor, and the clutch transmits zero force in
every throttle tick. The small motion is the loose-piece/contact response released by the lever,
not engine propulsion.

The distinction appears during pedal release. The raw command becomes zero immediately, while the
served pedal falls by 0.0625 per tick (DERIVED from the authored 4/s slew and 64 Hz clock). Its
take-up floor falls with it. At release tick 1, crank speed is 202.72093 rad/s and the floor is
200.27654 rad/s (MEASURED), admitting clutch impulse. Both runs retain side park and have the same
crank trajectory. The old F8 delivers 59.787137 kN through the clutch and both belts remain zero;
the fixed F4 delivers 295.0365 kN, which, with the sampled ground reaction, causes the parked rows
to break away on alternating ticks (MEASURED). Release ticks 1/3/5/7/9/11/13 end at belt speeds
0.36795294/0.38693446/0.44527423/0.5018024/0.4740664/0.45720527/0.46654296 m/s in the fixed run
(MEASURED). The next iteration counts each of those **seven** moving-belt ticks and asserts wake.
The first release count is zero in both runs; later counts depend on release mechanics.

For isolation, the old selector was temporarily replayed with the same final instrumentation and
all other fixes present: the restored `moving_ticks > 0` assertion failed with zero. Restoring the
fixed selector passes with seven. All temporary instrumentation and the selector mutation are
removed. The count difference is caused by item 1, but the mechanism is the release transient,
not a slower throttle phase or braking that happened before the first sample. The separate
explicitly coasting-belt wake witness remains. No contact law or release parameter was changed.

## Observations for the owner's feel

All probe readings here are MEASURED observations, not acceptance targets. Numerical comparisons
from prior reports remain references, never obligations. Raw logs live outside git under
`/tmp/drive-r4-*.log`; the existing belt-friction runner owns its CSV artifact.

### Brake interpretation

Per-side sliding rating is `mass * derive::G * sin(rated_slope) / 2`, independently of ground grip
(DERIVED). For the sheet's measured climb rating and mass this is 160.36 kN sliding and 192.44 kN
static per side (DERIVED); the independent mean brake is 192/230.4 kN with the shared StandIn
static ratio (DERIVED). The mean's force convention is total force at the mean belt coordinate.

The previous level service stop gave 5.806323 m/s², about 0.59 g (DERIVED from MEASURED starting
speed and stop travel). This exceeds the research's MEASURED fleet range 0.29–0.50 g and the period
manual's 0.36 g. It is an observation for feel, with no retuning. At the rated slope, equality of
sliding brake force and gravity balances the grade; stopping there also depends on the existing
rolling/contact response. A track that is not rolling still meets rolling resistance through hull
travel in the current contact law (`forces::rolling_resistance`); whether it should is an owner's
law question outside this lane.

### Slab pull

MEASURED, zero-based driven ticks: **F4 at 0 → F5 at 94 → F8 at 183**. The latter two changes pay
shift windows; final hull speed is **3.929259062 m/s**. In/Slip/Out occupy **61/155/40 ticks**.
The faulty-selector run's F8-at-0, 2.498057842 m/s and 0/256/0 regime counts are prior MEASURED
references, not targets. The corrected initial choice is within the sheet's launch range.

### Every 25-degree pivot release

MEASURED by the existing 64-second hold / five-second release probe. Held yaw and rpm average the
last three hold seconds. Travel is planar displacement over all 320 release ticks. The two along-
slope held yaw readings change materially with the weaker independent mean rating; no rating was
tuned in response.

| Case | Held yaw (rad/s) | Held crank (rpm) | Release travel (m) | Final belts (m/s) | Side-park ticks / release ticks |
|---|---:|---:|---:|---|---:|
| uphill | -0.004955066 | 2055.2193 | 0.082886255 | [0, 0] | 305 / 320 |
| downhill | -0.006217925 | 2055.0166 | 0.082052652 | [0, 0] | 305 / 320 |
| across | -0.006260733 | 2055.8626 | 0.080393597 | [0, 0] | 305 / 320 |

All three retain the mean row for the first 15 release ticks, then park. Hold travel is
34.493518829 / 21.897665024 / 12.318467140 m respectively; release headings are
3.123540401 / 3.083841562 / 1.721987605 rad (MEASURED). These observations include slope/contact
motion and do not assert that the neutral gearbox brake holds every orientation on that grade.

The shipped unstaffed proof passes: after the game's Driver station goes out of action, mean hold
hands to both side stops at release tick **1**, sleep begins at tick **77**, inside the derived
**1043-tick** watchdog (MEASURED). It asserts park present, mean latch absent, and actual Avian sleep.

The level service-stop probe is unchanged: 2.183719158 m/s start, 0.410641074 m peak travel,
5.806323051 m/s² equivalent deceleration (DERIVED from MEASURED speed/travel), park at tick 28,
233.407974 mm/s peak reverse speed and 8.016706 mm rock-back (MEASURED). The probe requests a
run-up of at least F3 and actually pulls the brake in F5; its label is not a gear constraint.

## Verification

MEASURED command results after the last source edit (temporary traces and the selector mutation
are absent; the independent rating remains in startup telemetry):

- `cargo fmt --all --check`: pass.
- CI clippy, `--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`: pass.
- `track::`: 377 passed, zero failures.
- `net::protocol`: 18 passed, zero failures.
- `tests::doc_citations`: three passed, zero failures.
- `spec::`: 54 passed, zero failures.
- Final direct wake and unstaffed proofs: each passes; release count remains seven, transfer/sleep
  remain ticks 1/77.
- Full library suite, once after the final source edit: **2847 passed, zero failures, 50 ignored**.
- Exact `check --no-default-features --bin overmatch-server`: pass. It retains three existing
  dead-code warnings for audio data and the UI-only steering readout; CI-feature clippy is clean.

All compile/test commands use `scripts/serial-cargo`, with `OVERMATCH_UNATTENDED=1` and the
runners' own headless mode. The explicitly requested server-only command uses no default features;
the other builds use `dev_ui,bitprobe`.

`main` is already an ancestor, with no new commit to merge. `PROTOCOL_REV = 105`, one above main's
104, and normalized own-wire-type hash **`0x322da02509ae4264`** stays pinned. This round changes
authoring data, not a wire definition or codec; the protocol tests verify that the pin still
matches. No pushes or merges into main are performed by this lane.
