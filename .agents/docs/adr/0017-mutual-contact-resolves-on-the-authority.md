# Mutual contact resolves on the authority; non-owned tanks stay interpolated

Tank-tank contact is resolved server-side, and opponents remain `Interpolated` rather than predicted. We reject predicting non-owned tanks: a client's own hull lives at the predicted tick and an opponent's collider at the interpolation tick (`design/timelines-and-shear.md`), so a local ram is sheared by ~RTT/2 + interpolation delay — and closing that gap by predicting both bodies replaces shear with *mutual misprediction*, fed through a contact solver, which is the one part of the simulation that **expands** perturbations rather than damping them ([[0016-replicate-causes-derive-consequences]], test 3).

## Considered options

A mutual, continuous interaction cannot be lag-compensated: un-shearing for one body shears the other, both are simultaneously querier and queried, and contact persists across ticks so there is no instant to rewind to. That leaves exactly three exits.

1. **Make shear zero by construction** — determinism, predict both, rollback. Rocket League ships this for car-car contact; its physics is trivially cheap by comparison and its tick is deterministic. Ours is neither, today. **Deferred, not rejected — see below.**
2. **Convert mutual into one-way** — per-contact-pair authority handoff (Fiedler). One body owns the contact and the other accepts the result; the non-owner feels the shove late. Unexplored for vehicles, and not obviously wrong.
3. **Resolve where shear is zero** — the server, which holds every entity at one index. **Chosen.** It is also what World of Tanks does (ram damage computed server-side from the contact area).

War Thunder ships option 1 *without* determinism — extrapolated remote vehicles — and reports vehicle-vehicle collision as its unsolved case: *"there is actually no good online solution for a colliding solution anyway."* That is evidence about option 1 minus determinism, not about option 1.

## Consequences

**Determinism comes before predict-everyone, not after.** With a non-deterministic contact solver, two predicted bodies both mispredict and each feeds the other's error through an expansive system. With forward determinism the divergence error class disappears and only irreducible misprediction remains. Revisit this ADR when — and only when — avian's entity-index-keyed constraint ordering is fixed (`scratch/upstream-reports/avian-solver-constraint-order.md`, avian #480/#734).

**Two further blockers, both found in lightyear 0.28 source.** The bot has no `ControlledBy` and no client authoring its input, so nothing rebroadcasts for it and a predicted bot would coast on a default command — this is really *predict-every-player*, mixed mode, and `ServoAngles`/`FireEvent` survive it. And reliable remote fire needs input-side rollback, which targets ticks not gated by state confirmation and would break `apply_net_health`'s tick-agnosticism (`net/protocol.rs`, `a96e9fd`).

**Ramming will still feel wrong**, and the shear is the reason, not the physics. Making the remote body `Kinematic` rather than `Static` would give the contact a correct relative velocity without contesting authority — a cheap, disposable prototype that does not reduce the shear.
