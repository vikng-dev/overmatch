# Drivetrain slice 2: review closure

Subject: Overmatch's multiplayer video-game driving physics, game state only. Lane
`lane/drive-crank`, bay 2. The game's damage-model proof represents an out-of-action Driver
station as game state only. The design narrative remains in
`.agents/docs/design/track-model/transmission-design.md`. No parameter was tuned to recover an
old output; observations below are for the owner's judgement.

## Checked review findings

Both supplied reviews were read in full and checked against source. Their actionable findings
are real. The requested implementation choices are:

| Finding | Disposition |
|---|---|
| P2-1: free-selection gaps | Fixed: free selection admits the launch range, closed-clutch ratios and the lowest crank-safe gear. The construction supplies every boundary. The sweep covers all four class drivetrains, both ladders, both speed signs and pedal hundredths through each ladder's crank-safe speed range. The reported bands and both rolling subcases failed before the fix. Both rolling subcases now assert a selected launch state. |
| P3-1: rest clause | Deleted: at rest or contact creep, upper ratios neither close their clutches nor qualify as the lowest crank-safe gear, which is already in the launch range. The creep/rollback witness remains. |
| P2-2: gearbox-brake rating | Fixed: the sheet's `mean_hold` carries only a StandIn label naming missing gearbox-brake torque and reduction. `TransmissionSpec::mean_hold_capacity_n` derives total mean force as twice the side-brake owner's per-side capacity: weight times sine of rated slope. A sourced torque and reduction would replace this rule. The admission test varies mass and slope and refuses an Unknown label. |
| P3-3: static factor | Fixed: the shared StandIn explicitly covers both side discs and the gearbox brake until either has a sourced lining ratio. |
| P2-3: wake without drive | Fixed: the throttle phase ends at the first positive clutch force, read through the existing bitprobe telemetry. Its finite watchdog derives spool time from crank inertia and minimum full-pedal net torque, adding pedal travel and one sensing window. The release no longer starts after an arbitrary half-second with zero clutch drive. |
| P3-2: solver panic | Reworded: runtime requires an independent optimal KKT basis resolvable in the loaded metric. Admission certifies only unloaded conditioning; it does not bound contact slopes. No stronger guarantee is claimed. |
| P3-4: watchdog units | Corrected the units and classification: `1/(damping_ratio * ride_frequency)` uses Hz and is a Fixture allowance of `1/damping_ratio` ride periods, equal to TAU amplitude-decay times, not one physical decay time. The existing conservative allowance and four-interval Fixture margin remain. Terrain grip is explicitly a watchdog estimate for flat fixtures, including slabs and loose pieces, rather than their actual contact force. |
| P3-5: unbraked paid shift | Confirmed and reported below, with no behavior change requested. |
| P3-6: closure sense | Fixed: the Driver already receives current mean belt speed. Its reading now retains that speed; free-selection shaft speed and landing price consume it. A sensor-to-selector witness skids the hull over stationary belts on every class and both ladders. Paid-cut forecasting retains its existing hull ledger. |
| P3-7: stale report | Replaced the stale report with current decisions and measurements, including the merged main commit and wire union below. |
| Lens: fixture brake formula | The fixture calls `brake_capacity_n` and the mean-capacity owner; neither force law is copied. |
| Lens: settling loop | `until_resting` returns elapsed ticks; the wake test uses it and retains the sleep assertion and measurement. |
| Lens: duplicate assertion | Deleted the final duplicate; its richer diagnostic is in the per-tick assertion. |
| Lens: sandbox readmission | The sandbox keeps the transmission and inertia admitted by `RigGeom::build`; retuning also consumes the admitted inertia. The chord-exact geometry rationale remains. |
| Lens: late inertia check | Running-gear admission returns its reflected inertia with the transmission. `TrackGear::from_construction` consumes it directly; no fallible recalculation remains. Geometry-only callers discard the extra return explicitly. |

Scope of the selection guarantee: a propulsive pedal with a sensing reading cannot remain
unchosen inside the top ratio's crank-safe shaft-speed range. Above that construction limit no
ratio can safely land; the existing open-clutch overrun remains deliberate. An initial ledger
without enough closed samples is also a named, bounded initial state. Neither is one of the
reported between-ratio gaps.

