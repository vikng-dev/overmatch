# lightyear 0.28: local_rollback components restored to add-time defaults via stale ConfirmedHistory seed

**Target:** lightyear 0.28 · **Severity for us:** MEDIUM (worked around) · **Status:** unfiled

## Suggested title

local_rollback::<C> components on replicated entities restore to their ConfirmedHistory seed
instead of the predicted value

## Mechanism

Components registered with `local_rollback::<C>()` (client-local state that must replay across
rollbacks, never replicated) get a `ConfirmedHistory<C>` seeded at component-add time when they
live on a replicated entity. On rollback, the restore prefers that stale confirmed seed over the
`PredictionHistory` value — so the replay starts from the add-time default rather than the
predicted state at the rollback tick, corrupting all carried state in the component (in our
case: `TankSim` — servo velocities, weapon reload, wheel brush anchors — and `DriveState`).

## Suggested upstream fix

local_rollback components on replicated entities should never consult ConfirmedHistory (there is
no server authority for them by definition); restore exclusively from PredictionHistory.

## Our workaround + removal condition

`strip_confirmed_history` observers (src/net/protocol.rs:199-242): strip the seeded
`ConfirmedHistory<TankSim>`/`<DriveState>` so replays restore the predicted value. Standing
tier-2 rule in AGENTS.md: any new rollback-registered + replicated-entity-attached carried state
must be registered there too. Remove when upstream restores local_rollback exclusively from
PredictionHistory.

## What fixing this unlocks for us

**Clean up.** The generic observer `strip_confirmed_history::<C>` and both registrations
(`net/protocol.rs`, `::<TankSim>` and `::<DriveState>`), and with them a standing discipline: ADR-0014
records that *"`strip_confirmed_history` stays load-bearing … deleting the strip guard would resurrect
the aim-desync class with correct-looking spawn code"* — i.e. today every new `local_rollback` component
that lands on the replicated tank root must be registered in the strip list or it silently restores to
its add-time default on the next rollback (the failure mode is a turret resolving away from the aim point,
not a crash). That trap is the thing worth deleting; the two lines of registration are incidental.

**Optimize.** Nothing measurable — the observers fire once per component add.

**Explicit non-payoff — this is NOT why `NetBelts` is pinned every tick** (checked, because it is the
obvious hypothesis). `apply_net_belts` overwrites the client's belt from server truth every tick instead
of using a lightyear-native predicted-component correction, and the reason (commit a2a0aed and the
`apply_net_belts` doc) is that `WeaponState::belt_remaining` lives inside `TankSim`, which is
**never replicated at all** — it is `local_rollback` client state, so there is *no* confirmed value to
roll back to, seeded or otherwise. That is our architecture choice (a fat sim struct stays off the wire),
not this defect. Fixing the seeding would make `local_rollback` restore from the *right local source*; it
would not give `TankSim` server authority, so the belt would still need to be told. The native path stays
closed for reasons that have nothing to do with this report.

**Explore.** Nothing. Honestly: this fix deletes a workaround and a footgun, and that is all it does.
