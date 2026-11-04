Subject: **the video game's environment destruction model; game state and presentation only.**

## One source solid

The prop pipeline now authors one convex source element, with an internal chunk table made only
by its construction pattern. Exterior rings never cap or partition the interior. The common
construction runtime, material laws, attachments, pattern dimensions and grip laws are unchanged.

DERIVED: for `r(z) = A (H-z)^k`, `r'' = A k(k-1)(H-z)^(k-2) <= 0` when `0 < k <= 1`.
For any two points inside the solid, the triangle inequality and radius concavity place their
interpolated point inside it too. Thus the solid is convex. The old positive radius floor at the
tip breaks that concavity; it is removed, leaving a point apex.

The species data explicitly states breast height, taper exponent and radial approximation
resolution. The exponent is a labelled silhouette taste, not a measured stem-form factor.
Resolution is **0.03 m radial chord error along the taper, DERIVED authoring decision**; the
**12-sided section, DERIVED resolution choice**, separately bounds circumferential radial loss
by `1-cos(pi/12)`. Rings are selected at maximum chord error from the derivative, with no
segment-length grid. An unrepresentable requested resolution is refused with its field and fix.

The checked source is Parry **0.27.0**, `ConvexPolyhedron::from_convex_mesh` in
`src/shape/convex_polyhedron.rs`, with `math::DEFAULT_EPSILON = Real::EPSILON`. The vendored
Avian default enables `f32` / `parry-f32`. Parry deletes an edge when adjacent unit normals have
`dot > 1-sqrt(f32::EPSILON)`: **1.505662561 degrees, DERIVED**. The planner refuses neighbouring
longitudinal faces inside that threshold; tests also check every shared-edge face pair of the
sampled species ranges. The shipped fir's tolerance admits a base ring and apex: one polygonal
cone. This is a deliberate coarse approximation of the taper, with the mass difference below
reported for judgement; no density, attachment or cell size compensates for it.

## Generator and data census

- Deleted `SEGMENT_MIN_M`, `SEGMENT_MAX_M`, `SEGMENT_TARGET_M`, `segment_count`, `segments`,
  segment-height decimal remainder handling and the segment-derived `boundaries` table.
- Deleted `TIP_RADIUS_MIN_M`, the positive tip-radius floor and the separate authored `r_tip_m`.
- Deleted `BAND_M`, `bands`, `band_solids`, per-band pieces and their internal caps/seams.
- Rebuilt `.tree.ron` as generator-only `TreeSource(species, seed, height_m, dbh_m)`.
  It carries no segment geometry or simulation state. The certificate's `sim_ron_sha` becomes
  `source_ron_sha`; no compatibility field is retained.
- Rebuilt Blender generation as a closed source mesh and a canopy at the source origin. Deleted
  the chained trunk nodes, `tree_segment` / `tree_part` metadata, cut-cap nodes and `CutWood`
  inspection material, per-segment canopy grouping and parent-height translations. Inspection
  nodes use `construction_part`; the live view still comes from common chunk geometry and dressing.
- Rebuilt prop admission, geometric identity and refusal tests around the source shape, complete
  outward triangle surface, one source piece, sourced shape data, seed identity and foliage.
  Removed segment numbering, chain/cap/radius-boundary and cross-runtime data-equality tests.
  Generic GLB container/buffer and certificate refusal witnesses remain. The asset-routing test
  also names the generator-only `TreeSource` in its mock worksheet; routing logic is unchanged.
- Deleted `assets/trees/wood.ron`: the old HP scale and duplicated structural facts. Common green
  wood materials and attachment rows remain their owners. Updated their stale data comments.
- The separate topology/reset tests now construct inline legacy state. Removed the test that
  expected a missing shipped legacy tree file and the shipped wood-row comparison. Runtime tree
  code and its remaining tests are left for the dispatched deletion session.
- `scripts/prop/render_tree.py` already frames meshes by bounds and needs no replacement.
  `tools/` has no tree-runtime logic after regeneration: masonry's local `band`, range descriptions
  and Rust's `std::slice` / `extend_from_slice` are unrelated to the removed system and remain.

## Geometry and collider evidence