No lens-backlog row names an edited drivetrain item. The merge brings main's existing backlog
forward. No new feature, operator switch, fallback or retained alternative is introduced. The
wake witness reads the existing bitprobe feature, whose tracked probe runners are its user.
The orchestrator owns the independent six-lens landing review; this lane neither pushes nor
merges into main.

## Wire union

Merged main at `8337127d33a32389e144d872d0bda5f48d53c146`, including timing slice 2's revision
105. The conflict in `src/net/protocol.rs` is resolved at **PROTOCOL_REV 106**. The normalized
own-wire-type union hash is **`0x4d79ba6d019c1235`**, printed by the merged manifest tripwire and
pinned. This includes the lane's `mean_hold`/`launch` transmission state and main's timing wire
changes. The drivetrain authoring schema and sensor reading do not add wire state.

## Observations for the owner

All readings below are observations, never numerical acceptance conditions. Logs are outside git
under `/tmp/drive-r5-*.log`; the existing belt-friction probe owns its per-tick CSV. No duration,
ratio, force, inertia or contact parameter was tuned for these outputs.

### Mean hold and side-park handover on a slope

DERIVED from the shipped mass, gravity and rated slope: total mean sliding/static capacities are
320.727/384.872 kN. Side capacities remain 160.363/192.436 kN each. The total mean rule shares the
service-brake grade; it does not assert a sourced gearbox torque or lining ratio.

MEASURED by the existing 25-degree pivot probe: each case begins parked, holds full steer for
64 s, then releases for 5 s. Held yaw averages the last 3 s. Downhill slide is displacement
projected onto the world's downhill direction, in the horizontal plane; surface distances are
DERIVED by dividing by cos(25 degrees). Planar hold travel also includes transverse motion.

| Initial facing | Held yaw (rad/s) | Planar hold travel (m) | Downhill slide, planar / surface (m) | Planar release travel (m) |
|---|---:|---:|---:|---:|
| uphill | -0.083504 | 12.2131 | 12.1813 / 13.4406 | 0.02349 |
| downhill | -0.124658 | 12.7913 | 12.7409 / 14.0580 | 0.04279 |
| across | -0.006261 | 12.3185 | 12.3041 / 13.5760 | 0.03299 |

MEASURED: every held tick retains the mean latch, with final hold belts exactly opposite:
[0.236439, -0.236439], [0.236587, -0.236587], [0.236801, -0.236801] m/s respectively.
All releases retain mean hold for 15 ticks, then side park for 305 ticks; final belts are zero.
The pure construction tests also retain exact mean rest through parked-to-pivot and
pivot-to-side-park handover across the supported class drivetrains and both load signs.

The stronger mean brake restores substantial uphill/downhill yaw, but the hull still moves
roughly 12 m downhill during a long pivot. That remains outside the stated believable expectation
that a tank asked to pivot within its rated grade does not slide away. It is **not** reported as
resolved. The belts retain exact zero mean while the hull slides at the contacts. This is hull/contact
slip while the mean constraint holds, not evidence that its rating should be increased again.
The across-slope case also makes very little yaw. These are owner-visible observations of the
remaining contact/steering behavior, left unchanged rather than hidden by a new rating or gate.

The game's damage-model unstaffed proof also passes: passive mean hold transfers to side park at
release tick 1; actual Avian sleep begins at tick 77, within the 1043-tick watchdog (MEASURED).

### Four feel observations

1. **Pedal dead time.** MEASURED: a sleeping standing-start fixture sends its first positive clutch
   force on completed driven tick 35, **0.546875 s** after demand. The slab pull independently
   records its first positive clutch force at zero-based tick 34 (the same completed step),
   **19.591 kN**. Before that the crank spools without track drive. DERIVED by reading merged main's
   `crank_guards` shows the same outcome for this input: its slipping-clutch floor is
   `launch_omega(propulsive).min(reachable)`; when the freely reachable crank remains below the
   floor, its entire available torque accelerates the crank and none reaches the belts. Its
   construction inertia and keyboard slew are the same. Main therefore also has spool dead time;
   this is a source conclusion, not a claimed measurement of main's exact duration. Presentation
   delay is additional, and the shipped sheet still has no engine sound loop.
