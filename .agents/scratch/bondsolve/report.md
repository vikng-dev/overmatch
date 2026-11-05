# Bondsolve verification

Subject: the game's environment destruction model (game state only).
Lane: `bondsolve`. The law and its derivations live in the construction design's **Structural calculation** section; this report records verification and outstanding evidence.

## Implementation decisions

- One bounded block-diagonal CG solve carries sustained and pulse load columns over the same owned joint graph. The columns keep finite pulse work distinguishable from force-controlled loading.
- Initial gravity is prepared once per admitted construction and scale, then shared immutably by instances. A changed owner obtains its own warm state. Initial overloads wait for the first authoritative stage.
- Joint resultants use exact circular area moments. Candidate cuts, section searches, punch rings, separate bearing searches, expanded inertia-source streams and same-tick failure loops are deleted.
- Source loads and fracture work are partitioned once among departing owners. A separation between two departing owners is charged half to each; one departing side pays the whole work. The launch debit sums coherent momentum before computing kinetic work.
- Required material shear moduli and missing fixing/root shear and separation facts are explicit `Unknown` data. Their analogues are not measurements. The opening-only exemption is deleted.
- The wall instrument uses the fixed clock, continues unfinished equilibrium without fresh input, and delivers one queued source operation per numerical tick.

## Analytical gates

| Case | Independently derived requirement |
|---|---|
| Wall under weight and side push | Base force and moment; normal traction `N/A + M/S` |
| Lintel on two piers | Each symmetric pier carries half the centred load |
| Cantilever | Each joint carries the force and moment of everything beyond it |
| Resting beam | Slide at pair friction times compression; tip at compression times bearing radius; invariant under reversed endpoints |
| Running-bond wall | Manufactured constant-translation jump recovers a staircase set of loaded seams, including both orientations |
| Finite blow | Beat traction strength and add the patch's `G_f A` of elastic work; sustained loading remains force-controlled |
| Cold/warm convergence | Force/reaction tolerance `1e-5`; cold resumed fixture within at most 33 rounds, warm within 32; relative preconditioned residual target `1e-8` |
| Idle | Zero sparse products; unchanged spawn instances share prepared equilibrium |
| Release work | Each input has one owner; the total outgoing kinetic debit pays one opening's work exactly in the central-impulse fixture |

The iteration budget is Resolution 32 rounds per active numerical tick. Pre-play preparation has a bounded allowance for conjugacy lost in finite precision, separate from that runtime budget.

## Operation-count observations

These are sparse-operation counts, not timing forecasts. The doubled graph is two translated copies of the shipped graph; it is not an authored house of doubled width.

- MEASURED cold 32-round active pass: house graph **221,544 joint visits**, doubled graph **443,088**.
- MEASURED intact-gravity solve: **868 rounds** to equilibrium, with **80 joint overloads** in the intact house graph. These are observations, never pass conditions. They require an owner's look at the resulting startup and failure paths.
- MEASURED one sparse round: **6,516 joint visits**. An unchanged equilibrated fixture performs **zero** sparse products.
- DERIVED source diff against the lane base: **7,976 lines removed, 3,989 added, net -3,987**. These are diff counts, including replaced tests and formatting, not a complexity metric.

## Validation and remaining work

- Workspace construction run: **208 game tests and 50 tooling tests passed**, with two existing measurement beds ignored.
- Final combined construction/substances/debris/wall-instrument/collider-friction run: **252 passed**, two existing measurement beds ignored.
- **Formatting and workspace/all-target CI clippy passed; all 48 whole-tree source/bake guards passed.**
- All runs are unattended, headless and muted through the machine build/run lock. No captures or visible binaries were taken.
- A broad headless caller run was stopped while diagnosing startup work. The serial rerun was also stopped at the session timebox, still inside `contact_law_shipped_brakes_hold_below_both_limits_and_move_above_the_lower`. It reported no failure before termination, but the full headless suite is **incomplete**, not passed. The targeted collider-friction guard passed in the 252-test run.
- No before/after ram timing runs were made. The lane rules explicitly reserve timing and captures for the orchestrator even when a brief requests them. Worst tick, p99, ticks over 15.625 ms and owner-stage share therefore remain unmeasured for this change. The orchestrator needs both baseline runs and both changed runs, the actual doubled-size construction pricing, and the play review.
- The prototype's pulse model is quasi-static over the numerical step, capped by stated peak force. It does not resolve wave propagation or cohesive softening; initial intact-house overloads and Unknown fixing/root facts need review in play rather than tuning to previous verdicts.

Session start: 11:33 local time, 2026-11-05. Source implementation is committed and green under the targeted checks above; the unfinished broad headless run was terminated before the 90-minute limit. No game or test process is left running by this lane.

Final edits remove stale retired-law doc links and record the stopped headless run; Rust behavior is unchanged from the passing implementation checks. Formatting was rechecked.

## Session 2 follow-up

The current handoff is [session 2](report-s2.md), with the individual gravity-joint audit and
pins reconciliation linked there. It supersedes this session's incomplete full-suite status;
the corrected session-2 library suite passed in full. Startup gravity cracking remains an
unresolved contact-law limit, not an accepted physical-soundness verdict.
