# Slice 1: the fallen tree

Subject: Overmatch is a video game. This slice makes the map's trees part of the game's damage
model: a tree is a hard body with an HP pool and a state; game rounds dose it, tanks push it, and
it falls or breaks. Vocabulary is the game's (`.agents/GLOSSARY.md`): dose, potency, HP pool
emptied, knocked out. Decided with Yan on 2026-10-26; the research behind it is in this directory
(`codebase-scout.md`, `engine-delta.md`, `games-and-dagor.md`, `authoring-tools.md`,
`tree-assets.md`, `tree-rendering.md`).

## Decisions taken (settled; do not reopen in the lanes)
- Trees first; buildings reuse the skeleton (identity, HP, state, wire word, client derive, props build).
- States on the wire, never physics. One word per tree.
- A fallen trunk blocks rounds and sight, NOT hulls: tanks roll over it without a bump this slice.
- Realistic sizes from a species table with provenance, not the graybox 14–19 m.
- The trunk is a stack of ~1.5–2 m segments; a cut snaps to a segment boundary; k = 0 is a whole-tree fell.
- Dose empties the pool → `Down { cut: segment of the emptying hit's entry height, dir: the round's horizontal direction }` (falls away from the shooter).
- Ram: per contact tick, `impulse × lever ≥ fell_k × dbh³` → `Down { cut: 0, dir: push direction }`. Thresholds are data, set from measurement.
- The resting pose is a pure function of (asset, cut, dir, terrain height grid): pivot at the cut, the far end dropped to the ground along `dir`. Every peer computes it; nothing else crosses the wire.
- No impostors, no debris theatre, no sound in this slice. The canopy is view-only.

## The contract (frozen; every lane reads it, no lane changes it without the orchestrator)

