Subject: the video game's environment destruction model; game state only. Code at `14d1ec81`.

# Review of the unification plan
**Verdict: not ready to queue.** Citations and design accounting mostly hold. Order and "no owner question" do not: the plan learns whether destruction fits on the server at T30, after every migration, on a tankless scenario on the wrong machine, and the ram says that is the first thing to learn. No settled ruling needs reopening; about ten edges move, four questions go to the owner, and the designs must enter the repo.

## 1. What the ram says
| `stats.txt` rows (MEASURED) | Awake/asleep | Tick p50/p95 ms | Physics p50 ms | Hz | CPU % |
|---|---|---|---|---|---|
| 10–100 s, quiet | 0/0 | 2.8 / 3.4 | .3 | 64.0 | 47 |
| 120 s, peak | 72/1 | 32.0 / 36.4 | 3.0 | 31.0 | 100 |
| 125–1050 s, medians of 186 rows | 50/23 | 19.7 / 23.1 | 2.1 | 49.5 (38–58) | 100 |
| 1095–1125 s, medians | 5/68 | 5.1 / 7.9 | .9 | 64.0 | 59 |

- One ram on one of 74 houses (`server.log:13`) held a one-core server over budget for 15.4 minutes, mostly with nobody connected. Match mean 51.7 Hz (DERIVED: 61,539 ticks over 1,190 s).
- DERIVED fit, 186 zero-client rows from 125 s: tick = 3.58 + .324 × awake ms (R² .97); physics = .76 + .027 × awake ms (R² .96). **92 % of an awake piece's cost is outside the physics step.** The budget is gone at 37 awake pieces.
- That cost is ours, not Avian's. A loose group always rechecks (`breakage.rs:1811-1812`); `weigh` rebuilds groups, incidence and graph over the whole source house (`fracture.rs:2110,2131,2156`; `Live::group` keeps it, `breakage.rs:513`): 1,164 chunks, 3,186 joints (MEASURED, house bake). It runs about once per awake piece per tick (MEASURED local: 9,017 calls/s at 140 awake, `diag-pieces-rest/report.md:35`).
- Settling failed too: about 50 awake for 925 s, then 41→14 in one row (1050→1055) with no log event, and 5→12 again at 1125–1135 with no client. `src/debris.rs:8` is now Avian sleep with no cap; fork F10's text still describes a 256 cap.
- The floor, with laws at zero cost and no tanks: **305–475 awake pieces** (DERIVED: 12.8 ms over .042 ms from the 990-body host fixture, or .027 ms from the fit), four to six such rams at once. At rest is cheap: .4 µs a piece on M4 (`night-pricing.md:234`), about 1.5 µs on the host at its 3.7× (DERIVED), about 8,500 pieces. So urban scale turns on peak awake pieces and settle time, not total pieces. Judgement: a few houses at once looks reachable; "mass destruction" is unproven.
- With tanks the budget is already gone: the owner's gate in #250 is "64 Hz … through a full 30-tank Battle with houses being destroyed, on the size we deploy"; tanks alone are 16.8–17.1 ms at minute 3 (DERIVED, #250). The plan never mentions #250, F10 or the word "tank".
- Three client disconnects at 121, 152 and 280 s (`server.log:199,226,334`), unattributed; no client log exists. Capture one in the replay below.

## 2. The findings that change the plan
- **A. Feasibility is learned last.** T27 and T30 sit at the end. T30's only trip wire "blocks the build", after all 26–34 hours. Its tool has no tank (`collapse.sh:2`) and runs `target/debug` (`collapse.py:122`) on an M4 whose timings "do not transfer" (`diag-collapse-cost/report.md:17-18`). Its scenario is twenty firs. The hosted path already exists (`scripts/edgegap/session.sh:26`).
- **B. T14–T17 put a dearer reader on the hot path, uncosted, before T27.** T16 replaces `weight_failure` and `section_pulse` (`fracture.rs:2854,3011`), which awake groups reach through `loaded_graph`'s frame reference (`fracture.rs:1337`). T15 makes removal history unbounded on purpose. The draft priced .3–.6 ms at a fifth hit, 24–56k evaluations at an assumed 10 ns (DERIVED, `break-survival-claude.md:28`); linear to the design's "machine gun over 300" gives 14–34 ms per hit (DERIVED). The draft's bar "a hit costs over 1 ms" reached neither the merge nor the plan. Fix: one open-point mask per plane, updated once per removal so a read never walks history, with a cost bar in T14's proof.
- **C. T9 reverses an owner ruling unannounced.** Fork F5: "half-angle settled 2026-10-23 (owner): measured, per plate material" (`playtest-forks/README.md:172-185`). T9 deletes the per-row field. Neither design nor plan names F5.
- **D. T5/T6 buy a yes/no gate with little visible effect, and move what the plan does not report.** See section 7.
- **E. The queue cannot start from the repo.** All four designs, `proto_fir.py`, the Python models and the ram data are untracked or in `/private/tmp`. T1, T10, T16 and T30 point at them with no path. Commit them to `.agents/docs/design/` before filing.

