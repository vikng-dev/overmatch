# Unpredictable authoritative facts adopt unconditionally

> **Status: accepted; landed local on `feat/authoritative-facts`, playtest pending. `PROTOCOL_REV`
> is now 25: the owner-private `HullShock` registration re-pinned it to 22 earlier on the same
> branch, naming the victim on `ImpactConfirm`/`RicochetKeyframe` moved it to 23, giving
> `HullShock` its own `opened` tick moved it to 24 — see *Correlating a spark with a fact* — and
> the 2026-07-31 amendment below (comparator ownership: the native `HullShock` condition is
> permanently inert) moved it to 25.**

A fact the client had no information to predict — that it was shot — reaches the player through a
forced rollback that **no threshold can veto**. `net::adoption` decides WHETHER the authority's
state replaces the client's, unconditionally and per fact; `net::render_error` decides HOW hard the
resulting discontinuity is smoothed — from what the rollback was ESTABLISHED to have delivered, per
predicted root, and **not** from the cause tag the adoption carries, which slice 4 found is a
different question (see *The `AdoptionCause` tag is NOT the presentation signal*). `HullShock` is the
first consumer of that primitive, not its design.

## Context

### Two questions that look like one

`net::protocol`'s registered comparators (`ROLLBACK_POSITION_M`, `ROLLBACK_VELOCITY` and their
siblings) answer a COST question: is the owner's own prediction wrong enough that re-simulating is
worth the CPU? That is a judgement about solver noise, it is deliberately coarse, and tightening it
is how this project has twice re-opened a jitter-storm class — the two tolerance bands that exist to
close those storms, `ROLLBACK_TANK_TRANSMISSION_OMEGA_E_RAD_S` and
`ROLLBACK_TANK_SERVOS_CURRENT_RAD`, are the scar tissue.

Being shot is a different question entirely. The client is not wrong; it was never in a position to
be right. Nothing in its inputs, its history, or the world it can see contains the information. This
is [[0016-replicate-causes-derive-consequences]]'s first test — *is the cause complete?* — answered
NO from the client's side, and the consequence is a delivery problem, not a correction problem.
Conflating the two is what broke.

### The arithmetic that makes the conflation fatal, not merely untidy

