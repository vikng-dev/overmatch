# Rubble bed drivable prototype: build plan

Subject: the game's destruction model (pieces, joints, loads, fill). Branch `lane/rubble-bed-drive`. Nothing was built or run for this plan; every source claim below was read at `main` 22bd33e8.

## Decisions

1. **The bed crosses the wire, as per-tile diffs.** This is also the production direction.
2. **The weld is deleted outright**, not bypassed. Sub-floor matter becomes fill through the existing `Shed` message.
3. **Bearing gets a geometric patch, a real underside, eccentric and slope terms, and a new give law.** Traction-inclined loading and compaction are deferred.
4. **The give law is new and unmeasured.** It replaces the fair test's fixed 2 mm/tick and reduces to it at rest. It is the plan's largest unknown.
5. **View:** a plain-material mound studded with the real shed chunks, half-buried and persistent.

## 1. How the owner plays it

- **The local server is a separate process.** `run_client` only connects to `127.0.0.1:5888` (`src/net/client.rs:48`, `choose_mode` at `:80`). No non-test code spawns a child, and `run_server` is called only from `src/bin/overmatch-server.rs:2`.
- **So the owner runs two commands:** `cargo run --release --bin overmatch-server`, then `cargo run --release --bin overmatch`.
- **Earlier drive, no wire:** `overmatch --offline` mounts sim and view in one app (`GamePlugin`, `src/lib.rs:932`). Milestone 1 uses it.
- **Why replicate, not derive:** bearing reads the server's solver loads, which no client has. Late joiners get no sheds (`src/net/construction.rs:14-16`), so deposits cannot be derived either.
- **Wire shape:** one entity per 8 m tile carrying `BedTile`, registered `replicate_diff()` beside the ledger (`src/net/protocol.rs:905`). The diff is `{ tick, changes: Vec<(u16 node, u16 depth_mm) }`. replicon's `Diffable` falls back to a whole snapshot past its history, which covers late join (`bevy_replicon-0.45.5/src/shared/replication/diff.rs:101-116`).
- **Lifetime:** tiles carry `WorldGeneration`, so `world_reset::start_next` retires them (`src/net/world_reset.rs:86-98`). Add a bed clear to `world::reconstruct` (`src/world.rs:43`).
- **Bump** `PROTOCOL_REV` 107 to 108 and the `WIRE_SURFACE` list (`src/net/protocol.rs:41`, `:820`).
- **One prototype shortcut:** the client applies a diff on arrival. Production holds it to the shared cursor by its tick (ADR-0037). The tick is on the wire now, so only the hold is missing.

## 2. Structure

New module `src/rubble_bed.rs` with `flow`, `bearing`, `deposit`, `view`, plus `src/net/rubble_bed.rs`.

- **Truth:** `BedTile { key: IVec2, depths: [u16; 32*32] }`. Nodes sit on a world lattice at 0.25 m (`Resolution`, three nodes across a track). Depth is in mm (`Resolution`). One unit is 6.25e-5 m³ (DERIVED, 0.0625 m² hat × 1 mm).
- **One publish system, same code on server, client and offline:** on `Changed<BedTile>` it rebuilds three things from the same samples.
  - `BedSurface` resource: per tile, a tile-local `HeightGrid` of 33×33 absolute tops (base `height_at` plus depth, border nodes read from neighbours) and a filled-quad mask.
  - The tile's static `Collider`, on `Layer::Terrain` with `FieldSurface`.
  - With a window, the mesh.
- **Reuse, do not rewrite:**
  - Add `HeightGrid::cast_ray_where(.., admits: impl Fn(i, j) -> bool)`. `cast_ray` (`src/terrain_grid.rs:373`) delegates to it with no arithmetic changed.
  - Add a `heightfield_collider` variant (`:738`) that marks all-zero quads removed. Verified in parry3d 0.31.1 `heightfield3.rs`: `set_cell_status` `:517`, `CELL_REMOVED` `:31`, and per-half `LEFT/RIGHT_TRIANGLE_REMOVED` for the fringe.
- **Surface rule for every reader:** the union of the base grid and the filled bed quads, which is exactly what the two colliders present.
- **Tick order (server, `FixedPostUpdate`, after `SplitSet`, before `SimPhase::Publish`):** deposits due, bearing, repose, one publish. Changed tiles wake sleeping tanks whose bounds overlap. Pieces wake through the existing `wake_on_static_moved` on `Changed<Collider>` (`src/debris.rs:266-291`).
- **No body pose, velocity or force is written.**

## 3. What replaces what

**Deleted:**
- `merge_fragments` (`src/debris/crumble.rs:239-314`).
- Its three calls: `crumble.rs:699`, `src/construction/breakage.rs:1693`, `src/construction/breakage/sweep_intake.rs:1416`. Keep the ascending sort each site relied on.
- `Live::rubble_joints` (`breakage.rs:553`, `:576`, `:595-604`).
- Its tests (`src/debris/crumble/tests.rs:612-662`) and the merge sentence at `sweep_intake.rs:1336`.

