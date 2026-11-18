# Where crushed rubble goes: the rubble bed (game state only; nothing built or run)

## Short answer

**Decision: matter too small to be a server body becomes a rubble bed.** The bed is a layer of loose fill on the ground that holds volume, slumps to a natural slope and gives way under load by spreading sideways. It is never a rigid body and never dynamic again.

**The missing truth:** Run 2 judged the load correctly and then sent the matter to decoration, which is the same as deleting it. Loose rubble is a granular material: it keeps its volume, cannot stand steeper than its slope of repose, and carries more the wider the patch that presses on it.

**What the tank does:** a proud crest under a track or the belly is narrow, so it carries little and flattens. The squeezed-out fill heaps beside it, the patch widens, and the tank comes to rest on a lower, wider bed that then holds.

**Second decision: delete the size-floor weld.** Run 2 shows welded heaps cannot hold as friction rests: 48 whole carriers left (MEASURED) of 50 awake at the intervention (MEASURED), 96% (DERIVED). A heap that is rigid until judged and gone once judged is a fiction with a pop in it. Sub-floor matter should enter the bed at the event that frees it (collapse, landing, hull cut), when things are already flying.

**What this costs:** a runtime height layer with its own collider, track reading, drawing and replication. It is not small. What it buys is a pile that is mostly static fill, so the awake-pile cost behind #379 largely goes.

**Not established:** every outcome. The probe below can refute it in one lane.

## The truth and the law

One state per ground cell: fill depth. Three laws act on it.

1. **Conservation.** A fragment `server_body` rejects adds its occupied volume, divided by the packing fraction, to the cells under its chunks where it lands. Nothing else adds fill and nothing removes it.
2. **Repose.** No bed surface is steeper than tan φ; the excess moves downhill.
3. **Bearing.** A patch (one body's or belt's contact region on the bed) carries at most `q = ½ γ B Nγ(φ)` over its area. Here γ is bulk weight (material density × packing × g) and B is the patch's narrow width. The constant of the form is Vesic's `Nγ = 2(Nq+1) tan φ`, `Nq = e^(π tan φ) tan²(45° + φ/2)`, recalled and not read.
   - Past `q`, every loaded cell lowers together and the displaced volume rises in the ring of free cells around the patch.
   - Fill never enters a cell above the underside of a body over it; the remainder moves outward.

States go one way: bonded body, then bed. The bed has no fixed/loose switch.

## Representation and data

A new module, `/Users/Yan/github/vikng-dev/overmatch/src/rubble_bed.rs`, holding sparse tiles of integer fill depth. Integer cells make conservation exact and need no float determinism.

| Number | Value | Job |
|---|---|---|
| Cell size | 0.25 m | `Resolution`: resolves a track (three cells) and belly-scale crests. Cost is Unknown; hosted narrow-phase time on a bed settles it. |
| Depth quantum | 1 mm (u16) | `Resolution`: below one tick's sink. |
| Tile | 32 × 32 cells, 8 m, 2 KiB (DERIVED) | `Resolution`: storage and replication unit. |
| Repose sweeps per tick | Unknown | `Resolution`: the remainder carries to the next tick. The probe's flow time settles it. |
| Friction angle φ | Unknown, 35–45° recalled | Fact, per rubble material. A geotechnical source for crushed brick fill settles it. |
| Packing fraction | Unknown, 0.55–0.64 recalled | Fact. Same source. |
| Sink per tick | at most g·dt² = 2.4 mm (DERIVED) | `StandIn` for the fill's rate law: one step of unresisted fall from rest. |
| `server_rubble_min_size_m` | 0.5 m, unchanged | `Taste`, same job. It no longer needs to clear the belly, because sub-floor matter is no longer a rigid lump. |

φ and packing start as one row in `assets/ballistics/physical_laws.ron`, labelled `StandIn` for per-material granular rows.

