# Handoff — authoritative-facts arc + simplifier experiment

**Written 2026-07-29. CLOSED 2026-07-30 at slice 3.13 — this is the arc's final record.** State is
the slice-3.13 commit on `feat/authoritative-facts`. Read this if the session ended mid-arc, or
before starting slice 4.

**Nine review rounds. Rounds 1–8 returned DO NOT SHIP; round 9 found no blocking runtime defect** and
one blocking documentation defect: a source scan that claimed more than a lexical scan can deliver.
Slice 3.13 strengthened what was cheaply strengthenable in that scan, and — the more important half —
wrote down exactly what it does and does not guarantee, here, in the test's own doc, and in ADR-0032.

§§ 2–4 below are a running record written at successive slices; where an early section describes the
tree at slice 3.11 it says so. **The final subsection of § 3 is the summary that supersedes them:
what is guaranteed mechanically, what is argued in prose, and what residuals are open.** Read that
first.

---

## 1. What this arc is

The server computes a hull impulse when a shell hits you (Δv = 0.1383 m/s for an 88mm). The client
never felt it. `ROLLBACK_VELOCITY = 1.0` m/s — the state-difference gate that decides whether an
authoritative value is worth adopting — is **7.2× larger than the impulse itself**, so that gate
could never once have passed a real hit.

The fix is not a smaller threshold. It is a separation that every shipped engine makes and this
codebase had not: **adopting authority is unconditional and server-authored; hiding the seam is
thresholded and view-only.** Being shot is a knowledge question, not a cost question. See
`.agents/docs/adr/0032-unpredictable-authoritative-facts-adopt-unconditionally.md` — that ADR is the
precedent document, and the next fact of this kind (a ram, a blast, a crew knockout, a track break)
should be implementable by reading it.

The primitive is `src/net/adoption.rs`. `HullShock` is merely its first consumer.

---

## 2. Where things stand

### `feat/authoritative-facts` — HEAD at slice 3.13, **698 lib tests passing**

```
(3.13) the participation scan says what it guarantees — wire unchanged at REV 24
c973bbb the staged fact's spark is retained per fact, and the participation mirror becomes
        two checked contracts — wire unchanged at REV 24                            (slice 3.12)
de50e3c docs: reconcile the latch audit against round 7's own 23 rows
77d5beb docs: the latch audit table is a re-derivation, not round 7's list verbatim
215c769 establish the hull's PARTICIPATION in the restore, not just what it would
        resolve — wire unchanged at REV 24                                          (slice 3.11)
b836406 docs: handoff — round 6 verdict, and the fix was only half a fix
a0ac961 revalidate the WHOLE rigid body at the request, not just the shove          (slice 3.10)
baade0b revalidate delivery at the REQUEST, not at the offer                        (slice 3.9)
6b902eb docs: handoff — round 5 verdict, the determinacy argument is false
906ff79 docs: handoff note for the authoritative-facts arc
4f523b1 delivery is ESTABLISHED before a rollback is asked for — wire unchanged at REV 24
2a2e282 the episode carries its own span, and `bypassed` is established — wire REV 24
8c33217 correlate the shove with its spark by IDENTITY — wire REV 23
88a7e29 docs(adr): 0032
6cf5553 the shove ordering rule is best-effort, and now says so
a0c831e the shove waits for its spark, bounded and instrumented
aa042ba generic unconditional adoption path
d8ff806 make the lead-0 delivery failure executable
4c13ffe docs(upstream): lightyear report — LOCAL DRAFT, NEVER FILED
8a2fe21 HullShock as built — correct shape, wrong delivery path
```

Wire is at **`PROTOCOL_REV = 24`**. Hashes, independently recomputed by review:
surface `0xf321_3c48_61b3_bfea`, types `0x268a_d4fb_e297_639b`, manifest `0x13cd_6a8e_562f_0315`.

### `chore/simplify` — 8 commits, a separate experiment (§6)

---

## 3. The review history — read this before trusting anything

Codex has reviewed this arc **nine times**. Rounds 1–8 returned DO NOT SHIP; round 9 found no
blocking runtime defect. (An earlier version of this line said six reviews and five DO NOT SHIP while
all six table rows said DO NOT SHIP; it was six of six, then seven of seven, then eight of eight.)
Every round through 8 found that the previous fix read as complete and was not — including the four
rounds that were themselves fixing a partial fix. **Twice a newly added test asserted the defect as
success, and three times a newly added test or document claimed coverage it did not have.**

