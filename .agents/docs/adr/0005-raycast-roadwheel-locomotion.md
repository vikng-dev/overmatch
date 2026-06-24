# Tracked locomotion via raycast roadwheels; the track is cosmetic

We model driving as a **raycast vehicle**: the hull is a single dynamic rigid body, and each roadwheel is a downward **ray with a spring-damper**, not a collider. Each ray's contact station does double duty — its spring force holds the hull up (**support**), and its normal load feeds a capped-Coulomb friction sample (**drive**: differential thrust plus skid-steer lateral resistance). The track belt is purely **cosmetic** and carries no physics.

This is the near-universal pattern for game tanks. It buys emergence: ride height, pitch under acceleration, roll on slopes, weight transfer, and how normal load splits between the tracks all fall out of the per-wheel springs — nothing scripted. The friction sample points and the suspension wheels are the *same* set of contact stations, so support and drive share one declaration without entangling (support computes per-station load; drive reads it).

## Considered Options

- **Track-as-physics** (simulate track links / a Verlet chain as bodies coupled to the drivetrain) — what a few heavy mil-sims do; expensive, unstable, and rejected even by most simulators. Out of scope.
- **Single-body force + yaw torque** (arcade) — cheaper, but steering and turning resistance must be faked, and it can't produce honest weight transfer or load-dependent grip.

## Consequences

- **The model contributes geometry only** — per-roadwheel hub origins and the effective radius (wheel radius + track thickness). All behaviour is data + rules.
- **Ride height equals the effective radius** — ~0.516625 m, the hub height in the model's loaded-on-flat-ground pose — so the track rests exactly on the ground, no baked-in offset. Track *sinking* into soft ground is deferred to a later, explicit **ground-type** mechanic (terrain-dependent), not a constant fudge.
- **Only roadwheels are ray sources** — the sprocket and idler carry no ground load and are visual-only.
- **Blender's Geometry-Nodes procedural track is offline-only** — it does not survive glTF export (procedural geometry and drivers are lost). Blender exports the static rig with named nodes; track *motion* (scroll, sag) is an engine concern.
