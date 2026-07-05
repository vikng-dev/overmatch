# Design sketch: sim/view split & the tank bake (tier 3)

**Status: PHASE 1 IMPLEMENTED & GRADUATED (ADR-0014); phase 2 (the offline bake, §8 step 4)
remains governed by this sketch.** Recorded 2026-07-05 from the MP-overhaul follow-up session
(agent + Yan); research folded into §7 the same day. Steps 0–3 of §8 landed 2026-07-05
(extractor + shadow `d4de242`; sim-spawns-whole `b380a55`; view-attach `262225b`; demolition —
see git log): the settled phase-1 decisions live in
[`0014-sim-view-split`](../adr/0014-sim-view-split.md). What this sketch still owns: §7's
artifact-format decisions and §8 step 4 (`tools/tankc`, the baked artifact + handshake hash, the
server shedding the glb) plus §9's phase-2 honesty test.

Related: [`0012-spec-driven-rig-binder`](../adr/0012-spec-driven-rig-binder.md),
[`0013-composable-rig-control`](../adr/0013-composable-rig-control.md),
[`rig-ron-sot-and-composability.md`](rig-ron-sot-and-composability.md) (superseded, kept for the
seam reasoning — this sketch is the anticipated "revisit" that doc reserved the right to).

## 1. Problem: the tank is born twice

lightyear's entire prediction/rollback model assumes a predicted entity is **born complete** —
every sim-relevant component exists the tick it spawns; history, rollback restore, and replication
all key off that. Our tank has a two-phase birth: the replicated root arrives at tick T, and the
actual sim body — servo frames, wheels, colliders, armor volumes — assembles asynchronously
whenever the glb scene instantiates and `on_tank_ready` binds it. Everything between those two
moments is the **bind window**, and every netcode casualty to date lives in it:

- `Position::PLACEHOLDER` NaN — physics ran before the body existed (fixed e130aaa)
- ghost child replication via `ReplicateLike` — replication walked a hierarchy that appeared late
  (fixed fb33443)
- the deleted step-7/8 machinery (history attach races, pose-history stripping, despawn grace) —
  rollback state attached late to late entities (retired 2bbf8c3)
- asset preloading (27a9676) — shrank the window to ~130ms; did not close it
- `ConfirmedHistory` seed poisoning — lightyear enshrined a mid-life-inserted `TankSim`'s add-time
  value as permanent "server truth"; every state rollback restored bind defaults, and the lazy
  servo-rest capture then baked the current lay into the servo zero (aim desync + gun visibly
  outside travel limits; stripped by `strip_confirmed_history`, `net/protocol.rs`, 2026-07-05)

That is one architectural mismatch surfacing five ways, not five bugs. Neither replicon's
documented blueprint pattern (marker + **synchronous** local construction via required components)
nor any lightyear example covers a predicted entity whose sim state materializes N ticks after
spawn. We have been improvising in a gap the libraries don't acknowledge.

**The standing rule adopted from this (tier 2, effective immediately):** *nothing
rollback-registered may be initialized from an asset or inserted late onto a replicated entity —
sim state must be constructible at spawn, synchronously, from data.* Tier 3 is the architecture
that makes the rule structural rather than disciplinary.

## 2. Target: sim skeleton from data, glb as view

The netcode-industry pattern (Quake lineage, Overwatch ECS talks — sim/view separation): the
simulation body is built synchronously from *data*; the art asset is a view that attaches whenever
it loads and only renders. Server and predicted client run entirely on the data-defined body.
The sim never waits for an asset because the sim never *reads* an asset.

End state: the server can spawn, simulate, and resolve combat for a tank **without the glb on
disk**. The client spawns the identical sim skeleton at tick 0 and parents the glb scene onto it
later as pure presentation. The bind window stops being a sim concept and becomes a ~100ms visual
pop-in, which no netcode has to care about.

## 3. What the sim actually reads from the glb today (inventory)

Full sweep 2026-07-05 (Explore agent, verified against source). The glb currently supplies four
categories; the RON supplies scalars only (it names nodes, never coordinates).

**Trivial to bake — one Transform/Vec3/Quat per named node:**
- servo rest quaternions (today lazily captured `tank.rs:981` — the exact field the
  ConfirmedHistory bug corrupted) and each servo's local transform + static intermediate chain
  transforms up to root (`rig_world_pose` chains, `tank.rs:524`)
- muzzle local pose (shell origin/bore, `shooting.rs:132-143`), barrel rest translation (recoil
  spring origin, `shooting.rs:61`)