## 3. Order
Real, among others: T11→T16, T13→T20→T21→T22→T24, T15→T17, T21→T23→T24. Merge order only, presented as dependency: T1→T2, T2→T5, T3→T9, T6→T10, T6→T17, T10→T11, T11→T12, T12→T13, T13→T14, T7→T15, T19→T22, T24→T25, T24→T26, T17/T24/T26→T27. The plan's schedule treats T2 as independent, contradicting its own edge.
- **T29's building half before T4:** the owner has done it; make it a replay (the drive in `diag-tree-crash/evidence/reproduce.py`, aimed at a house, on the one-core host, with a client log) so T4 and T27 have one before and after.
- **T27 before T14 and T12:** today's hottest code; T14–T17 do not rewrite `loose_groups`, `incident`, `graph` or `Live::group`. Add: a group with no live joint runs no section or ring work; `presses != held_presses` is an exact float compare (`breakage.rs:1811`) that solver noise defeats.
- **A hosted scale gate (split from T30) before T13:** T13 starts the data migration that runs to T26. Run it after T4 and T27, with bots, on one core. A failure goes to owner question 2, not to more of the queue.
- **T1 before T7 and T8:** both start beside T1 today and are wasted if T1 fails. T7 lands just before T15, T8 just before T24.
- **T14 inside T1 (T14 before T13):** the reader needs existing hulls, and T14 itself says to stay independent of the cutter. T1 then builds real code instead of a throwaway.
- **T17 before T6; T6 before T29:** T6's real consumer is the ram. It also opens seams for falling chunks (section 7), so measure it after T27.
- **T26 before T24, after T17:** one hull per freed group is the physics lever and is generic on `spawn_group`; trees then inherit it. Judgement.
- **T30's twenty-fir price before T24:** the bake and hulls exist at T21–T22, and the design's kill criterion belongs before cutover.
- **Restore "visible breaks miss the holes" to T1:** the design kills there (`break-survival-merged.md:56`); the plan defers it to T23/T24 and softens the remedy.

T1 is no longer the riskiest thing. Its question was answered in MODEL (seed medians 3–4); only real hulls are new. The server is the open risk.

## 4. Citations
Printed all of about 120 distinct `file:line` targets and read about 30 in context, weighted to scope. HEAD matches. All 17 cited commits exist with matching subjects; spine step 1 and pushing work are landed as claimed (`trees/motion.rs:172-184`, `breakage.rs:2097-2114`, `fracture.rs:775`).
- **Wrong:** T28's `scripts/perf/run-forest-cost.sh:1` was deleted in `17c937fa` (159 lines), so T28 has no runner and needs another session; no task records its "recorded pre-cutover revision". T12's `GLOSSARY.md:23` is a line about hit markers; the glossary has no structure entry.
- **Incomplete:** T7 lists only `protocol.rs`, but net-22 also names `src/tank/servo.rs::drive_servos`. T19's `ballistics.rs:2214` is an unrelated query field; the contract is `DeliveredLine` (`ballistics.rs:1157`).
- **Sound:** every other sampled line names what the plan says, including `fracture.rs:947`, `trees.rs:851`, `system_param.rs:240-261`, `contact_transfer.rs:261`, `substances.rs:1070,1789`.

## 5. Accounting
Steps 0–12, the six survival lanes, P1–P6, the delete list and the folded-in items are all placed. T24 really does own each named tree deletion. Gaps:
- **P0's shear stills and its kill** ("a plank flips … or 25 % of reach shows", `material-proxy-merged.md:68-71`) have no task. T1 claims P0 but omits them; T10 covers crushing and rear face; T11 only derives shear.
- **"One collider until disturbed"** is stated by no task, yet #150 closes on it. Name it in T21 or T24.
- **Owner's #250 gate, fork F10, fork F5:** no task and no disposition.
- **Small-arms line through brick** (MODEL: 7.9 mm passes .25 m first round, manual says 45) and **delay fuze** (`structures-fields-merged.md:65-66`): no home, and the first would show in the first playtest. File both.
- **Ratchet rows not sized:** T19 edits `ballistics.rs`, so rows ballistics-31 to 34 (`backlog.md:97,103-106`) close there, one moving the crew line rate's game-unit scale into data. T7 reaching `servo.rs` closes load-27 and load-31 (`backlog.md:21,25`), and load-27 says "round 3: audit first", a direction audit the plan omits.

