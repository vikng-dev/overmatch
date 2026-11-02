# Driving final-review remainder (#146)

Subject: the game's driving model. Where staffing is mentioned, the game's damage model means HP pools at stations only.

## Reverse construction and crank admission

Commit: `driving: give reverse gears their own steering rows (#146)`.

Verified Claude's reverse-row defect against the previous lane's executed red: forward speeds
[5,12], reverse [10], reverse tight radius 6 m read 3 m. Those numbers are inline Fixture data.
Each ladder now authors its own required radii. The observer, candidate pricing, steering solve
and debug HUD read the active ladder; the startup construction fingerprint includes reverse rows.
The regression checks the observer radius, candidate turn load, steering Jacobian and HUD together.
Missing rows name `transmission.steering.reverse_radii` at admission. The shipped sheet, spec,
inline bake sheet and Blender sheet change together.

The previous lane also executed a red for finite scalar inputs with overflowing crank energy.
Admission now bounds the virtual free tick's energy using the guarded crank plus the admitted
fuel-cut width; it refuses overflow before publishing parameters, naming engine inertia/curve
and crank energy. No engine or over-rev outcome changed. No wire type changed; `PROTOCOL_REV`
is deliberately unchanged.

MEASURED verification: formatting and CI clippy pass. The broad run passed 439 tests, including
both regressions and all headless checks, with four ignored; three test failures were corrected:
a reverse-index fixture needed matching rows to isolate u8 admission, and two overly broad test
edits incorrectly demanded an energy error for other refusals. The corrected affected-module run
passed 416 tests, including track, spec, bake, command, crew UI and whole-tree source guards.
No backlog row names these edited items. Existing unrelated bake rows name `src/bake.rs`, which
this change does not edit.

## Clock and queues

MEASURED start: 2026-11-02 21:48:36 UTC; the lane's sixty-minute boundary is 22:48:36 UTC.
Builds and tests use `serial-cargo` with the unattended/headless/mute environment. Queue waits
are taken from the build queue's queued/started timestamps, not inferred from compilation time.
The first correction waited behind another session's whole-library run; final totals follow below.

## Reachable notch admission

Commit: `driving: admit the notches the client actually reaches (#146)`.

Verified Codex's binary32 counterexample from the preceding lane. Admission now traverses the
actual tap transition in both directions from neutral and both clamped endpoints. The client
calls that same `DriveFeel::tap_notch` operation. MEASURED: step 0.4 reaches thirteen states;
one decrement from one equals tight_on 0.6 exactly, and admission refuses it. Moving that
Fixture threshold to 0.65 is admitted. The regression also checks transition closure.

Decision: a Resolution budget of 4,096 reachable states bounds startup memory and work; a finer
lattice is refused by name with an authoring fix, rather than making startup enumerate millions
of binary32 states. This is an admission cost bound, not a control threshold. Default shipped
keyboard feel is unchanged. Formatting, CI clippy and 417 affected-module/guard tests pass.

## Tank #2 and executed mutations

Commit: `driving: prove a second construction through the drive laws (#146)`.

Claude's launch-domain/mass coverage findings and Codex's selector-controlled part-pedal test
were verified by mutating the production functions, not clones. Each mutation was compiled,
executed, and restored before the next; no mutant or alternative remains in the tree.

| Production mutation | Executed command filter | MEASURED result |
|---|---|---|
| `launch_pick`: launch limit hard-coded to four | `launch_selection_obeys_declared_domain` | red: one failed test, construction launch limit violated |
| `coupled_mass`: hull mass hard-coded to 57 t | `coupled_mass_uses_each` | red: one failed test, 73,000 kg versus the independently derived 42,500 kg |
| `launch_pick`: partial pedals return `None` | `part_pedal_launches launch_selection_obeys_declared_domain` | red: two failed tests, affordable launch selection missing |

The second construction's declared limit of two is now checked by the generic domain assertion.
Capability comes from independent force affordability, so the selector cannot decide whether
sustained travel and settling assertions run. The shared inline `compact_spec` owns the fixture
mass, curve, ladders, radii, capacities and recirculation. It retains the seed's running-gear
geometry, but changes the driving construction; this is not a claim of a second shipped asset.

Fixture differences include a nonzero curve tail beyond governed rpm, friction 0.65 rather than
the arithmetic lab's 0.9, recirculation 0.65 rather than 0.9, and a lower power/weight. DERIVED
maximum authored-knot power/weight is about 6.52 kW/t versus the lab's 13.10 kW/t; these are curve
samples, not measurements of delivered power. No production tank-specific branch was added.

The validated spec goes through production `TrackGear` construction, `belt_tick`, `observe_rig`
and contact. The narrow integration fixture holds attitude/ride height on a ten-degree plane
and integrates longitudinal hull motion from emitted forces. MEASURED at 18,000 kg: grade
30,662.777 N, rolling load 6,955.8926 N, grip 178,446.06 N, final-second speed range
[3.1315382, 3.1315715] m/s. It sustains and settles at a real load. No rollback policy is tested.
Formatting, CI clippy and all 420 affected-module/guard tests pass after restoring production.

## Driver control seam and review dispositions

Commit: `driving: align Driver authority and HUD task rate (#146)`.

