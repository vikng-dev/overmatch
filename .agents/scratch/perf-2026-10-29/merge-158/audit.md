# #158 step 1: per-part view consumers, the untextured primitives, the merge plan

Subject: the game's render path, including the game's damage-model feedback view (the x-ray and
the pen cam). Damage terms follow AGENTS.md's vocabulary. Read-only audit at `43e763d` (after #172
and #173). Nothing was run except a parse of the glb's JSON chunk ([`census.py`](census.py)).
Labels: **MEASURED** (parsed from the shipped bytes), **DERIVED** (computed from measured or read
facts), **READ** (stated by code I read), **ASSUMED**. The Tiger is the fixture, and its numbers are
fixture data.

## In short

- **The 172 "untextured" primitives are not drawn without a material.** 171 of them are LOD rung
  records, which wear their source primitive's material, cloned at bind. The 172nd is the hidden
  link template's `Link_Box`. Every primitive the tank draws carries a glTF material. (MEASURED/READ, §2)
- **Splitting by render material:**
  - the hull becomes **3** meshes, or **2** if its one single-sided member is drawn double-sided;
  - the turret becomes 2;
  - the gun's elevation and its recoil are 1 each;
  - each roadwheel stays at 2.

  This needs the 8 untextured materials folded into one palette material. Without that, the hull
  is 7 meshes and the turret 4. (DERIVED, §2)
- **The merge only shrinks the hull, turret and gun group, from 183 drawn entities to 10 meshes.**
  The running gear holds 162 of today's 345 drawn mesh entities and is already one mesh per moving
  part per material. (DERIVED)
- **Eleven readers of per-part view identity must change** (§1A; the x-ray counts as three). Seven
  read only moving-part nodes or marker empties, and are served unchanged if the merged tree keeps
  them (§1B). The rest read no view parts at all (§1C).
- **One design point the plan must settle:** where the per-part tree lives once the view scene is
  merged. glTF node names are document-unique by law L2.DOCUMENT (`src/bake.rs:38`). (§3, D3)

## 1. Consumers of a tank's per-part view entities

### A. Reads per-part identity, so it changes

