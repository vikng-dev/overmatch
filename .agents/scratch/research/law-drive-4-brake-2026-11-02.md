# Brake law (#146)

Subject: the game's driving model and damage model, game state only. Crew are HP pools at
stations. This record owns this lane's measurements, dispositions and decisions.

## Law and decisions

- The park lever starts on. Under intent, the capacity-limited stop force carries only the
  shortfall against rollback; it has no release latch, cooldown or engagement blend.
  This one-way constraint participates whenever intent is present, including a paid cut.
- Motion against intent gets full service brakes, independent of pedal magnitude. A moving
  opposite-ladder request refreshes a stopped countdown; only stopped ticks consume it.
  Release cancels it and keeps park. A held request then pays one direction cut.
- `assets/driving/actuation.ron` owns `reversal_hold_s`, Taste, 0.5 s. DERIVED at the existing
  64 Hz solver clock: 32 stopped ticks. A direction request already made from parked rest
  needs only its direction cut; the pause belongs to braking a moving reversal.
- Zero intent below the active ladder's first-gear idle speed stops and parks. Releasing above
  that threshold coasts without service or lever braking. Steering still releases park for
  the neutral turn. An unstaffed station retains its lever; its remaining control law is lane 5's.
- Launch selection reads requested pedal; released park may still preselect at full pedal.
  At rest, held intent admits launch Slip from In as well as Out. A moving lug stays In.
- An upshift cannot land below the curve floor: C1 would immediately correct that engaged lug.
  The red partial-pedal runs exposed the resulting shift/launch cycle. This is a new eligibility
  consequence of C1, using the curve's own first point, with no new tuned threshold.
  Pricing below the reachable crank guard uses the guard, rather than an impossible locked crank.
- Negative shaft speed no longer forbids gear recovery. The ordinary spare-force comparison
  can choose a lower gear under full service braking; the declared step/window limits remain.
- `HillHold` is derived from the one-way brake force; `GradeLimit` from an unaffordable launch.
  Neither is an independent brake latch. Scheduler status is cleared when no cut is active.
- Repair brakes mask the retained client lever; ending repair resumes the same notch. The
  command and adapter tests cover both ends. Backlog driving-03 remains for the empty-station half.
- HUD ratchet: show effective held input, centralize possession resets, use a two-valued press
  direction, avoid unchanged node writes, derive gauge bottom from row height and a Taste gap,
  and assert required HUD rows. The offline auto-fire injection is test-only.
- Protocol revision 75 deliberately names the nine-field drivetrain state and its new wire pin.
  The trace/bitprobe projection carries the countdown. No client prediction, tank exception,
  physical contact law, brake capacity, engine curve or tank data was introduced or retuned.

## MEASURED proof

The initial replacement witnesses ran against the unchanged law: 69 passed, 5 failed.
They exposed the absent stopped pause, an engaged arrested launch, full-pedal selection at
half pedal, unrecoverable top-gear rollback, and an unusable quarter-pedal launch.
The first partial-pedal correction still allowed upshifts into C1's lug regime; the final
eligibility check removes that launch/shift cycle.

| proof | design expectation | MEASURED |
|---|---|---|
| S2 construction-scale fixture, 20 degrees | F4; about 36 kN spare over 191 kN; at most one cut; no rollback beyond rest resolution | 4,750 kg belt; F4; 36,176.594 N spare over 191,247.39 N; zero cuts; minimum belt speed 0; final 2.1343575 m/s |
| S2 independent construction | capable C3 launch, same class bounds | F2; 28,636.719 N spare over 88,913.266 N; zero cuts; minimum 0; final 3.3605297 m/s |
| shipped composition, 20 degrees | same class bounds | F4; 45.653297 m uphill in 30 simulated s; zero cuts; minimum belt speed 0 |
| shipped composition, 10 degrees | positive uphill travel | F4; 61.51133 m in 30 simulated s; zero cuts; minimum belt speed 0 |
| held reversal, both constructions | stopped pause, then exactly one cut | 32 stopped ticks; one 20-tick cut; reverse motion only afterward |
| release within stopped pause | parked, no reversal | passing sequence witness |
| released moving throttle | coast without brake | supported and brake-free force results equal |
| half-pedal 20-degree launch | select an affordable lower gear | construction-scale F2, +10,314.703 N spare; independent heavier fixture correctly reports no affordable half-pedal launch |
| backward top-gear entry on 30 degrees | recover using lower gearing and brakes | construction-scale: F8 rollback -118.101105 m/s before; F3 +1.4389343 m/s after; independent fixture +2.1252043 m/s after |
| repair brake | retain notch, resume when repair ends | both command-edge and adapter-resume witnesses pass |