A MEASURED 88 mm hull impulse is a Δv of 0.1383 m/s (`net::hull_shock_rollback`'s fixture constants,
the same number `net::adoption`'s own test asserts against). `ROLLBACK_VELOCITY` is 1.0 m/s. The
gate is DERIVED 7.2× larger than the event it would have to notice, so on the shipping build **it
could never once have passed a real hit**. The authoritative post-hit velocity arrived, was compared,
was judged close enough, and was discarded — every time, by design.

Nor is the fix to lower the bar. That bar's job is deciding whether solver noise is worth
re-simulating; a bar low enough to see 0.14 m/s is a bar that sees contact noise, and this repo has
the storm captures to show what that costs.

### And on the shipping build the comparator would not even be asked

Worse, and independent of magnitude. lightyear places the client's input timeline at
`remote + rtt/2 + (jitter·jitter_multiple + tick·jitter_margin) + 1 + error_margin − input_delay`.
At loopback both `rtt` and `jitter` are zero, so every measured term vanishes and only the constants
survive: `jitter_margin` 1.0 + the fixed 1 + `error_margin` 1.0 − `SHIPPING_INPUT_DELAY_TICKS` 3 =
**exactly 0**, and the sync controller may sit up to `error_margin` behind without correcting, so the
realised integer lead ranges over `{−1, 0}`. That is a steady-state property of the configuration,
not a transient.

At that lead two skips in the receive path are both reachable at once, read from the published
`lightyear_prediction-0.28.0` sources (a crates.io checkout, not vendored — our `[patch.crates-io]`
block touches only `bevy_*`):

- `src/registry.rs:426-428` runs the registered comparator only when `confirmed_tick < current_tick`.
  At lead 0 they are equal. The trace it emits instead promises a later check; no later check exists.
- `src/rollback.rs:583` returns early from the completed-tick scan for any entity whose Replicon
  `ConfirmHistory` contains the completed checkpoint — which is every actively driven tank.

Each skip is written as though the other one covers the case. Nothing we can register is out of
reach of both. `StateRollbackMetadata::request_forced_rollback` is: `check_rollback` consumes it
before every policy branch and regardless of `RollbackMode`. That is the whole reason this module
exists at the layer it does.

The full analysis, its verification ledger, and two candidate upstream fixes are in
`upstream/lightyear-confirmed-state-at-or-ahead-of-local-tick-never-reconciled.md`. **That document
is a LOCAL DRAFT. Nothing has been filed, reported, or acknowledged upstream**, and no claim here
rests on upstream agreeing with it.

### The part worth keeping: our own fixtures hid it

`net::arrival_rollback` and `net::hull_shock_rollback` both build an 8-tick client lead and
deliberately anchor the entity's `ConfirmHistory` away from the completed checkpoint. Both were
green. Both were always green. Neither world is one the shipping configuration can produce, so
neither could ever have observed either skip. **A fixture built to isolate a mechanism can isolate
it right past the gates that decide whether the mechanism ever runs.** `net::lead_zero_rollback`
exists to hold the shipping regime instead, and its lead-arithmetic guard fails loudly the moment
anyone edits one of the four constants that cancel.

### Prior art for the shape

The split is what shipped engines do, and it is offered as evidence that the shape is right rather
than as something we copied. Source's `PreEntityPacketReceived`/`RestoreData` copies every networked
predicted field from the authority with error checking OFF; the tolerance it keeps feeds only
`cl_showerror` and a decaying camera offset, i.e. the presentation half, and is unreachable from the
acceptance path. Unreal's `bForceNextClientAdjustment` lets the server compel a client correction
that no error metric would have requested. Overwatch's netcode rolls back on every authoritative
packet and treats smoothing as a separate, purely visual concern. Three different engines, one
seam in the same place.

## Decision

**Adopting authority and hiding the seam are separate mechanisms with separate rules.**

- ADOPTING AUTHORITY is unconditional. If the authority says a fact happened, the client adopts it —
  no magnitude test, no tolerance, no policy branch. The only questions asked are whether the
  adoption is *possible* (readiness) and whether it has already happened (dedupe).
- HIDING THE SEAM is thresholded, view-only, and lives in `net::render_error` — and since slice 4 it
  is not hidden at all when the correction is established to carry the fact.

`AdoptionCause` is the tag that connects them, and its two variants are not cosmetic. A
`Misprediction` is the client's own error — the correct state was always knowable locally, so the
view layer should hide the seam as hard as it can. An `ExternalEvent` is a correct physical event
arriving late; smoothing it away would be smoothing away the hit itself, so it must stay SHARP.
Arbitration is asymmetric on purpose: `ExternalEvent` always wins a contested tick, because
mis-tagging a real event as drift would smooth a hit away, while the reverse mistake only leaves an
error un-hidden.

Note what has NOT been decided: the coarse comparator thresholds are untouched. This ADR adds a
layer beside them; it does not re-litigate them, and a future fact of this kind must not either.

## The mechanism, as a recipe

`net::adoption` is a primitive. What the author of the NEXT fact — a ram, a blast, a crew knockout,
a track break — actually has to write is two things:

1. **A replicated, owner-predicted component carrying IDENTITY, not magnitude.** `HullShock` is the
   model: a monotonic (wrapping) episode `count`, the authority `tick` the count changed on, and a
   `ShockCause` tag. No force, no direction, no application point. The shove is never on the wire —
   it exists only as the authority's end-of-tick rigid-body state, and the rollback is what delivers
   it. A counter rather than an event makes "have I already realized this?" a comparison instead of a
   subscription that can miss, because replication carries STATE, not TRANSITIONS.
2. **A producer system in the `OfferAuthoritativeFacts` set** that re-derives its offer from
   replicated state every frame and hands it to `AuthorityAdoption::offer` unconditionally. Offering
   is idempotent; the ledger, not the producer, decides. `offer_hull_shock_adoptions` is the worked
   example, and its trigger is the EXACT comparator `hull_shock_mismatch`, never a magnitude.

Everything after the offer already exists and should not be re-implemented per fact:

- **Readiness.** Split in two. The generic half (`request_staged_adoption`) requires a completed
  replication checkpoint at or past the producing tick, a local tick that has REACHED it, and an age
  inside `PredictionManager`'s `max_rollback_ticks`. The component-specific half is the producer's:
  for the hull, that its `PredictionHistory` is retained at the producing tick and that every
  authority-tracked part of its rigid-body state (`Position`, `Rotation`, `LinearVelocity`,
  `AngularVelocity`) can be restored there FROM AUTHORITY. A pose restored to the producing tick
  beside a velocity that stayed at `now` is not a tick either peer ever had; if completeness cannot
  be PROVEN, the offer waits and is re-derived next frame. The gate fails CLOSED on a missing
  `ConfirmedHistory`, which is not a formality: `prepare_rollback` restores a component that has no
  confirmed history from the client's own `PredictionHistory` **even on a state rollback**, so a
  missing history is precisely the case in which authority does not reach.

  **And EXISTENCE IS NOT DELIVERY** — the fourth review's finding, and the strongest single rule in
  the module. `prepare_rollback` restores `get_state_at_or_before(rollback_tick)`, so proving a
  sample exists there proves only that lightyear will restore *something*. Over a confirmed velocity
  history whose newest sample at or before the producing tick predates the event, that something is
  a PRE-hit velocity: the rollback lands, the seam costs a render hitch, and the shove is not there.
  The velocity half of the gate is therefore the DELIVERY predicate at the tick the request would
  target — the same predicate, over the same buffers, that decides `bypassed`. A fact whose restore
  provably cannot carry the shove is never offered, never staged, and never costs a rollback; the
  offer is re-derived every frame, so it costs nothing to wait. It is also not a regime the shipping
  build reaches: every impulse of an episode CHANGES the hull's velocity, and `HullShock` and the
  velocities ride the same replication group, so the message that first carries the episode carries
  the velocity it produced and stamps both with the same tick.
- **One slot, arbitrated.** `StateRollbackMetadata` holds a single `Option<Tick>`, and a second
  claimant does not queue — it silently narrows the first one's target. `ForcedRollbackSlot::claim`
  is therefore the only production caller of `request_forced_rollback` anywhere in `src/`, pinned by
  a source scan in the module's own tests. `net::watchdog` was migrated onto it and tags its claim
  `Misprediction`. Without that scan the one-slot rule and the cause tag are advice.
- **Dedupe.** `FactId` is `(source, entity, producer sequence, certifying checkpoint)`. Bevy's
  `Entity` carries its own generation, so a despawn/respawn already compares unequal and needs no
  parallel epoch counter. Exact-`FactId` dedupe alone is not enough: a re-send of the same episode
  under a LATER checkpoint is a different `FactId`, so a per-entity sequence watermark is what stops
  one hit buying two rollbacks.
- **Give up loudly.** A fact whose producing tick has aged past the replay window is dropped with a
  warning, not retried. Retrying forever is how a delivery mechanism becomes a storm, and
  manufacturing a client-side impulse instead would be a lie about what happened.
- **A LOCAL staging clock.** `AuthorityAdoption` stamps the local tick a fact was FIRST staged on.
  `AuthoritativeFact::produced_at` is a SERVER tick that already carries the transit, so any patience
  budget measured from it is spent before the client ever sees the fact — adopting instantly in
  exactly the high-latency case a budget exists for. This was wrong in the first draft of the
  ordering rule and is the correction most likely to be re-made by the next author.

### The restore shape, and the one extension that cannot reuse it

A forced rollback to tick `T` restores end-of-`T` state and replays `T+1 ..= current`. **At lead 0
that replay loop runs zero times**, so anything depending on a system re-running is silently dropped.
Two shapes follow, and they are not interchangeable:

- a **state checkpoint** produced at `T` → restore end of `T`. Correct at zero replay, because
  `prepare_rollback` itself writes the live component before the loop that does not run.
- an **event that must EXECUTE at `T`** → restore end of `T−1`, then replay `T`.

`AuthoritativeFact` is deliberately checkpoint-shaped: `produced_at` IS the restore tick. That is
the only shape a hit can have, precisely because nothing about the impulse rides the wire. A future
fact whose realization must be RE-EXECUTED needs the `T−1` variant and, with it, a proof that its
realizing systems are idempotent under replay. The readiness gate makes that extension safe when
someone writes it: requiring that the client has RUN the producing tick already guarantees the `T−1`
shape at least one replay tick.

### Two ledgers, two questions

`AuthorityAdoption` is a plain resource and is never registered for `local_rollback`.
`HullShockLedger::realize` IS rollback-tracked, correctly — it answers "has replay re-realized this
episode from the restored history?", which must rewind with everything else. Using one mark for both
would rewind the "I already asked" bit on every rollback and request forever. The distinction is
easy to lose and expensive to lose.

## What this does NOT guarantee

### The spark-before-shove rule is BEST EFFORT

`net::protocol::ImpactConfirm` is a message drained in `Update`, so the earliest the ballistics
march can present its impact is the NEXT frame. `HullShock` is replicated state, adopted in
`PreUpdate` of the frame it arrives. Two facts the authority produced in one tick therefore land one
client frame apart, shove first — and a hull that lurches before anything is seen to hit it reads as
a bug, where the opposite order reads as physics. So an adoption carrying a `VisualClaim` is held
until this client has PRESENTED one of the HITS the fact is made of, bounded by
`ORDERING_BUDGET_TICKS`.

**It is a preference, not a barrier, and it cannot be made a guarantee at this layer.** lightyear
restores every registered predicted component that has a `ConfirmedHistory` from that history on any
CONFIRMED-STATE rollback, whatever caused it, and nothing in that restore consults this module. A
`Position` mismatch, `net::watchdog`'s claim, or lightyear's own `HullShock` comparator will each put
the authority's post-hit hull velocity on the live hull while the staged fact is still waiting for
its spark. This module owns which rollback it ASKS for; it does not own which rollbacks happen. Only
a barrier on the shove's APPLICATION could close that, and this slice did not build one.

An INPUT rollback's RESTORE is emphatically NOT in that set, and the first draft of the retirement
rule got this wrong: `prepare_rollback` restores from `PredictionHistory` for `Rollback::FromInputs`,
which for a hull the client never knew was hit is its own un-hit prediction. Closing a staged fact
against one would lose the shove outright. `retirement` is therefore given the rollback KIND and
refuses to retire against an input rollback at any depth. Our client disables input rollback
(`net::client::shipping_rollback_policy`), which is why this never shipped; a test asserts that
policy so the invariant cannot be switched off silently, but the rule does not rely on it.

The narrower true statement, because the broad one is not lightyear's behaviour: an input rollback's
REPLAY can still install the fact. `snap_to_confirmed_during_rollback` takes `Single<&Rollback>` and
never branches on the variant, so at every replayed tick it overwrites any predicted component that
has an exact confirmed sample there. An input rollback starting BEFORE the producing tick therefore
replays through it and puts the authority's post-hit velocity live anyway. `retirement` still answers
`Keep` — it cannot observe the replay — and this module then asks for a state rollback that
re-installs state already live. The cost is one redundant rollback, i.e. one render hitch; the
impulse is never re-executed, because it only exists as restored state. Accepted and recorded.

The gap is therefore MEASURED rather than argued away: `OrderingTally::bypassed` counts every
confirmed-state rollback this module did not order that delivered a still-waiting fact, and it rides
the `net::diagnostics` line beside `released_on_impact` and `released_on_budget`. "Delivered" is
ESTABLISHED, not inferred from the rollback's depth: `restore_carries_the_shove` reads the same
confirmed velocity histories `prepare_rollback` restored from and asks what state that lookup
resolves to, because a deep rollback over a history whose newest sample predates the hit restores a
PRE-hit velocity and carries nothing. A best-effort rule with a bypass counter is honest; a guarantee
this shape cannot keep is not. The counters are also the evidence that would justify paying for a
real barrier — until they read non-zero on a real link, building one would be speculation.

**The tick that predicate compares against is the event's, not the sample's**, and the fourth review
caught it comparing the wrong one. `AuthoritativeFact::produced_at` is the tick of the CONFIRMED
SAMPLE that certified the fact — the right restore target, because replication stamps a change with
the tick it was SENT and the client can only ask for a tick the checkpoint reached. But that tick can
sit later than the tick the authority's state actually settled on, and at any send interval above one
tick it ordinarily does. An episode that closes at 100 and materializes in a checkpoint at 104 is
delivered in full by a rollback to 104, because `get_state_at_or_before(104)` resolves the tick-100
velocities — the authority's own post-hit state. A rule demanding the sample be at or after 104
called that nothing. `AuthoritativeFact::settled_at` is the second tick, carried beside the first:
for a `HullShock` episode it is the CLOSE tick, because every impulse the episode is made of landed
in `[opened, tick]` and the end-of-`tick` velocity therefore contains all of them. The invariant
`settled_at <= produced_at` is self-enforcing rather than asserted — the gate resolves a sample at or
before `produced_at` and requires it at or after `settled_at`, so a fact that violated it could never
pass and would never be requested.

**And the same predicate now decides our own retirement.** `retirement` previously answered `Adopted`
from the claim identity alone, so a rollback this module ordered onto a history that resolved to a
pre-event velocity closed the fact permanently having delivered nothing — silently, on the success
path. `Adopted` now means installed AND carried. Installed-but-not-carried is a third outcome:
logged at ERROR, counted in `OrderingTally::undelivered`, and still closed, because a retry re-reads
the buffers that restore just read at the same target tick and cannot improve, and looping on it is
the storm this module exists to avoid.

**Where that predicate is ASKED took a fifth review, and the answer is: at the request, not at the
offer.** The version that reached round 5 asked it once, in `offer_hull_shock_adoptions`, on the
argument that whether `prepare_rollback` will carry the shove is already determined at offer time and
cannot change while `produced_at` is fixed. That argument is false twice over. Confirmed history is
not append-only — `ConfirmedHistory::insert_raw` does a sorted MIDDLE insertion with same-tick
replacement, `SameAsPrecedent` re-resolves against a late preceding sample (lightyear ships a test
for it), and replicon's mutation transport is unordered with history-enabled entities accepting older
mutations. And the control flow never revalidated: an `ExternalEvent` fact stays staged for the
visual budget, often into later frames; re-offering the same identity leaves the staged fact
untouched; a later offer pass that fails readiness merely skips the hull; and `request_staged_adoption`
then consumed that staged fact and claimed the slot on an answer computed frames earlier. No test
noticed, because every fixture in the arc built a static history and never moved it.

So the predicate is asked TWICE and only the second one is load-bearing. The offer's gate is an
ECONOMY — there is one staging slot and an undeliverable fact should not occupy it.
`request_staged_adoption` re-runs it over the histories as they are at that moment, immediately
before claiming, and that is the evaluation the transaction rests on. **A failed revalidation is a
WAIT, not a drop**: the whole point is that the answer can change, so nothing is claimed, nothing is
tallied, and the fact stays staged. The wait is bounded by the replay-window check that runs first in
the same function — once the fact's age passes `RollbackPolicy::max_rollback_ticks` it is closed with
a WARN naming it, and the local tick advances at least once per tick, so the stall cannot be
unbounded and the give-up cannot be silent.

**WHAT is re-asked took a sixth review, and the first fix covered half of it.** Readiness is a claim
about the hull's whole rigid body: the two velocity histories that carry the shove and the two pose
histories that have to survive the restore beside them, because a pose restored to one tick next to a
velocity left at another is not a state either peer ever had. The offer proved all four; the request
re-proved only the velocities. So a late authoritative REMOVAL of `Position`, middle-inserted between
the episode's close and the restore target, passed revalidation, the slot was claimed,
`prepare_rollback` answered the removal by taking `Position` off the hull, and the fact closed as
`Adopted` — because both velocities genuinely were carried, and every counter this module keeps is
defined on the velocities. No tripwire could fire. Both sites now go through one function,
`restore_is_deliverable`, whose signature is what stops the request's query shrinking again.

**The two halves get DIFFERENT predicates, and that asymmetry is load-bearing.** `LinearVelocity` and
`AngularVelocity` carry the fact — a `HullShock` episode is a velocity impulse and nothing else — so
they get the strong question: the restore must resolve a value the authority held at or after
`settled_at`. `Position` and `Rotation` carry none of the event; the impulse changes velocity and the
pose only moves as later ticks integrate it, so a pose sample from before the close is still the
authority's genuine pose at the restore target. They get the weaker question the offer already asked
of them — will the restore leave a rigid body standing at all, or delete a component / leave the
client's own prediction under an authority label. One predicate for all four is wrong in both
directions. The weak one on the velocities is the slice-3.7 defect verbatim. The strong one on the
pose is strictly stronger than what it replaces, so every verdict it changes turns a restorable pose
into a wait — and a wait costs a shove, because the fact sits in the single staging slot until the
replay window drops it.

**The reachability argument that used to be attached to that was FALSE, and the seventh review killed
it.** It said replication transmits only components that CHANGED, so a hull standing still when it
was shot publishes no new `Position` and the strong predicate would stall it. Three facts against
that: `src/net/physics.rs` disables Avian's island sleeping for network physics; Avian's
`writeback_solver_bodies` takes `&mut Position` and `&mut Rotation` for every `SolverBody` on every
physics step; and both the hit and the `HullShock` close happen in `FixedUpdate`, ahead of Avian's
`FixedPostUpdate` step. So even a numerically stationary hull has both pose components marked changed
before the checkpoint that carries its `HullShock`, and the pose is confirmed at or after `settled_at`
in the ordinary case. **The asymmetry is unchanged and still correct** — it rests on what the
components MEAN for this fact, not on a claim about the wire, and no shipping shape is claimed for it
now. The other half of the old argument survives review and is a real property, just not the one that
makes the weak predicate right: a forced request makes `check_rollback` skip its policy branch, so the
`SameAsPrecedent` markers that branch writes are not available to date a pose forward. The unit
fixture pinning the policy (`the_pose_is_asked_a_weaker_question_than_the_shove_and_that_is_deliberate`)
now says that it pins the POLICY and that its 20-tick-old pose is chosen to separate the two
predicates, not claimed to be what the wire produces.

The same round found the predicate reading one lookup where `prepare_rollback` reads another.
`ConfirmedHistory` stores an authoritative REMOVAL as an ordinary entry, so a late removal
middle-inserted between the event and the restore target makes `get_state_at_or_before` resolve
`Removed` — `prepare_rollback` answers that by taking the velocity OFF the hull. The present-value
iterator skips removals and would have reported the older sample the removal shadows, i.e. would have
called it a delivery. `restore_carries_the_shove` now asks lightyear's own lookup first and uses the
present-value scan only for the TICK it resolved at; `authority_reaches` fails closed on `Removed`
for the same reason. A `SameAsPrecedent` entry still counts at its own tick, and that is correct
rather than lenient: the marker asserts the authority still held that value there.

**With the revalidation in place, `Undelivered` is unreachable against pinned lightyear 0.28** — and
the proof is three steps rather than an assurance. (1) The branch is same-frame: it needs both
`adoption.requested` and this module's own installed claim, and `ForcedRollbackSlot`'s claim is
consumed every frame, so both hold only in the `PreUpdate` run that revalidated. (2) Nothing writes
confirmed history in between: `net::watchdog` is read-only, `check_rollback` consumes the forced
request first and then skips its whole policy branch — the only writer of `ConfirmedHistory`
REACHABLE IN THIS GAP — and replicon receive already ran. (3) The two lookups agree, by
construction. The sixth review corrected step 2's wording: an earlier draft called that branch "the
only caller of `ConfirmedHistory::add_unchanged`", which is false — `ConfirmedHistory::push_unchanged`
calls it too, and lightyear's interpolation invokes that path. It does not break the proof, for a
reason that has nothing to do with call counts: that path runs in `Update`, on `Interpolated`
entities, while this gap is inside one `PreUpdate` on a `Predicted` hull. Steps 2 and 3 are
properties of a DEPENDENCY, not of this crate, and a lightyear bump
can retire either without touching a line here. That is exactly why the branch survives its own
proof: a success path that quietly loses the shove is the defect that survives review, "unreachable"
is what each of the previous rounds believed about the branch it was about to lose a shove in, and
the counter is the tripwire that says the proof stopped holding.

### PARTICIPATION took a seventh review, and the audit that came out of it

Rounds 5 and 6 both found the same defect shape — *a value latched at one schedule point and consumed
at another* — and both fixes landed on exactly the instance the finding named. Round 7 stopped
hunting instances and audited the class: every value `src/net/adoption.rs` establishes at one point in
the schedule and acts on at another. **Two defects, showing up at four points; every other value was
safe for a stated reason.** The audit is the table below.

**It did not close the class, and the eighth review is the proof.** Round 8 found a third defect
inside a row round 7 had already assessed as safe: the `PresentedHit` ledger's capacity argument,
whose derivation counted TICKS while its eviction spends ENTRIES. Slice 3.11 had made `retirement` a
second consumer of that row and correctly noted the justification now carried two loads — and the
justification was wrong for both. Read the *Is this the class?* note at the end of the table before
treating the class as shut, and note what the round-8 finding says about a row whose assessment reads
"derived": a derivation is a claim, and this arc has now lost one of them.

**The HIGH one was the hull's participation in the restore itself.** Everything rounds 5 and 6 fixed
is about what a rollback would RESOLVE. None of it asks whether the hull is in the rollback at all.
lightyear's `prepare_rollback::<C>` query is
`(Entity, Option<&mut C>, &mut PredictionHistory<C>, Option<&mut ConfirmedHistory<C>>)` filtered
`Without<DisableRollback>`, so membership is exactly two conditions — and this module expressed them
once, as a `Without` filter on the offer's query. A filter cannot be handed to the function that owns
the readiness question, and neither the request nor the post-`Prepare` proof re-established it.

The chain is production-reachable and needs no adversarial timing. `net::rig`'s
`upgrade_predicted_to_dynamic` inserts `DisableRollback` in `Update` when a late `Predicted` marker
promotes an already-attached rig; `enable_rollback_after_first_tick` removes it in `FixedLast`. Bevy
runs `RunFixedMainLoop` — and with it that remover — BEFORE `Update`, so an insertion necessarily
survives to the next `PreUpdate`, which is the schedule this module lives in. A fact staged while the
hull was eligible was then requested on a hull that was not: revalidation passed (it asked only about
histories), the slot was claimed, `prepare_rollback` skipped the hull for every component, and
`confirm_forced_rollback` computed `carried` from a `ConfirmedHistory` lookup **no restore had
performed**. `carried` came back true and the fact closed as `Adopted`. Nothing delivered, nothing
counted, on the success path. The same hole made a rollback somebody ELSE ordered close the fact as
`Delivered` and increment `bypassed` for a hull it never touched.

**Fixing only the request filter would have been another partial fix**, which is the trap rounds 5 and
6 both fell into, so the fix is two changes. `prepare_restores` is the membership question, mirroring
both of lightyear's conditions rather than paraphrasing them, and it is asked in both places:
`restore_is_deliverable` calls it FIRST (before any history is consulted — a hull the restore skips is
not waiting for a better sample), and `confirm_forced_rollback` calls it over the two velocity
components its verdict is defined on. An excluded hull therefore produces a WAIT at the request, and
if a future edit ever claims anyway it produces `Undelivered` — loud and counted — never `Adopted`.
Both queries carry the condition as DATA now; the `Without` filter is gone, because a filter is the
one shape the shared predicate cannot be given.

That the archetype cannot move between `Prepare` and the proof is a checked claim, not an assumption:
both run inside one `PreUpdate` with only Bevy's own sync point between them, `prepare_rollback`'s
commands touch the restored component and `PreviousVisual` and nothing else, and lightyear's only
writer of `DisableRollback` is `check_rollback`'s deterministic skip-despawn bookkeeping — which runs
before `Prepare` and only over entities in `PredictionManager::deterministic_skip_despawn`.

**The LOW one corrupted the metric that decides a design question.** `AuthorityAdoption::ordering` was
an `Option<bool>` whose `None` carried three different situations: the rule had not been consulted
(every early return in `request_staged_adoption` ahead of `clear_to_order`), the fact had no visual to
be ordered against (a `Misprediction` returns before latching), and the rule genuinely had the fact
and was holding it. `retirement` read that one `None` as "the spark is pending" and incremented
`bypassed` in all three. No simulation state depended on it — but `bypassed` is precisely the number
that would justify replacing this best-effort rule with a real presentation barrier, so an inflated
count would mislead the decision it exists to inform.

The latch is now a three-state `Ordering` enum (`Unasked` / `HoldingForSpark` / `Released`) and every
exit from `clear_to_order` writes it, so the one state `retirement` must not observe is the one no
exit writes. And `spark_pending` is no longer read off the latch at all: it is a question about what
has been DRAWN, so it asks `ImpactPresentation` — the same ledger the rule reads — while using the
latch only for the thing a latch can answer, that a released fact is released. A spark drawn between
the frame the rule last ran and the frame the rollback lands now counts, which the latch could not
see.

### The latch audit — every value established at one schedule point and consumed at another

Round 7's contribution beyond the two fixes, and the reason this ADR carries a table at all.
**Adding a new latched value to `net::adoption` means adding a row here**, stating what establishes
it, what consumes it, and what stops the answer moving in between. A row that cannot state the third
is a defect.

**PROVENANCE, and it is the most useful thing on this page.** The table below is a RECONCILIATION of
two independent enumerations of the same class over the same source: round 7's, and the implementing
session's, made without having seen round 7's. Counted in the table's own rows: **29 rows — 21 BOTH,
4 REVIEW, 4 IMPL.** Each pass therefore accounts for 25 of them and each missed four the other
found. That asymmetry, not the row count, is the finding: see *Is this the class?* below.

Provenance is marked per row. `BOTH` = independently found twice. `REVIEW` = round 7 had it and the
implementer's pass did not. `IMPL` = the reverse.

**The counts above are round 7's inventory and are not re-derived per slice.** Slice 4 struck one row
out (`ForcedRollbackSlot::installed`, deleted rather than guarded) and added one (`SharpCorrection`,
which is a message and not a latch, listed so the class stays enumerated rather than because it needs
defending). Both are marked in place; the 29/21/4/4 figures are left as the reconciliation produced
them, because the paragraph above is a finding about two enumerations of one file at one moment, and
editing its arithmetic every slice would destroy the only thing it says.

**An earlier version of this paragraph said 23 / 25 / 27, which is arithmetically impossible with
four each way, and the eighth review caught it.** The 23 is round 7's own count in round 7's own
granularity; this table is finer in at least two places (the four-history readiness is split into its
velocity and pose halves, which were two separate review findings, and the staged transaction is
split into `AuthorityAdoption::staged` and the conditions on it). Two counts taken at different
granularities are not comparable and nothing may be derived from their difference — which is exactly
what the old sentence did.

#### Offer-time latches, consumed at the request or later

| Latched value | Later consumer | Assessment | Prov. |
|---|---|---|---|
| Offer-time `hull_shock_mismatch` between confirmed and predicted `HullShock` | the staged transaction itself | Safe. The authority episode is HISTORICAL — it cannot un-happen — and if another rollback delivers it first, confirmation retires it. (The implementer's pass called this "not carried", which is wrong: the staged fact's existence *is* the latched verdict.) | BOTH |
| `FactId::entity` | request, confirm | Safe. Bevy generations make a despawn/respawn compare unequal, so reincarnation cannot collide; a despawn becomes a `hulls.get` failure and `Unready::Hull`. | BOTH |
| `FactId::source`, `sequence`, `checkpoint` | offer dedupe, close | Safe. Source and sequence are historical; the checkpoint is exact identity and logging only. Never an input to readiness or delivery. | BOTH |
| `cause` | slot identity, retirement | Safe. Immutable classification, and same-tick slot arbitration preserves `ExternalEvent`. | BOTH |
| `produced_at` | restore target, age/window, retirement | Safe as a TARGET. It is the tick the fact is about; the frontier and replay-window reachability are both re-checked at the request. | BOTH |
| `settled_at` | velocity delivery predicate | Safe under the fixed-step WRITER invariant: the simulation produces one final hull value per lightyear tick, so `insert_raw`'s same-tick replacement is idempotent in fact. Note the shape of that reason — it holds because of who writes the data, not because the type forbids the violation, so it does not survive a change of writer. | BOTH |
| `visual` (`victim`, `from`, `through`) | presentation ordering | Safe as historical authority identity — set membership, never a window over local time, which is what round 2 killed. Ledger eviction is conservative in the ordering rule; see the `PresentedHit` row for the consumer slice 3.11 added. | BOTH |
| Offer-time four-history readiness, velocity half | request, Prepare | Safe since `baade0b`: re-read at the request. Round 5's defect. | BOTH |
| Offer-time four-history readiness, pose half | request, Prepare | Safe since `a0ac961`: re-read at the request through the same signature. Round 6's defect. (Round 7 carried these as one row; they are split here because they were two separate review findings.) | BOTH |
| Offer-time `Predicted` / `Remote` / `HullShock` history membership | request, Prepare | Safe, and for a STRONGER reason than the lifecycle one. Round 7's argument is that no surviving-entity removal path exists and a despawn becomes a `hulls.get` failure — true. The structural argument is better because it survives a lifecycle change: `prepare_rollback`'s query does not consult `Predicted`, `Remote`, or the `HullShock` histories at all, so losing any of them could not exclude the hull from the restore. Losing `Predicted` would exclude it from `check_rollback`, which a FORCED request bypasses anyway. | REVIEW |
| **Offer-time absence of `DisableRollback`** | request, Prepare, confirm | **DEFECTIVE.** A production writer (`net::rig`, in `Update`) inserts the marker between the offer and the request, and nothing re-checked it. Fixed by slice 3.11 — `prepare_restores`, asked at the request and after `Prepare`. | BOTH |
| Offer-time `PredictionHistory` membership for the four rigid-body components | request, Prepare | Safe, and **it was never a live defect** — an earlier draft of this table said "defective by omission" and overstated it. `add_prediction_history` inserts the buffer and lightyear never removes it on a surviving entity: a component removal is recorded INTO the history (`add_predicted(tick, None)`), the buffer stays. It is asked anyway, because it is the second half of `prepare_rollback`'s membership condition and its safety is a DEPENDENCY lifecycle property with no tripwire. Round 7's participation rows name only `DisableRollback`. | IMPL |
| Offer-time checkpoint frontier and `LocalTimeline::tick` | — | Not carried. The request re-reads `ReplicationCheckpointMap`, and `age` and `waited` are recomputed from the current tick every frame. | IMPL |

#### Module state carried across frames

| Latched value | Later consumer | Assessment | Prov. |
|---|---|---|---|
| `AuthorityAdoption::staged` | request, confirm | Safe. It IS the transaction; every *condition* on it is re-established at the request, and re-offering the same identity deliberately leaves it untouched. (A finer split of round 7's "staged transaction" consumer.) | IMPL |
| `staged_at` | ordering wait | Safe. A LOCAL monotonic patience origin — a re-offer must not restart it, which is why it is not re-stamped — and it resets on reconnect and on close. | BOTH |
| `ordering = Released` (was `Some(true/false)`) | retries, retirement | Safe. Both verdicts are irreversible historical decisions, and the latch is what makes the tally count facts rather than frames. | BOTH |
| **`ordering = Unasked` (was `None`)** | retirement's `spark_pending` | **DEFECTIVE.** The same `None` also meant "not evaluated yet" and "there is no visual", and all three were counted as a pending spark. Fixed by slice 3.11 — three-state enum, every exit writes it, and `spark_pending` asks the presentation ledger. | BOTH |
| `requested` | retry logic, retirement | Safe. Re-derived every frame against pending metadata / manager state, and our own adoption additionally requires the exact installed claim. | BOTH |
| `adopted` ledger and sequence watermarks | later offers | Safe under the documented one-owner bound; the bounded ring drops the OLDEST and the watermark covers what it drops; reconnect resets timeline-specific identity. | BOTH |
| **`PresentedHit` ledger entries** | `clear_to_order`, **and `retirement` since slice 3.11** | **WAS DEFECTIVE.** Entries are historical and cannot become false, but the CAPACITY argument was: `MAX_PRESENTED_HITS` was derived as `SHOCK_EPISODE_TICKS + ORDERING_BUDGET_TICKS`, i.e. as a budget in TICKS, while eviction is spent per ENTRY — and nothing bounds entries per tick, since every authority armor impact is appended and impacts are broadcast to every client. A staged fact's own spark could be drawn and then evicted by unrelated hits, after which `retirement` read "never drawn" and reported a BYPASS for a spark the player had seen. Slice 3.11 made this the row's second consumer and inverted the direction of the error; slice 3.12 removed the dependency instead of resizing it. The staged claim's drawn state now latches on `ImpactPresentation::watched`, keyed by the claim's authority identity, established at `AuthorityAdoption::offer` and outside the FIFO entirely. **A residual survives, and slice 3.12's account of it was wrong in both directions — the ninth review's Low.** The buffer still SEEDS that latch, so a spark drawn before its fact was staged is exposed to eviction; the exposed interval is the whole PRE-STAGING window (the authority coalesces for up to `SHOCK_EPISODE_TICKS` before publishing, while the first hit's `ImpactConfirm` broadcasts at once, so the early spark can precede publication as well as replication and staging), and with no entries-per-tick bound 64 is headroom over it rather than a guarantee. Nor is the residual only conservative: an unseeded latch is a release-on-budget in `clear_to_order`, which is conservative, **and** a false `spark_pending: true` in `retirement`, which INFLATES `bypassed`. Accepted as a known best-effort limitation of the telemetry — no delivery decision reads the buffer — with `bypassed` read as an upper bound. | BOTH |
| `OrderingTally` | `net::diagnostics` | Counters are monotonic and session-scoped on purpose (reconnect clears `presented` and deliberately not the tallies). Round 7's caveat — `bypassed` can receive a defective classification — was the `ordering == None` defect and is closed. | BOTH |

#### Request-time latches, consumed later in the same `PreUpdate`

| Latched value | Later consumer | Assessment | Prov. |
|---|---|---|---|
| Request-time checkpoint frontier and age | slot claim, Prepare | Safe. Neither the timeline nor the frontier changes in that gap: `LocalTimeline` advances in `FixedFirst`, and replicon receive ran before this module. | REVIEW |
| Request-time four-history verdict | Prepare | Safe with respect to history mutation: `check_rollback` consumes the forced request FIRST and then skips its policy branch, the only writer of `ConfirmedHistory` reachable in that gap. **This is the load-bearing row behind the `Undelivered` unreachability proof above** — and the implementer's pass discussed it in prose without listing it, which is exactly the kind of omission a table is supposed to prevent. | REVIEW |
| **Request-time Prepare participation** | Prepare, confirmation | **DEFECTIVE.** Histories may be perfectly valid while `DisableRollback` excludes the entity — the two questions are independent and only one was asked. Fixed by slice 3.11. | BOTH |
| `ForcedRollbackSlot::claim` | lightyear's `Check`, confirmation | Safe. The exact target is re-checked against the manager, the claim is `take`n unconditionally every frame so it cannot survive one, and a source scan pins that all production requesters go through the slot. | BOTH |
| `max_rollback_ticks` | request | Safe. Read fresh from the `PredictionManager` each frame. | IMPL |

#### Post-`Prepare` latches

| Latched value | Later consumer | Assessment | Prov. |
|---|---|---|---|
| `started` and rollback kind (`RestoresFrom`) | retirement | Safe. Read directly off the installed current rollback and consumed immediately, in one system. | BOTH |
| **`carried`** | retirement | **DEFECTIVE.** Safe only if `Prepare` selected the hull, and it did not ask — it read a `ConfirmedHistory` result that is a COUNTERFACTUAL when eligibility excluded the entity. This is the same defect as the participation rows seen from the consumer end, and listing it separately is what makes "gate the request" visibly insufficient on its own. Fixed by slice 3.11. | REVIEW |
| ~~`ForcedRollbackSlot::installed`~~ | ~~retirement, logging~~ | **DELETED by slice 4 (2026-07-30), which is what the expiry date bought.** The row read "safe TODAY, and the reason has an expiry date: it is freshly overwritten after `Prepare`, derived from the current `started`, and **has no external consumer yet**. `net::render_error` becomes one in slice 4, at which point this row must be re-assessed rather than inherited." The re-assessment found the field's *timing* was never the problem — the value is semantically the wrong answer for the consumer that was going to read it (see *The `AdoptionCause` tag* below) — so the field and its accessor are gone rather than guarded. `confirm_forced_rollback` now takes the claim into a LOCAL and consumes it in the same statement sequence; no confirmed value is stored anywhere. **The audit is one row shorter, not one row safer.** | BOTH |
| `SharpCorrection` (the presentation occurrence slice 4 added) | `net::render_error::capture_render_error` | Not a latch, and deliberately shaped so it cannot become one. It is a message, written after `Prepare` from the ESTABLISHED `Retirement`, and DRAINED after `Rollback` but before `EndRollback` in the same `PreUpdate` — unconditionally, whether or not any root matches it — so nothing survives the frame. It also names the exact `Entity`, generation included, so a despawned victim's occurrence cannot match its replacement. Pinned by `net::render_error::an_occurrence_is_drained_on_the_frame_it_is_written_even_with_no_rollback_to_apply_it_to`, `..._occurrences_spanning_adjacent_message_buffers_are_both_drained_by_one_capture`, and `..._naming_the_previous_incarnation_of_an_index_cannot_sharpen_the_current_one`. | IMPL |

#### Is this the class?

**No, and the reconciliation is the evidence.** Two careful enumerations of one class over one file, done independently, each missed four rows the other found:

- Round 7 lacked the `PredictionHistory` membership condition — the second half of `prepare_rollback`'s own query filter, sitting directly beside the `DisableRollback` condition it did find twice — the offer-time checkpoint frontier and `LocalTimeline::tick`, `AuthorityAdoption::staged` as a row of its own, and `max_rollback_ticks`.
- The implementer's pass lacked the request-time four-history verdict, which is the load-bearing step of this ADR's own `Undelivered` unreachability proof; the request-time frontier and age; the offer-time `Predicted`/`Remote` membership; and `carried` as a row in its own right.

If prose enumeration by a careful reader saturated, at least one of the two passes would have been complete. Neither was. So **29 rows is the best current inventory of the class, not a demonstration that the class has 29 members.** Treat it as a checklist that has caught real defects, not as a proof of exhaustiveness.

**What the misses are LIKE, stated at the strength the evidence supports.** Six of the eight non-shared rows are conditions asserted structurally — a query filter, a schedule adjacency, a derived local — rather than named as a field, so there is a real TENDENCY for structural assertions to be missed. It is a tendency and not an established bias, and an earlier version of this paragraph overstated it into "both passes missed in the same direction", which the eighth review killed with the table's own contents. **Two of the eight are named fields**, and both were missed by round 7: `AuthorityAdoption::staged` and `max_rollback_ticks`. (The `staged` row describes itself as a finer split of round 7's "staged transaction" consumer while being marked `IMPL`; both are true, and together they are the counterexample — round 7 named the transaction, the implementer's pass named the field.) Overstating a methodological finding is the exact failure this arc keeps catching in its own tests, and the document that records that lesson does not get to commit it.

#### What was built instead of the saturating scanner

The saturating form of this table would be: every field of every resource this module owns × every system that reads it, wherever writer and reader sit at different schedule positions, plus every condition a query asserts at one site and another relies on. **Round 8 assessed that and rejected it**, for reasons that survive re-reading: Bevy's schedule position is a composed partial order across plugins, sets and `run_if`s; field reads hide behind methods and helpers; whether a second site *relies* on a query condition is semantic rather than syntactic; it would flag historical identities, telemetry counters and irrelevant filters; and the allowlist needed to quiet it would become another hand-maintained copy of this table. A gate that cries wolf gets disabled.

**One contract and one guard rail were built instead**, and between them they would have caught the round-7 `Without<DisableRollback>` defect from both ends. They are not two contracts of equal weight, and an earlier version of this section implied they were.

- **THE CONTRACT — `net::lead_zero_rollback::prepare_restores_exactly_the_components_the_predicate_names`**, a runtime conformance matrix. Over `DisableRollback` present/absent × each of the four `PredictionHistory<C>` present/absent (32 archetypes), it runs lightyear's own `RollbackSystems::Prepare` and asserts, per component, that what was restored is what `prepare_restores` says, and that the whole-body verdict is the conjunction. This is what makes the query-mirror a CHECKED contract rather than a reading of the dependency's source at a point in time — and it closes the concern the slice-3.11 handoff filed against itself, that the predicate mirrors lightyear's query without observing its effect. It is load-bearing: it observes real behaviour, and a lightyear bump that moves the membership conditions fails here.
- **THE GUARD RAIL — `net::adoption::the_three_participation_sites_ask_one_shared_question`**, a lexical source scan over one file, read with comments and literals stripped. `Without<DisableRollback>` may not appear in the module; `DisableRollback` and the four rigid-body `PredictionHistory<..>` types may be named exactly once each, in the `RollbackParticipation` declaration, which forecloses an import alias as well as a re-expression; `RollbackParticipation` itself may be named exactly five times in production, every occurrence accounted for by name — the declaration, the `RollbackParticipationItem` accessor `impl`, and one query in each of the offer, the request and the confirmation — so a fourth consumer is red WHEREVER it is written, including as a method in an `impl` or a `fn` in a nested `mod`; the top-level functions naming the type are additionally derived from the source and each must route through `prepare_restores` via the two accessors; and `prepare_restores` may be named nowhere else in production, counted on the bare identifier so a turbofish or path-qualified call is caught too.

**The occurrence count replaced a line-shape heuristic, and the tenth review is why.** Slice 3.13 derived the consumer set from every *column-zero* `fn`, which skipped every indented line — so a consumer written as a method or inside a nested module was absent from the derived list and passed all four rules, and the helper's own unit test asserted that omission as though it were intended. An `impl` or a nested module is **ordinary organization, not evasion**, so "a fourth consumer is caught by construction" was overstated in the same narrower form the ninth review had already blocked once. Counting occurrences of the type name is the rule shape the membership types already used; it is blind to indentation, line shape and rustfmt, and it needs no parser. Both mutations — the nested module and the method — were verified red.

**What the guard rail guarantees, precisely — and this is the ninth review's finding.** It defends
against a future author *accidentally* re-expressing the participation condition. That is the real
threat model here: every defect this arc found was written by somebody who believed the condition was
already asked. It does **not** defend against deliberate evasion, and no lexical scan can — a macro
can generate a query type it never sees, a site can keep a live-looking `.whole_body()` call and
branch on something else, and `prepare_restores` is `pub(super)` while the scan reads only
`adoption.rs`. **An earlier version of this section said the offer-only re-expression "cannot be
written without turning it red". That is false and the ninth review killed it** — it can be,
deliberately. It was the same class of overstatement this arc has spent nine rounds catching in its
own documents. The module's older `only_the_forced_rollback_slot_requests_a_forced_rollback` carries
the identical limitation and now says so in its own doc.

**An AST-based check was assessed in slice 3.13, declined, and the reasoning was wrong.** It was
declined on the grounds that item walking defends only against deliberate evasion. The tenth review
corrected that: robust item walking would also have protected against **ordinary methods and nested
modules**, which the column-zero heuristic could not see — a materially different trade than the one
recorded. **The verdict stands and the reason is now different.** What made an item walk worth its
cost was exactly the ordinary-organization hole, and the occurrence count closes that hole with no
parser at all. What is left over for a walk is macro-generated query types, dataflow (a live-looking
`.whole_body()` call beside a different verdict), and callers in other files — and a `syn` item walk
over this one file closes none of those either. So a real Rust parser in the test tree (`syn` as a
dev-dependency, plus its own item-walking and macro-expansion caveats) would now buy nothing this
arc has evidence for. Reconsider if a lexical evasion is ever actually observed, and not before.

Neither can pass vacuously: the matrix's all-present cell demands four restores, so a run in which no rollback installed fails immediately, and its live values are distinct from both the authority's and `default()`; the scan panics rather than passing when an item it names is gone, and its two helpers — the comment/literal stripper and the top-level-item finder — carry their own unit tests, because every rule rests on them. The stripper's is table-driven over twenty shapes (nested block comments below the top level, `/*/`, multi-hash raw strings containing their own shorter terminator, comment markers inside strings and string markers inside comments, `'\''`, `'\\'`, `'"'`, byte characters, raw identifiers, lifetimes adjacent to quotes, and unterminated literals and comments at end of file), each asserting that nothing hidden leaks, that no code is eaten, and that char and line counts are unchanged. The item finder's now asserts its column-zero limitation TOGETHER WITH the occurrence count that covers it, rather than asserting the limitation alone. Both checks were verified RED by mutation before being kept; slice 3.13 re-verified the scan against an import alias, a turbofish direct call, a fourth column-zero consumer, and an inline comment quoting a forbidden spelling (which must NOT trip it), and slice 3.14 against a consumer in a nested `mod` and a consumer as a method in an `impl`. **What they do not do is saturate the table** — they enforce one condition each, exactly, and the table remains a discipline for everything else.

### Correlating a spark with a fact is an identity test — and it took a wire field

The rule is only as good as its answer to "is this spark THIS shove's?", and a first draft answered
it with a time window over the LOCAL tick each spark was drawn on. That is unanswerable in the
client's clock: a local tick measures transit and frame scheduling, so no window over it can tell a
coalesced episode's own early spark from a different hit's spark. The counter-example that killed it:
hit A publishes at tick 100, hit B lands at 104 and is deferred to 116, and if B's own visual is lost
then A's spark — 16 local ticks away — satisfied B's window and released B's shove.

The replacement is set membership on two AUTHORITY facts:

- **Which episode.** `AuthorityImpact::tick` is the server tick the impact resolved on, i.e. the tick
  `HullShockLedger::arm` ran. The episode names its own span: `HullShock::opened` is the tick of its
  FIRST impulse and `HullShock::tick` the tick it closed on, so `[opened, tick]` is the set of
  impulse ticks it covers.
- **Whose hull.** `AuthorityImpact::victim` names the body the authority gave the impulse to — the
  same body whose ledger it armed.

**The span is carried, and the second draft's derivation of it was the third review's finding.**
Deriving `(close − SHOCK_EPISODE_TICKS, close]` looks equivalent and is not: `close_episode` defers
only behind an OPEN episode, so the first impulse a fresh `HullShockLedger` ever sees publishes
immediately and spans ONE tick. The derived window claimed fifteen ticks that episode never covered
— and `tank::spawn` gives every fresh hull a default ledger while `net::server` respawns by
despawning the entity and spawning a new one with the SAME `CombatantId`, so those fifteen ticks are
exactly where the previous life's hits and their drawn sparks live. A prior life's spark could
release a respawned hull's first shove. Carrying `opened` makes the span exact for every episode by
construction: it is stamped on the first tick a pending impulse is observed and cleared on
publication, so `opened ≤ close` and the next episode's `opened` is strictly greater than this one's
`close`. Consecutive episodes span disjoint ranges, and a fresh ledger's first span reaches back over
nothing — its lower bound is at or after the tick the entity was spawned.

Both halves of that are claims about PLAIN numeric order and are **not wrap-general**, which is worth
saying because the ledger's deferral test one line away deliberately IS wrap-aware
(`now.wrapping_sub(last)`). The tick counter the spans live in is lightyear 0.28's `Tick`: a `u32`
compared with plain `u32::cmp` and advanced with SATURATING arithmetic, on that crate's documented
assumption that a session never reaches the ~828-day boundary. At saturation the timeline freezes, so
a pending episode stalls rather than wrapping into a span that falsely covers an older spark — the
safe direction. Anything that made the counter genuinely wrap invalidates the ordering argument and
not the deferral one.

Both halves cost wire. `PROTOCOL_REV` 23 added `victim: Option<CombatantId>` to `ImpactConfirm` and
`RicochetKeyframe` (both, because a ricochet arms an episode exactly as an embed does); REV 24 added
`opened: u32` to `HullShock`. The impact tick needed nothing new — `impact_tick` and `bounce_tick`
already rode both facts and were already buffered client-side in
`SanctionedTerminal`/`SanctionedBounce`.

**`victim` IS new exact target information, published deliberately.** The earlier claim that a
public `Position` already made it derivable by proximity does not survive inspection, and it
contradicted the reason the field was added: `ImpactConfirm::position` is a point on a SURFACE while
a replicated `Position` is a tank ROOT, so nearest-root is a different question with a different
answer whenever hulls are adjacent or overlapping; the poses an observer would compare against are
its own interpolated ones, on a different timeline from `impact_tick`; and the fact is broadcast
`NetworkTarget::All`, so the inference would have to hold for clients who never witnessed the
contact. If geometric inference were exact enough, the field would not have been needed. What it buys
is per-hull correctness of the spark/shove correlation — an inexact victim releases the tank beside
the one that was hit. What stays owner-private is unmoved, and is what made `HullShock` private in
the first place: how many times a hull has been hit, with what worst cause, and for how much damage.

### There is still a second delivery path, and ping decides when it runs — SUPERSEDED

> **Superseded by the 2026-07-31 amendment below.** The capture this section asked for was taken
> (`design/hullshock-delivery-capture-2026-07-31.md`); it corrected this section's premises — the
> lead gate is necessary but not sufficient, real latency does not imply positive lead (measured
> −0.76 tk at 40/5), and the "only path that has ever run on a real link" claim was false — and
> the second path is now closed. The original text is kept for the record:

`HullShock` remains registered with a native `.with_rollback_condition(..)` over
`hull_shock_mismatch`. That comparator is a pure function of two component values, so it cannot
consult the presented-hit ledger or anything else this module knows, and its
receive-time dispatch is gated on `confirmed_tick < current_tick` — FALSE at the zero/negative lead
loopback produces, TRUE on any link with real latency. So the module documented here is the live
route in playtest and the other one is the live route in WAN play. **That is open work, not a solved
problem.** Making adoption the sole route is a one-line change plus a rewrite of
`net::hull_shock_rollback`'s positive control, and it was deliberately not bundled: it would retire
the only delivery path that has ever run on a real link in favour of one whose readiness gates have
so far been exercised only in fixtures. It would also SHRINK the hole above, not close it — every
other rollback cause still restores the same state.

### The `AdoptionCause` tag is NOT the presentation signal — slice 4's finding (2026-07-30)

This section used to read: "`ForcedRollbackSlot::installed` is read only inside `net::adoption`
today. The distinction between hiding a seam and keeping one sharp is carried, logged, and unused;
`net::render_error` acts on it in the next slice."

It does not, and it cannot. The tag records who **claimed** the forced-rollback slot; what the view
layer needs is what the rollback **delivered**. This ADR already documents both ways those disagree,
in `confirm_forced_rollback` / `retirement`:

- `Retirement::Delivered` — another subsystem's confirmed-state rollback carried the staged hit onto
  the live hull. The tag reads `None` or `Misprediction`, so a cause-tag reader would smooth away a
  hit that is already live. `net::lead_zero_rollback::a_rollback_this_module_did_not_order_delivers_the_shove_and_is_counted`
  is that case end to end, and it is the fixture that goes red if the signal is ever re-derived from
  the tag.
- `Retirement::Undelivered` — our own `ExternalEvent` claim was installed and `prepare_rollback`
  restored a pre-hit velocity. The tag says "keep this sharp"; there is no hit in that correction to
  keep sharp, so it would expose a seam for nothing.

On top of that, `installed` was a single global `Option` carrying no target entity, while sharpness
is per predicted root: one rollback corrects every armed root, and only one of them was shot.

**So the signal is `SharpCorrection`** — an entity-keyed, one-shot message emitted from the
established `Retirement` (`Adopted` and both `Delivered` variants; never `Keep` or `Undelivered`,
pinned exhaustively by `only_the_two_retirements_that_delivered_the_fact_keep_the_seam_sharp`), and
consumed in the same `PreUpdate`. `ForcedRollbackSlot::installed` is deleted. The tag's remaining
jobs are same-tick slot arbitration (`AdoptionCause::wins_over`), logging, and the `ExternalEvent`
predicate on the message.

**What the consumer does with it**: `net::render_error` does not accumulate that root's correction at
all. Not "decay it faster" — that still delays and attenuates the hit and introduces a feel threshold
with no meaning — and not a smoothed/sharp split of one correction, because the corrected pose is the
nonlinear result of the impulse, ordinary divergence, replay and contacts and the provenance to
decompose it does not exist. Every other root in the same rollback smooths normally, and an older
offset already decaying on the sharp root keeps decaying.

## Numbers, and which of them are DERIVED

| Number | Standing |
|---|---|
| 88 mm hull Δv, 0.1383 m/s | MEASURED; pinned by a test that fails if it ever exceeds `ROLLBACK_VELOCITY` |
| `ROLLBACK_VELOCITY` 1.0 m/s is 7.2× that | DERIVED from the two values |
| loopback client lead 0, and −1 under the deadband | DERIVED from four sync constants; guarded by `net::lead_zero_rollback`'s lead-arithmetic test |
| `SHOCK_EPISODE_TICKS` = 16 | CHOSEN inside a DERIVED band of `1 ..= 23` — see below |
| `ORDERING_BUDGET_TICKS` = `RICOCHET_HOLD_TICKS` = 16 ticks | DERIVED — see below |
| `MAX_PRESENTED_HITS` = 64 | **CHOSEN for headroom, and it used to claim to be DERIVED** — see below |
| a ~10-tick delivery catch-up, ~156 ms, and a <120 m dead zone | **DERIVED, and never measured** |

**`SHOCK_EPISODE_TICKS` is a judgement, not a derivation.** Only the CEILING is arithmetic:
`ROLLBACK_POSITION_M` 0.05 m ÷ 0.1383 m/s = 0.3615 s = 23.14 ticks, past which ordinary position
reconciliation would have caught the drift unaided and the episode is no longer delivering the shock
any earlier than the fallback it replaces. 16 keeps 7 ticks of margin under that; the last 30 % of
the band would spend nearly all of it for ~1 fewer render hitch per second. The floor is empty —
every window in the band beats per-pellet and none of them gets the hitch rate below ~3 per second.

**`ORDERING_BUDGET_TICKS` inherits `RICOCHET_HOLD_TICKS`**, the window a cosmetic shell already
spends frozen at armor waiting for the same authority verdict. Past it the shell dissolves and no
impact is ever presented for that shot, so a longer wait cannot buy a visual — it can only make the
shove lag something that is never coming.

**`MAX_PRESENTED_HITS` is the one number in this table that lost a derivation.** It was
`SHOCK_EPISODE_TICKS + ORDERING_BUDGET_TICKS` = 32, on the reasoning that nothing older than that
span of ticks can still be asked about. The span is right; the units are not. Capacity is spent per
ENTRY and the derivation counted TICKS, and nothing bounds entries per tick — every authority armor
impact from every combatant lands in the same buffer. What made this matter rather than merely being
untidy is the second consumer slice 3.11 added: an evicted spark is a conservative false negative in
`clear_to_order` and an INFLATED `bypassed` in `retirement`, and `bypassed` is the number that would
justify replacing the ordering rule with a real presentation barrier. The fix is not a bigger number:
the staged fact's drawn state now latches per fact, outside the buffer.

**The buffer's remaining job is to SEED that latch, and the ninth review found slice 3.12's
description of what that leaves exposed too narrow in both directions.** It is not "one replication
gap". A coalesced episode's first hit broadcasts its own `ImpactConfirm` immediately while the
episode itself is withheld for up to `SHOCK_EPISODE_TICKS`, so the early spark can precede the
episode's PUBLICATION as well as its replication and the client's staging — the exposed interval is
the entire pre-staging window, and every impact from every combatant lands in the same buffer across
it. With no entries-per-tick bound, **64 is headroom over that window, not a bound on it.** And the
error is not purely conservative: an unseeded latch is a release-on-budget in `clear_to_order`, which
is, but the same unseeded latch is a false `spark_pending: true` in `retirement`, which inflates
`bypassed` for a spark the player saw. That residual is ACCEPTED as a best-effort telemetry
limitation — no delivery decision reads this buffer — and `bypassed` is therefore an upper bound.
Removing it means keying drawn state before the fact exists, which is a design change rather than a
constant.

The last row needs saying out loud, because it is the shape of number this repo has been burned by
before. The chain is: a representative ~10-tick catch-up at 64 Hz is 156.25 ms; at the Tiger's
authored 773 m/s that is 120.8 m of flight; below roughly that range a shell's entire flight is
consumed by the catch-up, so no spark can precede the shove and the ordering rule has no headroom to
work in. Every step after the 10 is arithmetic. **Nothing measures the 10.** No capture exists, no
constant in the tree is set from it, and it decides nothing in the code today — which is exactly why
it has survived unpinned and why it is recorded here as an estimate rather than as a finding. A
`~125 m` figure of the same shape circulated in this codebase for weeks in 2026-07 before turning out
to be derived, never measured, and 2.5× too large.

## Consequences

- **~~Every adopted fact costs one render hitch.~~ Since slice 4 (2026-07-30) an adopted fact costs
  none.** This bullet used to read: "A forced rollback is a pose discontinuity; `net::render_error`
  smooths it into `RenderErrorOffset` and the shipped smoothing still leaves one frame of render
  freeze per rollback." Both halves were wrong. `net::render_error` now refuses to accumulate a
  correction established to have delivered the fact, so the shove is presented on the frame it lands
  — that was the residual this bullet called "`render_error`'s cost to remove", and removing it is
  what slice 4 did. And "render freeze" overstated what the smoothing ever did to the frames it does
  smooth: the offset is DECAYED before it is applied. DERIVED from the constants at 64 Hz, 95.305%
  retention holds through 0.25 m; the 3 m/s cap first binds at ~0.553 m and makes MORE of the old
  pose survive as error grows. Exactly 2 m is capped and smoothed; values above it snap. The SIM
  never stops — `Position`, `Rotation`, the velocities, replay and fixed ticks all continue,
  because that layer writes only `Transform`. What `SHOCK_EPISODE_TICKS`
  still buys is the RATE at which coincident ordinary misprediction is re-presented: 64/16 = 4 per
  second per hull under sustained fire, against a DERIVED ~15 per second at 900 rpm cyclic if every
  pellet published its own episode.
- **The coarse comparator gates are unchanged**, so no jitter-storm class is re-opened by this work.
  That was the constraint the design was built around, not a happy outcome.
- **`request_forced_rollback` now has exactly one production call site**, enforced by a source scan
  rather than by convention, and every forced rollback in the tree carries a cause.
- **Every condition this module acts on is re-established where it is acted on**, including the
  hull's membership in `prepare_rollback`'s query — and the full list of values latched at one
  schedule point and consumed at another is the audit table above, which a new latched value is
  expected to extend.
- **That membership condition is now one mechanical contract with a guard rail on top, not a prose
  assurance — and the difference between those two is stated rather than blurred.** The runtime
  matrix holds `prepare_restores` against lightyear's own `Prepare` over all 32 archetypes the two
  conditions can produce; that is the load-bearing check. The source scan holds the condition to a
  single spelling and a single predicate, which defends against an ACCIDENTAL re-expression and, by
  its nature, against nothing deliberate. Neither saturates the audit table, and the rejection of the
  scanner that would have tried to — and of an AST-based scan that would harden the guard rail — is
  recorded above rather than left as an open intention.
- **`HullShock` stays owner-private** through `CombatDisclosure`: persistent, per-target, aggregate
  state. Public `ImpactConfirm` is not a contradiction — transient, per-shot, spatially anchored, and
  broadcast including to clients who will never render it. Naming a victim on it IS a deliberate
  disclosure decision rather than a no-op, and it is recorded as one above; the aggregate line the
  policy actually draws is unmoved.
- **The trace keeps `shk` as its own stream, and it hashes every field.** Folding it into the
  divergence hash corrupts that metric: the owner legitimately disagrees with the authority for the
  whole delivery window of every hit, and folded windows read as unexplained drift. REV 24's `opened`
  was added to the wire and NOT to the hash, so two peers could disagree about which hits an episode
  covered and still produce identical `hshk` diagnostics; it is hashed now, and the exhaustiveness
  test destructures `HullShock` so the next field cannot be forgotten the same way — an enumerated
  list of "every field" is a list that rots.
- **What would verify this is a real two-client capture on a jittered link, and it has not been
  taken.** *(SUPERSEDED 2026-07-31: the capture was taken — see the amendment below and
  `design/hullshock-delivery-capture-2026-07-31.md`.)* The fixtures prove the mechanism delivers at leads 0, −1 and 8; they say nothing about how
  often the ordering rule is bypassed, how often the budget expires, or how the hitch feels. Those
  three numbers are already instrumented and reported; reading them is the next evidence, and no
  claim in this ADR should be promoted to a product fact before then.
- **The wire moved to `PROTOCOL_REV` 24, and has not moved since.** *(SUPERSEDED 2026-07-31:
  REV 25, the amendment below — reconciliation semantics only, bytes unchanged.)* Client and server ship together
  behind a version-exact handshake, so the cost is one coordinated release; `WIRE_TYPES_HASH` and the
  wire-manifest fingerprint were re-pinned in that diff, and `WIRE_SURFACE_HASH` is untouched because
  no type was added, removed, renamed or reordered. `AuthoritativeFact::settled_at` is deliberately
  NOT a wire field: it is derived client-side from `HullShock::tick`, which REV 24 already carries.
  A field would have added a way for the two to disagree.
- **The lead-0 fixture measures a FLOOR, not a constant.** `PRESENTATION_DELAY_TICKS` is the one
  fixed step the schedule's ordering forces between a checkpoint's arrival and the frame that can
  present its spark. Bevy runs `FixedMain` zero or more times per frame, so a catch-up frame can put
  several ticks between the arrival `PreUpdate` and the adopting one. The fixture pins that the rule
  adds nothing to that minimum; it does not claim shipping always produces the number.

---

## Amendment (2026-07-31): comparator ownership — the native `HullShock` condition is inert

**Decision.** `HullShock` remains explicitly registered `.replicate().predict()`, but its rollback
condition is permanently inert (`|_, _| false` — explicit, because an omitted condition falls back
to `PartialEq::ne` and silently re-arms the trigger). `net::adoption` is the sole INTENTIONAL
present-value `HullShock` delivery policy at every prediction lead. `hull_shock_mismatch` survives
as the fact detector, called directly by adoption on the confirmed histories. `PROTOCOL_REV` 24 →
25 (the REV-21 precedent: identical bytes, different reconciliation semantics); wire surface and
type hashes unchanged; only the manifest fingerprint re-pinned.

**The evidence that gated it** (`design/hullshock-delivery-capture-2026-07-31.md`, five seeds,
80/10, two clients + dedicated server). Adoption is clean in play: 176–183 facts per run staged,
passed readiness, adopted, released on impact — zero budget releases, zero undelivered, zero
replay-window drops, zero waits. The native comparator ordered exactly FOUR rollbacks per run —
the first round of every MG belt, landing the shove a measured 1–3 ticks before its spark — and
nothing else (the ~190 `HullShock` `trg` rows per run were receive-time TRIPS riding adoption's
own forced rollbacks; trip-slot attribution is not order attribution). The old second-path
section's premises did not survive the capture: receive-time dispatch needs more than
`confirmed_tick < current_tick` (state mode `Check`, `should_check_mismatch_at`, unpruned
history), and real latency does not imply positive lead (measured −0.76 tk at 40/5) — which path
runs was never loopback-vs-WAN.

**The structural carve-out, without which "sole" would be false.** Lightyear's presence
mismatches (`(Some, None)` / `(None, Some)`) order rollback without consulting any registered
condition. No production lifecycle reaches that shape — `HullShock` rides every spawn bundle, is
never removed server-side, and respawn REPLACES the entity, so the client always receives it on
the init/seed path that records no mismatch — and `net::hull_shock_rollback` pins the exception
(`a_presence_mismatch_still_rolls_back_without_the_comparator`). Likewise, once ANY state
rollback is ordered, `prepare_rollback` restores `HullShock` from confirmed history regardless of
trigger: delivery by an unrelated rollback remains an observable preemption, which is why
`Retirement::Delivered`, `OrderingTally::bypassed`, and the competing-rollback fixture all
survive unchanged. Ordering stays BEST EFFORT — this amendment moves trigger ownership; it is not
an application barrier.

**Belt-first execution now** (the class the old trigger defeated): state arrival → staged → held
for its spark → the march draws the episode's own hit → released on impact → forced rollback at
the producing tick → sharp retirement. If the spark never draws — the cosmetic carrier rides a
loss-bounded unordered channel — the CHOSEN 16-tick budget releases the shove anyway, still
sharp: `a_missing_spark_spends_the_budget_and_still_delivers_sharply`. Both sequences are pinned
in `net::hull_shock_rollback` on the production registration at the positive lead the native
trigger used to own; the lead-0/−1 fixtures in `net::lead_zero_rollback` were already
adoption-shaped and are untouched.

**Acceptance instrument.** Per-fact ownership telemetry (`k:"fact"` rows in `SPIKE_TRACE`:
staged with the visual-claim span, waiting, released, requested, retired with route and
`carried`, dropped, spark) landed BEFORE the comparator flip so baseline and treatment captures
read identically. The A/B gate on the same five seeds: every client-observed fact terminates as
adopted / delivered / dropped / undelivered; `bypassed → 0` with `adopted` absorbing the old
bypasses (recorded: 185–190 per run against a planning estimate of ≈ 181–183);
`undelivered` and drops still 0; `trg == 0` for `HullShock` as SECONDARY wiring evidence only
(the flip silences that instrument by construction, so it cannot be the primary gate). The
acceptance denominator is CLIENT-OBSERVED state transitions with sequence deltas recorded — the
measured 5-of-190 sequence gap (send-window coalescing hypothesis) stays open work, not a blocker.

**A/B result (2026-07-31, same five seeds, per-fact rows on both arms).** Treatment:
`bypassed = 0` on every seed (baseline 3–6 — wider than belt-first-only; seed 5 had six,
including mid-belt), routes 100 % adopted (185–190 per run), `undelivered`/drops/unterminated
all zero, zero budget releases (the seed-5 ricochet case released on impact), max hold 3–4 ticks
against the 16-tick budget, `HullShock` `trg` zero, and total rollbacks DOWN ~10–20 % (269–289
vs 301–349) — the native trigger had been ordering duplicate rollbacks adoption then re-ordered.
Accepted.

**Still open after this amendment:** the sequence-gap denominator (server-side per-episode
telemetry), the real-link frequency and feel of visual-loss budget releases, and the application
barrier — only if unrelated-rollback bypasses ever become material, which the capture found no
evidence of.

## Related

[[0014-sim-view-split]] · [[0015-divergence-doctrine]] ·
[[0016-replicate-causes-derive-consequences]] · [[0021-fire-replication-architecture]] ·
[[0029-weapon-gate-is-tick-correlated-authority-state]] · [[0030-servo-pose-is-owner-reconciled]] ·
`upstream/lightyear-confirmed-state-at-or-ahead-of-local-tick-never-reconciled.md`