2. **Paid upshift to a slipping upper gear.** MEASURED on the 4-second slab pull, zero-based driven
   ticks: **F4 at 0 -> F5 at 94 -> F8 at 183**. F8 is selected at **3.2806 m/s (11.810 km/h)**;
   after its 20-tick cut, first take-up is tick 203 at **3.2938 m/s (11.858 km/h)** hull speed,
   with **3.0136 m/s** belt speed. It slips for all **53 remaining steps, 0.828125 s**, with no
   closure before capture ends. This is a lower bound on the full slip duration, not an invented
   completion time. At the end the hull is **3.929259 m/s (14.145 km/h)**, still slipping. DERIVED:
   F8's full-pedal 2100-rpm floor closes at **8.82778 m/s (31.78 km/h)** belt speed, from its
   45.4-km/h-at-3000-rpm speed anchor. The paid-choice rule can select a viable slipping ratio;
   the free-selection fix deliberately does not change that rule.
3. **Unbraked paid change from a stall.** DERIVED from control ordering: an along-ladder request
   releases park before the Driver starts a paid cut; the cut opens the clutch and disables the
   one-way launch lever. Thus that case has no commanded stop for the **20-step / 0.3125-s**
   window (ceil(0.31 s * 64 Hz), from the sheet and clock). Merged main's `update_park_latch`
   retains park below its rest threshold during that window. The lane behavior is unchanged;
   clutch-out motion on a grade is a feel decision, not silently restored to main's behavior.
4. **Level service stop.** MEASURED: **2.183719 m/s** initial hull speed, **0.410641 m** peak forward
   stopping travel, stop at tick **26**, park at tick **28**, **233.408 mm/s** peak reverse speed
   and **8.017 mm** rock-back. DERIVED equivalent constant deceleration `v^2/(2*travel)` is
   **5.806323 m/s^2 = 0.591878 g**. The probe asks for a run-up to at least F3 and actually brakes
   in F5; the label does not constrain the braking gear. No brake or contact number was tuned.

### Wake witness

MEASURED: the loose-piece fixture sleeps after 58 settling ticks. Its first clutch drive occurs
at tick 35 inside the 67-tick inertia/torque watchdog, after 0.06582 m of total hull motion.
Release exercises 533 moving-belt ticks and then sustained sleep; the external-push arm moves
0.003470 m along its push in two ticks and also returns to sleep. These are observations, not
held tick counts. The raw-intent park latch versus slewed-pedal fuel split still exists on both
this lane and main; release can send residual drive into its own side brakes.

The review's watchdog concern is a label error: the retained suspension allowance is
`1/damping_ratio` periods at a frequency in Hz, equivalent to TAU amplitude-decay times. A trial
using one actual decay time shortened the loose-piece sleep watchdog too far and failed the wake
proof. No model parameter changed; the final code keeps the existing conservative Fixture
allowance and states its units accurately.

## Verification

Required final checks and observation runs:

- `cargo fmt --all --check`: pass.
- CI clippy, `--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`: pass.
- `track::`: 379 passed, zero failures.
- `net::protocol`: 19 passed, zero failures, including the merged wire pin.
- `trace::`: 10 passed, zero failures, including the state-hash pins.
- `tests::doc_citations`: three passed, zero failures.
- `spec::`: 55 passed, zero failures, including stand-in admission and its rated-grade rule.
- Exact `check --no-default-features --bin overmatch-server`: pass; three existing dead-code
  warnings for audio data and the UI-only steering readout remain. CI-feature clippy is clean.
- Wake, unstaffed, slab-pull, 25-degree pivot and service-stop probes: pass. Their emergent
  readings above are observations; a passing probe does not certify the slope-feel expectation.
- Full library suite, once after the last source edit: **2838 passed, zero failures, 50 ignored**,
  in 389.02 s (MEASURED).

The red selection run failed both strengthened cases before the selector fix. A trial converting
the conservative watchdog allowance to one decay time failed the wake proof; the final period
label and existing allowance pass. Clippy's new-test `err().expect()` finding was replaced by
`expect_err` before the final successful check. No temporary instrumentation or alternative
implementation remains.

All builds use `scripts/serial-cargo` and `OVERMATCH_UNATTENDED=1`. The explicitly requested
server-only check uses no default features; all other builds use `dev_ui,bitprobe`. No source
edit follows the start of the final full library suite.