MEASURED last-second mean/spread after 60 simulated seconds, shipped composition, firm ground:

| grade | pedal | affordable launch | speed m/s | spread m/s |
|---|---|---|---|---|
| level | 0.25 | yes | 1.2710024 | 0.00022745132 |
| level | 0.50 | yes | 7.076938 | 0.000008583069 |
| level | 0.75 | yes | 10.221836 | 0.057992935 |
| 10 degrees | 0.25 | no | 0.000000142794 | 0.000000664145 |
| 10 degrees | 0.50 | yes | 0.653188 | 0.00001513958 |
| 10 degrees | 0.75 | yes | 2.127964 | 0.000054121017 |

The class test independently covers level, 10 and 20 degrees on both construction fixtures,
with every affordable notch sustaining positive, ordered travel. Unaffordable notches remain
held against rollback. Shipped supply was MEASURED whole, staffed and task rate 1.0 throughout.
The steady acceptance is the existing class bound of one percent of governed top speed.
The 30-degree fixture retains its original uphill-travel assertion: the new stronger zero-rollback
assertion is specific to S2's 20-degree witness. MEASURED during 30-degree spool, the static brake
reaches 144 kN per side and a belt crosses -0.05 m/s; this is a capacity breach, not a new capacity
or a reason to retain the old latch. The moving-entry fixture now explicitly releases park,
matching the moving state it claims; a fresh rest-state constructor starts parked.

## Deletion

Deleted the hill-hold state, update/release paths, cooldown, entry/swap thresholds, hold blend,
back-driven intent arm and old moving-swap tests. The force parameters and both adapters no
longer carry the brake-blend saturation scale. The sandbox's separate slip-color scale retains
its actual visual consumer. No 16,000 kg driving fixture existed at this base; it was already
absent. MEASURED dedicated transmission tests: 2,413 -> 2,663 lines, net +250, with 99 removed
and 349 added/changed lines in the diff. MEASURED transmission implementation diff: net -145
lines. Canonical drivetrain state falls from ten fields to nine.

## Reviewer dispositions

