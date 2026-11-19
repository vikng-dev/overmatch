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

The local extension exposes scalar surface coordinates in `SolverBody` and their coefficients
on `ContactConstraint`. An optional friction hook runs under the existing graph coloring, after
the normal row. It also receives warm start after the rigid-body warm impulse. The hook owns
its actual applied accumulator; its readback must not subtract an unapplied warm-start share a
second time. Ordinary contacts keep the upstream path. Hook function pointers are transient;
an adapter reattaches them after serialization.

The game-side seam at `src/avian_ext/surface_motion.rs` solves the two tangent coordinates on a
Coulomb disk using the sum of rigid-body and surface inverse inertias. The same impulse changes
all participating coordinates before the next contact. Zero-mobility directions are explicit
in the positive-semidefinite metric. The only numerical policy is resolution of the small
metric and disk minimization; there is no speed bound or material parameter in the hook.

The native-constraint witnesses vary the body/surface mass ratio, warm-start scale and friction
budget, derive the expected impulse from those inputs, and check energy dissipation. Another
witness covers two finite surfaces. The production-adapter regression proves that already
delivered contact history cannot be integrated as a new belt load.

An upstream API could choose another representation for extra surface coordinates. This draft
records the needed ownership, warm-start and readback contracts, rather than proposing a game
law inside Avian.