- roadwheel station local poses — suspension ray origin + down dir per wheel (`driving.rs:201`),
  plus wheel count/side (today derived from `Wheel_L_/R_<n>` name pattern, `tank.rs:496`)
- `Center_Of_Mass` empty position (`driving.rs:100-123`)
- turret pivot offset (camera reads it once, `camera.rs:86-94`)

**Structural/topology (data, not geometry):** the name→node map, the gun→turret ancestor walk
(`tank.rs:855-864`), volume→owner mapping (`ballistics.rs:527-538`).

**Mesh-derived — the dominant scope:**
- **armor volume trimeshes** (~45 concave volumes, `TrimeshFromMesh` on the Armor layer,
  `tank.rs:813`): the penetration march raycasts them for entry point, normal, perpendicular
  thickness, slope span (`ballistics.rs:517-608`). Plate thickness is deliberately geometric —
  the mesh IS the armor model.
- **vehicle collision hulls** (`*_Collider` → `ConvexHullFromMesh`, `tank.rs:692`).

**Already data (no bake needed):** mass + inertia extents (authored, `NoAutoMass` — no density
path to bake), drivetrain/suspension scalars, servo tuning + role/axis + travel, weapon
ballistics, recoil spring, RangeTable inputs, volume material/hp/crew/ammo facets, view configs.

**View couplings that constrain the split (§6):** both cameras, the gunner optic, the bore/intent
HUD, component-HP labels, and the cook-off turret launch all address sim rig-node **entities by
handle** (`rig.gun/turret/muzzle/hull`, volume nodes, wheels). Cook-off
(`damage.rs:548-580`) additionally *reparents the glb turret node* into a free rigid body.

## 4. Who needs what (the data split, made coherent)

| Category | Server (sim authority) | Client sim (prediction) | Client view |
|---|---|---|---|
| Rig frames (servo chain, wheels, COM, muzzle) | always — colliders and muzzle ride the chain | always | reads poses to render |
| Collision hulls | always | always | never |
| Ballistic trimeshes | resident from spawn (must be raycastable), touched only on shot resolution | same (predicted impacts) | never |
| Visual meshes | **never** | never | always |
| Behavior scalars (RON) | always | always | HUD reads some |

Note the correction to the intuitive read: Ballistic is lazy in *CPU* terms only — the march
raycasts a physics layer, so the trimeshes must be registered in the physics world from spawn.

## 5. Authoring decision: compile, don't merge

Agreed baseline: **Blender owns spatial truth** — hand-maintaining coordinate systems in RON is
the wrong tool; the model defines how a tank looks and behaves spatially.

The considered alternative — move *all* data (scalars too) into Blender custom properties and
drop the RON — is rejected for now, on three grounds:

- **Variants:** N stat-sheets sharing one model (production variants, MP balance forks) need a
  data layer outside the .blend; all-in-Blender regrows that layer with extra steps.
- **Tuning loop + review:** balance scalars change constantly; text RON is hand-editable,
  git-diffable, reviewable. Blender custom props mean open-file → edit → re-export → re-bake per
  tweak, with opaque diffs.
- **Type fidelity:** glTF `extras` are untyped JSON; enums/units/validation degrade.

What DOES move model-ward: **part identity/roles** ("this node is a Yaw servo", "this mesh is
Ballistic") — meaningless without the node tree, already half-encoded as name suffixes
(`_Collider`, `_Ballistic`, `Wheel_L_1`). These become naming conventions + small `extras` tags.
(Exporter fidelity = open research, §7.)

**The shape:**
- Blender/glb: geometry, node tree, transforms, part identity/roles.
- Thin RON: behavior scalars; the variant/balance layer. Shrinks (loses node-plumbing), survives.
- **Build step ("tank compiler")** joins both → **one baked artifact** the runtime loads.
- The runtime never joins anything. Today's bind-time fail-fast contract (RON node missing from
  model, model part undeclared) becomes a **build error** instead of a runtime panic on connect.

If after another tank or two the RON has shrunk to nothing worth keeping, deleting it then is a
cheap, reversible call — per ADR-0012's own revisit clause.

## 6. Architecture decisions

**A. Where extraction runs — two mounting points, one extractor.** Write
`extract(glb) → TankGeometry` (frames, hulls, trimesh data, topology) as a **pure library
function**.
- *Phase 1 (kills the bind window now, zero pipeline infra):* call it at preload. The glb is
  already a hard pre-connect load gate (`PendingTankAssets`); extract into a plain resource, spawn
  the full sim skeleton synchronously from it at tick 0. Scene instantiation becomes view-attach.