| Round | On | Verdict | What it found |
|---|---|---|---|
| 1 | `a0c831e` | DO NOT SHIP | Ordering claimed a guarantee it cannot keep; wait clock measured server-fact age including transit |
| 2 | `6cf5553` | DO NOT SHIP | Episode correlation swapped one invalid rule for another; `retirement` blind to rollback kind |
| 3 | `8c33217` | DO NOT SHIP | Disjointness held only while `last_bump_tick` was `Some` — a fresh hull's first episode claimed 15 ticks it never held, and a prior life's spark could release a respawned hull |
| 4 | `2a2e282` | DO NOT SHIP | Correlation **confirmed settled**; `Adopted` still retired facts whose restore carried nothing |
| 5 | `4f523b1` | DO NOT SHIP | The determinacy argument is **false** — confirmed history is not append-only, and the predicate is proven at staging but never revalidated at the request. Fixed by slice 3.9 (below); round 5 passed C, E, F, G, H |
| 6 | `baade0b` | DO NOT SHIP (1 High, 2 Low) | The revalidation covered **half the predicate**: only the velocities, while the offer proved all four rigid-body histories, so a late `Position` removal reached a claimed rollback that deleted the component and closed as `Adopted`. Plus two overstated coverage claims in the new tests and a false comment. Fixed by slice 3.10 (below); round 6 passed A, B, C, D, E, H |
| 7 | `a0ac961` | DO NOT SHIP (1 High, 3 Low) | **Audited the CLASS, not another instance.** 23 latched values assessed, 21 safe with stated reasons, 2 defective: the hull's *participation* in `prepare_rollback` was latched at the offer and never re-established (silent loss — `DisableRollback` arrives, the restore skips the hull, `carried` is computed from a lookup no restore performed, the fact closes as `Adopted`), and `ordering == None` carried three meanings so `bypassed` was inflated. Plus a false justification for the pose asymmetry and a wrong count in this document. Fixed by slice 3.11 (below) |
| 8 | `215c769` | DO NOT SHIP (1 Low) | The presentation ledger's capacity was DERIVED in ticks while eviction is spent per ENTRY, so an evicted spark inflated `bypassed`; plus an impossible 23/25/27 count and an overstated "both passes missed in the same direction". Fixed by slice 3.12 (`c973bbb`), which also built the two mechanical checks round 8 asked for |
| 9 | `c973bbb` | No blocking runtime defect | The 32-cell matrix is genuinely non-vacuous, the eviction latch is wired at every seam, the ADR's 29/21/4/4 accounting is exact, the wire is untouched. **The blocker was documentation: the participation source scan claimed more than a lexical scan can deliver** (six named evasions), and two residual descriptions were wrong — pre-staging eviction can feed a FALSE value to `retirement`, not only a conservative one to `clear_to_order`, and the exposed window is the whole pre-staging span, not "one replication gap". Fixed by slice 3.13 |

**The operational lesson, and it is the important part of this document: a green test suite has
never once caught one of these.** The tests were written by the same reasoning that produced the
bug. What caught them, every time, was Codex reading our code against lightyear's actual source in
`~/.cargo/registry/.../lightyear_prediction-0.28.0/`. Do not report "N tests passing" as evidence on
this work. Keep the adversarial review in the loop until a round comes back clean.

**The second lesson, which rounds 5 and 6 taught together: a fix aimed at one instance of a defect
tends to land on exactly that instance.** Round 5 found readiness proven at staging and acted on
later; the fix revalidated at the request — for the two components the finding named. Round 6 found
the other two still stale. When a review names a defect, ask what CLASS it belongs to and enumerate
the members before writing the fix. Round 6's Low findings are the same shape at test level: the
fixture exercised the mechanism the finding named and quietly did not exercise the one it claimed to.

### What is now settled (round 4 verified, do not re-litigate)

- The single same-tick `arm` path; `close_episode` runs after `ProjectileMarch` in the same tick.
- Episode spans are disjoint **by construction** — publication clears `opened_at`, so the next
  span must start strictly later. No appeal to `SHOCK_EPISODE_TICKS` arithmetic.
- Respawn is structurally safe, including a late-arriving prior-life `ImpactConfirm` (it keeps its
  older server tick and so cannot drift into the new span).
- The lead-zero fixture matches the four components production actually predicts: `Position`,
  `Rotation`, `LinearVelocity`, `AngularVelocity`.
- All wire accounting.

### What round 5 found, and what slice 3.9 did about it (`baade0b`)

`4f523b1` strengthened readiness instead of detecting failure after the fact, on this argument:
at offer time, whether `prepare_rollback` will carry the shove is **already fully determined** and
cannot change while `produced_at` is fixed, because `get_state_at_or_before(produced_at)` only
resolves samples ≤ `produced_at` and confirmed history only grows forward. So retrying is not merely
unbounded — it is futile.

**Round 5 confirmed that argument is FALSE**, on two independent grounds, and slice 3.9 landed
against it:

1. **Confirmed history is not append-only.** `ConfirmedHistory::insert_raw` does sorted *middle*
   insertion with same-tick replacement; `SameAsPrecedent` explicitly changes its effective value
   when a late *preceding* sample arrives (lightyear ships a test for it at
   `lightyear_core-0.28.0/src/confirmed_history.rs:648`); and replicon mutation transport is
   unordered, with history-enabled entities accepting older mutations.
2. **The control flow never revalidates.** The predicate gates the *offer*; an `ExternalEvent` fact
   then stays staged for the visual budget, often into later frames; a later offer pass that fails
   readiness merely `continue`s and leaves the staged fact alone; and `request_staged_adoption`
   consumes it and claims the rollback without re-running the predicate. The same-frame premise is
   also literally false — lightyear's `Check` path calls `ConfirmedHistory::add_unchanged` for all
   four relevant types between the gate and `Prepare`.

   *One correction to that last sentence, found while fixing it and worth keeping:* `add_unchanged`
   is reached only from `check_rollback`'s policy branch, and `check_rollback` consumes a pending
   forced-rollback request BEFORE that branch and then skips it entirely. So on the frame this module
   claims, it cannot run. It runs on the frames the fact spends WAITING — which is the whole window
   the finding is about, so the finding stands; the "between the gate and `Prepare`" framing does not.

   *A correction round 6 made to my own round-5 reasoning, and the more useful of the two:* I argued
   that `insert_raw`'s same-tick REPLACEMENT could not change an already-stored value, because two
   different authoritative values cannot map to one lightyear tick. **That is not true, and it is not
   the proof.** Replicon checkpoints and lightyear ticks are not in bijection — several checkpoints
   can land on one tick, and each would replace the entry the previous one wrote. Production is safe
   for an entirely different reason: the hull values in question are authored by the fixed-step
   simulation, which produces one final value per lightyear tick, so the replacement is idempotent in
   fact rather than by construction. Keep the real reason. An invariant that holds because of who
   WRITES the data is a different, weaker thing from one that holds because the type cannot express
   the violation, and only the second survives a change of writer.

