# Handoff — Tiger track-link texturing (Blender-authored, export-level)

**Date:** 2026-07-27 · **Branch:** `main` · **Repo:** `/Users/Yan/Desktop/github/vikng-dev/personal/overmatch`

## Goal

Move the track shoe's look out of game code and into the asset. Yan is new to 3D/texturing, so
this session was part teaching, part implementation. Running gear (road wheels, sprockets, idlers,
final drives) is the **explicitly deferred next slice** — see "Next up".

---

## STATUS: implementation landed, NOT yet seen running

`cargo check` and `cargo clippy --all-targets` were clean on the change. **Nothing has been
observed in-game or in the sandbox.** No visual confirmation in Bevy exists.

### Blocked on someone else's in-flight work

The lib **does not currently compile**, and it is not from this work. A concurrent agent (Yan
said "an agent is already on it" re: terrain tile scale) edited during this session:

- `src/net/server.rs` (mtime 01:17:56) — `E0277` Vec2+Vec3, `E0425` missing `override_spawn_pos`, `E0061` arity
- `src/terrain_grid.rs` (mtime 01:15:45)
- `src/net/client.rs`, `debug_hud.rs`, `protocol.rs` newly dirty (were clean at session start)

**Consequence: no test baseline was ever established.** A full `cargo test` before the breakage
showed **5 failures** — all in transmission/gear logic, none near `link_view`:

```
headless_test::real_tiger_f8_30_deg_rollback_rescues_to_capable_gear
headless_test::synthetic_weak_powertrain_30_deg_rollback_reports_grade_limit
track::transmission::tests::hill_hold_engages_only_near_rest
track::transmission::tests::protective_rescue_lands_at_or_below_ceiling_or_holds
track::transmission::tests::sequential_rescue_requires_adjacent_landing_safe
```

These are **presumed pre-existing** (Yan has uncommitted transmission work — memory notes an
uphill F1-pin fix pending review for alpha.12). The attempt to confirm by reverting `link_view.rs`
and re-running was defeated by the concurrent compile break. **Re-confirm before blaming this work.**

---

## What changed

### 1. `assets/tiger_1/tiger_1.blend` — unwrap + material

- `Link` object smart-UV-projected (angle limit 66°, island margin 0.003) → 156 islands,
  **1.2× stretch** (p95/p5), 56.4% UV coverage.
- **Tiling baked into the UV coordinates** at **0.35 m per repeat** (~2.1 repeats along the
  725 mm shoe). No Mapping node — see the binding constraint below.
- New material `Mat_Track_Link`: albedo + roughness + normal @ 512², `Metallic = 1.0` flat.
- `Link_Box` deliberately left materialless (unchanged behaviour; game hides it).

### 2. `assets/tiger_1/tiger_1.glb` — re-exported

Verified **link-only** by structural diff against the pre-change glb: 81 nodes / 67 meshes
unchanged, +1 material, link primitive gained `TEXCOORD_0` and a material (was `mat=None`).
Material indices renumbered (harmless — resolution is structural, not by index).
Link textures cost **0.846 MB**; glb stayed ~65 MB.

Exported ORM verified correct: `R=1` (unused), `G=roughness` 0.33–0.65, `B=1.0` (metallic).

### 3. `src/track/link_view.rs` — reads the material instead of building one

- Removed the hardcoded `StandardMaterial { base_color: 0.10/0.10/0.11, roughness 0.85,
  metallic 0.4 }`.
- Added `materials_of: Query<&MeshMaterial3d<StandardMaterial>>`; dropped
  `ResMut<Assets<StandardMaterial>>`.
- Mesh lookup now also yields the entity, so the material is read off the **same primitive**.
- Missing material → `error_once!` refusal (no fallback), matching the module's existing
  fail-loud scale-contract policy.
- Added `LINK_MATERIAL` const + two module-doc sections.
- Fixed a stale doc claim: the shoe mesh is named **`Link`**, not `Tiger_track`.

---

## Facts worth not re-deriving