## 6. Scope
The ratchet: a lane closes rows naming a file it edits; "Only P1 rows get lanes of their own" (`lenses/README.md:21-23`). Every row these tasks absorb is P2 or P3.

| Task | Needed? | Cost to change |
|---|---|---|
| T3 | No. No lane edits `field_casts.rs`; it is here only because the brief asked to classify it | Drop: saves a session. If T9–T11 redden the digest, that lane closes ballistics-28 |
| T7 | Ratchet only, because `PROTOCOL_REV` lives in `protocol.rs:278`. net-21 is P2, ties to #267 (owner: after early access), and its row allows "or refuse the rig" | Shrink to refusing the rig. net-22 says "decide with issue 187": needs an owner carve-out or that decision. Saves 1–2 sessions, a wire bump and an audit |
| T8 | Yes: T24 edits `debris_stress.rs` (`TreePiece`, `:289`). net-16 is stress-scenario tidying the ratchet still demands | Keep; move to just before T24 |
| T9 | No: spall armour data, no effect on walls, roofs or trees | Hold on owner question 3 |
| T12 | Words yes, blocker no | Any time; run early to keep later diffs clean |
| T5 | Only if section 7's table says so | Drop: saves a session and a required datum |

T25 and T26 stand by the "whole of the spine" ruling. T2, T10, T11, T13–T24 and T28–T30 belong.

## 7. Doctrine, as cost
`mean_force` feeds only the yes/no `yields` (`fracture.rs:927-939,2624-2650,3029-3050,3383-3387`). What T5/T6 change, all DERIVED; FIXTURE 50 t hull; brick 8.6 GPa, chunk .065 m² × .25 m (`materials.ron:537`); wood 118 MPa, .4 m × .7 m (`materials.ron:440`):

| Case | Gate opens today, blow ÷ step | With T5/T6, v√(mk) |
|---|---|---|
| Hull on 3 m brick wall, 8.6 kN in bending | 2.7 mm/s | .8 mm/s |
| Hull on .40 m fir, 232 kN at 1 m | .07 m/s | .23 m/s |
| Hull on .65 m fir, about 1 MN at 1 m | .31 m/s | .97 m/s |
| 27 kg chunk at 5 m/s on a chunk, seam ring 34.5 kN | stays shut, 8.6 kN | open, 870 kN; then 95 against 10.4 N·s opens it |

- Blows only exist above .15 m/s (`contact_transfer.rs:291`), so the gate is nearly always open for a hull. A hull on brick pulses for 14.9 ms against a 15.6 ms step. The only hull change is a crawl into a big fir.
- The steel side moves stiffness at most 4 %, force at most 2 %, at equal geometry: "each side contributes" fails the owner's bar.
- **The change the plan does not report is the last row:** falling chunks start opening seams, so a collapse makes more pieces on the hot path. T6's "report changed ram outcomes" misses it. Require pieces per collapse in its report.
- **Simpler true model:** a blow reads momentum and pair work only (landed, `ff37724`); a sustained press reads force. Delete the force gate for bodies. No contact time, no stiffness, no step. It meets #256's "no longer reads the solver step" and #245's aim ("erases ram or don't"), not their wording ("a contact has a duration", "stiffness of the pair, in data"). It loses a crawl-speed hold on big firs. Run the table in code as T6's first half hour; a visible case restores one-sided stiffness.
- T17's 600 Pa is labelled wind, but the design says "this is where toughness lives … 300, 600, 1,200 Pa give 9.5, 5, 4 shots": two jobs. T21's 30 % and T14's sampling need `Resolution` labels; T26's "allowance" has no number or derivation.
- **No rule for one kind of object survives in physics**: T24 is thorough, T23's branch cards are view only, T16's two-support moment is the accepted door's stopgap. T18's zero-work root plate is a missing fact only rams read (`structures-fields-merged.md:54`), so T29's ram shows it.