### 1. Asset trio, `assets/trees/<id>/`
`<id>.glb` (view), `<id>.tree.ron` (sim data), `<id>.lod.json` (LOD certificate, same shape as the tank's), built by `scripts/prop/build.py build assets/trees/<id>/<id>.blend`, the `.blend` generated headless by `scripts/prop/gen_tree.py --species norway_spruce --seed N --out …`. Pattern: `scripts/tank/build.py` (ADR-0035/0036).

`<id>.tree.ron`:
```ron
TreeAsset(
    species: "norway_spruce",       // key into assets/trees/species.ron
    dbh_m: 0.42,                    // trunk diameter at 1.3 m; the ram threshold reads dbh³
    segments: [                     // bottom to top; segment k = index; segment 0 at the base
        Segment(height_m: 1.6, r_bottom_m: 0.24, r_top_m: 0.23),
    ],
)
```
The canopy appears only in the view. The sim side is built from this file at spawn (ADR-0014), never from the glb.

### 2. View glb node contract
- Root node `<id>`, +Y up, metres.
- A CHAIN: `trunk_00` is a child of the root; `trunk_{k}` is a child of `trunk_{k-1}`, its origin at its own bottom centre, placed `height_m[k-1]` along the parent's +Y. Rotating `trunk_k` about its origin therefore moves everything above it: that is the fall.
- Children of `trunk_{k}`: `cap_bottom_{k}` and `cap_top_{k}` (cut-wood discs at the segment's ends; the runtime hides them until a cut reveals the pair at the boundary), and optionally `canopy_{k}` (that whorl's cards).
- Every one of these nodes carries glTF extras `{"tree_segment": k, "tree_part": "trunk" | "cap_bottom" | "cap_top" | "canopy"}`. The runtime reads the extras; names are for humans. (Blender exports custom properties as extras only when the exporter's custom-properties option is on; the props build script owns that.)
- Mesh LOD rungs: the trunk through the existing pipeline with the segment boundary loops preserved (each segment is its own mesh, so border locking suffices); the canopy as two card densities. No impostor.

### 3. Species table, `assets/trees/species.ron`
```ron
{
    "norway_spruce": Species(
        height_m: (25.0, 40.0),                 // mature range; provenance in a comment, cited
        dbh_m: (0.30, 0.60),
        substance: "Softwood",                   // materials.ron key; Lane B adds the entry
        fell_k: (0.0, Unknown("set from Lane C's measured push/ram impulses")),   // N·s·m per m³ of dbh³
        hp: (0.0, Unknown("balance data; set so a 75 mm AP round through the trunk does not fell a mature spruce")),
    ),
}
```
`(value, Measured(..) | Unknown(..))` pairs follow `assets/materials/materials.ron`.

### 4. State and transport
`TreeState { Standing, Down { cut: u8, dir: u8 } }`; `dir` is the fall azimuth quantised to 256 steps; one 16-bit word. Server sets it once (`set_if_neq`), clients derive on arrival and never latch (ADR-0016 rules; precedent `NetTankStatus`, `src/net/disclosure.rs`). Transport: recommended a single replicated `DestructionLedger` on a world entity mapping map instance id → word (the Dagor pattern; a late joiner gets the whole ledger); the alternative is one replicated entity per tree, whose cost is the replicon per-tick entity × component × client scan (`engine-delta.md` row on §2.1). Lane B chooses and states why.

### 5. Layers and bodies
- Standing: one static body per tree, a compound of capsules (one per segment), memberships `[Terrain, Armor]`, with `BallisticVolume` of the species' substance so the union walk charges chord × factor and the dose feeds the pool.
- Down: the stump (segments `0..cut`) keeps the standing memberships; the lying piece (segments `cut..`) is a static compound on `[Armor]` only.
- Canopy and caps: no collider.

### 6. Identity
`level.json` instance ids are kept at spawn as a component. The prototype library leaves the code table (`src/scatter.rs::PROTOTYPES`) for data keyed by the ids the map already uses; Lane B chooses between reading the file's own `prototypes` block and a sidecar, and says why.

### 7. Client derive
On a state's arrival: rotate `trunk_{cut}` about its origin from upright to the resting pose over `T = clamp(0.08 × fallen_length_m, 1.0, 3.0)` s (placeholder, tune by eye), reveal the two caps at the boundary, end exactly on the resting pose the sim computed. The sim collider is down from the moment the state changes; the view lags it, which is harmless since clients predict nothing (ADR-0037).

## Lanes and freezes
| Lane | Owns (writes only here) | Reads |
|---|---|---|
| A, asset | `scripts/prop/**`, `assets/trees/**` | everything |
| B, runtime | `src/**`, `assets/materials/materials.ron`, `assets/maps/**`, `.agents/GLOSSARY.md`, design docs | everything |
| C, measurement | a throwaway branch, nothing landed | everything |
Lane A ships a first, ugly `assets/trees/fir_1/` trio within its first hour and refines after, so Lane B tests against real files. Until then Lane B uses an in-code fixture `TreeAsset`.

## Gates (every lane that writes code)
`cargo fmt --all --check`; `scripts/serial-cargo clippy --locked --all-targets --features dev_ui,bitprobe -- -D warnings`; targeted `scripts/serial-cargo test --lib --features dev_ui,bitprobe <modules>`; Python: `python3 -m pytest scripts/prop`. Compile only through `scripts/serial-cargo`.

## Measurements this slice must produce (MEASURED, labelled)
- Lane C: per-tick hull–trunk normal impulse for a sustained push and for rams at 5, 10, 20 km/h, against a static capsule r 0.25 m, in `track_sandbox`; the contact point height.
- Lane B: frame cost on the map with 635 segmented trees vs today's cones (`SPIKE_FRAME_COST`, same flight), and the server tick cost of 635 compound static bodies.

## Amendments after Codex's design review (2026-10-26, `codex-slice1-design/verdict.md`): these override the sections above where they differ
A1. **Bodies.** No `Collider::compound`: the walk's convex collector assumes one entry/exit interval and compound shapes are uncertifiable (`src/ballistics/collect.rs:164, 425`). One static body per tree, each segment its own child entity with its own convex collider under the tree's `BallisticVolume` (ancestry lookup: `src/ballistics.rs:1839`). A segment's shape is the convex hull of two 12-gon rings (r_bottom at 0, r_top at height_m), so boundaries are flat and exact; no capsules. The cut segment is found from the hit's entry height in the instance's local frame against the segment prefix sums (`cut` = the segment containing the height; a height exactly on a boundary belongs to the upper segment); avian's hit data carries no subshape identity.
A2. **Fallen filters.** The lying piece: memberships `[Armor]`, filters `NONE` (the cast-only armour pattern); rays select by membership, hulls (filters `ALL`) never contact it.
A3. **Word encoding.** `Standing = 0`; `Down = ((cut + 1) << 8) | dir`; explicit encode/decode (not serde-derived); `cut < segments.len() ≤ 255`; non-canonical words rejected on decode.
A4. **Transition tick.** Each ledger entry carries the server tick of the transition: `(index: u16, word: u16, tick: u32)`. Presentation fuses to the cursor (ADR-0037, `src/net/cursor_queue.rs`): animation phase = cursor time − tick; a late joiner's old entries are already at rest. "Nothing else crosses the wire" is amended to this.
A5. **Caps.** Interior cut k ≥ 1 reveals `cap_top_{k-1}` and `cap_bottom_{k}`; cut 0 reveals `cap_bottom_0` only. Segment nodes: identity rotation, unit scale, the translation chain only. Extras bind view parts; the RON alone supplies sim pivots and dimensions (ADR-0014).
A6. **Schema.** Drop `dbh_m`: DBH is derived at load from the authored taper at 1.3 m (× the instance's uniform scale). Load-time validation rejects: non-finite or non-positive dimensions, discontinuous radii (`r_top[k] ≠ r_bottom[k+1]`), unknown species or substance, non-positive `hp` or `fell_k`, non-uniform or negative instance scale, tilt. Placeholders in `species.ron` are therefore positive provisional values with `Unknown(..)` provenance, never zero. HP uses the existing integer pool representation (`src/ballistics.rs:1112`); species-fixed, size-independent for this slice (noted as a later refinement).
A7. **Ram aggregation.** Per tick, start empty; sum over the hull–tree contacts of `impulse_i × lever_i`; compare once; no accumulation across ticks (repeated sub-threshold ticks never fell). Read `ContactPoint.normal_impulse` after `SolverSystems::StoreContactImpulses` in `PhysicsSchedule` or after the physics step (`avian3d-0.7.0/src/dynamics/solver/plugin.rs:419, 720`). Units: impulse N·s, lever m, `fell_k` N·s/m² (dbh in m, cubed). Fall direction from the contact normal into the tree, horizontal; never from hull velocity (zero under a sustained push). Lane C pins tick length, substeps and solver settings in its report.
A8. **Transport.** One globally visible ledger entity replicated with lightyear's native Replicon diff replication (`.replicate_diff()`, `lightyear_replication-0.30.1/src/registry/replication.rs:213`); entries keyed by a digest-bound sorted numeric tree index, never the string ids; fixed-width words. The map handshake digest (`src/map.rs:379`) extends to the resolved tree sim data, species parameters and the prototype mapping.
A9. **Resting pose, fully specified.** Inputs: the asset, the instance transform (position, uniform scale, yaw), `cut`, the DECODED quantised `dir` (both authority and clients use the decoded bin), and `HeightGrid::height_at` (`src/terrain_grid.rs:362`), nothing else. Pivot = world position of segment `cut`'s bottom. The lying piece is the rigid rotation of the stack about the horizontal axis perpendicular to `dir` through the pivot by angle φ, where φ is the smallest angle in [0°, 100°] at which any sample point (each segment boundary and the tip, each offset downward by its local radius) reaches the terrain; found by bisection, 24 iterations, fixed order; sample xz clamped to the grid at the map edge; if nothing touches by 100°, φ = 90°. Trig through one pinned source (a checked-in table of 256 `(cos, sin)` f32 bit patterns for the bins, or `libm`), never platform `sin_cos`. A test pins the raw pose bits for fixture cases (flat, slope, edge; several cuts and bins), and CI's Linux run must match the checked-in macOS bits.
A10. **Derive, fully.** Both Standing and Down are reconstructed from the state (colliders, caps, nodes) on state change AND on view attach (`WorldInstanceReady`); derived markers are inserted and removed (`src/net/disclosure.rs:90`). Sim readiness never waits for the view.
A11. **Emptying context.** The walk emits a contextual notification when a pool empties, carrying the emptying hit's entry point and direction (seam near `ShotDose::settle`, `src/ballistics.rs:464`; `src/ballistics/mechanism.rs:98`); a tree reads `cut` and `dir` from it. A field dose (a burst) that empties a tree fells it whole: `cut 0`, `dir` from the burst centre to the tree.
A12. **View.** `VisibilityRange` goes on the rendered primitive entities, not the chain nodes. One canopy card density this slice (the card ladder is deferred). All 635 views may attach in one pass; the cost is measured, not assumed.
A13. **Certificate.** Trees have no `.sim.glb`: the tree certificate replaces `sim_glb_sha` with `sim_ron_sha` (sha256 of the `.tree.ron` bytes); Lane A writes it, Lane B makes `src/geometry_lod/certificate.rs` and the loader (`src/geometry_lod.rs:172`) accept it generically. If that resists, trees load at LOD0 and the certificate is ignored, stated honestly.
A14. **Playtest fork (orchestrator files it).** The sim collider is down from the transition tick while the view falls for 1–3 s; cover is briefly misleading. Kept for the slice; the alternative is snapping the view. Settled by play.
A15. **Resting pose, amended (2026-10-27, #121 resolved, #140).** The first contact is bracketed by a fixed-step scan of the tilt range before the bisection (contact is not monotone across a ridge). For a cut above the base the butt slides off the stump and seats beside it on the fall's side (its edge against the stump's edge, resting on the terrain); the piece's tilt is found about the seat by the same search over a wider range, so it lies along the ground. `Resting` carries the hinge tilt, the final tilt and the seat; sim and view compose the same bits. The view falls in two stages: the hinge turn, then the drop to the seat. Provisional: the destination is a server-simulated rigid body replicated like a hull (playtest fork F8). Brief: `rest-pose-brief.md`.