**Unchanged:** `server_body`, the 0.5 m floor, `route`, the `Dust` entries and `Shed`.

**Sub-floor matter:** every `Shed` is by construction a fragment leaving simulation (`crumble.rs:748-778`). A server system reads the same `Shed` messages `publish_sheds` reads and deposits the chunks' volume divided by the packing fraction. `route` is not touched.

**Where it deposits:** milestone 1 deposits in place, as the fair test did. Milestone 5 deposits where the decoration lands, by extracting `decor::landing` (`src/debris/decor.rs:105`) into a function the server shares. A shed that never lands inside the map deposits nothing; log it.

**Pieces at or above the floor:** still server bodies. They rest on the tile collider, crumble on a landing blow as today, and their sub-floor fragments become fill. The fair test measured 7 bodies after collapse and 0 after the first pass (MEASURED), so expect the tank to convert the whole pile to fill on one pass.

## 4. Bearing law

Constants of the form are recalled, not read: Vesic `Nγ`, the Meyerhof effective width, the `1 − 0.4 B/L` shape factor and the `(1 − tan β)²` ground factor. Label each `Unknown` until a source is read. With 38°, 0.60 and Brick 1,845 kg/m³ the strip capacity is 423.6 kPa per metre of width (DERIVED; matches the column probe's 423.7).

| Term | Prototype | Reason |
|---|---|---|
| Geometric patch | In | The sparse solver points understated area, the report's first named error. A solid body's patch is the nodes whose upward `SpatialQuery` ray meets it within `NarrowPhaseConfig::contact_tolerance` of the bed top. A belt's patch is the nodes under its loaded samples. Patches are 8-connected sets; B and L come from second moments. |
| Real underside | In | The same upward ray caps heave, for hulls, pieces and standing walls alike. |
| Eccentric load | In | `B' = B − 2e`, from the resultant against the patch centroid. A belly resting on one edge of a crest is the beaching case. |
| Slope and crest | In | Factor `(1 − tan β)²`, β the steepest fall across the patch's free ring. This is the term the mound stall pointed at (0/3 crossings, MEASURED). |
| Give | In, new | An overloaded patch resists with its capacity only. It lowers by `(V − Q)·dt² / m_eff` per tick, `m_eff` from the body's mass and inertia at the resultant (`ContactTransfers::inertia`, `src/contact_transfer.rs:296`; Avian mass components for belt-only tanks). The fraction carries per body. |
| Commit | In | One publish per tick, after bearing and repose. The four-tick cadence did not remove the parked penalty (MEASURED). |
| Traction-inclined load | Deferred | Its effect, digging while slipping, cannot be told from the slope term by feel in one build. |
| Compaction, mixed materials | Deferred | Needs per-cell state. Bulk weight is the running mean of deposited mass over volume, a `StandIn`. |

- **Give at rest:** with no capacity it equals g·dt² = 2.4 mm (DERIVED), the fair test's rate.
- **Heave:** withdrawn units go to the nearest free ring under the cap, exact in integers.
- **Repose:** relaxes real facets downhill only, with a per-tick sweep budget (`Resolution`); the remainder carries to the next tick.

**Authored numbers, all in data:**
- `rubble_bed.friction_angle_deg` 38 and `packing_fraction` 0.60 in `assets/ballistics/physical_laws.ron`, both `Unknown` (a geotechnical source for crushed masonry settles them). The owner edits these two to feel loose (34°, 0.55) or stiff (44°, 0.64).
- Grip on the bed reads the `gravel` row through one accessor, labelled `StandIn` for a `rubble` row.

## 5. Readers

All report references verified against source, within a few lines.

| Reader | Where | Prototype |
|---|---|---|
| Belt oracle | `src/track/oracle.rs:515` | In. A `Topped { field, bed }` oracle unions the bed cast; wrap at `src/track/sim.rs:1578`, `src/track/stop_contacts.rs:175`, `src/track/view.rs:719`. `profile_may_hit` returns true where a tile exists. |
| Force-column plane | `src/track/world_contact.rs:26-36`, `world_contact/terrain.rs:14` | In. The column takes the bed facet where its quad is filled. |
| Grip, hard belt contacts | `world_contact.rs:345`, `stop_contacts.rs:229` | In, through `Topped::ground_at`. |
| Hull and piece contact | `src/world.rs:303-310` pattern | In, the tile collider. |
| Camera and aim casts | `src/world.rs:618` | In for free: the client has the same collider. |
| Decoration rest | `src/debris/decor.rs:232-236` | In, with reseating (section 6). |
| Round march | `src/ballistics.rs:900-903`, `:2135` | In, about 15 lines once the bed cast exists. |
| Spawn placement | `src/terrain_grid.rs:201`, `src/net/server.rs:1345` | In. A tank spawned inside a mound would start under a hollow shell. |
| Track prints, ground dust | `src/track_prints.rs:520`, `src/vfx/muzzle.rs:693` | Wait. No ruts or dust colour on the bed. |
| Scenario and script tools | `src/net/debris_stress.rs:432`, `src/tank/battle.rs:601`, `:652` | Wait. |
| Frozen-island support | `src/debris/freeze.rs` | Wait; covered only as far as `Changed<Collider>` reaches. |

## 6. View

- **Mesh:** one per tile, filled quads only, smooth normals, world-space UVs. A plain `StandardMaterial` with one colour, a `Taste` in `assets/vfx/presentation.ron` ("fines between chunks"). The construction material needs tangents and lane UVs (`src/construction_surface.rs:172-179`), so it is not used.
- **Chunks:** today's shed decorations are the real chunk meshes, but they fade after 6 to 12 s and cap at 256 (`presentation.ron:9`, `decor.rs:24`). On a filled quad they no longer expire, they follow the bed top each frame, and they are drawn part-buried by one `Taste` share.
- **Cap:** raise `MOST` to 1024 (`Resolution`, draw calls). One house sheds 348 to 370 groups (MEASURED, crush probe), so that holds about 2.8 houses (DERIVED).
- **What it will look like:** a crushed-masonry mound, smooth at quarter-metre scale, studded with half-buried brick chunks that sink and rise with it. Chunks have no collider, so a hull can clip through one. Pieces above the floor sit on top, not seated in.

## 7. Size and order

Line counts are DERIVED from the size of the analogous code read here and from the fair test's estimate for the matching slices. The fair test priced production at 2,650 to 4,050 lines before view.

| Milestone | Part | Lines |
|---|---|---:|
| 1 | Tiles, surface, collider, caster hook | 340 |
| 1 | `Topped`, column plane, three sites, grip, tank wake | 165 |
| 1 | In-place deposit from `Shed`, data rows | 150 |
| 1 | Repose | 220 |
| 1 | Mesh and material | 130 |
| 2 | Weld deletion | 15 changed, about 155 deleted |
| 3 | Wire, tile lifetime, bytes-per-second log | 160 |
| 4 | Bearing | 410 |
| 5 | Landing deposit, chunk seating | 150 |
| 6 | Rounds, spawn | 35 |
| all | Targeted tests | 250 |
| end | Scratch report | 80 |

Total about 2,100 added or changed (range 1,900 to 2,600) and about 155 deleted.

- **Milestone 1** is drivable with `--offline`: what is shed today stops vanishing.
- **Milestone 2** ends beaching on welded lumps.
- **Milestone 3** makes the owner's two-process run work.
- **Milestones 4 and 5** are what the owner judges.

**Tests** (each names its invariant): integer volume conserved through deposit, heave and repose; no facet past the repose bound; bed cast, `height_at` and the collider agree at seeded points; a diff round-trips and a snapshot equals the diff sequence.

**Report:** run `scripts/serial-cargo` only, with rustfmt per file. The lane's report goes to `.agents/scratch/rubble-bed-drive-prototype.md`.

## 8. What the owner should look for

1. Drive at a collapsed house: does the hull hang on a lump with tracks slipping, or sink in and pull through?
2. After one pass and after three: is there still a mound worth steering around, or a flat smear?
3. Hit a crest at speed: does the tank press it down, or pitch over it like a ramp?
4. Stop on the mound: does it settle a few centimetres and go still, or keep creeping?
5. Look at it from the commander's view: rubble, or a sand heap with bricks stuck in?

**It cannot tell them:**
- Hosted cost or bandwidth.
- What a second player sees at real latency, or late join under load.
- Whether 38° and 0.60 are right.
- Ruts or dust on the bed.
- Whether freeze can be deleted.
- Behaviour with many tanks.

## 9. Risks of being misled

- **Loopback hides the wire.** Per-tick diffs are free locally. The server logs bed diff bytes per second so the report carries a number; the design's 13 to 26 KB/s was a guess.
- **Arrival-time presentation.** Locally the bed and hull agree; on a hosted link the bed leads the hull by the view delay. The same gap applies to the presented round march against the authority's.
- **The give law is unmeasured.** If crests vanish at a touch or the hull digs trenches, the fault is this law or the recalled factors, not the two data numbers.
- **Release on an M4 is not two hosted cores.** The upward rays are new work, about 340 per hull footprint per tick (DERIVED, 3.5 × 6 m assumed, not read).
- **Spatial query freshness.** I did not check when Avian refreshes its query tree relative to writeback. The builder must, or the underside is a tick stale.
- **The look can flatter.** Persistent chunks are view only; the collision surface is the smooth mound beneath them.
- **Fill against standing walls** is capped by the same ray, but how it reads is not established.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/src/debris/crumble.rs
- /Users/Yan/github/vikng-dev/overmatch/src/terrain_grid.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/oracle.rs
- /Users/Yan/github/vikng-dev/overmatch/src/net/protocol.rs
- /Users/Yan/github/vikng-dev/overmatch/src/debris/decor.rs