#### The fix

**The predicate is now asked at the request transaction**, in `request_staged_adoption`, over the
histories as they are at that moment, immediately before the slot claim. The offer's identical gate
stays, demoted in writing to an ECONOMY: one staging slot, don't fill it with an undeliverable fact.

**A failed revalidation is a WAIT, not a drop.** Nothing is claimed, nothing is tallied, the fact
stays staged. The bound is the replay-window check that already runs first in the same function:
once the fact's age passes `RollbackPolicy::max_rollback_ticks` it is closed with a WARN naming it,
and the local tick advances at least once per tick — so the stall is bounded and the give-up is loud.

**A second, unbriefed defect fell out of the same reading.** `restore_carries_the_shove` was scanning
present values only, while `prepare_rollback` reads `get_state_at_or_before`, which resolves
authoritative REMOVALS. A late removal middle-inserted between the event and the restore target made
lightyear delete `LinearVelocity` from the hull while our predicate, seeing only the older sample the
removal shadowed, called it a delivery. The predicate now asks lightyear's own lookup FIRST and uses
the present-value scan only for the tick it resolved at; `authority_reaches` fails closed the same
way. A `SameAsPrecedent` entry still counts at its own tick, deliberately — the marker asserts the
authority still held that value there, so a restore resolving it installs the authority's real state.

**`Undelivered` is now unreachable against pinned lightyear 0.28**, on three steps, all in the
`retirement` doc: (1) the branch needs both `adoption.requested` and this module's own installed
claim, and the slot's claim is consumed every frame, so it is same-frame with the revalidation;
(2) between them, `net::watchdog` is read-only, `check_rollback` consumes the forced request and
skips the whole policy branch — the only writer of `ConfirmedHistory` reachable in that gap — and
replicon receive already ran; (3) the two lookups now agree by construction. Steps 2 and 3 are
DEPENDENCY properties. A lightyear bump can retire either without touching a line here, which is
exactly why the branch, its ERROR log and its counter all stay.

**The test that was missing for five rounds now exists.**
`net::lead_zero_rollback::a_late_replicated_change_is_revalidated_before_the_request` stages the fact
on the arrival frame, then moves the confirmed history through lightyear's own insertion API — a
newer sample (compressed to an unchanged marker) plus a removal stamped before it, middle-inserted —
and asserts the module waits, tallies nothing, and leaves the fact staged. Its sibling
`a_revalidation_that_never_passes_is_dropped_at_the_replay_window` carries the same run past the
window and asserts the fact is closed. Both were verified RED against the pre-3.9 code, where the
module claimed the slot on the staging-time answer and `prepare_rollback` deleted the hull's
`LinearVelocity` outright.

**Round 5 passed C, E, F, G and H** — the close tick is the right comparison tick, the real-restore
fixtures genuinely run lightyear's `Prepare`, the fixture helpers can no longer manufacture
impossible shapes, the state hash is right with both re-pins correct, and the wire is byte-identical
at REV 24 with `settled_at` correctly derived rather than transmitted. Do not re-litigate those.

### What round 6 found, and what slice 3.10 did about it (`a0ac961`)

Round 6 passed almost everything: the gap between revalidation and `slot.claim` is inert, the
predicate's lookups match `prepare_rollback` for present values / removals / markers / empty
histories, the narrow `Undelivered` unreachability proof holds, the age check runs before the failed
revalidation and drops at 101 for a 100-tick window, the fixtures use lightyear's real sorted API
with a genuinely middle-inserted removal, and the wire is clean at REV 24.

**The one High: the revalidation covered half the predicate.** The offer checks all four rigid-body
histories; `request_staged_adoption` re-checked only `LinearVelocity` and `AngularVelocity`. A late
authoritative REMOVAL of `Position` therefore survived — the offer pass correctly stopped offering
the hull and that changed nothing (re-offering never touches a staged fact), the velocity-only
revalidation passed, the slot was claimed, `prepare_rollback` answered the removal by taking
`Position` OFF the hull, and `confirm_forced_rollback` closed the fact as `Adopted` because both
velocities were genuinely carried. **Every counter this module keeps is defined on the velocities, so
nothing could fire.** Exactly the round-5 defect, still live for the other half.

