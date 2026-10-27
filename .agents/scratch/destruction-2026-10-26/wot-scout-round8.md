# WoT scout, round 8: the native layer (received 2026-10-28)

Source: the `master` session in `../bounty`, by message, from a static decompile of its WoT client
build v.2.4.1.0 (full writeup there: `re-study/wot/README.md`, "Native layer (Round 8)"). Design
input only; WoT is arcade, structure not numbers. Subject: the game's destruction and netcode
mechanisms; nothing here is code to transplant.

## Destruction is Havok's toolkit, not a custom system
- **Authoring (baked offline):** fracture patterns (Voronoi, slice, wood, cut-out, pie, exact) run as a
  parallel task graph; fractured pieces get inner-face textures applied automatically.
- **Runtime:** a destructible is a breakable compound shape held by breakable constraints; pieces
  detach as a signalled hierarchy (added / removed / merged); a dynamic fracture context drives
  runtime fracture.
- **WG layer:** debris is an ECS (a debris component and a spawner component, with
  activate / deactivate / change reactions); gameplay calls are `wg_*` (destroy a fragile, set a
  destructible active, query its effect category / name / scale / tint, is-it-a-bush, play a
  module's destruction animation).

## A toppled destructible's rest lean is a solve, not a simulation
`wg_solveDestructibleFallPitch`: a fixed-point iteration of `X = (A/k)·sin(X) + C` — `A` a
gravity-torque amplitude, `k` a torsional restoring stiffness, `C` a base angle. At most 15
iterations; early out on convergence (step under epsilon) or divergence (step grows); refuses a
near-zero stiffness; pre-checks a contraction condition `A² ≥ k²·((C−X₀)² + ε)`. Compare F8 and
`trees::resting_pose`: WoT's rest pose is also scripted and deterministic, a lean against a spring,
not a contact search against the terrain. What `A`, `k` and `C` are bound to is not yet read.

## Netcode (confirms round 3)
Remote entities interpolate through a collection of per-quantity sub-filters with an
interpolation-type enum and latency/timestamp bookkeeping; vehicles have their own filter
(physics, track speed, a stabilised matrix and yaw/pitch/roll sync, hull aiming pitch), gun angles a
sub-filter, wheeled vehicles steering and scroll. Remote hulls are interpolated and stabilised,
never re-simulated (our one timeline, ADR-0037, holds the same shape).

## Hull marks and queries
Damage decals on hulls are a sticker model fed by ray projection (`WGStickerModel`,
`setTextureData`); segment queries are `wg_collideSegment`; collision filters include
ignore-destructibles and destructible-only predicates.