- *Phase 2 (server sheds the glb, checks move to build time):* the same call runs in an offline
  tool; output serialized as the baked artifact; runtime loads artifact only. This is when RON+glb
  contract validation becomes a build failure.

**B. Sim skeleton representation:** keep child entities (servo frames, wheel stations, collider
nodes), spawned synchronously from `TankGeometry` — Avian wants collider entities and
`rig_world_pose` keeps working; only the *source* of their local transforms changes (data instead
of scene nodes). Carried state stays root-resident in `TankSim` (unchanged; the rollback story is
already correct post-1b).

**C. View-attach contract:** when the glb scene instantiates, a name-keyed bind parents render
nodes onto (or maps them to) the sim skeleton's parts. Consumers re-point:
- pose-followers (cameras, HUD dots, optic): read sim-part poses — most can read baked scalars or
  the sim entities directly; render smoothing (`interpolate_servos`, `apply_recoil` writes) moves
  to the **view** nodes.
- `sync_optic_render_layer` walks render meshes — becomes a view-tree walk.
- cook-off turret launch: sim decides + detaches the sim turret subtree; the view mirrors the
  detach (its turret nodes follow the new free body). The view must be detachable along sim part
  boundaries — a constraint on the bind mapping, worth an explicit part-subtree table in the
  artifact.

**D. Artifact identity:** client and server must provably run the same bake — hash the artifact
into the connect handshake; version the artifact format.

## 7. Open research (in flight 2026-07-05, fold findings here)

1. **Avian raw-data colliders** — RESOLVED (vendored pass, 2026-07-05; avian3d 0.7.0 /
   parry3d 0.27.0):
   - Exact equivalences proven at the parry call level: `TrimeshFromMesh` ≡
     `Collider::trimesh_with_config(verts, indices, TrimeshFlags::MERGE_DUPLICATE_VERTICES)`
     (avian `parry/mod.rs:1174`); `ConvexHullFromMesh` ≡ `Collider::convex_hull(points)` — it
     ignores indices entirely (`parry/mod.rs:1243`). So raw-data construction reproduces today's
     shapes byte-for-byte. Note: avian's own mesh extractor bails on unindexed primitives
     (`parry/mod.rs:1499`) — our extractor shouldn't require indices for the hull path.
   - `Collider` serde round-trips (under `serialize`, currently only pulled in via the repo's
     `net` feature), BUT the wire format is parry-0.27 internals (BVH layout included, shape
     stored twice) with **no version stability** — a baked parry blob dies on every
     avian/parry upgrade. **Decision: do NOT serde colliders; bake raw geometry + a shape
     recipe** (see the determinism-driven format below).
   - **Determinism:** parry's cross-platform story is gated behind `enhanced-determinism`
     (not enabled; not needed for this). Shape *construction* uses IEEE-exact ops only, but the
     incremental convex-hull algorithm is tie-break sensitive to 1-ULP differences — the one
     component risky to run independently on two platforms. **Artifact format therefore: per
     collision proxy, run the hull OFFLINE once and store the pre-hulled verts+face indices,
     reconstructed at load via `SharedShape::convex_mesh` (no hull computation at runtime, fails
     loudly at bake time instead of silently at spawn); per armor volume, store verts/indices
     with duplicates pre-merged offline, rebuild as plain trimesh.** Client and server then
     construct identical shapes from identical bytes by construction.
2. **glTF-as-data access** — RESOLVED (same pass):
   - The `gltf` 1.4.1 crate is already a direct dependency AND already parses this exact .glb
     offline in the CI bind-contract test (`src/spec.rs:317`) — phase 2's parser exists in tree.
   - Phase-1 preload extraction is fully feasible with no scene spawn: `Assets<Gltf>` →
     `GltfNode` (local `Transform`, `extras`, names) → `GltfMesh`/`GltfPrimitive` →
     `Assets<Mesh>` → `ATTRIBUTE_POSITION.as_float3()` + `indices()`. glTF meshes are labeled
     subassets present the moment the glb asset is Loaded — i.e. already guaranteed by the
     existing `PendingTankAssets` gate.
   - Coordinate caveat: bevy_gltf 0.19's glTF→Bevy coordinate conversion is opt-in and OFF by
     default (repo never configures it), so offline `gltf`-crate transforms match runtime
     one-for-one. If the app ever opts in, the bake must replicate the conversion.