Both sites now call one function, `restore_is_deliverable`, and its signature is what stops the
request's query shrinking again. **The two halves keep DIFFERENT predicates, deliberately:** the
velocities CARRY the fact (a `HullShock` episode is a velocity impulse and nothing else), so they get
the strong `restore_carries_the_shove`; the pose carries none of the event — the impulse changes
velocity, and the pose only moves as later ticks integrate it — so it keeps the weaker
`authority_reaches`, which asks only that the restore not delete the component or fall through to the
client's own prediction. One predicate for all four is wrong in both directions, and the second
direction is the non-obvious one: `restore_carries_the_shove` on the pose is strictly stronger, so
every verdict it changes turns a restorable pose into a WAIT — and replication transmits only
components that CHANGED, while the `SameAsPrecedent` markers that would date a stationary hull's pose
forward come from `check_rollback`'s policy branch, which our own forced request skips. A hull that
was standing still when it was shot would stall until the replay window dropped its shove.
`confirm_forced_rollback` deliberately stays velocity-only; it asks whether the dv is already live,
and refusing there would only re-request state that cannot change.

`a_late_pose_removal_is_revalidated_before_the_request` runs the real `Prepare` and was verified RED
on the deleted `Position` — a clean assertion, not a panic. `the_pose_is_asked_a_weaker_question_than_the_shove_and_that_is_deliberate`
pins the asymmetry, which the integration fixture cannot see: that fixture would pass just as happily
against a module that had tightened the pose to the velocity predicate.

**Two Lows, both about tests overstating coverage.** The `SameAsPrecedent` marker in the revalidation
fixtures sat at 106 while the restore target was 104, so the lookup never reached it and only the
removal at 102 was load-bearing — moved to 103, where `get_state_at_or_before(104)` lands ON the
marker and resolves back through it, with asserts in the fixture that keep both orderings true. And
the replay-window fixture observed the fact closed at age 101 but would have passed on a give-up
anywhere from 17 to 100; it now runs twice, one tick apart, and asserts the ages — staged at exactly
100, closed at exactly 101.

**One literally-false claim, corrected in both the `retirement` doc and ADR-0032.**
`check_rollback`'s policy branch is not "the only caller of `ConfirmedHistory::add_unchanged`":
`push_unchanged` calls it too, and lightyear's interpolation invokes that path
(`lightyear_interpolation-0.28.0/src/plugin.rs`, `Update`, `With<Interpolated>`). The proof survives
for a reason unrelated to call counts — that path runs in `Update` on `Interpolated` entities, while
the gap is inside one `PreUpdate` on a `Predicted` hull — and that is now the wording.

### What round 7 found, and what slice 3.11 did about it

**Round 7 did not look for a third instance. It enumerated the class.** The defect shape both rounds 5
and 6 found is *a value latched at one schedule point and consumed at another*, and round 7 assessed
every such value in `src/net/adoption.rs`: **23 rows, 21 safe with stated reasons, 2 defects appearing
at 4 of them.** That enumeration is the round's most valuable artifact, and ADR-0032 now carries it as
**the latch audit table** — latched value → what establishes it → what consumes it → why the answer
cannot move in between. Adding a new latched value to this module means adding a row; a row that
cannot state a reason is a defect.