| Consumer | Where | What it reads per part today | End state |
|---|---|---|---|
| **Hull marks** (view) | `src/vfx/hull_marks.rs:210` `draw_marks`, `:256` `views.get(part)`, `:271` `spawn_decal` | The struck part's `ViewNode` (any `SimParts` entry, static plates included). The decal hangs under it in the part's frame. | **Part data from the asset.** Anchor under the nearest *moving* ancestor's view node, and pre-multiply the part's rest pose relative to that ancestor. The pose comes from the blueprint's node table (`TankBlueprint.geometry.nodes[..].root_position/root_rotation`, the sim artifact's extraction at startup). The wire stays as it is: `HullMark.part` still indexes `SimParts` names (`src/hull_marks.rs:250`, `:258`, sim side `:285`). |
| **X-ray: dressing** | `src/damage_cam/pen_cam/game/xray.rs:735` `dress`; classification `:792` → `src/damage_cam/pen_cam/view.rs:192` `chain_and_tier` → `Tier::of` (`view.rs:59`); reads `Hierarchy` (`xray.rs:360`) | Per primitive: its `GltfMaterialName`, which decides substance (the tier), and its node-name chain, which decides the volume and so the component's role. Each mesh's `StandardMaterial` is swapped for its tier's `LookMaterial`. | **Split-out at the moment of change, from asset part data.** X-ray on: hide the subject's merged meshes, then spawn per-part copies from the per-part records (meshes and material names from the glb asset, rest poses from the node table) under the live moving-part view nodes, so servos and recoil still follow. `dress` then runs over them as it does today. X-ray off: despawn and unhide. One subject tank only. |
| **X-ray: mask copies** | `xray.rs:825` `mask_copy(...)` → `src/damage_cam/pen_cam/stage.rs:635` | One copy per primitive, on that tier's mask layer: Body for armour, detail and running gear, Scene for components (`look.rs:1328`). A merged mesh mixes layers. | Served by the split-out above: each per-part copy carries its own mask copy, as today. |
| **X-ray: component tints** | `xray.rs:643` → `stage.rs:174` `component_meshes`, `stage.rs:851` | The meshes of the components the record names, found by node-name chain. Fill and outline copies hang under each. | Served by the split-out. Component shapes already exist as asset data (`src/damage_cam/shapes.rs`, from the sim artifact). |
| **Damage-model feedback view: pen cam stage, demo view, kill-cam shooter** | `src/damage_cam/pen_cam/view.rs:109` `StageView::spawn`, `:313` `prepare_stage`, `:222` `classify`, `:422` demo `spawn`; `stage.rs:446` `rebuild`; `src/damage_cam/pen_cam/killcam.rs:745` | Their **own instance** of the view glb, classified and dressed per primitive, at `SourceDetail`. They never touch the live tank. | **Part data from the asset:** load the per-part scene (§3, D3). No logic change. |
| **LOD bind** (`geometry_lod`) | `src/geometry_lod.rs:290` `resolve_chains`, `:378` `attach_rungs`, `:425`–`:437` material clone, `:494` `adapt_bands` | Per scene primitive, by `GltfMeshName` and mesh asset id: its chain, its band, and one coincident sibling per rung. | Unchanged code. The merged records are scene primitives with their own chains, keyed by mesh asset id. |
| **LOD certification** | `scripts/tank/build.py` census (step 2, `:475`), `scripts/tank/chains.py`, `scripts/tank/trio.py:101` `census`, `:235` `strip_rungs`, `:284` `sim_bytes`; `src/geometry_lod/certificate.rs:46` `mesh_count` | One chain per source primitive (`<mesh>#<prim>`). `mesh_count` splits the scene's meshes from the rungs. The sim artifact is a byte-strip of the view glb. | **Part data from the asset, built at bake.** Merged records are appended and censused like any primitive, and their chains are cut and certified. The per-part records stay first, so the sim strip and `sim_glb_sha` are unchanged. `mesh_count` gains a merged region, a change to ADR-0035's schema. |
| **View-vs-sim shadow guard** | `src/bake.rs:1376` `shadow_compare_on_instance_ready`; test `bake.rs:1844`–`:1858` | Every scene node against the blueprint (bit-exact local and root poses), and each ballistic node's primitive bytes. The test asserts that the sim and view extractions are equal. | Re-stated at bake: each merged record equals its members under their rigid chains, bit for bit. The runtime guard runs on the per-part scene. The test pins extraction to the per-part scene. |
| **Sandbox armour overlay** (instrument) | `src/sandbox.rs:621` `ArmourPrimitives`, `:640` `draw_armour_overlay` | Per primitive: nearest `ViewOf` → sim part with `ArmorVolume`, then a copy on the overlay layer. | **Part data from the asset:** an instrument loads the per-part scene, because an instrument draws what it measures (`geometry_lod.rs` `SourceDetailOnly`). No logic change. |
| **Track sandbox mesh layers** (instrument) | `src/track_sandbox/mesh_layers.rs:229` `classify`, `:306` `tag_view_meshes` | Per primitive, by node name. | Same as the sandbox: the per-part scene. |
| **Track link template** | `src/track/link_view.rs:657` `hide_link_template`, `:676` `bind_link_template` | The live scene's `Link` and `Link_Box` entities, matched by `Name`: hidden, then read for the mesh, material and pin markers. | **Part data from the asset:** `Gltf::named_meshes`, materials and the node table. The merged scene carries no template, so there is nothing to hide. This must land first. |

### B. Reads moving-part nodes or marker empties only: unchanged if the merged tree keeps them