DERIVED from the fir generator source: `H = 27.02 m`, `dbh = 0.4 m`, breast height `1.3 m`,
`k = 0.8`, so `r(0) = 0.2080470203 m`. The exact taper has
`V = pi r(0)^2 H / (2k+1) = 1.4131389582 m³` and, at the material's **850 kg/m³, MEASURED
transcription of its declared density**, mass **1201.1681145 kg, DERIVED**.
The cone's maximum longitudinal radial deficit is **0.0170432119 m, DERIVED**.

MEASURED by the admitted construction bake: **1 element, 37 chunks, 1.169522 m³, 994.094 kg**,
with **126 joints and 6 anchors**. The house bake is byte-identical. Relative to the exact taper,
the baked mass is approximately **17.24% lower, DERIVED**; this is the geometric approximation's
outcome, not a strength target. The emitted source has no intermediate horizontal face.

The dressing was regenerated and checked against every exported canopy corner. Its bytes are
unchanged; `ConstructionLibrary::load` resolves every card base against the new chunk table, and
the shipped-map/common-detachment tests passed through that admission.

MEASURED runtime counts below come from
`headless_test::shipped_map_constructions_use_complete_common_spawn_state`, mounting the production
composition on Kalinovo's shipped height grid. `scatter::spawn` calls `construction::spawn`, which
spawns an intact collider per source element. There is now one element per fir: **one collider per
intact tree**, meeting M6's contract without a runtime special case. Disturbance replacement remains
the common path; M12 still owns its further representation work.

The before census is MEASURED in `m11-session-1.md` on the same admitted geometry. It is a
construction-only census; terrain and vehicle colliders are outside it.

| Kalinovo / kind | Instances | Before colliders | Before per instance | After colliders | After per instance |
|---|---:|---:|---:|---:|---:|
| fir_1 | 635 | 8,255 | 13 | 635 | 1 |
| house_1 | 74 | 4,070 | 55 | 4,070 | 55 |
| Total | 709 | 12,325 | — | 4,705 | — |

## Verification

- **30 prop tests passed, MEASURED**, including pinned Blender generation, deterministic export,
  byte-for-byte source re-export, geometric/refusal mutants, source/data identity and dressing.
- The construction bake certified and admitted its serialized output before writing it. Its
  house output was unchanged; its fir output is the new single-element construction.
- **358 game-library tests passed, with 7 existing ignored tests, MEASURED**, covering
  `construction::`, `map::`, `scatter::`, `trees::`, `net::world_reset::`, all of `headless_test`,
  and the required whole-tree `doc_citations`, `ui_ascii`, `net_boundary` and `bake::` guards.
  **47 tooling tests passed, MEASURED** in the same workspace invocation.
- The separate production census recheck passed and printed the counts above. It overlaps the
  targeted suite; it is not an additional distinct invariant.
- Formatting and CI-feature workspace/all-target clippy passed. The routing mock's final schema
  label change passed shell syntax checking; its selection logic is untouched.

All Cargo builds and game/asset executions used the machine lock. Runs were unattended, headless
and muted.
The rebase-check process already running before this dispatch used the earlier test binary and
failed its now-deleted shipped legacy fixture during data replacement. The rebuilt session tests
above pass, including all reset tests and their real-UDP lifecycle witness.

The diff was self-reviewed against all six lenses. No backlog row names an edited path. The
census covers `scripts/prop/`, tree/species data, materials' stale references, `tools/` and the
asset-routing fixture. Blender's backup of the old source was removed. The dispatched runtime
and its consumers still await their own sessions; this is not the final whole-tree deletion claim.

## Handoff

Session 3 owns shared ledger/carrier identities and causes, reconstruction, presentation-time
membership, reconnect/reset, registration and the deliberate protocol revision bump. Session 4
owns the ballistics ratchet; session 5 deletes the runtime tree code and remaining contracts;
session 6 supplies the joined production witness, UDP drive, observer/reconnect proof and final
whole-tree deletion census. The earlier session report's five-session handoff is superseded by
this dispatch's six-session allocation.

No pricing, captures or production drive are taken here. `net-22`, `servo.rs`, `load-27`,
`load-31`, grip dwell, `GRIP_SHEAR_FLOOR_M` and `piece_impulses` remain outside this change.
