# lightyear_avian3d 0.28: blanket ApplyPosToTransform poisons child-collider attachments

**Target:** lightyear_avian3d 0.28 (lightyear repo) · **Severity for us:** CRITICAL (fixed 33cc4e4) · **Status:** unfiled

## Suggested title

AvianReplicationMode::Position rewrites child-collider local Transforms from render-blended
state (compounding attachment drift)

## Mechanism (probe-confirmed + code-verified)

`AvianReplicationMode::Position` mounts `sync_position_to_transform` (plugin.rs:175, PostUpdate)
AND `sync_received_position_to_transform` (plugin.rs:176/653-661, PreUpdate), and registers
`ApplyPosToTransform` as a **required component of `Position`/`Rotation`** (plugin.rs:620-623,
"make sure PositionToTransform sync also runs for Interpolated entities"). **Scope (verified
2026-07-10): the blanket registration is NOT Position-mode-only** — it lives in
`sync_position_to_transform` (plugin.rs:613-624), which is also mounted by
`AvianReplicationMode::Transform` (plugin.rs:302) and `PositionButInterpolateTransform`
(plugin.rs:245/250); every mode that syncs position to transform carries the poisoned
requirement. Side effect: every
CHILD COLLIDER (they carry `Position`/`Rotation` as collider required components, no
`RigidBody`) enters avian's `position_to_transform` write set (`PosToTransformFilter =
Or<(With<RigidBody>, With<ApplyPosToTransform>)>`, avian3d-0.7.0 physics_transform/mod.rs:254-257).

That system (mod.rs:318-349) rewrites the child's LOCAL `Transform` as its sim-world `Position`
`reparented_to(parent GlobalTransform)` — but in PostUpdate the parent's GlobalTransform is
render-blended (frame interpolation / visual correction) and one `TransformSystems::Propagate`
stale. Each render frame deposits the sim-vs-render difference into the collider's authored
attachment; avian's `propagate_collider_transforms` then folds it into `ColliderTransform` next
tick — a compounding feedback loop. The child's local Transform is authored constant INPUT;
deriving it from world state inverts the data flow.

## Measured consequence

Healthy driving: attachment offset constant to 0.1 mm over 15 s. During prediction-rollback
correction storms (cm-scale visual corrections held across ~8 render frames/tick): the hull
collision proxy ratcheted 2–13 cm/tick, measured **2.8 m above the hull** — after which every
hc=0 / contact-loss reading was avian being honest about a collider that had levitated away.
Self-sustaining: unsupported root falls → position rollback → corrections → more poison. Also
silently corrupts any child-collider-based damage geometry.

## Suggested upstream fix

Exclude non-`RigidBody` child colliders (`ColliderOf` without `RigidBody`) from the blanket
`ApplyPosToTransform` requirement, or provide a per-entity opt-out. Child colliders' local
transforms should never be derived from world Position.

## Our workaround + removal condition

`AuthoredLocalTransform` marker + a pair of order-independent `On<Add>` observers stripping
`ApplyPosToTransform` from marked entities (src/tank.rs, commit 33cc4e4; despawn-safe, both
ends). Remove when upstream excludes child colliders from the requirement.

## What fixing this unlocks for us

**Clean up.** The whole shield in `src/tank.rs`: the `AuthoredLocalTransform` component, both observers
(`shield_authored_collider_transform`, `shield_late_authored_marker` — two of them purely so the shield
is insertion-order-independent), the `authored_attachment()` bundle helper and its use at every
child-collider spawn site, the two observer registrations in `sim_plugin`, and ADR-0015's Layer-2 row 2.
It also retires a **standing authoring rule**: today every new child collider (armor volume, future
track station, any damage geometry) must remember to carry the marker or it silently re-arms the
poisoning write — a footgun with no compile-time guard. That rule disappears with the defect.

**Optimize.** Not the observers (they fire at spawn and cost nothing at steady state) — the recovered
cost is the **divergence the poisoning manufactured**, which is one of the two entries ADR-0015 names as
the reason the rollback bars are coarsened to 5× the reference (`ROLLBACK_POSITION_M` = 0.05,
`net/protocol.rs:1038`) rather than the 1 cm / 0.01 rad reference values. Those bars are a ratchet: this
fix plus [avian-solver-constraint-order.md](avian-solver-constraint-order.md) are the two conditions
ADR-0015 names for tightening them. Note the shield already removed the *live* cost here (post-shield
solo rollbacks are 2–4 per 20 s vs the pre-shield storm), so the recovery is bookkeeping we have
already banked — an upstream fix banks the *deletion*, not new headroom.

**Explore.** Nothing new becomes possible — the shield is complete (it excises the write rather than
undoing it, on both ends). This one is honestly just: delete the workaround, delete the rule, stop
having to explain it to every future collider.
