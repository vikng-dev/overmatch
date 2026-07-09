# Replicate causes, derive consequences

The wire carries the minimal set of facts a client cannot work out for itself; every other machine re-derives the rest by running the same simulation. A consequence is derivable iff **its cause is available to us** and **its dynamics tolerate that cause's staleness** — barrel recoil derives from a replicated fire event because a fire event is a complete cause and a damped spring is contractive; health does not derive, because a hit resolution is neither.

## The three tests

Ask, in order:

1. **Is the cause complete?** Can the whole future be computed from information you already hold? A projectile's is: `(origin, direction, speed, fire tick)` plus physics — *a projectile has no free will*. A tank's is not: it depends on a human's next input.
2. **Is the interaction one-way or mutual?** A shell striking a hull is one-way, and therefore also the only class where lag compensation is structurally possible. A collision is mutual: resolving it for one body changes the other. See `design/timelines-and-shear.md`.
3. **Are the dynamics contractive or expansive?** Do perturbations decay or grow — the Lyapunov question, not a vibe. A servo chasing a target contracts error. A damped spring contracts error. **A contact solver expands it.** This is distinct from [[0015-divergence-doctrine]]'s *continuity* rule: a contact solver is roughly continuous and still expansive.

Pass all three and derive. Fail any and replicate.

## Consequences

**Absorbing state needs no rollback history.** Health, death, ammunition and cook-off flags are monotonic — a replay can never *un*-kill — which is why they replicate cheaply and why `ComponentHealth`/`Dead` carry no `PredictionHistory`.

**Every pump must state its tick-alignment invariant.** A hand-written `publish_*`/`apply_*` pair copies a value across a tick boundary on which it may not be valid, and nothing in its signature forces anyone to say which boundary is safe. Both defects the 2026-07-09 slice found were that same defect: the input bridge re-latched command edges onto starved ticks (`701d0a7`), and `apply_net_health` is tick-agnostic and safe only because a rollback-policy setting in another module happens to be `Check` (`0fa6cd8`, corrected in `a96e9fd`). Write the invariant, or you have not finished.

**A derive must be one implementation, not two that agree today.** `shooting::kick_recoil` and `ballistics::advance_shell` each exist once and are called from both the local and the remote path. A derive that branches differently per end is two implementations wearing one name.

**`ServoAngles` is replicated because its cause is unavailable, not because it fails the dynamics test.** Servo lay is about as contractive as anything in the sim; the aim input simply is not on the wire. It becomes derivable exactly when remote inputs are — and the bot, having no client to author its input, would still need it (`FireEvent` likewise). An earlier revision of this ADR called it *misfiled*; that was a promise the architecture cannot keep.

## What this ADR does not say

It does not say the wire should carry only inputs. Deriving *every* consequence leaves only causes on the wire, which is deterministic lockstep — rejected (`design/sim-divergence-and-determinism.md §4.4`) because the slowest peer gates everyone and one divergence desyncs permanently with no authority to re-anchor. State replication stays. Derivation is an optimisation that shrinks what must be reconciled, exactly as [[0015-divergence-doctrine]] treats determinism as the rollback-killer rather than a correctness requirement.

Nor is derivation free. A derived consequence drifts when its cause is late or lost — which is precisely what test 3 is for. A dropped `FireEvent` costs a tracer and a barrel kick, never a hit, because the hit is absorbing and replicated.