**What the law gives (DERIVED, assuming φ = 38° and 1,900 kg/m³ brick; I did not read the row's density):**
- Capacity is 437 kPa per metre of patch width.
- A track run 0.72 m wide carries 315 kPa against the nominal 107 kPa (DERIVED, track architecture doc), so a flat bed holds the tank.
- A 0.25 m crest carries 6.8 kN, so anything on the tank flattens it.
- A belly carrying 0.72 W (403 kN) needs about 0.95 m² of bed.

**The one-surface invariant** (`/Users/Yan/github/vikng-dev/overmatch/src/terrain_grid.rs:12`) is kept, not broken. `HeightGrid` stays immutable and identical on every peer. Each tile is a second slab with the same rule: one set of samples feeds every reader.

Readers that must take the top surface:
- `BlockField::contact_with_grid` (`src/track/oracle.rs`), as a third term beside the blocks and the grid.
- `Plane::sample` (`src/track/world_contact/terrain.rs`).
- `ground_at`, through a new `"rubble"` row in `assets/terrain/ground_types.ron` for grip and rolling resistance.
- A static heightfield collider per tile, marked `FieldSurface` so belts do not count it twice. Cells with no fill are removed.
- `decor::landing`, spawn height, and the client mesh.

**Softness:** the tank rides the bed as ground, but its give is the bearing law. `softness_mm_per_kpa` is print depth only, and is zero on the bed.

## The options

| Option | Verdict | Why |
|---|---|---|
| a. Compacted bed | Chosen, as a height layer | A single collider or box per heap cannot merge, overlap or yield, and needs an arbitrary height. |
| b. Lower the floor | Rejected | Bodies scale as V / size³. The stuck case's 61 bodies (MEASURED) become up to 488 (DERIVED); at 0.12 ms per awake body (DERIVED, #379) that is 59 ms a tick (DERIVED) against 15.6 ms. It also only moves the size at which matter vanishes. |
| c. Re-weld the remainder | Rejected | A weld fixes its chunks' relative pose, so a heap cannot flatten. Chunks that move first need (b)'s bodies, and the merged lump is again rigid at belly height. Merge and open is a two-way state. |
| d. Ground yield | Deferred, same representation | The design derived 0.09 m sinkage on firm dirt and none on paving, so it cannot carry the fix. It needs fill below zero, which means holes in the terrain collider. A block pressed into the bed already sinks by the bearing law. |

## What a player sees

- **Collapse:** large bonded pieces tumble as bodies. Smaller matter flies as today's decoration, and a mound rises where it lands and slumps.
- **Pass 1:** the tracks climb the mound. Crests flatten under them with fill heaving to the sides. A belly that touches sinks at up to 0.153 m/s (DERIVED, g·dt), so a 0.29 m sink takes at least 1.9 s (DERIVED). Run 2's fall peaked at 1.19 m/s (MEASURED).
- **Passes 2 and 3 in the same lane:** nothing more gives; the lane is firm. A new lane flattens new crests.
- **Parked:** the bed is static, no body is awake, and the tank sleeps.
- **Look:** a smooth mound may read as a dust heap. Drawn chunks seated on the bed are a later, view-only slice.

## Tick order and momentum

1. The track pass reads the surface from the last commit. Belt contacts with `collider == None` (`src/track/rubble.rs`, `record_static_contacts`) are the belt's load on the field.
2. The physics step solves hull and piece contacts on the tile collider.
3. After `LandingSet` and `SplitSet`, deposits due this tick commit in entity order.
4. Bearing runs per tile from last step's `ContactTransfers`, in parallel by tile; heave across a tile border commits in tile order.
5. Repose runs its bounded sweeps.
6. Changed tiles publish their collider, track term and diff.

No pose, velocity or force of any body is written. The loaded cells drop less than one tick of free fall, so the hull is always within a tick of contact. Its momentum goes into the ground, as on terrain. Matter in flight delivers none, which the first design recorded as an existing accepted gap.

## Server cost and body counts

| | Today | With the bed |
|---|---|---|
| Awake bodies, stuck case | 49–53 (MEASURED) | 0–3 (MEASURED in run 2's arm, where the pile was removed; DERIVED to carry over, because the bed is static) |
| Bodies per house pile | 15 at rest, 32–39 after a pass (MEASURED) | Bonded fragments of 0.5 m or more only. Unknown; the probe counts them. |
| Press solves on heaps | Run 2's arm raised joint-solver work on piles | None: no heaps exist. |
| Hull against ground | About 30 terrain triangles under a hull | Up to about 670 bed triangles (DERIVED, assuming a 3.5 × 6 m footprint I did not read). Unknown time; this is the main new cost. |
| Bed law | none | At most 1,024 integer cells per active tile per tick; zero at rest. |

## Wire verdict

- `Dust` and `Shed` are unchanged; `Shed` stays presentation.
- The bed is state every client needs, including late joiners, who get no sheds. It goes as tiles replicated by diff: changed cells, with a snapshot for a late joiner. This is a second user of the ledger's `replicate_diff`.
- Clients derive nothing, so no determinism burden is added.
- Estimated 13–26 KB/s per client while a hull is sinking, zero at rest (DERIVED guess from 50–100 changed cells a tick at 4 bytes; unmeasured).
- Tiles spawn whole at runtime and nothing rolls back, so ADR-0014 holds.

## What it deletes

- `merge_fragments` (`src/debris/crumble.rs:245-314`) and `rubble_joints` (`src/construction/breakage.rs:553`, plus its uses at `:576`, `:595`, `:1697` and `sweep_intake.rs:1420`).
- The first design's slice 2 (welds as rests and their ledger lifecycle), which is never built.
- The rule that unattached small matter becomes decoration no hull or round meets.
- #374's Unknown.
- The cause of #379, if the body counts hold.
- **Conditionally,** `/Users/Yan/github/vikng-dev/overmatch/src/debris/freeze.rs` (76,085 bytes MEASURED) and the ADR-0014 exception. Freeze exists to stop a sleeping pile's near pairs costing narrow phase. If piles hold few bodies its saving shrinks with them. This needs the hosted re-measure; I did not establish it.
- **Deferred, not deleted:** the press pass on bonded groups. It is built only if bonded blocks still beach the tank on bare ground.

## The settle experiment

**Why that one flipped:** it held a two-valued state per piece (static or dynamic), re-decided every tick by a feasibility verdict at its threshold. The switch itself dropped the piece's contacts and redistributed the load, which changed the next verdict. That produced 199–359 switches a second (MEASURED) and a changed first-touch response from the contact rebuild.

**Why the bed differs:** it has no verdict to reverse. Its change is dissipative: yielding lowers the loaded cells and widens the patch, which reduces the pressure that caused it.

**Where it can still fail:**
- **Creep.** Solver force noise near the limit ratchets a parked hull downward. Run 1 measured swings of 2–49 kN a tick on a patch of about 80 kN. Belts are safer, because their load is a smooth spring law.
- **Contact rebuild.** Replacing a tile's collider each tick during a sink resets the hull's contacts there, which is the mechanism the settle probe blamed.
- **A round or blast on the bed** does what it does to terrain: nothing moves. There is no piece to free, so no thaw path, and bed matter never flies again.
- **A second collapse on a bed** adds fill and bodies land on it as on ground. Fill arriving under a hull is capped at its underside.

## Cheapest refuting experiment (slice 0, report only)

A disposable arm in `src/headless_test/track_scorecard/rubble_climb.rs` on the existing `Bed` and the run 2 starts.

**Setup:**
- `merge_fragments` returns its input unmerged.
- `route`'s light fragments deposit in place (no flight).
- The bed is one tile of static cuboid columns on `Layer::Terrain`, without `FieldSurface`. Belts read them through the existing "every other solid" path and hulls through Avian, so no track term, wire or drawing is needed.
- Integer repose and the bearing law run on solver transfers and belt contacts.

**Binary gate:** on the stuck start, does the tank drive on by run 2's gate, with zero whole-carrier pops and bed-plus-body volume at 100%?

**Absurd ends, measured after passes 1–3 on the free start:**
- Hangs: belly share above 50% in the final second.
- Gone: the hull stays within 1 cm of its rest height across the old footprint, or the bed's maximum height is under 0.05 m.
- Blocked: the tank does not cross within the control's time.

**Also recorded:** bed height and footprint area per pass, awake bodies per tick, bodies after collapse, hull vertical speed and acceleration, and a 10 s park on the bed (travel against the settle probe's 3 cm, cells changed per second).

**Caveat:** columns have steps up to 0.175 m (DERIVED, cell × tan 35°) that can snag a hull. A "blocked" result needs a heightfield tile before it counts as a refutation.

## Slices that each land whole

1. **The bed as a surface, fed by today's sheds.** Tiles, every reader, replication, the mesh, deposit at landing, and repose. Welds still exist. What is shed today stops vanishing.
2. **Bearing** under hulls, pieces and belts.
3. **Delete the weld.** Sub-floor matter enters the bed at its freeing event. This is the slice that frees the tank.
4. **Re-measure #374, #312 and #379 hosted.** Decide freeze's deletion and whether the press pass is needed.
5. **View:** drawn chunks seated on the bed.

## Risks and what I could not establish

- **Mounds as obstacles.** A repose slope of 35–45° is at or past track grip, so a tall mound may stall the tank until the bearing law flattens its toe. The level-ground bearing form also overstates a crest's capacity. If the tank hangs on mounds a player would expect it to squash, the missing term is the slope-crest reduction, not a tuned number.
- **Rounds.** `src/ballistics/contact_classes.rs` holds the ground as a `HeightGrid`. I did not trace how a round's march would meet the bed.
- **Track field caching.** `TrackField` rebuilds on `TerrainMap.revision`. I did not find which consumers cache against it or what a per-tick bed change costs them.
- **Parry.** Per-cell triangle removal is present in the parry3d 0.27.0 source in the local registry; the lock pins 0.31.1, which I did not check.
- **A fresh collapse without welds.** How many bodies remain and how the mound looks are unmeasured; run 2 converted welds mid-run.
- **Not established:** hosted cost of any part, replication volume, whether fill overlapping standing walls reads wrong, the patch-width rule for long strips, and spawn placement on a bed.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/src/debris/crumble.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/oracle.rs
- /Users/Yan/github/vikng-dev/overmatch/src/terrain_grid.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/rubble.rs
- /Users/Yan/github/vikng-dev/overmatch/src/net/construction.rs