`control_authority` is the one absence encoding. `DriveIntent::Unmanned` and its test arms are
gone; a request can remain present while staffing masks its actuation. A phase hold still
works service brakes without staffing. The shared `DriveSupply::served_task_rate` gives both
fueling and the actual status-panel system zero for unstaffed controls, including an available
Backup path. The mounted-panel regression checks absent/staffed controls under ordinary input
and phase holds, then the legitimate no-controlled-tank hide state. The input regression now taps
while the Driver station is empty and proves that staffing resumes the latest notch.

`driver` reads intent from its input rather than a redundant parameter. Required weight and
belt inertia enter its selectors through `DriveConstruction`, with one owner of the grip
requirement; downstream selectors no longer validate and then re-expect optional grip. Invalid
active gears and missing/duplicate required crew widgets panic with their construction invariant.
The reverse steering solve already lost its clamped index and invalid-detent fallback in the
reverse-row commit. Legitimate unclamped-root absence stays named in S3, which now requires at
least one actual root and therefore cannot silently skip all delivery/settling assertions.

The false Brake/park and clutch-work docs, live-crank pricing-test claim, no-op backward-crank
staffing arm and edit-history comments are corrected. The upshift refusal and its accurate
reason remain unchanged. The engine-control file path was already correct at this lane's start;
it was not a current defect. The inverse-only unit test remains a narrow inverse-law witness;
S3, not that unit test, owns the shipped delivery claim.

The first whole-library run passed 2,248 tests and exposed one device fixture failure. A focused
red named the missing crew bar: that minimal app installed `crew_ui` without `UiFonts`, and its
ignored system-validation errors had concealed the skipped match-entry spawner. The fixture now
supplies its fonts, fixed time and weapon clock before match entry. Its device test passed after
the correction. No damage-model behavior changed; this is the game's UI/device composition.

Ratchet: view-14 and view-15 are closed. The crew-panel and crew-bar sites are removed from
view-08 after verifying their cardinality guarantee; that row still tracks its untouched reticle
sites. No other backlog row names a file edited by this lane. Self-review covered error handling,
branching, base truths, testing and brevity. No protocol/wire type changed, so `PROTOCOL_REV`
remains unchanged. Both rounds of the three production mutations executed and went red; the
second round ran after the checked-construction/authority interface change.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | Reverse rows read forward construction | Verified red; observer, pricing, solve and HUD read independent data | driving: give reverse gears their own steering rows (#146) |
| Claude / resumed admission review | Finite scalar inputs overflow crank energy | Verified red; refused before parameter publication | driving: give reverse gears their own steering rows (#146) |
| Codex | Decimal notch admission misses binary32 collisions | Verified thirteen-state transition closure and refusal | driving: admit the notches the client actually reaches (#146) |
| Claude + Codex | Second-construction coverage and selector-controlled assertions | Verified with actual production mutants and loaded spec/contact composition | driving: prove a second construction through the drive laws (#146) |
| Claude | HUD rate, staffing/notch semantics and duplicate absence encoding | Verified with mounted panel, device and adapter tests | driving: align Driver authority and HUD task rate (#146) |
| Claude | Redundant intent, false docs, stale tests and required-construction outcomes | Verified against source and affected tests | driving: align Driver authority and HUD task rate (#146) |
| Claude | S3 can skip every root | Verified missing coverage requirement; a root is now mandatory | driving: align Driver authority and HUD task rate (#146) |
| Claude | Engine data-path sentence | Not a current defect; already points at assets/driving/engine.ron | No path correction needed |
| Claude | Inverse witness does not call step | Not a defect for its narrow claim; shipped delivery now has a non-vacuous S3 witness | driving: align Driver authority and HUD task rate (#146) |

## Decisions and player outcome

Decisions: require independent reverse rows rather than infer them from forward ratios; refuse
unrepresentable crank energy at admission; enumerate the client's exact tap with a bounded
startup budget; share one inline second spec while explicitly reusing seed geometry; price test
capability independently of the selector; execute actual production mutations and remove them;
keep authority as the sole absence fact and share served work with the HUD; use required driving
construction in selectors; fail fast on invalid gears/widgets; require an S3 root while permitting
named out-of-class inverse cases; preserve the narrow inverse unit test; correct the minimal
device fixture's inputs; preserve all reserved calls and the wire revision.

Players stop seeing Drive throughput credited to controls nobody staffs: the panel shows zero,
then the served Backup-path rate when controls are staffed. Existing shipped driving feel is
unchanged. A differently constructed reverse ladder gets its own physical and displayed radii.
Notch behavior is accurately stated and checked: taps during absence still edit the local lever.
In my judgment the requested driving bar clears apart from the owner's reserved calls: rollback
that service brakes cannot arrest, sub-idle landing/upshift policy, over-rev outcome and empty-
station descent. No new model test or predicate was added for those decisions; S3's requested
coverage check strengthens its existing witness only. Nothing is pushed or merged.

## Final gates and measured finish

Formatting, CI clippy, the focused device regression and all 420 affected-module/whole-tree
source guards pass. The corrected whole-library run passes: **2,249 passed, 26 ignored**, with
no failed tests. The ignored tests remain the repository's opt-in beds; none was added here.
The Blender fixture also passes Python syntax parsing. Final production contains no mutant.

MEASURED ready to commit: 2026-11-02 22:40:49 UTC; elapsed 52.22 minutes.
MEASURED completed build-queue wait: 862 seconds across 28 commands,
with a longest wait of 417 seconds. These waits are included in the lane clock.
There was no capacity stop, capture, pricing run, window, push or merge. All launched
checks terminated normally or returned their recorded test/build failure; no lane process remains.
All four requested items are complete within the sixty-minute boundary.