3. **Blender exporter fidelity** — RESOLVED (web pass, 2026-07-05):
   - Object/mesh/material/**empty** custom props export reliably to `extras` under
     `Include > Custom Properties` (`export_extras`). Types that survive: str/int/float/bool,
     arrays, JSON-compatible nested dicts (exporter: glTF-Blender-IO `com/extras.py`).
   - Gotcha: object props → `node.extras`, mesh-DATA props → `mesh.extras` (shared across objects
     sharing a mesh) — author tags on the OBJECT. RNA-defined props (PropertyGroup/Enum) do NOT
     export; failures are silent — the compiler must validate tags and fail loudly.
   - Avoid: props on bones/Actions/collections (patchy/undocumented), per-instance overrides of
     props inside linked collections (library-override friction).
4. **Prior art** — RESOLVED (web pass, 2026-07-05):
   - **Blenvy is dead** (alpha, pinned Bevy 0.14, unmerged 0.15 PRs since Oct 2024) — do not
     adopt. Its post-mortem lesson: it reimplemented RON serialization in Python and died keeping
     up with Bevy releases. Our own-compiler plan sidesteps that class entirely.
   - **Skein (bevy_skein 0.6, Bevy 0.19, maintained)** is the successor: components as plain JSON
     under a namespaced `extras` key, decoded by the engine's own deserializer. Borrow patterns
     (namespaced key, flat JSON, validate-in-compiler), not the runtime crate — we compile to an
     artifact, we don't spawn components at load. (Upstream standardization: bevy#21038.)
   - **Offline glTF→collider baking:** no maintained off-the-shelf Rust tool exists — teams roll
     a headless step on the `gltf` crate + parry3d/avian types (all renderer-free; parry shapes
     are serde-serializable). Bevy's asset processor is not yet a compelling host. Industry
     precedent is exactly our shape: Source compiles collision to a separate `.phy` the server
     loads (never the render mesh); Unreal/Flax cook dedicated collision data.
   - **Numbers-in-Blender verdict supports §5:** ecosystem consensus frames Blender-authored
     extras as structural/spatial + role tags, NOT balance scalars (iteration cost, opaque binary
     diffs, silent-drop type unsafety, and no variant story without duplicating Blender data).
     Rule of thumb adopted: *needs a 3D viewport to author → Blender; needs a diff to review →
     RON.* Explicit `extras` role tags are preferred over parsing name suffixes long-term —
     equally reliable to export, less fragile — with the compiler validating either way.

## 8. Implementation plan

Steps are ordered so every one lands green (gates + harness + SP smoke) and the extractor is
proven equivalent BEFORE anything switches to it. Format details (baked serialization, extras
tags) may firm up when §7 research lands; the structure below doesn't depend on them.

### Step 0 — shadow extractor (no behavior change)

Build `TankGeometry` + the pure `extract(glb assets, spec) → TankGeometry` library fn (frames,
wheel stations, COM, muzzle/barrel, part topology, hull + trimesh vertex data). Mount it at
preload. Then, in `on_tank_ready`, **shadow-compare**: assert every value the scene walk reads
equals the extracted value (exact — same source bytes, same floats). Ship it asserting in debug /
logging in release. This is the migration's load-bearing trick: the extractor is verified against
the living architecture while the living architecture still runs, so the switch in step 1 changes
*where data comes from* with proof it's the same data. Also add a golden test: extract the Tiger,
snapshot-check the artifact summary (counts, key transforms, vertex totals).

### Step 1 — sim spawns whole (kills the bind window)

Flip the spawn: build the sim skeleton (root + servo frames, wheel stations, collider entities,
armor-volume entities, `Rig`/`TankSim`/indices) synchronously from `TankGeometry` — SP at spawn,
server at connect-spawn, client the moment the replicated root lands (`NetTank` add). The scene
is no longer the sim constructor. `on_tank_ready`'s sim half dies here; what survives becomes the
step-2 view binder. Lazy captures die with it: servo `rest`/`captured` (delete the `captured`
field outright — `ServoState` shrinks to true per-tick state), `RecoilParams.rest`, the camera's
turret-pivot capture, the COM `GlobalTransform` read.

### Step 2 — view attach

The instantiated glb binds to the existing sim skeleton by the artifact's part table: visual
nodes parented to (or pose-mirroring) sim parts. Re-point the render-side writers and readers:
`interpolate_servos` / `apply_recoil` write **view** node transforms; cameras/HUD/optic read sim
poses or view nodes explicitly (each consumer from §3's coupling list gets an explicit decision);
`sync_optic_render_layer` walks the view tree; cook-off detaches the sim subtree and the view
mirrors it. This is the highest-touch step (many files, feel-test sensitive) — its own commit(s),
windowed feel test before proceeding.

### Step 3 — demolition (the anti-artifact pass) — DONE 2026-07-05

Everything that exists only because the scene used to construct the sim gets **deleted in the
same PR series**, not left to rot (see the wariness this plan exists to serve). The checklist:

- [x] `on_tank_ready` scene walk as sim constructor; the runtime name→entity `index` (step 1)
- [x] runtime name-pattern matching (`Wheel_L_/R_<n>`, `*_Collider`, `_Ballistic` suffix logic) —
      moved into the extractor (`TankGeometry::roadwheels`/`collision_proxies`); the runtime never
      parses node names for sim meaning. (`bind_tank_view`'s hide rule still name-matches — view
      meaning, deliberately out of scope of this rule.)
- [x] the gun→turret ancestor walk at bind (`first_ancestor_in`) — topology is extracted data
      (`first_geometry_ancestor`, step 1)
- [x] sorted-by-name index assignment as a *determinism device* — the wheel sort lives inside the
      extractor; servo/weapon sorts stay at spawn over the spec maps (they merge into the artifact
      at phase 2)
- [x] `ServoState.rest`/`captured` and every other lazy capture (step 1)
- [x] Static-until-bind + `activate_bound_rig`: deleted. Each spawn path sets `RigidBody` in the
      spawn flush by role (server always Dynamic; client Dynamic-if-Predicted else Static), with an
      `Add<Predicted>` upgrade observer for the marker-lags-pose case (vendored-source verified:
      `Predicted` is a replicated marker on its own visibility path, not guaranteed to ride the
      pose's init message). The pose gate — lightyear's replicated-pose arrival — is the one gate
      that remains, as predicted.
- [x] `PendingTankAssets` as a *connect* gate: kept for phase 1 (the glb is the extractor's source
      and the shadow's subject), re-documented as such; the server drops it at phase 2
- [x] bind-window diagnostics: `report_orphan_transforms` deleted (ghost-child/bind races are
      structurally impossible); `count_rig_binds` deleted — its build-exactly-once invariant is now
      enforced by the spawn gates themselves (`attach_replicated_rig`'s `Without<RigidBody>` filter
      cannot match a built tank twice; the server drains its pending queue), not merely observed;
      `nan_tripwire`/`fixed_nan_probe` kept and re-scoped as general NaN-class guards
- [x] docs: ADR-0012's runtime-join sections marked superseded by
      [`0014-sim-view-split`](../adr/0014-sim-view-split.md); tier-2 rule added to AGENTS.md;
      sim-body/view/bind-window(retired) terms added to the glossary
- [x] grep-sweep for "bind window" / "bound rig": every remaining mention describes the view
      attach or accurately-dated history of the retired window

What deliberately survives: `TankSim` root-residency + rollback registration (already correct
post-`strip_confirmed_history`), `Rig` handles, `ServoIndex`-family (assignment source changes),
the RON spec (thinner), `rig_world_pose` (now walking data-spawned entities).

### Step 4 — phase 2: the offline bake

`tools/tankc` (workspace bin) = the same extractor + the RON, emitting the serialized artifact
(format per §7 research) + build-time contract validation (today's bind panics become tool
errors). Runtime gains an artifact loader; server loses the glb load path (feature-gate
`bevy_gltf` out of the server build if practical). Artifact hash goes into the connect handshake.
CI: re-bake and diff against the committed artifact so model/RON edits can't silently drift.

### Verification at every step

`/verify`-grade: clippy both feature sets, 14+ tests, headless harness short+long vs the current
baselines (short ~2, long ~38-40, 0 NaN), SP windowed smoke after steps 1-2, full feel test after
step 2, and step 4 ends with §9's phase-2 honesty test.

## 9. Definition of done

- *Phase 1:* every sim-read of scene-node data at spawn is gone; the sim skeleton spawns whole at
  tick 0 from `TankGeometry`; SP + MP harness baselines hold; the tier-2 rule holds by
  construction for all existing state.
- *Phase 2 (the honesty test):* the server binary simulates a full duel — driving, aiming, firing,
  penetration, cook-off — **with the glb deleted from its disk**. A partial bake leaves a partial
  bind window; this test is what "done" means.
