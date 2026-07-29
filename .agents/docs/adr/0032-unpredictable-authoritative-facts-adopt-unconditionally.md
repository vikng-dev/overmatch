# Unpredictable authoritative facts adopt unconditionally

> **Status: accepted; landed local on `feat/authoritative-facts`, playtest pending. `PROTOCOL_REV`
> is now 24: the owner-private `HullShock` registration re-pinned it to 22 earlier on the same
> branch, naming the victim on `ImpactConfirm`/`RicochetKeyframe` moved it to 23, and giving
> `HullShock` its own `opened` tick moved it again — see *Correlating a spark with a fact*.**

A fact the client had no information to predict — that it was shot — reaches the player through a
forced rollback that **no threshold can veto**. `net::adoption` decides WHETHER the authority's
state replaces the client's, unconditionally and per fact; `net::render_error` decides HOW hard the
resulting discontinuity is smoothed, from a cause tag the adoption carries. `HullShock` is the first
consumer of that primitive, not its design.

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
- HIDING THE SEAM is thresholded, view-only, and lives in `net::render_error`.

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
confirmed velocity histories `prepare_rollback` restored from and asks whether the sample it resolved
is at or after the producing tick, because a deep rollback over a history whose newest sample
predates the hit restores a PRE-hit velocity and carries nothing. A best-effort rule with a bypass
counter is honest; a guarantee this shape cannot keep is not. The counters are also the evidence that
would justify paying for a real barrier — until they read non-zero on a real link, building one would
be speculation.

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

### There is still a second delivery path, and ping decides when it runs

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

### The `AdoptionCause` tag currently has no consumer

`ForcedRollbackSlot::installed` is read only inside `net::adoption` today. The distinction between
hiding a seam and keeping one sharp is carried, logged, and unused; `net::render_error` acts on it in
the next slice. Until then every adopted fact is smoothed exactly like a misprediction.

## Numbers, and which of them are DERIVED

| Number | Standing |
|---|---|
| 88 mm hull Δv, 0.1383 m/s | MEASURED; pinned by a test that fails if it ever exceeds `ROLLBACK_VELOCITY` |
| `ROLLBACK_VELOCITY` 1.0 m/s is 7.2× that | DERIVED from the two values |
| loopback client lead 0, and −1 under the deadband | DERIVED from four sync constants; guarded by `net::lead_zero_rollback`'s lead-arithmetic test |
| `SHOCK_EPISODE_TICKS` = 16 | CHOSEN inside a DERIVED band of `1 ..= 23` — see below |
| `ORDERING_BUDGET_TICKS` = `RICOCHET_HOLD_TICKS` = 16 ticks | DERIVED — see below |
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

- **Every adopted fact costs one render hitch.** A forced rollback is a pose discontinuity;
  `net::render_error` smooths it into `RenderErrorOffset` and the shipped smoothing still leaves one
  frame of render freeze per rollback. `SHOCK_EPISODE_TICKS` caps that at 64/16 = 4 per second per
  hull under sustained fire, against a DERIVED ~15 per second at 900 rpm cyclic if every pellet
  published its own episode. The residual is `render_error`'s cost to remove — a reason to fix the
  smoothing, not to widen the episode window.
- **The coarse comparator gates are unchanged**, so no jitter-storm class is re-opened by this work.
  That was the constraint the design was built around, not a happy outcome.
- **`request_forced_rollback` now has exactly one production call site**, enforced by a source scan
  rather than by convention, and every forced rollback in the tree carries a cause.
- **`HullShock` stays owner-private** through `CombatDisclosure`: persistent, per-target, aggregate
  state. Public `ImpactConfirm` is not a contradiction — transient, per-shot, spatially anchored, and
  broadcast including to clients who will never render it. Naming a victim on it IS a deliberate
  disclosure decision rather than a no-op, and it is recorded as one above; the aggregate line the
  policy actually draws is unmoved.
- **The trace keeps `shk` as its own stream.** Folding it into the divergence hash corrupts that
  metric: the owner legitimately disagrees with the authority for the whole delivery window of every
  hit, and folded windows read as unexplained drift.
- **What would verify this is a real two-client capture on a jittered link, and it has not been
  taken.** The fixtures prove the mechanism delivers at leads 0, −1 and 8; they say nothing about how
  often the ordering rule is bypassed, how often the budget expires, or how the hitch feels. Those
  three numbers are already instrumented and reported; reading them is the next evidence, and no
  claim in this ADR should be promoted to a product fact before then.
- **The wire moved to `PROTOCOL_REV` 24.** Client and server ship together behind a version-exact
  handshake, so the cost is one coordinated release; `WIRE_TYPES_HASH` and the wire-manifest
  fingerprint were re-pinned in the same diff, and `WIRE_SURFACE_HASH` is untouched because no type
  was added, removed, renamed or reordered.
- **The lead-0 fixture measures a FLOOR, not a constant.** `PRESENTATION_DELAY_TICKS` is the one
  fixed step the schedule's ordering forces between a checkpoint's arrival and the frame that can
  present its spark. Bevy runs `FixedMain` zero or more times per frame, so a catch-up frame can put
  several ticks between the arrival `PreUpdate` and the adopting one. The fixture pins that the rule
  adds nothing to that minimum; it does not claim shipping always produces the number.

## Related

[[0014-sim-view-split]] · [[0015-divergence-doctrine]] ·
[[0016-replicate-causes-derive-consequences]] · [[0021-fire-replication-architecture]] ·
[[0029-weapon-gate-is-tick-correlated-authority-state]] · [[0030-servo-pose-is-owner-reconciled]] ·
`upstream/lightyear-confirmed-state-at-or-ahead-of-local-tick-never-reconciled.md`
