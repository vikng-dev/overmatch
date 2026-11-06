# Cheaper collider bookkeeping

Subject: the video game's environment destruction model; game state only.

The invariant is the collider representation for a given departure: source chunk address, convex
shape, collision layers, occupied mass and inertia, friction, sensor status, ballistic material
and owning body. Entity allocation order is free to change server physics observations.

## Reuse

Standing chunk colliders transfer to the departing body as contact hulls. Existing contact and
occupied sensor entities transfer again when a body splits; chunks with no prior collider still
create one. The shared construction installer mounts the existing stale-contact cleanup rule.
Owner-local collider lookup is prepared on workers; structural changes remain ordered.

Verified against installed Bevy 0.19.1 and Avian 0.7.0 sources:
- Bevy `ChildOf` relationship insertion/discard maintains `Children`; discard unlinks before a
  retiring parent's linked despawn can reach a transferred child.
- Avian `ColliderHierarchyPlugin::on_collider_body_changed` observes `ChildOf` insertion and
  updates `ColliderOf`; its relationship hook computes the new `ColliderTransform`.
- `ColliderTreeUpdatePlugin` observes collider and `ColliderOf` insertion, removes the old proxy
  and inserts the new body's proxy. `update_moved_collider_aabbs` reads changed shape/pose.
- Avian broad-phase pairs retain their prior body IDs. The game's `collider_rehome::plugin`
  discards stale pairs by disabling/enabling the moved collider, before the next physics step.
- `ColliderBackendPlugin` recomputes cached shape mass on insertion even when supplied manually.
  Supplying a precomputed `ColliderMassProperties` therefore does not avoid that work.

Reuse saves entity allocation/deletion and initial component setup. It still pays for shape
replacement, mass calculation, hierarchy changes, broad-phase proxy transfer and contact cleanup.

The class witness compares fresh and reused departures before and after physics ticks, then splits
the group and compares again. It includes cached mass properties and checks that reused entities
survive retirement of their old body. A second witness reaches reuse through the shipped law/refit
installer. Existing headless checks cover the full spawned-body contract.

## Measurements

MEASURED in-process command-application microbenchmark: one batch of 600 departing chunks,
1,200 resulting debris colliders, and 600 existing standing colliders. Each variant uses 17 fresh
app fixtures, with geometry preparation and command queuing outside the timed interval. Median
application only, under the machine lock; no frame/tick timer, ram timing harness, capture or UDP.

| Step | Collider creates | Collider deletes | Median, ms | Decision |
|---|---:|---:|---:|---|
| Before | 1,200 | 600 | 6.076167 | Reference |
| Reuse | 600 | 0 | 5.406584 | Keep; 11.0% lower median |

MEASURED headless tracked recipe, from first occupied contact. This is the historical in-process
witness, not session 7's separate UDP replay; it has a smaller collapse burst. Emergent outputs are
observations and no historical-speed assertion is retained.

| Step | +1 m/s | +2 m/s | +5 m/s | +10 m/s | Chunks by +2 | Mass by +2, kg |
|---|---:|---:|---:|---:|---:|---:|
| Before | 10.813708 | 10.441001 | 10.005212 | 9.994593 | 106 | 2,495.940 |
| Reuse | 10.813708 | 10.441001 | 10.005212 | 9.994593 | 106 | 2,495.940 |

MEASURED 400 N press: both remove zero chunks; detached exact cut 108,133.96 N in both.

MEASURED collider entity lifecycle counts through contact +320, including every collider category:
1,005 creates / 695 deletes before; 806 / 550 after reuse. Later solver-order changes move the
collapse counts; subtraction is not a claim that identical law events occurred in that window.
The per-tick rows below cover the largest baseline events; full per-tick counts and raw logs are
in the lane handoff folder as `sweepfix-s8-*-counts.json` and `sweepfix-s8-*.log`.

| Simulation tick | Before creates/deletes | Reuse creates/deletes |
|---|---:|---:|
| 1263 | 131 / 2 | 131 / 2 |
| 1264 | 82 / 10 | 80 / 8 |
| 1265 | 123 / 41 | 123 / 41 |
| 1266 | 90 / 76 | 90 / 76 |
| 1269 | 79 / 27 | 69 / 17 |
| 1275 | 34 / 17 | 17 / 0 |
| 1287 | 91 / 59 | 91 / 59 |
| 1301 | 105 / 48 | 85 / 32 |
| 1302 | 78 / 110 | 87 / 114 |
| 1316 | 50 / 63 | 0 / 6 |

## Authority and validation

Clients receive `DebrisPose` and `Piece` on separate replicated carriers. Server publication copies
`Position` and `Rotation` in `net::debris_stress::publish_poses`; client `follow_pose` writes the received
pose into `Transform`. `carry_construction_piece` selects the shared interpolation cursor. Those
paths use chunk IDs and replicated poses, not server collider entity order (ADR-0037).

Reuse: formatting and CI clippy pass; required targeted tests: 289 passed, 6 ignored, zero failed. No strengths, assets,
drive settings, debris geometry or network source changed.
