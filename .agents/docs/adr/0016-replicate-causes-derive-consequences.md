# Replicate causes, derive consequences

The server and every client run the same simulation. What goes on the wire is therefore not "the state
the client needs to draw" but **the minimal set of facts a client cannot derive for itself** — and the
rest is re-derived locally by the identical sim. This ADR states the classification that decides which
is which, and names the invariant that the hand-written publish/apply pumps have been relying on
without saying so. It builds on [[0015-divergence-doctrine]] (which governs *why* the two sides drift)
and on [[0014-sim-view-split]]'s standing rule that sim state must be constructible at spawn.

## Context: two state representations and a pump between them

Combat replication grew one component at a time, and each new synced behavior arrived as the same
three-part tax: a wire component, a `publish_*` system on the authority, an `apply_*` system on the
replica, plus an ordering constraint between them.

| sim state | wire component | pump |
|---|---|---|
| `ComponentHealth` (per volume) | `NetHealth(Vec<f32>)` | `publish_net_health` / `apply_net_health` |
| `TankSim::servos` | `ServoAngles` | `publish_servo_angles` / `apply_servo_angles` |
| launched turret pose | `LaunchedTurretPose` | `publish_launched_turret_pose` / `apply_launched_turret_pose` |
| `TankCommand` | `ActionState<TankCommand>` | `bridge_action_state_to_tank_command` |

The tax is real — it is why barrel recoil went unimplemented for a whole slice: syncing it *looked*
like it needed `NetRecoil` plus two more systems. But the tax is the symptom. The disease is that the
table above mixes two completely different kinds of fact, and the pumps hide which is which.

The codebase already contained the right pattern and had not named it. `apply_net_health` writes
replicated HP into local health *before* `DamageConsequences`, and death, crew loss, cook-off and
knockout then **emerge locally on every machine** from that one replicated fact. Nothing about death
is on the wire. That is the whole idea, generalized below.

## The decision

Classify every piece of state by **whether it self-corrects**, and let that decide the wire.

**1. Continuous, self-correcting, spring- or servo-like → DERIVE.** Barrel recoil is a damped spring
returning to battery. Servo lay converges on its commanded target. A reload is a countdown from a fire
tick. Error in these is transient by construction: the system eats its own drift, so a client that
computes them from a replicated *cause* converges on the server's answer without ever being told the
answer. These must not be on the wire.

**2. Discrete, path-dependent, absorbing → REPLICATE as truth.** Health, death, ammunition, cook-off
flags. No self-correction exists, the states are absorbing, and a mispredicted death is unrecoverable.
`NetHealth` stays. Note the corollary that makes this class cheap: because these states are monotonic,
they need no rollback history — a replay can never *un*-kill.

**3. Chaotic rigid-body → REPLICATE the body.** Tank pose, the launched turret. Contact chaos means a
client's independent integration will not track the server's, and no amount of derivation fixes it.
`LaunchedTurretPose` stays; so do `Position`/`Rotation`/velocities.

By this rule the table above is misfiled in exactly two places. `ServoAngles` is a **consequence** of
the aim input, replicated only because a remote tank has no input to derive it from. And `FireEvent`
exists only because a remote tank runs no local `fire()`. Both are artifacts of a missing cause, not
facts that need transmitting — see *Consequences* below.

## The real reason: pumps leave a temporal invariant unstated

Hand-written pumps are not wrong because they are hand-written. They are wrong because a bare
`apply_*` system copies a value across a **tick boundary on which that value may not be valid**, and
nothing in its signature forces anyone to say which boundary is safe. A first-class replicated
component with history cannot express the bug; a hand-written copy can, silently.

Two instances, one live and one latent, both found by auditing against this rule:

- **`bridge_action_state_to_tank_command` (live bug, fixed in `701d0a7`).** lightyear extrapolates a
  starved input stream by holding the last `ActionState` forever. Hold-last is the correct
  extrapolation for `TankCommand`'s *levels* and *absolutes*, and wrong for its *edges* — the bridge
  re-latched `fire_primary`/`crew_swap` every tick, defeating `consume_edges`, so a server whose
  client's uplink starved fired an unrequested shot per reload cycle. The unstated invariant was: *an
  edge is only valid on a tick a real input arrived for.* The fix is to say so, by consulting the
  input buffer.

- **`apply_net_health` (not a bug, invariant now documented in `0fa6cd8`).** It applies newest-confirmed
  health to whatever tick is being simulated, forward or replayed. Forward is correct — that is just
  prediction. Backward would not be, because the drive/reload/fire capability gate rides the `Dead`
  marker, which is monotonic and never rolled back, so a replayed pre-death tick would suppress thrust
  the forward sim applied. It is unreachable **only because state rollback runs in `RollbackMode::Check`**,
  which starts every rollback at `last_confirmed_tick` and only on a mismatch detected there. Nothing
  in the system said so. Now it does.

The second is the more instructive: the safety of a pump depended on a rollback-policy setting in an
entirely different module, and would silently break if that setting changed. That is the cost of an
unstated invariant, and it is why the classification matters more than the boilerplate.

## Consequences

**Barrel recoil is a derive, and landed as one (`71987cf`).** `FireEvent` already carried an
entity-mapped `shooter`; it now also names the weapon slot. No impulse rides the wire — each machine
reads `Weapon.recoil.kick` from its own RON spec, keyed by `WeaponIndex`, and both ends derive the
identical kick. No new replicated component. The wire carries *which weapon fired*; the spring is a
consequence.

**`ServoAngles` and `FireEvent` are deletable, not fixable.** Both are consequences on the wire,
present only because a remote tank has no inputs. Give a client the opponent's inputs and
`drive_aim_servos` and `fire()` run locally on the remote tank; the servo lay, the tracer, the barrel
kick, the reload timer and the hull shove all fall out of the shared sim, and both components have
nothing left to carry. `FireEvent` is therefore best read as a **symptom of not having remote inputs**
rather than as a protocol feature. That change is [[0015-divergence-doctrine]]'s territory and is not
decided here.

**Adding a new synced behavior starts with the classification, not with a component.** If it
self-corrects, find the cause that is already on the wire (or put the *cause* there) and derive it. If
it is absorbing, replicate it and note that it needs no history. If it is a chaotic body, replicate the
body.

**Every pump must state its tick-alignment invariant in its doc comment**: which tick the value it
copies is valid for, and what makes that safe during rollback replay. A pump that cannot answer is a
bug that has not been found yet.

## What this ADR does not say

It does not say the wire should carry only inputs — that is deterministic lockstep, rejected
([[0004-avian-physics]]-era, reaffirmed) and still rejected. State replication remains the
architecture; the authority re-anchors clients regardless, and derivation is an optimization that
reduces what must be corrected, exactly as [[0015-divergence-doctrine]] treats determinism as the
rollback-killer rather than a correctness requirement.

It does not claim derivation is free. A derived consequence drifts if its cause is late or lost. The
classification is precisely the argument that class 1 tolerates that drift and classes 2 and 3 do not.
A dropped `FireEvent` costs a tracer and a barrel kick, never a hit — because the hit is class 2.
