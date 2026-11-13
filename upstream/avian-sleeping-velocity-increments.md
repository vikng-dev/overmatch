# Local draft: a sleeping body's velocity increments leak into its first step awake

Subject: the game's rigid-body rest, server physics; game state only. No external issue or message
sent.

Avian 0.7's `pre_process_velocity_increments` and the world-frame constant force systems
(`apply_constant_forces`, `apply_constant_linear_acceleration`,
`apply_constant_angular_acceleration`) write `VelocityIntegrationData` for every body that has it,
while `clear_velocity_increments` clears only bodies with a `SolverBody`. A sleeping body loses its
solver body and keeps the data, so every sleeping step adds gravity and scales the sum by the
substep length `h`, settling at `g·h/(1 − h)`. On waking, each of the step's `N` substeps applies
that stale increment: an extra `N·g·h²/(1 − h)` of velocity along gravity, and a sleeping body's
constant force or waking force is scaled away instead of applied once.

The local patch builds the increments over the bodies it clears (`With<SolverBody>`). A free body
held asleep then gains exactly one step's gravity in its first step awake.

Retire this patch when the pinned engine builds and clears velocity increments over the same body
set. See the vendored `OVERMATCH_PATCH.md` inventory.