`LAW` below means `driving: brake to rest before reversing (#146)`.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex gear review P1 | full-pedal launch pick bypasses capable requested-pedal gear | verified red; construction-scale half-pedal 20-degree result now F2 with positive spare | LAW |
| Codex gear review P1 | arrested In launch bypasses lug correction forever | verified red; rest intent enters Slip regardless of previous regime; S2 starts In | LAW |
| Codex gear review P1 | negative-shaft gate prevents rollback recovery | verified red: top-gear rollback; ordinary lower-gear recovery now succeeds on both constructions | LAW |
| Claude clutch P2 | moving sub-idle price invents launch Slip | original broad finding already fixed by `driving: select gears by landed spare force (#146)`; its direct In witness remains; sub-guard price now also respects the reachable crank floor | prior gear subject / LAW |
| Claude clutch P2 | launch needs prior Out | verified red; same cause as arrested-launch P1 | LAW |
| Claude clutch P2 | rev-match fuel lacks authority/window gate | source verified, not red-tested or changed in this bounded law step | carried |
| Codex clutch P2 | algebraic notch admission misses accumulated f32 rounding | source verified against the reported 0.1-step boundary; not red-tested or changed | carried |
| Codex clutch P3 | exact slew tick count rejects admitted 0.64 rise taste | source verified; not red-tested or changed | carried |
| Claude clutch P3 | stale guard/catch and clutch-state comments | most named prose already replaced by the gear law; remaining free-torque source wording is carried | partial already gone / carried |
| Claude clutch P3 | empty-station first-drag sample is transient | source verified; moving fixture's retained park was corrected, but the first-sample assertion is still carried | carried |
| Claude clutch P3 | moving-lug witness starts In, misses Out landing | source verified; existing moving-lug invariant retained, added Out-landing witness still carried | carried |
| Claude clutch P2/P3 | zero-rate clutch work and empty-station idle conflict | left deliberately for lane 5 as directed | lane 5 |
| ratchet driving-03 | repair brakes silently retain notch | verified source; contract stated beside both transitions and both witnesses pass; row narrowed to empty station | LAW |
| ratchet driving-01/07/08/09 | gauge input, source contract, unchanged writes, duplicated offset | verified source at each site; effective input and derived placement replace the discrepancies | LAW |
| ratchet driving-04 | taste literals in tests | command/HUD assertions derive the owner taste; row narrowed to untouched death-flow input tests | LAW; remainder open |
| ratchet driving-05/10 | repeated resets, numeric two-valued direction | verified source; one reset owner and enum direction | LAW |
| ratchet net-13 | production-only test injection branch | verified source: only tests insert it; injection now cfg(test) | LAW |
| ratchet view-08 | required HUD rows can silently disappear | verified source; drive HUD asserts both rows; untouched consumers remain in the narrowed row | LAW; remainder open |

## Remaining work

S4 certification is not completed. Use the prior gear record's fresh-app/entry plan, uninterrupted
travel, positive support, positive speed and small equilibrium spare. Existing concrete/dirt/gravel
F8/F8, mud F6/F6 and sand F6/F7 observations are not renewed certification after these gear-price
changes. The primary law, required ratchet and gates consumed this lane's bounded session; do not
infer any converged or split ground from an unchanged observational fixture.

Untouched backlog remainders: driving-03 empty station; driving-04 death-flow input; view-08
crew/sight widgets; load-03/07/08/09 in bake. No capture, performance pricing, push, merge, release,
issue comment or worktree operation was performed.

## Gates and boundary

MEASURED start: 11:30:35 UTC; feature work stopped at 12:24:32 UTC, before the
12:30:35 UTC lane boundary. Mandatory validation and its golden-digest correction continued afterward.
The last targeted run was 100 passed and one overbroad 30-degree assertion failed; that assertion
is now scoped to the specified 20-degree proof without changing the 30-degree travel gate.
Final clippy, whole-library and guard results are recorded below after completion.


The first full library was MEASURED **2,149 passed, 1 failed, 26 ignored**. Its only failure
was the expected sample digest for the former transmission projection. The exhaustive field
inventory and wire pin already passed. MEASURED sample hashes, old -> new: transmission
4,617,683,446,189,386,887 -> 12,149,225,890,944,991,752; simulation
14,652,388,418,780,606,089 -> 6,398,580,170,003,897,360; rollback
902,652,863,300,454,137 -> 16,574,472,650,204,573,274. Every other digest field is unchanged.
The pin now names those measured nine-field bytes; no behavior or acceptance bound changed.
The first CI clippy passed; it is repeated after this test-only correction.


MEASURED final gates: `cargo fmt --all --check`; CI clippy with `--locked --workspace
--all-targets --features dev_ui,bitprobe -- -D warnings`; **45 passed** for the explicit
`doc_citations ui_ascii net_boundary bake::` guards plus the corrected digest witness;
whole library **2,150 passed, 26 ignored, 0 failed**. The whole library also completes every
targeted law, command/HUD, headless and trace test. Validation completed at 12:44:05 UTC;
its extension after the hour contains only the digest-pin correction and queued required checks.
Final comment/doc corrections remove the obsolete sandbox metadata claim and describe the
actuation record's mixed labels; neither changes executable behavior. No lane process remains.