**The table in the ADR is a RECONCILIATION, and the reconciliation is the interesting part.** The
implementing session enumerated the class independently before receiving round 7's list. Counted in
the table's own rows the merge is **29 — 21 BOTH, 4 REVIEW, 4 IMPL**, so each pass accounts for 25 and
**each missed four rows the other found.** Round 7 lacked the `PredictionHistory` membership condition
(the second half of `prepare_rollback`'s own query filter, sitting beside the `DisableRollback`
condition it found twice), the offer-time frontier/tick, `AuthorityAdoption::staged`, and
`max_rollback_ticks`; the implementer's pass lacked the request-time four-history verdict — which is
the load-bearing step of the ADR's own `Undelivered` unreachability proof — plus the request-time
frontier/age, the offer-time `Predicted`/`Remote` membership, and `carried` as a row in its own right.
Every row is provenance-marked in the ADR. (This paragraph read 23 / 25 / 27 until round 8; those
numbers cannot be reconciled with four each way. The 23 is round 7's count in its own coarser
granularity and is not comparable to a re-derived table's row count.)

**So the honest verdict is that the enumeration is NOT saturating.** If prose enumeration by a careful
reader converged, at least one of two independent passes would have been complete; neither was. 29
rows is the best current inventory, not a proof that the class has 29 members.

**And the direction of the misses is a TENDENCY, not a bias — this paragraph overstated it and round 8
caught it.** Six of the eight non-shared rows are conditions asserted structurally (a query filter, a
schedule adjacency, a derived local) rather than named as a field. **Two are named fields** —
`AuthorityAdoption::staged` and `max_rollback_ticks`, both missed by round 7 — so "named fields were
found by both every time" is false against the table's own contents. Overstating a methodological
finding is the exact failure this arc keeps catching in its tests; the document recording that lesson
does not get to commit it.

**The High: the hull's PARTICIPATION in the restore was latched at the offer.** Everything rounds 5
and 6 fixed is about what a rollback would RESOLVE; none of it asks whether the hull is in the
rollback at all. `prepare_rollback`'s query is filtered `Without<DisableRollback>` and REQUIRES a
`&mut PredictionHistory<C>` per component, and this module expressed that once — as a `Without` filter
on the offer's query, which is the one shape it cannot hand to the shared predicate.

The chain needs no adversarial timing. `net::rig::upgrade_predicted_to_dynamic` inserts
`DisableRollback` in `Update`; `enable_rollback_after_first_tick` removes it in `FixedLast`; Bevy runs
`RunFixedMainLoop` before `Update`, so the marker necessarily survives to the next `PreUpdate`. The
request revalidated only histories and claimed the slot, `prepare_rollback` skipped the hull for every
component, and `confirm_forced_rollback` then computed `carried` from a `ConfirmedHistory` lookup **no
restore had performed** — that buffer answers "what WOULD a restore resolve", and is unchanged by the
hull having been skipped. `carried` came back true and the fact closed as `Adopted`. A silent loss on
the success path, again. The same hole let somebody else's rollback close the fact as `Delivered` and
increment `bypassed` for a hull it never touched.

**Codex's warning was that fixing the request filter alone would be partial, and it is honoured:
`prepare_restores` is asked in BOTH places.** `restore_is_deliverable` calls it first, before any
history is consulted (a hull the restore skips is not waiting for a better sample), and
`confirm_forced_rollback` calls it over the two velocity components its verdict is defined on. An
excluded hull now produces a WAIT at the request; if a future edit ever claims anyway it produces
`Undelivered` — loud and counted — never `Adopted`. Both queries carry the condition as data; the
`Without` filter is gone. That the archetype cannot move between `Prepare` and the proof is checked,
not assumed: one `PreUpdate`, one Bevy sync point, `prepare_rollback`'s commands touch only the
restored component and `PreviousVisual`, and lightyear's only writer of `DisableRollback` is
`check_rollback`'s skip-despawn bookkeeping, which runs before `Prepare` over a different entity set.

`net::lead_zero_rollback::a_hull_excluded_from_prepare_is_never_requested_and_never_adopted` is the
test, and it was verified RED against the pre-3.11 code with the exact defect signature: the fact
CLOSED, `released_on_budget: 1`, 16 ticks replayed by a real rollback, and the hull's `HullShock` still
`default()` — a full rollback ordered and executed while the hull it was ordered for was untouched.
Two unit fixtures cover the seam and the predicate.

**Low 1: `ordering == None` carried three meanings and corrupted `bypassed`.** "Not yet evaluated",
"there is no visual", and "the spark is genuinely pending" were one value, and `retirement` read all
three as the third. `bypassed` is the number that would justify replacing this best-effort rule with a
real presentation barrier, so an inflated count misleads exactly the decision it informs. The latch is
now a three-state enum (`Unasked` / `HoldingForSpark` / `Released`) with every exit from
`clear_to_order` writing it, and `spark_pending` no longer reads the latch for the pending question at
all — it asks `ImpactPresentation`, the same ledger the rule reads, so a spark drawn since the rule
last ran counts.

**Low 2: the pose-asymmetry justification was factually wrong, though the asymmetry is right.** The
claim that a stationary shipping hull publishes no new `Position` is false: `net::physics` disables
Avian island sleeping, Avian's `writeback_solver_bodies` takes `&mut Position`/`&mut Rotation` for
every solver body every step, and both the hit and the episode's close happen in `FixedUpdate` ahead
of that step — so a stationary hull's pose IS marked changed before the checkpoint carrying its
`HullShock`. **The weak pose predicate stays**, because it is correct for a reason that was never
about the wire: the pose carries none of the event, so a recency clause on it can only refuse restores
that are sound. Codex confirmed the other half independently (a forced request skips `check_rollback`'s
policy branch, so it writes no `SameAsPrecedent` markers). The claim is replaced in `adoption.rs` and
ADR-0032, and the tick-80 unit fixture now says it pins the POLICY and that its number is chosen to
separate the two predicates, not claimed as a shipping shape.

**Low 3: this document's own review count was internally false** — six reviews and five DO NOT SHIP,
against six table rows all reading DO NOT SHIP. Corrected above; it is seven of seven now.

**Wire is UNCHANGED at `PROTOCOL_REV` 24.** No wire type, field, order or registration was touched;
every change is client-local rollback bookkeeping.

### What round 9 found, and what slice 3.13 did about it

**Round 9 found no blocking runtime defect**, and confirmed four things that had been asserted rather
than verified: the 32-cell conformance matrix is genuinely non-vacuous, the per-fact eviction latch is
wired at every seam that reads drawn state, the ADR's 29 / 21 BOTH / 4 REVIEW / 4 IMPL accounting is
exact, and the wire is untouched at REV 24.

**The blocker was a document overclaiming, which is this arc's signature failure.** ADR-0032 said an
offer-only re-expression of the participation condition "cannot be written without turning it red".
It can — the scan was evadable by an import alias, a turbofish call, a macro-generated type, a dead
`.whole_body()` call beside a different verdict, a fourth consumer that simply never names
`RollbackParticipation`, and inline comments or string literals, since the stripper removed only
full-line `//` comments.

Slice 3.13 did two things, in this order.

**(a) Strengthened what is cheaply and reliably strengthenable.** The stripper is now a real lexer —
inline and block comments, strings, raw strings, byte strings and character literals, with lifetimes
correctly not mistaken for character literals — and it has its own unit test, because every rule rests
on it. The membership conditions are pinned on the TYPE NAMES (`DisableRollback`,
`PredictionHistory<Position>`, …) at exactly one occurrence each in production rather than on one
`Has<..>` spelling, so an import alias is red. `prepare_restores` is counted on the bare identifier,
so a turbofish or path-qualified call is caught. **The consumer list is DERIVED from the source** —
every top-level `fn` naming `RollbackParticipation` — and asserted to be exactly the three, so a
fourth consumer that names the type fails by construction instead of waiting for someone to extend a
hard-coded list.

*Deliberately not built:* a dead-call check (needs dataflow, not text) and a cross-module scan for
`prepare_restores` (it would have to infer which files are `#[cfg(test)]`-gated modules, and a gate
that cries wolf gets disabled).

**(b) Stated precisely what it guarantees**, in the test's doc, in ADR-0032 and here. See the next
subsection.

Two documentation corrections landed with it, both round 9 Lows — see *Residuals* below.

### What is guaranteed mechanically, what is argued in prose, and what is open

**This is the arc's summary. It supersedes the running record above where they disagree.**

**Guaranteed mechanically — a test fails if it stops being true:**

- The hull's membership in `prepare_rollback`'s restore, as `net::adoption::prepare_restores` states
  it, matches what lightyear's own `RollbackSystems::Prepare` actually restores, per component, over
  all 32 archetypes the two membership conditions produce
  (`prepare_restores_exactly_the_components_the_predicate_names`). **This is the load-bearing
  contract of the participation work.** A lightyear bump that moves a condition fails here.
- The shove is delivered at leads 0, −1 and 8 through the real `Prepare` seam, with live restored
  velocities asserted.
- A hull excluded from `Prepare` is never requested and never adopted.
- A late replicated change — a removal middle-inserted before the restore target — is revalidated at
  the request, and a revalidation that never passes is dropped at the replay window, at exactly the
  age asserted.
- `request_forced_rollback` has exactly one production call site (source scan).
- Every `HullShock` field is hashed into the `shk` trace stream, by destructuring.
- The wire is byte-identical at `PROTOCOL_REV` 24 with all three hashes re-pinned.

**Argued in prose, with a tripwire but no proof:**

- `Retirement::Undelivered` is unreachable against pinned lightyear 0.28. Steps 2 and 3 of that proof
  are DEPENDENCY properties; the branch, its ERROR log and its counter all survive the proof on
  purpose.
- Nothing moves the hull's archetype between `Prepare` and the post-`Prepare` proof. **This is round
  8's open item (a) and it is still open** — a schedule-adjacency argument with no tripwire.
- The latch audit table is the best current inventory of the "latched at one schedule point, consumed
  at another" class, **not a proof that the class has 29 members.** Two independent enumerations each
  missed four rows the other found.
- `MAX_PRESENTED_HITS = 64` is a CHOSEN retention depth. It was `DERIVED` until round 8 killed the
  derivation.

**Guarded, not guaranteed** — `the_three_participation_sites_ask_one_shared_question` is a lexical
scan of one file. It defends against a future author *accidentally* re-expressing the participation
condition, which is the real threat model here: every defect this arc found was written by somebody
who believed the condition was already asked. It does **not** defend against deliberate evasion and
cannot without real AST analysis (a macro can generate a query type it never sees; a site can keep a
live-looking `.whole_body()` call and branch on something else; `prepare_restores` is `pub(super)` and
the scan reads only `adoption.rs`). The older
`only_the_forced_rollback_slot_requests_a_forced_rollback` carries the same limitation and now says
so. **An AST check was assessed and not built**: `syn` in the test tree, plus item-walking and
macro-expansion caveats, to move from "accident" to "evasion" against a threat model with no
evidence behind it. Reconsider only if a lexical evasion is actually observed.

**Residuals, open, with their real scope:**

1. **The pre-staging spark window.** `ImpactPresentation::watched` latches a staged fact's drawn state
   per fact, so eviction cannot touch it from staging onward. Before staging, the answer lives only in
   the `MAX_PRESENTED_HITS` FIFO. Round 9 corrected two things slice 3.12 got wrong here.
   **(i) The exposed interval is the whole pre-staging window, not "one replication gap"** — the
   authority coalesces impulses for up to `SHOCK_EPISODE_TICKS` before publishing an episode while the
   first hit's `ImpactConfirm` broadcasts immediately, so an early spark can precede the episode's
   *publication* as well as its replication and the client's staging; with no entries-per-tick bound,
   64 is headroom over that window and not a bound on it. **(ii) A lost entry is not always
   conservative** — it is a release-on-budget in `clear_to_order`, which is, but the same unseeded
   latch is a false `spark_pending: true` in `retirement`, which INFLATES `bypassed`. Accepted as a
   best-effort telemetry limitation: no delivery decision reads the buffer, and **`bypassed` must be
   read as an upper bound.** Removing it means keying drawn state before the fact exists.
2. **`ordering == Released` short-circuits `spark_pending`** at `adoption.rs`'s `retirement`, and the
   latch is still reachable from `Unasked` and `HoldingForSpark`. Round 9 confirmed this is correctly
   characterised; it stays as-is.
3. **The post-`Prepare` archetype-stability argument** (round 8's item (a), above).
4. **Nothing has been measured.** No two-client capture on a jittered link exists. The bypass rate,
   the budget-expiry rate and the hitch feel are instrumented and unread. No claim in ADR-0032 should
   be promoted to a product fact before that capture.

---

## 4. Running at handoff

Nothing. Rounds 7, 8 and 9 are all answered as of slice 3.13; no job is in flight, the branch is
clean, no agent holds a lock, and the simplifier is idle. **The arc is closed** — the next work is
slice 4, below.

---

## 5. What is next, in order

1. **Round 8 ran and slice 3.12 answered it. What is left of the three questions:**

   - **(a) STILL OPEN.** The audit's soundness argument for the post-`Prepare` proof is a claim about
     what runs between `RollbackSystems::Prepare` and `RollbackSystems::Rollback` — a DEPENDENCY
     property like the `Undelivered` proof, with no tripwire. Round 8 did not close it.
   - **(b) CLOSED by the conformance matrix, and it was weaker than filed.** The concern was that
     `prepare_restores` mirrors lightyear's QUERY rather than observing its EFFECT. Two things
     answer it now. First, the suite was never actually silent about the effect:
     `the_zero_lead_shove_arrives_with_no_replayed_tick_at_all` and
     `a_rollback_this_module_did_not_order_delivers_the_shove_and_is_counted` both run the real
     `Prepare` seam and assert live RESTORED velocities, so a lightyear change that altered the
     restore would already have tripped them — that materially weakened the concern as filed, and it
     should have been recorded when the concern was written. Second,
     `net::lead_zero_rollback::prepare_restores_exactly_the_components_the_predicate_names` now runs
     the real `Prepare` over all 32 archetypes the two membership conditions can produce and asserts,
     per component, that what was restored is what the predicate says. The mirror is a checked
     contract.
   - **(c) ANSWERED, with a NO on the broad form.** Round 8's verdict on the saturating scanner —
     every resource field × every cross-schedule reader — was: do not build it. Bevy's schedule
     position is a composed partial order across plugins, sets and `run_if`s; reads hide behind
     methods; whether a second site *relies* on a query condition is semantic; it would flag
     harmless historical identities, telemetry counters and irrelevant filters; and the allowlist
     would become another hand-maintained copy of the table. One contract and one guard rail were
     built instead — the matrix above, and
     `net::adoption::the_three_participation_sites_ask_one_shared_question`, a lexical source scan
     that pins the participation condition to one spelling (`RollbackParticipation`) and one
     predicate across every consumer it can see. Both were verified RED by mutation. **Round 9
     found the ADR describing them as two equal contracts and the scan claiming evasion-proof
     coverage; slice 3.13 corrected both** — see *What round 9 found* above. Neither saturates the
     table; the table stays a discipline for everything else.

   Of the two rows carried forward as scheduled re-assessments: `ForcedRollbackSlot::installed` is
   still safe partly *because it has no external consumer*, which slice 4 removes. The `PresentedHit`
   eviction argument **was the round-8 Low and is now fixed** — its capacity derivation counted TICKS
   while eviction spends ENTRIES, so a staged fact's own drawn spark could be evicted by unrelated
   impacts and `retirement` would then report a BYPASS for a spark the player had seen. The staged
   claim's drawn state latches per fact now (`ImpactPresentation::watched`, established at
   `AuthorityAdoption::offer`, keyed by the claim's authority identity), outside the FIFO. A residual
   survives across the pre-staging window; round 9 corrected its stated scope, and *Residuals* above
   is the accurate description.
2. **Slice 4 — `render_error`** (task #32, held all session). Fix the one-frame render freeze per
   rollback, and honour the `AdoptionCause` tag so an adopted authority impulse stays sharp instead
   of being smoothed like a misprediction. `AdoptionCause` currently has **no consumer** —
   `ForcedRollbackSlot::installed()` is read only inside `net::adoption`. Held deliberately: I did
   not want presentation logic built on an adoption path that kept changing.

   **Slice 4 is also when the `ForcedRollbackSlot::installed` audit row comes due.** ADR-0032 marks
   it *Safe TODAY, and the reason has an expiry date*: it is freshly overwritten after `Prepare`,
   derived from the current `started`, and **has no external consumer**. `net::render_error` becomes
   that consumer. The row must be RE-ASSESSED, not inherited — a cross-schedule reader in another
   module is a new latch question, and the arc's whole lesson is that the second reader is where the
   condition drifts. Do it before writing the smoothing, not after.
3. **#36 — the native-comparator bypass.** `HullShock` still carries a
   `with_rollback_condition(..)`, a second delivery path that ping influences. Originally scoped
   around "don't disturb the registration"; that constraint was imaginary (see §7) so re-scope it
   rather than inheriting the shape.
4. **#34 — measure what is only derived.** The ~10-tick catch-up, ~156 ms and <120 m dead zone are
   DERIVED and were never measured; before ADR-0032 they existed nowhere in the repo at all. Also
   queued: `jitter_margin` 1.0→4.0, quantize, predict the impact.
5. **#29 — three dangling identifiers** found by the doc gate. Ground truth established:
   `apply_tank_spec` → `tank::spawn_complete_tank`; `net::client::focus_menu` →
   `overlay::focus_declare` (verified by the actual call to `collapse_focus`). **`PUFF_CAP` is not a
   rename** — the puffs no longer have their own cap, they share `BILLBOARD_CAP`, the very constant
   whose doc cites it, so that sentence needs a rewrite. Also decide whether the gate should exempt
   deliberate past-tense citations ("the old `focus_menu`" in `overlay.rs:185/332/343` is history,
   not rot).

---

## 6. The simplifier experiment (separate, `chore/simplify`)

Session-experimental rule: an async Codex simplifier working in
`.claude/worktrees/simplifier`, with the orchestrator responsible for merging. **Not promoted —
do not persist the rule without Yan's decision.**

Delivered: a ranked backlog at `.agents/docs/simplification-backlog.md` (9 entries, plus an honest
record of areas surveyed and found clean, so later runs don't re-search them), and **5 of 9 entries
done**. The best find deleted a verbatim copy of `rig_world_pose` from `bake.rs` — inside code doing
bit-exact `to_bits` comparisons, where a drifted copy would have made the verifier validate a
different composition and still pass.

Remaining: entry 8 (only applicable one left), 3 and 6 blocked by the active-net exclusions, 7 wants
focused tests on its input-closure conventions first.

**Recommendation on record:** do not promote "always one instance running." The value is real but
the backlog is finite — nine items, five cleared in an afternoon, and the tree is otherwise clean.
Promote a *periodic survey-then-batch* instead; the survey is the expensive and valuable part and
only needs redoing after significant new code lands.

---

## 7. Operational notes that cost real time to learn

**Wire changes are a non-issue and must not be avoided.** Client and server ship together and the
handshake is version-exact, so a mismatched peer is *refused*, not desynced. A wire change costs a
coordinated release, which every release already is. Never design around one, never stop to ask.
The only obligation is honesty: bump `PROTOCOL_REV`, re-pin deliberately, and say in the commit
what moved and why. (Related: `4f523b1` *declined* an authorized wire field because `settled_at`
derives from `HullShock::tick`, which REV 24 already carries — "a wire field would only have added
a way for the two to disagree." That is the right instinct, not timidity.)

**Determinism is not a refactor brake either.** Same reason. `PROTOCOL_FINGERPRINT` hashes the wire
surface, never the sim math.

**Codex job tracking — the plugin keys its registry on `git rev-parse --show-toplevel` of your
cwd.** A worktree therefore gets a *separate registry*, and querying from the wrong directory
returns "No job found" for a perfectly healthy job (upstream #524). This caused a real incident:
I read that as a dead job, committed a file out from under a live agent, and launched a duplicate
into its worktree. **Always pass `--cwd <root>` on every call — dispatch, status, result, cancel.**

- The registry has `MAX_JOBS = 50` and prunes by deleting the `.json` *and* `.log`. It sits at the
  ceiling. **Snapshot every result to disk on completion** or it will vanish.
- `status` reports the last value *written*; nothing reconciles it against the OS, so a dead worker
  reads `running` forever. Check the pid from `status --json` as well.
- Never call `result` bare — it resolves to "newest finished job", which is a different run the
  moment anything else finishes. Always pass the explicit id captured at dispatch.
- The `codex:codex-rescue` subagent returns in ~20s having only *launched* the job, so its
  completion notification is structurally meaningless. Dispatch directly and wait with a
  backgrounded shell loop instead (`await-codex.sh` in the session scratchpad).
- Codex's sandbox cannot bind loopback UDP (six tests fail `PermissionDenied`) and cannot write
  `.git` or `.agents`. Both are configurable — `network_access` and `writable_roots` — but see the
  open decisions below.

**`cd` persists between Bash calls in this harness.** Both of the above incidents trace to one
stale cwd. Prefer absolute paths and `git -C`.

---

## 8. Decisions waiting on Yan

1. **Grant Codex write access to `.git`?** Recommendation: **no.** The carveout currently enforces
   the `checkout`/`restore`/`stash`/`clean` prohibition *structurally* rather than by instruction,
   and `writable_roots` is coarse — you cannot grant "commit" without also granting `reset --hard`.
   The commit workflow already works with the orchestrator as the gate. (Note: an earlier version of
   this argument cited uncommitted work in the main tree; that work is now committed at `84473ae`,
   so the argument rests on the structural point alone.)
2. **`network_access = true`?** It is **not** loopback-scoped — the emitted Seatbelt policy is bare
   `(allow network-outbound)` / `(allow network-inbound)`. Recommendation: enable per-run for tasks
   that need the UDP tests, not globally in `~/.codex/config.toml` where it would apply to every
   Codex invocation on the machine.
3. **Victim identity on the wire.** `ImpactConfirm` and `RicochetKeyframe` now carry
   `victim: Option<CombatantId>`, needed for per-hull correlation. The justification has been
   rewritten honestly — it **is** new exact target information, deliberately published, because
   proximity inference was not exact enough (that is precisely why the field was added). Policy
   unreversed pending Yan.
4. **Simplifier promote/drop** — see §6.
5. **`PUFF_CAP`** — needs a doc rewrite, not a substitution; confirm the intended reading.

---

## 9. Hard rules in force

- **NEVER** `git checkout`, `git restore`, `git stash`, `git clean`.
- **NEVER** bare `cargo fmt` — per-file `rustfmt --edition 2024 <file>`. `cargo fmt --all --check`
  is fine as a check.
- One cargo command at a time; `pgrep -f "cargo|rustc"` first.
- Do **not** push, merge to main, tag, or release without explicit approval. Commits to the working
  branch are authorized.
- Do **not** file, open, or comment on anything in any external repository. The lightyear writeup at
  `.agents/docs/upstream/` is a **local draft only** — Yan sends it, if ever.
- Codex is the **verification/review lane only**; Claude agents implement. (The simplifier is Yan's
  explicit, session-scoped exception.)
