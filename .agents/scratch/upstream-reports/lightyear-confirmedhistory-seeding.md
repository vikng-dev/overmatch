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
