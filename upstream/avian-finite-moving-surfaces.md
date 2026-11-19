# Optional finite-inertia moving surfaces

Local proposal, unsent. Based on avian3d 0.8.0. Subject: the game's tank driving physics,
game state only.

Avian's `tangent_velocity` deliberately prescribes a surface speed. It is suitable for an
infinite-inertia conveyor. Overmatch incorrectly used it for finite belts and applied the
returned friction to their inertia one tick later. In a wedge, opposing normal reactions could
grow while the prescribed belt kept supplying work; the delayed reaction then reversed the
finite belt. This is an adapter defect, not a claim that Avian violates its conveyor contract.

Classification: **game divergence, through a narrow fork hook**. The finite-surface law has
one game-side owner, `src/avian_ext/surface_motion.rs`; this is not an upstream conveyor bug fix.

The existing 0.8.0 mechanisms cannot supply that coordinate:
- `CollisionHooks::modify_contacts` (`src/collision/hooks.rs`) runs before constraint generation,
  has read-only ECS access, and sees the preceding tick's impulses. A modified
  `ContactManifold::tangent_velocity` remains prescribed throughout contact relaxation.
- `RigidBody::Kinematic` (`src/dynamics/rigid_body/mod.rs`) ignores contact forces, so moving a
  kinematic belt body retains the infinite-momentum contract.
- A dynamic body's `SolverBody::velocity_at_point` is translation plus rigid rotation. A belt's
  tangents follow a closed, noncircular outline on a hull-fixed collider, while its material
  travel has independent inertia. Neither a translating nor a rotating rigid body expresses
  that motion; separate physical links would replace the admitted belt model and geometry.

The local extension exposes adapter-typed, cloneable transient scratch in `SolverBody` and
surface-coordinate coefficients on `ContactConstraint`. One process-wide friction callback is
registered before any coordinates are bound. It runs under graph coloring after both normal
passes, with pass identity, and receives warm start after the rigid-body warm impulse. The seam
chooses when to solve. Its accumulator records the applied impulse; game-side readers share
one accessor for that convention. Ordinary contacts retain stock friction. Scratch is rebuilt
before solving, including after deserialization.

The game owns the surface trait, capped contact row and drivetrain implementation; their
mechanical law and measurements live in `.agents/docs/design/track-model/track-contact.md`.

Native-constraint witnesses cover mass ratios, scaled warm starts, energy, and both bodies'
linear and angular momentum at nonzero anchors. Production scheduling witnesses compare final
solver speeds to the next drivetrain state directly; an awake parked-slab witness varies slope
and substep length. Contact history is reporting only. The fork adds ownership and dispatch,
with no belt law, drivetrain policy or work tally.

An upstream API could choose another representation for extra surface coordinates. This draft
records the needed ownership, warm-start and readback contracts, rather than proposing a game
law inside Avian.
