# Adopt Avian for physics

We adopt **Avian** (`avian3d`) as the physics engine rather than `bevy_rapier`. Avian is ECS-native — rigid bodies and colliders are components on the same entity, with no separate physics world to keep in sync — which fits our sim-in-`FixedUpdate`, plugin-per-feature architecture and keeps the hull's `Transform` single-sourced (the concern flagged when the hull becomes dynamic). We accept Avian's relative youth (fewer features and community resources than the more mature Rapier) as a worthwhile trade for the tighter integration.

## Considered Options

- **bevy_rapier** — more mature and battle-tested, but wraps the standalone Rapier engine and keeps its *own* physics world, projected back onto the ECS. That second representation of the hull is a second source of truth — exactly the seam we're avoiding. The two engines have similar APIs, so switching later if we hit a missing feature or a solver bug is not prohibitive.

## Consequences

Pinned to **`avian3d` 0.7** against **Bevy 0.19** (see [[bevy-019-scene-rework]] in agent memory for the engine bump). Physics runs in `FixedUpdate`, consistent with the existing sim-in-`FixedUpdate` bet.