| Consumer | Where | Reads |
|---|---|---|
| View bind | `src/tank/view.rs:43` `bind_tank_view` (`:74` hidden proxies, `:111` launched-turret repair) | Node names joined to `SimParts`. Static part names simply find no view node. |
| **Shadow proxies (#171)** | `src/tank/view.rs:136` → `src/tank/shadow_proxy.rs:162` `plan`, `:202` `bind`, `:231` `switch_tank_casters` | A proxy node's parent (its assembly) and the articulated nodes, which are fenced. Proxies stay **separate cast-only meshes** under their moving part: a render scope, not a render material. The merged mesh inherits the assembly's scope (`render_policy`, nearest ancestor). |
| Servo view poses, barrel recoil | `src/tank/servo.rs:283` `interpolate_servos`; `src/tank/view.rs:142` `sync_view_barrels` | Servo and barrel view nodes. |
| Track rig | `src/track/view.rs:386` `bind_track_rigs` (`:429` drive wheels by literal name) | Wheel `ViewNode`s, and the sprocket and idler view nodes. |
| **Aim** (bore dot), camera, reticle | `src/aim.rs:544` `update_bore_indicator` (muzzle view node); `src/camera.rs:572`; `src/sight/reticle.rs:400`, `:624` | The muzzle empty and the gun (`Main_Gun_Pitch`) view node. **Keep the muzzle empties**, or the dot falls back to the tick-rate sim muzzle (`ViewNode::resolve`). |
| Kill-cam frame, x-ray anchor | `src/damage_cam/pen_cam/game/killcam.rs:207`; `xray.rs:543` | The hull view node. |
| **Cook-off turret launch** | sim `src/damage.rs:606` `launch_turrets_on_cookoff`; view `src/tank/view.rs:163` `detach_view_on_turret_launch`, `:111` repair | The turret's sim and view nodes only: the whole subtree is reparented under the free body. **Served by the moving-part split:** the turret's merged meshes, its gun and its turret-borne parts leave together. The merge must not fold any turret-borne piece into the hull. |

### C. Reads no per-part view entity

| Consumer | Where | Why |
|---|---|---|
| **Picking** (aim ray) | `src/aim.rs:59` `aim_distance`, `:349` `commit_aim` | An avian `SpatialQuery` ray over terrain and **sim** volume colliders. There is no mesh picking on tanks: no `Pickable` or `MeshRayCast` anywhere in `src/` (READ, grep). |
| **Captures** | `src/dev_capture.rs:177` (FOLLOW reads the controlled tank's root `Transform`) | Camera placement and readback only. These remain the parity instrument (§3). |
| Pen cam depth pictures, component shapes | `src/damage_cam/pen_cam/picture.rs`, `src/damage_cam/shapes.rs` | Already asset data: the sim artifact's extraction. |
| Render scope, caster classes | `src/render_policy.rs:519`; `render_cost.rs:183` | Nearest scoped ancestor, generic; the caster class sits on the tank root. |

## 2. What the 172 material-less primitives draw with

**Census of `assets/tiger_1/tiger_1.glb` (MEASURED):**
- 233 primitives in 230 meshes, 87 nodes, 13 materials.
- Only 62 primitives are referenced by a node, the scene's own. The other 171 are rung records named
  in `tiger_1.lod.json`.
- **172 carry no glTF material:**
  - all 171 rung records;
  - `Link_Box#0`, the one source primitive without one.
- Textured materials: **Mat_Visual, Rubber, Mat_Track_Link** only. All other materials are factor-only.
- The sim glb has the same 87 nodes and the same 62 source primitives, with matching material names
  and index and vertex counts (62 of 62).

**What each draws with:**
- **The 171 rung records draw with their source's material.** `attach_rungs` clones the rung-0
  entity's `MeshMaterial3d` handle onto every rung sibling (`geometry_lod.rs:425`, `:437`; READ).
  The shipped-asset test refuses a rung that brings its own material (`geometry_lod.rs:526`; READ).
- **`Link_Box` draws nothing.** It is hidden with its `Link` template (`link_view.rs:657`; READ).
  Without the hide it would render as glTF-default white (module doc, READ).
- **Every drawn primitive has a glTF material.** Of the 85 drawn scene primitive entities (wheel
  meshes count once per node), **18 are textured**: 2 Mat_Visual decor shells and 16 Rubber tyres.
  **67 draw with factor-only materials:** a base colour, roughness and metallic factor, and no
  texture (MEASURED; metallic is glTF's default 1.0 where the factor is absent).

| Factor-only material | Base colour (linear) | Metallic | Roughness | Double-sided | Drawn primitives |
|---|---|---|---|---|---|
| RHA | 0.35 0.40 0.45 | 1.0 | 0.55 | yes | 19 |
| MildSteel | 0.48 0.47 0.44 | 1.0 | 0.60 | yes | 26 (16 wheel discs, 6 smoke launchers, sprockets, idlers) |
| GunSteel | 0.25 0.28 0.33 | 1.0 | 0.45 | yes | 6 |
| Cast | 0.45 0.40 0.35 | 1.0 | 0.65 | yes | 4 |
| `Flesh` (crew-station boxes) | 0.80 0.55 0.45 | 0 | 0.90 | yes | 5 |
| Ammunition | 0.75 0.60 0.25 | 1.0 | 0.40 | yes | 4 |
| EngineBlock | 0.20 0.20 0.23 | 1.0 | 0.70 | yes | 2 |
| Mat_RunningGear_Paint | 0.10 0.08 0.07 | 0 | 0.60 | **no** | 1 (`Hull_Decor#0`) |

**Meshes per moving part** (DERIVED: `census.py` with the sheet's servo, barrel and roadwheel nodes
plus the four drive wheels, and hidden = colliders, shadow proxies and the link template). In
StandardMaterial, texture set *and* factors *and* cull mode are what a draw binds
(`vendor/bevy_pbr-0.19.1-scalar-math/src/pbr_material.rs:1219`: `CULL_FRONT`/`CULL_BACK` sit in the
pipeline key). So factor-only materials collapse into one render material only if the bake moves the
factors into the mesh: a palette texture with UVs remapped per member.

| Moving part | Drawn primitives today | + rung siblings | Meshes, one per glTF material | One per render material (palette, cull mode kept) | Palette, one cull mode |
|---|---|---|---|---|---|
| **Hull** | 24 | 51 | 7 | **3** | 2 |
| **Turret** (`Turret_Yaw`) | 16 | 54 | 4 | **2** | 2 |
| **Gun elevation** (`Main_Gun_Pitch`: mantlet, mask, coax body) | 3 | 9 | 2 | **1** | 1 |
| **Gun recoil** (`Main_Gun_Recoil`: breech, barrel) | 2 | 7 | 1 | **1** | 1 |
| Coax recoil / hull MG pitch / hull MG recoil | 1 / 2 / 1 | 3 / 7 / 3 | 1 / 2 / 1 | 1 / 1 / 1 | 1 / 1 / 1 |
| Hull MG yaw (the ball sits on the hull node) | 0 | 0 | 0 | 0 | 0 |
| **Each roadwheel** (×16) | 2 | 7 | 2 | **2** | 2 |
| Each sprocket (×2) / idler (×2) | 1 | 4 / 3 | 1 | 1 | 1 |
| **Total** | **85** | **260** | 54 | **46** | 45 |

Notes:
- The gun is **two** moving parts: the mantlet elevates but does not recoil. "Gun (elevation and
  recoil)" therefore yields 2 meshes, not 1. The same law also yields the coax and hull-MG parts,
  the sprockets and the idlers. In the sheet those last four are literals in
  `track/view.rs:429`, not data.
- The three shadow proxies stay as 3 separate cast-only meshes. They are not counted above.
- **Where the entities go** (DERIVED; the rung count of a merged mesh is ASSUMED at today's mean of
  260 / 85 ≈ 3.1):
  - The hull, turret and gun group drops from 183 drawn mesh entities to 10 meshes, about 41 with
    rungs.
  - The running gear keeps its 162 entities: 16 × 2 tyre/disc meshes plus 4, with their rungs.
  - Per tank: about 345 → about 200.
  - The wheels already batch by shared mesh and material on the GPU, so their remaining cost is
    CPU, per entity.

## 3. Proposed implementation: the asset-bake merge

**Decisions to take first**

- **D1, render material:** fold the 8 factor-only materials into one palette material, or keep
  them separate.
  - **Recommend the palette.** It is one StandardMaterial with an uncompressed RGBA8 base-colour and
    metallic-roughness palette, its own nearest-filtered, unmipped sampler, and each member's UVs
    remapped to its palette entry. It is exact to 8 bits per channel.
  - Keeping them separate costs: hull 7, turret 4.
- **D2, cull mode:** keep `Hull_Decor#0` single-sided (hull = 3), or draw it double-sided
  (hull = 2). This is decided by eye.
- **D3, where the per-part tree lives.** **Recommend:** the door-owned Scene 0 stays per-part,
  unchanged, and is what the sim strip, the instruments, the pen cam and the split-outs read. The
  merged tree is appended as **Scene 1**, and the game loads `Scene(1)` (`src/tank/spawn.rs:157`).
  - Scene 1 holds the moving nodes, the marker empties (muzzles), the shadow proxies and the merged
    records.
  - Its moving nodes repeat Scene 0's names, so L2.DOCUMENT (`bake.rs:38`, `:1075`) must become
    per-scene uniqueness.
  - `Gltf::named_nodes` then holds one handle per repeated name (`bevy_gltf` loader `:1006`
    overwrites). Bit-equal transforms make either handle correct. No game code reads `named_nodes`
    today (READ).
  - The alternative, building per-part copies from the sim glb, has no UVs, tangents or textures, and
    so cannot serve the pen cam.
- **D4, x-ray route.** **Recommend** the per-part copies under the live moving nodes (§1A).
  - The other option is swapping the whole view instance to Scene 0, which reuses the bind. But the
    track rig re-binds on re-instance and drops its belts for a frame (`track/view.rs:355`
    `rebind_on_reinstance`). Hull-mark decals die with the old instance while `DrawnMarks` still
    lists them.

**Steps** (each one generic, with nothing per tank)

1. **Data.** Moving parts are the sheet's `articulated_nodes()` (`src/spec.rs:1128`) plus the track's
   drive wheels, which move from code literals into the sheet.
   - Visual-only parts merge into their carrier.
   - Colliders, shadow proxies and the link template stay out of the merge, by sheet role.
2. **Bake stage**, document surgery in `scripts/tank`, in the style of `trio.py`: stdlib and numpy,
   no `bpy`, so it is testable without Blender.
   - Group drawn primitives by (moving part, render material).
   - Transform members into the moving part's frame. The chains are rigid, and L2.UNIT_SCALE already
     refuses scale on rigid chains.
   - Remap palette UVs.
   - Unify the vertex layout. The factor mesh needs POSITION, NORMAL and UV0, because the palette has
     no normal map; 10 of today's drawn members lack TANGENT (MEASURED). Textured meshes keep TANGENT.
     TEXCOORD_1 (wheels only) is read by no material (MEASURED) and is dropped.
   - Append the records and Scene 1 after the door's prefix.
3. **Chains.** The census includes the merged records, and `chains.py` cuts them.
   - Per-part chains are dropped, since no consumer bands per-part meshes any more. The rung bytes
     today are 4.27 MB of the glb's 7.65 MB of geometry (MEASURED).
   - The certificate's `mesh_count` partition gains a merged region. Record it in ADR-0035 once it is
     understood, per the ADR timing rule.
4. **Consumers.** Land the §1A list in this order:
   1. link template from the asset;
   2. hull-mark anchor;
   3. x-ray split-out;
   4. the per-part scene for the pen cam and the sandboxes;
   5. the bake guard re-stated.

   Then switch the game to `Scene(1)`.
5. **Tests**, class invariants only:
   - every drawn Scene 0 primitive lands in exactly one merged record;
   - merged positions equal the members under their chains, bit-exact;
   - no member crosses a moving node;
   - triangle totals are equal;
   - a mark on a static plate keeps its world pose through the new anchor;
   - x-ray off restores every handle and visibility.
6. **Price** it with #152's A/B/B/A runner at 30 tanks. The orchestrator measures; lanes do not.

**Risks**

- **LOD pops get bigger.** A whole moving part switches rung at once instead of staggering per
  plate. In exchange, merged decimation closes the seams ADR-0035 lets per-primitive chains open (up
  to the sum of two deviations). The outline-at-range rule (#151) still holds by the certified
  deviation.
- **Merged decimation can drop thin members** (smoke launchers, cupola detail) earlier than their own
  chains did. The deviation must be measured over the merged surface.
- **The palette must not go through basisu.** The KTX2 path is lossy, so the palette needs a raw
  image, or vertex colours plus one roughness, which changes the look.
- **ADR-0035's trio laws** ("sim = byte-strip of view", `mesh_count`) and L2.DOCUMENT change wording.
  The sim bytes and the wire do not change, so the server needs no redeploy if `sim_glb_sha` holds.
- **Per-part records stay resident.** All glb meshes are loaded as assets, about 3.4 MB of source
  geometry (MEASURED glb bytes). That is a memory cost, not frame time.
- **Coarser culling:** a merged hull is culled by one larger bound.
- **Detachment beyond the turret** (a part knocked off) has no path yet. It would be a split-out at
  that moment: hide the merged mesh, then spawn the part and a "remainder". Nothing in the game
  needs it today.

**What the owner judges by eye**

Side-by-side stills, before and after, from hidden captures on one fixed step
(`OVERMATCH_CAPTURE_POSE`/`_FOLLOW`/`_STEP_S`). Per-pixel diffs only point at where to look:

1. Close (5 m) and mid (10 m), all around the tank, at noon and at a low sun: the palette look, and
   D2's cull change.
2. At each merged chain's switch distances, which come from the new certificate, in third person
   and in the gunner optic: the size of the pops.
3. Shadows at 10, 50 and 200 m, including across #171's 120 m switch.
4. X-ray on and off, on the player's own tank, with a record playing: it must match today, since it
   draws the same per-part meshes.
5. A hull mark on a static plate and one on the turret, during traverse and recoil.
6. A turret launch: the merged turret leaves whole, and the hull interior shows through the ring as
   it does today.

## Found on the way (ADR-0034, not in scope)

Per-tank literals in logic that sit next to this work:
- `src/geometry_lod.rs:65` `TIGER_ID`;
- `src/tank/spawn.rs:110`–`:118` the `TIGER_*` paths;
- `src/damage.rs:628`, `:647`, the launched turret's mass and inertia box;
- `src/track/view.rs:429`, the drive-wheel node names.