## 8. Task quality, five riskiest
| Task | Startable from its text and the repo? | Proof | Over 90 minutes? |
|---|---|---|---|
| T1 | No. "Offline experiment" is outside the repo; laws are `pub(crate)` (`src/buildings.rs:40-41`), so "tools-only" cannot call them; tools clips only in 2D (`geom.rs:12`), and the 3D cutter is T20's | The 10 % and one-shot gates are good; mutants are moot in a throwaway | Yes |
| T5 | No. "Occupied geometry" is undefined for belt contacts (`body_collider: None`, `contact_transfer.rs:82`); the final row has no stiffness field | Pins admission only; nothing shows a verdict moves. Add: doubling the datum flips a visible case, or it retires | No |
| T15 | Mostly; the union's representation is unstated | Good mutants, no cost bound | Borderline |
| T17 | Yes | Good mutants; wrong holds by eye only | Yes: 51 of the 139 tests in `fracture.rs` and `breakage/tests.rs` call the rewired laws directly (MEASURED by grep), and helpers reach more |
| T24 | Yes; the best proof in the plan, the compound mutant does fail | Good | Clearly: `TreeState` alone is 137 lines in 9 files, 46 tree and ledger tests, wire, LFS, a real drive. Two sessions on one branch, merged once |

T29's mutants cannot run in a capture. T30 has no pass threshold and also holds the final lens review. T11 gives no numbers for its one wood row. T4's proof is sound, but its tool is the Mac run: repeat it on the host.

Estimate: 26–34 h counts no fix rounds after four lens boundaries and sizes T1, T17, T24, T29 and T30 at one session. Three sessions each plus one fix session per boundary, less four dropped sessions, gives **about 40–50 working hours** (DERIVED, my assumptions).

## 9. Owner questions
1. **How much destruction must one server carry at once?** On 1 November you set the bar: 64 ticks a second, 30 tanks, houses coming down, one core. Today one rammed house uses the whole core for 15 minutes with nobody connected, and 30 tanks alone sit at the limit. *Blocks:* everything from the bake migration on. *Recommend:* keep the bar and make it concrete, say 30 tanks, five houses down within 30 seconds, twenty over a Battle (my stand-in numbers). *Costs:* about two sessions and one hosted run before bake work starts.
2. **If one core cannot carry it, what gives first?** Choices: settled rubble stays still until disturbed; fewer, larger pieces; small rubble fades; more server cores; pieces run on each player's machine, so players stop agreeing on what stops a round. *Blocks:* the shape of the plan's last third, only if question 1's run fails. *Recommend:* answer after that run; my order is the first three, then cores, never the last. *Costs:* the first three are visible, and your note on the bar said "never a quality cut", so this amends it; cores are what you declined for now.
3. **Spall cone width: per material, or one number?** You ruled per material on 23 October. The plan makes it one number without saying so. Eleven of twelve rows copy the one measured value. *Blocks:* one tidy-up task that walls, roofs and trees do not need. *Recommend:* keep your ruling and drop the task. *Costs:* twelve identical numbers stay until another material is measured.
4. **Next build and your time, a cost you have not been told.** The next build comes after all thirty tasks: planned 26–34 working hours, my read 40–50. Twenty of thirty tasks name something for you to look at, excluded from the estimate. The tree crash stands until the tree cutover, as you ruled. *Blocks:* nothing; it is the price of that ruling. *Recommend:* keep the ruling and batch your looks into four sittings: first broken fir, walls after the survival switch, first tree cutover, final. *Costs:* a wrong look can travel further before you catch it.

## 10. Low-stakes defaults; strike any
- How long a contact lasts is not modelled (section 7): a ram is read by momentum and work.
- Metal stays rigid. Your ruling left it open and the plan closed it; nothing here needs it, but the destruction epic still names "small vehicles crushed by tanks".
- Other players' turrets stay as they are until the own-turret question is decided, and aiming rigs the stream cannot carry are refused.
- Wood: plank and trunk are two rows (448 and 850 kg/m³, 74 and 36 MPa) and "one row" means species. The plan reads it as one row with no numbers, which doubles a roof's weight or halves a fir's.
- A fir's root plate keeps no work until after the first playtest, so slow rams topple firs too easily (blow capacity 2,441 against about 57,000 N·s·m, DERIVED in the design).
- The 600 Pa keeps its value and is labelled as taste, not wind.
- A result to report, not a call: machine guns and APCR stop felling firs (today 187 and 7 shots MEASURED; over 300 and 52 MODEL).

## 11. Where the plan holds up
T4 is early and library-first, which the ram supports. T1 rightly precedes runtime and asset migration. T24's proof carries the real witness and the real drive. Trees lose their exceptions by deletion, not wrapping. T19 is a real gap, correctly placed before group hulls. The field-stop deferral and the honest "no safe faster order" on the crash both stand. Its reading of "structure" is right: `Structure` today is the multi-material graph, and renaming it construction avoids a false refusal.
