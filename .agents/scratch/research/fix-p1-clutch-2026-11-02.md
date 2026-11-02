# Clutch lane verification (#146)

Subject: the game's driving and damage model, game state only; crew are HP pools at stations.
The lane fixes the clutch, fuel and guard findings against its starting tree. Gear selection and M
belong to lane 3; the ungated protective upshift, controls' rest, backward crank and ceiling belong
to lane 5. None of those paths was changed here. No in-scope review finding was rejected.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Both | P1: moving shift Out persists after authority loss | Verified red through `step`: Out instead of In after the paid window. | `driving: end an empty-station shift interruption (#146)` |
| Claude | P1: empty-station idle catch supplies propulsion | Verified red: the lab coast received +55,724 N per belt with zero task rate. | `driving: require working controls for the idle catch (#146)` |
| Codex | P2: staffed zero-rate task powers a neutral pivot | Verified independently with the old source rule: the pivot sustained 500 rpm after four simulated seconds. | `driving: require working controls for the idle catch (#146)` |
| Claude | P2: a moving lug restarts launch Slip | Verified red (Slip instead of In). The restored forward test also exposed In entering Slip after near-rest brake settlement. | `driving: separate launch and park from a moving lug (#146)`; `driving: pin clutch recovery and share the launch law (#146)` |
| Claude | P2: park breach closes the clutch; test removes transmission capacity | Verified red (In instead of Out) with a whole transmission. The test now pins the parked clutch state. | `driving: separate launch and park from a moving lug (#146)` |
| Claude | P2: catch/guard saturation premise is not admitted | Verified red: droop 150 and band 100 did not panic. Admission now names the file, fields and repair. | `driving: separate launch and park from a moving lug (#146)` |
| Codex | P2: endpoint-derived persistent notch misses admission | Verified red: step 0.4 admits the approximately 0.6 notch reached from a clamped endpoint. | `driving: separate launch and park from a moving lug (#146)` |
| Claude | P3: part-pedal torque mutant survives endpoint tests | Verified red with `command * command`: at command 0.25, lab net torque was -122.22223 instead of 290.27777 N m. | `driving: pin clutch recovery and share the launch law (#146)` |
| Claude | P3: deleted forward-motion assertion | Restored the original eight-tick observation; it was red at -0.38867676 m/s until launch admission required prior Out. | `driving: pin clutch recovery and share the launch law (#146)` |
| Claude | P3: stale clutch prose and deleted identifiers | Verified against current symbols and laws; corrected the asset capacity description, data-owned widths and design narrative. Also corrected the state inventory to 17 fields, including `band_confirm_ticks`, and its server-stream contract. | `driving: pin clutch recovery and share the launch law (#146)` |
| Claude | P3: gzip timestamp rationale missing | Verified `.mtime(0)` remained without its reproducibility reason; restored that reason. | `driving: pin clutch recovery and share the launch law (#146)` |
| Claude | P3: instrument duplicates launch law | Verified the copied formula; the probe now calls `TransmissionParams::launch_omega`. | `driving: pin clutch recovery and share the launch law (#146)` |
| Backlog driving-04 | Slew test pins authored timing | Verified the literal tick count; it now derives travel ticks from `drive_feel().rise_per_second`. The row retains its untouched command, HUD and network portions. | `driving: separate launch and park from a moving lug (#146)` |

## Measured empty-station behavior

`headless_test::drive_load::empty_driver_station_rest_and_descent` mounts the shipped simulation on
a uniform 10-degree plane. It empties the Driver station's HP pool and marks that station out of
action. The fixture observes an already-set park lever for four simulated seconds, then resets
the complete test rig to a 5 m/s downhill entry for eight seconds. Entry gear is the upper-middle
portion of the authored ladder. The numeric measurements are observations, not vehicle pass limits.

- MEASURED parked rest: `park=true`, clutch Out, crank 0 rpm after four seconds; hull displacement
  0.00020859459 m (0.209 mm rounded). The existing lever remains set; an empty station does not set
  an unset park lever. The transmission unit test covers both prior lever states.
- MEASURED 5 m/s entry on 10 degrees: 10.172409 m/s after eight seconds, F8, clutch In,
  crank 2413.494 rpm. Both induced fuel and catch were zero throughout. Forty interruption ticks
  occurred through the existing protective shifts; every moving tick outside those windows was In.
  The initial motor-force sample was -753.621 N total, a transient sample rather than steady drag.
- The measurement repeated bit-for-bit after the final test refinement. The disconnected crank
  assertion checks non-increasing energy rather than requiring every valid engine to stop within
  this fixture's four-second observation.

## Decisions

1. Reuse mean speed, prior clutch state and the existing park latch; add no state. A moving
   interruption expires with its window. Prior Out persists at rest without authority, and the
   park lever keeps neutral through a capacity-breached slide.
2. Launch Slip begins only from Out at rest under intent, then retains its target until shaft
   speed catches it. An already-engaged lug remains In, including near-rest brake settlement.
   There is no hard-coded launch-gear ordinal; lane 3 owns which gear is selected.
3. The catch requires control authority and scales by the Drive task rate once. Fuel and the idle
   guard share this capacity. A zero-rate task may spend stored crank energy but cannot replenish
   it. Without the catch, the disconnected crank runs down under construction drag; a moving
   engaged clutch continues transmitting motoring drag.
4. Require guard band >= catch droop: saturation by the guard is the actual premise. An exact
   two-to-one ratio would impose an additional constraint that the law does not need.
5. Admit notches reachable from zero and both clamped endpoints. Keep arbitrary otherwise-valid
   notch steps rather than require steps that tile the unit interval.
6. Restore the original forward-recovery observation after eight fixture ticks. Do not strengthen
   it into a never-reverse claim. Its red trace showed the unchanged gear cascade and then a new
   launch spool from In; requiring prior Out removes that cut without changing gear decisions.
7. Give the launch formula one owner and expose it only within the crate for instrumentation.
   Keep the gzip timestamp invariant beside its code. Keep the design narrative in its design doc.
8. Leave PROTOCOL_REV at its actual starting value, 70. Wire types, field order and the physical
   meanings of Out, Slip and In are unchanged; no new wire fact was added.
9. Close only the touched drive-test portion of driving-04. Keep the other named files and findings
   in the backlog for their owning lanes. No other open backlog row names an edited file.
10. Leave M, gear decisions, protective-upshift authority, controls' rest, the backward crank and
    ceiling to their assigned lanes. The measured descent still exercises the protective windows.

## Validation and handoff

All executable defects above have red evidence before their fixes; prose and duplication findings
were checked directly against source. Every commit passed formatting, CI clippy, targeted tests
and `doc_citations`. Final combined checks passed 147 tests, including `ui_ascii`, `net_boundary`,
`bake::`, the clutch/drive tests, bitprobe and the shipped headless fixture. The whole lib run passed
2,154 tests with 27 existing ignored tests. After that run, only the headless test assertion was
made independent of engine construction; its dedicated rerun passed, and the final 104 module/
citation checks, formatting and CI clippy passed. Production code did not change after the whole
lib run. No windows, captures, performance pricing, issue comments or pushes were made.

The lane stopped implementation changes at the 60-minute boundary while its final checks were
queued; the already-started checks completed shortly afterward. All work is committed on the lane.
