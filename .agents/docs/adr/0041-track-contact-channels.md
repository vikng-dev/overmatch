# 0041 — The belt meets the world through three channels and one law

Status: ACCEPTED 2026-11-08 by the owner. Subject: the video game's tank driving physics; game
state only.

## Decision

- **Ground and fixed structure take the sampled support**: spring, damper and per-element grip at
  48 stations by 3 lateral points, outside the solver, reading one height-grid facet plane per
  column for terrain and clipped convex faces for solids. The stations and points are a
  `Resolution` number.
- **Walls and steps take the outer-face stop**: solver colliders hung on the hull, their surfaces
  moving at belt speed, so friction against a wall can lift the hull wherever the belt wraps an end
  wheel.
- **Loose pieces take belt segments**: solver slabs along the belt's travel that meet only
  `Layer::Debris`, each carrying the calibrated law as a compliant contact (a local patch to the
  vendored Avian, drafted under `upstream/`). A piece takes its equal and opposite reaction through
  the solver. A tank wholly on pieces with no drive demand sleeps with its pile through the
  engine's island sleep; drive demand ends that sleep (`track::sim::wake_driven_tanks`). A tank with
  any spring contact is kept awake by its sampled force, and its pile with it.
- **The boundary is the layer a collider was built on.** Nothing asks what a body is or how heavy.
- **One support law**: `k'` and `c'` per metre of belt and the droop envelope are shared by the
  channels. A shoe is a rigid plate, so a segment takes `k' × length` whatever the width a piece
  covers, shared among the pieces under it.
- **One drivetrain reader** returns solver friction, the stop's and the segments', to the
  transmission.

The narrative, known differences and measurements are in
`.agents/docs/design/track-model/track-contact.md`.

## Why

The belt's contact is cheap where the world is simple and honest where the world moves. The
physics step with the belt's support in the sampled channel measured 1.0 ms against 9.0 with the
belt fully in the solver (30 tanks); solver contacts give a loose piece the reaction the sampled channel would have to imitate with a law of
its own, and that imitation measured both worse and larger. Rejected: the belt fully in the
solver, a custom piece reaction law, exact-triangle terrain clipping, and a segment law scaled by
covered width. Each is costed in the design doc.

## Consequences

- [[0025-belt-force-locomotion]] stands as the locomotion model. This ADR replaces its contact
  geometry: stations are a fixed count round the loop rather than pitch-spaced; terrain is a facet
  plane per column rather than the analytic field; support acts along the surface normal; a stop
  collider caps the travel; and pieces leave the spring channel.
- The partial-coverage difference between the sampled support and a segment is a named property,
  not a defect to tune.
- Sim state stays spawn-built ([[0014-sim-view-split]]): stops and segments are cut from
  construction data with the hull. Everything here is server-simulated ([[0037-one-authoritative-timeline-and-view-overlays]]).