**Binding constraint — why tiling lives in the UVs.** `bevy_gltf` 0.19 honours
`KHR_texture_transform` **only on `base_color_texture`** (`bevy_gltf-0.19.0/src/lib.rs:118`,
footnote `:126`, upstream bevy #15310). A Blender Mapping node would tile albedo and leave
roughness/normal at 1×. Documented in the module doc so it isn't "simplified" away.

**Export settings are plain defaults.** `bpy.ops.export_scene.gltf(export_format='GLB')` with
*no other arguments* reproduces the shipped pipeline — verified by dry-run export from the
unmodified blend, diffed to **zero** structural difference and identical size. Generator string
matches (`Khronos glTF Blender I/O v5.1.20`, Blender 5.1.2). `extras` are **not** exported.

**Tangents.** Not exported. `bevy_gltf` mikktspace-generates them when a normal map is present
(logs a warning suggesting pre-computation). `mirrored_mesh` already flips tangent `x` and `w`
handedness correctly — its comment anticipated exactly this change.

**Source pack.** `~/Downloads/worn-metal4-bl/` (FreePBR packaging: `-bl` suffix, `_Normal-ogl`).
2048² originals. Measured: `Metallic` is **flat 1.0** → dropped for a flat value. `ao` is nearly
flat (mean 0.978, min 0.63) → **useless**, real AO must be baked from geometry. `Height` shallow,
unused. Only **3 of 6 maps** are needed.

**Downscale to 512² done outside Blender** with ImageMagick — albedo resized in **linear light**
(`-colorspace RGB … -colorspace sRGB`), data maps resized **raw**. Roughness mean preserved
(0.431338 → 0.431335). *Gotcha hit and fixed:* `bpy.types.Image.pack()` on a FILE-source image
re-embeds the **original file**, silently discarding an in-memory `.scale()` — first attempt
shipped 2048² maps and grew the glb 13 MB.

**Texel density reference** (all measured):

| surface | density | notes |
|---|---|---|
| `Hull_Visual` / `Turret_Visual` @4k atlas | **322 px/m** each | deliberately matched; the house standard |
| link @512 tiled at 0.35 m | 1463 px/m | ~4.5× hull — room to drop to 256² if needed |
| `Wheel_L_3` (existing UVs) | 387 px/m, **5.0× stretch**, 2.9% coverage | orphaned/junk UVs |

---

## Known risks (unresolved, flagged to Yan)

1. **No `EnvironmentMapLight` anywhere in `src/`** (only `DirectionalLight`). A `metallic = 1.0`
   surface is almost entirely environment reflection, so the link will land **markedly darker and
   flatter in-game than in Blender**. Likely the first "this looks wrong" report. Budget for an
   env map.
2. **No mips.** `bevy_image-0.19.0/src/image.rs:1136` — `mip_level_count: 1`. glb-embedded PNGs
   ship without a mip chain; 194 link instances at grazing angles is a textbook shimmer/thrash
   case. This is the same pathology `scripts/encode-terrain-ktx2.sh` documents costing 30 fps.
   Proper fix if it bites: KTX2 loaded asset-side (rejected for now — Yan explicitly wants
   texturing handled at Blender/export level, not in game code).
3. **License is unresolved.** The pack ships **no license file** (7 image files only). Repo
   convention is a `cc.txt` provenance note per pack (all terrain packs have one, all CC0).
   FreePBR terms are *not* CC0. **Must be confirmed before shipping** — Overmatch is a commercial
   Steam title.
4. `doubleSided: true` on the new material — consistent with **all 16** materials in the file, so
   not introduced here, but it means no backface culling anywhere. Separate cleanup.

---

## ⚠️ Asset-safety warning

**`assets/tiger_1/tiger_1.blend` is NOT in version control.** `.gitignore:6-7` ignores
`*.blend`/`*.blend1`; the `*.blend` LFS rule in `.gitattributes` is dead as a result. Only the
`.glb` is tracked.

- Recovery is **only** Blender's `.blend1` rotation.
- A safety copy of the pre-session blend is at
  `<scratchpad>/tiger_1_ORIGINAL.blend` — **preserve it**.
- **Reported to Yan:** the save rotation consumed the older 26 Jul 16:40 `.blend1`. The 18:30
  pre-edit `.blend` survived as the current `.blend1`, so nothing needed was lost — but there is
  now one fewer backup generation than before.

---

## Next up — running gear (Yan's stated next slice)

Measured groundwork already done:

- **22 objects** (16 road wheels, 2 sprockets, 2 idlers, 2 final drives) render as **flat 0.5
  grey** — stub materials `hull`/`hull.001`/`hull.002` carry a roughness map only
  (`baseColorTexture=False` in the glb). Visually this is *worse* than the track was.
- **No existing atlas fits them.** `Mat_Visual`, `hull.004` and `tracks.004` were each rendered
  onto the wheels; all land on unrelated regions (grille, plating). Wheel UVs are standalone and
  distorted — they need re-unwrapping regardless.
- **Rim band is cleanly separable**: `Wheel_L_3` = 268 faces → **164 disc-facing** (|n.x|>0.9),
  **96 rim-band** (|n.x|<0.3), 8 bevel. So a two-slot split (rubber tyre / steel hub) needs **no
  UV work at all** — flat values export fine through glTF. That was the recommended Stage 0.
- Wheel is **777 mm** dia × 549 mm along the axle.
- Open question for Yan: early-production Tiger I (rubber-tyred road wheels) vs later
  (steel-rimmed, internal rubber) — changes the look; current geometry commits to neither.

Also noted but out of scope: the blend carries **29 materials from 3 stacked import generations**
(glTF, USD, partial), and every wheel has 10 material slots of which 1 is used.

---

## Reusable scripts (scratchpad, `/private/tmp/claude-502/…/scratchpad/`)

| file | purpose |
|---|---|
| `apply2.py` | **the landed change** — unwrap, tiling-into-UVs, material, save blend, export glb |
| `glbdump.py` | dumps glb structure to JSON for diffing (`glbdump.py in.glb out.json`) |
| `dryexport.py` | dry-run export used to prove default settings match the pipeline |
| `uvdemo.py` | texel-density / island / stretch measurement + UV-layout PNG renderer |
| `wornmetal.py` | tiling-scale A/B (renders the link at several metres-per-repeat) |
| `metal512/` | the corrected 512² maps that are now packed into the blend |
| `tex/` | all teaching + verification renders |
| `link_view.MINE.rs` | backup of the edited source |

---

## Suggested skills

- **`/code-review`** — review the `link_view.rs` diff **once the tree compiles again**. Focus on
  the material-refusal path and that no fallback material crept back in.
- **`/run`** — the real gap. Launch the game or `cargo track` sandbox and *look at the links*.
  Everything here is statically verified only.
- **`/security-review`** — not needed for this work.

## Immediate actions for the orchestrator

1. **Land or revert the concurrent `net/` + `terrain_grid.rs` work** so the tree compiles.
2. Re-run `cargo test`; confirm the 5 transmission failures are pre-existing and unrelated.
3. Visually verify the links (expect them darker than the Blender renders — see risk 1).
4. Resolve the FreePBR licence + write `assets/tiger_1/` provenance before any release tag.
5. Consider putting `tiger_1.blend` under LFS properly (drop the `.gitignore` lines) — it is
   currently the single least-protected artifact in the repo.
